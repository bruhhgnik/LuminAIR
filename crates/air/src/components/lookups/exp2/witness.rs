use luminair_utils::TraceError;
use num_traits::One;
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};
use stwo_air_utils::trace::component_trace::ComponentTrace;
use stwo_air_utils_derive::{IterMut, ParIterMut, Uninitialized};
use stwo_prover::{
    constraint_framework::{logup::LogupTraceGenerator, Relation},
    core::backend::simd::{
        m31::{PackedM31, LOG_N_LANES, N_LANES},
        qm31::PackedQM31,
        SimdBackend,
    },
};

use crate::{
    components::{Claim, InteractionClaim},
    preprocessed::PreProcessedColumn,
    utils::{pack_values, TreeBuilder},
};

use super::{
    table::{
        Exp2LookupColumn, Exp2LookupTraceTable, Exp2LookupTraceTableRow,
        PackedExp2LookupTraceTableRow,
    },
    Exp2LookupElements,
};

/// Type alias for the claim associated with the Exp2Lookup component's trace.
pub type Exp2LookupClaim = Claim<Exp2LookupColumn>;

/// Number of main trace columns for the Exp2Lookup component (only multiplicity).
pub(crate) const N_TRACE_COLUMNS: usize = 1;

/// Generates main trace and interaction data for the Exp2Lookup component.
///
/// Takes the `Exp2LookupTraceTable` (containing multiplicities), processes it into
/// a single main trace column, and prepares data for the LogUp interaction.
pub struct Exp2LookupClaimGenerator {
    /// The raw trace data (multiplicities) for the Exp2Lookup.
    pub inputs: Exp2LookupTraceTable,
}

impl Exp2LookupClaimGenerator {
    /// Creates a new `Exp2LookupClaimGenerator` with the given `Exp2LookupTraceTable`.
    pub fn new(inputs: Exp2LookupTraceTable) -> Self {
        Self { inputs }
    }

    /// Writes the main trace column (multiplicities) and returns data for interaction.
    ///
    /// Standard procedure: pads, packs, calls `write_trace_simd`,
    /// adds main trace to `tree_builder`, returns `Exp2LookupClaim` and `Exp2LookupInteractionClaimGenerator`.
    /// Returns `TraceError::EmptyTrace` if the input table is empty.
    pub fn write_trace(
        mut self,
        tree_builder: &mut impl TreeBuilder<SimdBackend>,
    ) -> Result<(Exp2LookupClaim, Exp2LookupInteractionClaimGenerator), TraceError> {
        let n_rows = self.inputs.table.len();

        if n_rows == 0 {
            return Err(TraceError::EmptyTrace);
        }

        let size = std::cmp::max(n_rows.next_power_of_two(), N_LANES);
        let log_size = size.ilog2();

        self.inputs
            .table
            .resize(size, Exp2LookupTraceTableRow::padding());
        let packed_inputs = pack_values(&self.inputs.table);

        let (trace, lookup_data) = write_trace_simd(packed_inputs);

        tree_builder.extend_evals(trace.to_evals());

        Ok((
            Exp2LookupClaim::new(log_size),
            Exp2LookupInteractionClaimGenerator {
                log_size,
                lookup_data,
            },
        ))
    }
}

/// Populates the main trace column (multiplicity) and `LookupData` from packed rows.
///
/// - The main trace column directly takes the `multiplicity` values.
/// - `LookupData` also stores these multiplicities for the interaction phase.
/// Returns the `ComponentTrace` and `LookupData`.
fn write_trace_simd(
    inputs: Vec<PackedExp2LookupTraceTableRow>,
) -> (ComponentTrace<N_TRACE_COLUMNS>, LookupData) {
    let log_n_packed_rows = inputs.len().ilog2();
    let log_size = log_n_packed_rows + LOG_N_LANES;

    let (mut trace, mut lookup_data) = unsafe {
        (
            ComponentTrace::<N_TRACE_COLUMNS>::uninitialized(log_size),
            LookupData::uninitialized(log_n_packed_rows),
        )
    };

    (
        trace.par_iter_mut(),
        lookup_data.par_iter_mut(),
        inputs.into_par_iter(),
    )
        .into_par_iter()
        .for_each(|(mut row, lookup_data, input)| {
            *row[Exp2LookupColumn::Multiplicity.index()] = input.multiplicity;

            *lookup_data.multiplicities = input.multiplicity;
        });

    (trace, lookup_data)
}

/// Intermediate data structure for the Exp2Lookup LogUp argument.
/// Only stores the multiplicities, as the values come from the preprocessed LUT.
#[derive(Uninitialized, IterMut, ParIterMut)]
struct LookupData {
    /// Multiplicities for each entry in the Exp2 LUT.
    multiplicities: Vec<PackedM31>,
}

/// Generates the interaction trace column for the Exp2Lookup component's LogUp argument.
///
/// This LogUp argument connects the multiplicities (from the main Exp2Lookup trace)
/// with the actual input/output values from the preprocessed Exp2 LUT.
pub struct Exp2LookupInteractionClaimGenerator {
    /// Log2 size of the trace.
    log_size: u32,
    /// Multiplicity data for the LogUp argument.
    lookup_data: LookupData,
}

impl Exp2LookupInteractionClaimGenerator {
    /// Writes the LogUp interaction trace column to the `tree_builder`.
    ///
    /// 1. Initializes a `LogupTraceGenerator`.
    /// 2. For each entry:
    ///    a. Retrieves the input (`lut_col_0`) and output (`lut_col_1`) values directly from the
    ///       preprocessed `Exp2PreProcessed` columns (`lut`).
    ///    b. Retrieves the `multiplicity` from `self.lookup_data`.
    ///    c. Combines `[input, output]` from the LUT with `elements` (Exp2LookupElements) to form the denominator.
    ///    d. The numerator for the LogUp fraction is `-multiplicity`.
    ///    e. Writes the fraction to the LogUp column.
    /// 3. Finalizes the generator, adds the interaction column to `tree_builder`, returns `InteractionClaim`.
    /// This proves that `sum_i (multiplicity_i / (alpha_0 * lut_input_i + alpha_1 * lut_output_i + beta)) = 0`
    /// when balanced with the accesses from the `Exp2Component` trace.
    pub fn write_interaction_trace(
        self,
        tree_builder: &mut impl TreeBuilder<SimdBackend>,
        elements: &Exp2LookupElements, // Randomness for Exp2 LUT (input, output) combination
        lut: &Vec<&dyn PreProcessedColumn>, // References to the two preprocessed Exp2 LUT columns
    ) -> InteractionClaim {
        let mut logup_gen = LogupTraceGenerator::new(self.log_size);

        let mut col_gen = logup_gen.new_col();
        let lut_col_0 = &lut.get(0).expect("missing exp2 col 0").gen_column().data;
        let lut_col_1 = &lut.get(1).expect("missing exp2 col 1").gen_column().data;
        for row in 0..1 << (self.log_size - LOG_N_LANES) {
            let multiplicity: PackedQM31 = self.lookup_data.multiplicities[row].into();
            let input = lut_col_0[row];
            let output = lut_col_1[row];

            let denom: PackedQM31 = elements.combine(&[input, output]);
            let num: PackedQM31 = -PackedQM31::one() * multiplicity;

            col_gen.write_frac(row, num, denom);
        }
        col_gen.finalize_col();

        let (trace, claimed_sum) = logup_gen.finalize_last();
        tree_builder.extend_evals(trace);

        InteractionClaim { claimed_sum }
    }
}