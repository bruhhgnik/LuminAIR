use luminair_air::{
    components::{
        add, lookups, max_reduce, mul, recip, sin, sqrt, sum_reduce, exp2, LuminairComponents,
        LuminairInteractionElements,
    },
    pie::{LuminairPie, TraceTable},
    preprocessed::{lookups_to_preprocessed_column, PreProcessedColumn, PreProcessedTrace, SinPreProcessed, Exp2PreProcessed},
    settings::CircuitSettings,
    LuminairClaim, LuminairInteractionClaim, LuminairInteractionClaimGenerator,
};
use luminair_utils::LuminairError;
use stwo_prover::core::{
    backend::simd::SimdBackend,
    channel::Blake2sChannel,
    pcs::{CommitmentSchemeProver, PcsConfig},
    poly::circle::{CanonicCoset, PolyOps},
    prover,
    vcs::blake2_merkle::{Blake2sMerkleChannel, Blake2sMerkleHasher},
};

use crate::LuminairProof;

/// Generates a STWO proof for the computation graph execution.
///
/// Takes the `LuminairPie` (containing execution traces) and `CircuitSettings`.
/// It orchestrates the STWO proving protocol:
/// 1. Sets up the prover, channel, and commitment scheme.
/// 2. Commits to the preprocessed trace.
/// 3. Commits to the main execution trace components (add, mul, sin, etc.).
/// 4. Commits to the interaction trace.
/// 5. Executes the Stwo prover.
/// Returns a `LuminairProof` containing the claims and the STARK proof.
pub fn prove(
    pie: LuminairPie,
    settings: CircuitSettings,
) -> Result<LuminairProof<Blake2sMerkleHasher>, LuminairError> {
    // ┌──────────────────────────┐
    // │     Protocol Setup       │
    // └──────────────────────────┘
    tracing::info!("Protocol Setup");
    let config: PcsConfig = PcsConfig::default();
    let max_log_size = pie.execution_resources.max_log_size;
    let twiddles = SimdBackend::precompute_twiddles(
        CanonicCoset::new(max_log_size + config.fri_config.log_blowup_factor + 2)
            .circle_domain()
            .half_coset,
    );
    // Setup protocol.
    let channel = &mut Blake2sChannel::default();
    let mut commitment_scheme =
        CommitmentSchemeProver::<_, Blake2sMerkleChannel>::new(config, &twiddles);

    // ┌───────────────────────────────────────────────┐
    // │   Interaction Phase 0 - Preprocessed Trace    │
    // └───────────────────────────────────────────────┘

    tracing::info!("Preprocessed Trace");
    // Convert lookups in circuit settings to preprocessed column.
    let lut_cols = lookups_to_preprocessed_column(&settings.lookups);
    let preprocessed_trace = PreProcessedTrace::new(lut_cols);
    let mut tree_builder = commitment_scheme.tree_builder();
    tree_builder.extend_evals(preprocessed_trace.gen_trace());
    // Commit the preprocessed trace
    tree_builder.commit(channel);

    // ┌───────────────────────────────────────┐
    // │    Interaction Phase 1 - Main Trace   │
    // └───────────────────────────────────────┘

    tracing::info!("Main Trace");
    let mut main_claim = LuminairClaim::default();
    let mut interaction_claim_gen = LuminairInteractionClaimGenerator::default();
    let mut tree_builder = commitment_scheme.tree_builder();

    for table in pie.trace_tables.clone() {
        match table {
            TraceTable::Add { table } => {
                let claim_gen = add::witness::ClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.add = Some(cl.clone());
                interaction_claim_gen.add = Some(in_cl_gen);
            }
            TraceTable::Mul { table } => {
                let claim_gen = mul::witness::ClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.mul = Some(cl.clone());
                interaction_claim_gen.mul = Some(in_cl_gen);
            }
            TraceTable::Recip { table } => {
                let claim_gen = recip::witness::ClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.recip = Some(cl.clone());
                interaction_claim_gen.recip = Some(in_cl_gen);
            }
            TraceTable::Sin { table } => {
                let claim_gen = sin::witness::ClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.sin = Some(cl.clone());
                interaction_claim_gen.sin = Some(in_cl_gen);
            }
            TraceTable::SinLookup { table } => {
                let claim_gen = lookups::sin::witness::ClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.sin_lookup = Some(cl.clone());
                interaction_claim_gen.sin_lookup = Some(in_cl_gen);
            }
            TraceTable::SumReduce { table } => {
                let claim_gen = sum_reduce::witness::ClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.sum_reduce = Some(cl.clone());
                interaction_claim_gen.sum_reduce = Some(in_cl_gen);
            }
            TraceTable::MaxReduce { table } => {
                let claim_gen = max_reduce::witness::ClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.max_reduce = Some(cl.clone());
                interaction_claim_gen.max_reduce = Some(in_cl_gen);
            }
            TraceTable::Sqrt { table } => {
                let claim_gen = sqrt::witness::ClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.sqrt = Some(cl.clone());
                interaction_claim_gen.sqrt = Some(in_cl_gen);
            }
            TraceTable::Exp2 { table } => {
                let claim_gen = exp2::witness::Exp2ClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.exp2 = Some(cl.clone());
                interaction_claim_gen.exp2 = Some(in_cl_gen);
            }
            TraceTable::Exp2Lookup { table } => {
                let claim_gen = lookups::exp2::witness::Exp2LookupClaimGenerator::new(table);
                let (cl, in_cl_gen) = claim_gen.write_trace(&mut tree_builder)?;
                main_claim.exp2_lookup = Some(cl.clone());
                interaction_claim_gen.exp2_lookup = Some(in_cl_gen);
            }
        }
    }
    // Mix the claim into the Fiat-Shamir channel.
    main_claim.mix_into(channel);
    // Commit the main trace.
    tree_builder.commit(channel);

    // ┌───────────────────────────────────────────────┐
    // │    Interaction Phase 2 - Interaction Trace    │
    // └───────────────────────────────────────────────┘

    tracing::info!("Interaction Trace");
    let interaction_elements = LuminairInteractionElements::draw(channel);
    let mut interaction_claim = LuminairInteractionClaim::default();
    let mut tree_builder = commitment_scheme.tree_builder();
    let node_elements = &interaction_elements.node_elements;
    let lookup_elements = &interaction_elements.lookup_elements;
    if let Some(claim_gen) = interaction_claim_gen.add {
        let claim = claim_gen.write_interaction_trace(&mut tree_builder, node_elements);
        interaction_claim.add = Some(claim)
    }
    if let Some(claim_gen) = interaction_claim_gen.mul {
        let claim = claim_gen.write_interaction_trace(&mut tree_builder, node_elements);
        interaction_claim.mul = Some(claim)
    }
    if let Some(claim_gen) = interaction_claim_gen.recip {
        let claim = claim_gen.write_interaction_trace(&mut tree_builder, node_elements);
        interaction_claim.recip = Some(claim)
    }
    if let Some(claim_gen) = interaction_claim_gen.sin {
        let claim = claim_gen.write_interaction_trace(
            &mut tree_builder,
            node_elements,
            &lookup_elements.sin,
        );
        interaction_claim.sin = Some(claim)
    }
    if let Some(claim_gen) = interaction_claim_gen.sin_lookup {
        let mut sin_luts = preprocessed_trace.columns_of::<SinPreProcessed>();
        sin_luts.sort_by_key(|c| c.col_index);

        let claim =
            claim_gen.write_interaction_trace(&mut tree_builder, &lookup_elements.sin, &sin_luts);
        interaction_claim.sin_lookup = Some(claim)
    }
    if let Some(claim_gen) = interaction_claim_gen.sum_reduce {
        let claim = claim_gen.write_interaction_trace(&mut tree_builder, node_elements);
        interaction_claim.sum_reduce = Some(claim)
    }
    if let Some(claim_gen) = interaction_claim_gen.max_reduce {
        let claim = claim_gen.write_interaction_trace(&mut tree_builder, node_elements);
        interaction_claim.max_reduce = Some(claim)
    }
    if let Some(claim_gen) = interaction_claim_gen.sqrt {
        let claim = claim_gen.write_interaction_trace(&mut tree_builder, node_elements);
        interaction_claim.sqrt = Some(claim)
    }
    if let Some(claim_gen) = interaction_claim_gen.exp2 {
        let claim = claim_gen.write_interaction_trace(&mut tree_builder, node_elements, &lookup_elements.exp2);
        interaction_claim.exp2 = Some(claim)
    }
    if let Some(claim_gen) = interaction_claim_gen.exp2_lookup {
        let mut exp2_luts = preprocessed_trace.columns_of::<Exp2PreProcessed>();
        exp2_luts.sort_by_key(|c| c.col_index);
        
        // Convert to Vec<&dyn PreProcessedColumn>
        let exp2_luts_dyn: Vec<&dyn PreProcessedColumn> = exp2_luts.iter().map(|c| *c as &dyn PreProcessedColumn).collect();
        
        let claim = claim_gen.write_interaction_trace(&mut tree_builder, &lookup_elements.exp2, &exp2_luts_dyn);
        interaction_claim.exp2_lookup = Some(claim)
    }
    // Mix the interaction claim into the Fiat-Shamir channel.
    interaction_claim.mix_into(channel);
    // Commit the interaction trace.
    tree_builder.commit(channel);

    // ┌──────────────────────────┐
    // │     Proof Generation     │
    // └──────────────────────────┘
    tracing::info!("Proof Generation");
    let component_builder = LuminairComponents::new(
        &main_claim,
        &interaction_elements,
        &interaction_claim,
        &preprocessed_trace,
        &settings.lookups,
    );
    let components = component_builder.provers();
    let proof = prover::prove::<SimdBackend, _>(&components, channel, commitment_scheme)?;

    Ok(LuminairProof {
        claim: main_claim,
        interaction_claim,
        proof,
    })
}
