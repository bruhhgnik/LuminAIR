use num_traits::identities::Zero;
use stwo_prover::core::backend::simd::m31::PackedBaseField;
use stwo_prover::core::backend::Column;
use stwo_prover::core::{
    backend::{simd::SimdBackend, Col},
    fields::m31::BaseField,
    poly::circle::{CanonicCoset, CircleEvaluation},
};

use crate::components::{AddClaim, TraceColumn, TraceEval};

/// Generate trace for element-wise addition of two vectors.
pub fn gen_add_trace(
    log_size: u32,
    lhs: &[PackedBaseField],
    rhs: &[PackedBaseField],
) -> (TraceEval, AddClaim, Vec<PackedBaseField>) {
    // Calculate trace size and initialize columns
    let trace_size = 1 << log_size;
    let mut trace = Vec::with_capacity(3);
    for _ in 0..3 {
        trace.push(Col::<SimdBackend, BaseField>::zeros(trace_size));
    }

    // Calculate actual size needed
    let size = lhs.len().max(rhs.len());

    // Prepare output data
    let mut c_data = Vec::with_capacity(size);

    // Fill trace and generate output data
    for i in 0..trace_size {
        if i < size {
            // Get values with broadcasting
            let lhs = lhs[i % lhs.len()];
            let rhs = rhs[i % rhs.len()];
            let out = lhs + rhs;

            trace[0].set(i, lhs.to_array()[0]);
            trace[1].set(i, rhs.to_array()[0]);
            trace[2].set(i, out.to_array()[0]);

            if i < size {
                c_data.push(out);
            }
        } else {
            // Pad remaining trace with zeros
            trace[0].set(i, BaseField::zero());
            trace[1].set(i, BaseField::zero());
            trace[2].set(i, BaseField::zero());
        }
    }

    // Create domain
    let domain = CanonicCoset::new(log_size).circle_domain();

    (
        trace
            .into_iter()
            .map(|eval| CircleEvaluation::new(domain, eval))
            .collect(),
        AddClaim {
            log_size,
            _marker: std::marker::PhantomData,
        },
        c_data,
    )
}

/// Enum representing the column indices in the Add trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddColumn {
    /// Index of the `lhs` register column in the Add trace.
    Lhs,
    /// Index of the `rhs` register column in the Add trace.
    Rhs,
    /// Index of the `res` register column in the Add trace.
    Res,
}

impl TraceColumn for AddColumn {
    fn count() -> (usize, usize) {
        (3, 0)
    }
}
