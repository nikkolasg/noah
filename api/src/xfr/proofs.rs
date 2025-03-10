use crate::anon_creds::{ac_confidential_verify, ACCommitment, ACConfidentialRevealProof};
use crate::parameters::bulletproofs::BulletproofParams;
use crate::parameters::params::{BULLET_PROOF_RANGE, MAX_CONFIDENTIAL_RECORD_NUMBER};
use crate::xfr::{
    asset_record::AssetRecordType,
    asset_tracer::RecordDataEncKey,
    structs::{
        AssetRecord, BlindAssetRecord, OpenAssetRecord, TracerMemo, TracingPolicies, XfrAmount,
        XfrAssetType, XfrBody, XfrRangeProof,
    },
    XfrNotePoliciesRef,
};
use bulletproofs::RangeProof;
use linear_map::LinearMap;
use merlin::Transcript;
use noah_algebra::{
    prelude::*,
    ristretto::{
        CompressedRistretto, PedersenCommitmentRistretto, RistrettoPoint, RistrettoScalar,
    },
    traits::PedersenCommitment,
    utils::{min_greater_equal_power_of_two, u64_to_u32_pair},
};
use noah_crypto::{
    basic::{
        chaum_pedersen::{
            chaum_pedersen_batch_verify_multiple_eq, chaum_pedersen_prove_multiple_eq,
            ChaumPedersenProofX,
        },
        elgamal::ElGamalCiphertext,
        pedersen_elgamal::{
            pedersen_elgamal_aggregate_eq_proof, pedersen_elgamal_batch_verify,
            PedersenElGamalEqProof, PedersenElGamalProofInstance,
        },
    },
    bulletproofs::range::{batch_verify_ranges, prove_ranges},
};

const POW_2_32: u64 = 0xFFFF_FFFFu64 + 1;

pub(crate) fn asset_amount_tracing_proofs<R: CryptoRng + RngCore>(
    prng: &mut R,
    inputs: &[AssetRecord],
    outputs: &[AssetRecord],
) -> Result<Vec<PedersenElGamalEqProof>> {
    let mut pks_map: LinearMap<RecordDataEncKey, Vec<(&AssetRecord, &TracerMemo)>> =
        LinearMap::new(); // use linear map because of determinism  (rather than HashMap)

    // 1. Group records by policies with same asset_tracer public keys
    // discard when there is no policy or policy asset tracing flag is off.
    collect_records_and_memos_by_keys(&mut pks_map, inputs, outputs);

    // 2. Do asset tracing for each tracer_key.
    let mut proofs = vec![];
    for (tracer_pub_key, records_memos) in pks_map.iter() {
        let mut transcript = Transcript::new(b"AssetTracingProofs");
        let proof = build_same_key_asset_type_amount_tracing_proof(
            prng,
            &mut transcript,
            &tracer_pub_key,
            &records_memos,
        )
        .c(d!())?;
        proofs.push(proof)
    }
    Ok(proofs)
}

fn build_same_key_asset_type_amount_tracing_proof<R: CryptoRng + RngCore>(
    prng: &mut R,
    transcript: &mut Transcript,
    pub_key: &RecordDataEncKey,
    records_memos: &[(&AssetRecord, &TracerMemo)],
) -> Result<PedersenElGamalEqProof> {
    let mut m = vec![];
    let mut r = vec![];
    let mut ctexts = vec![];
    let mut commitments = vec![];

    for (record, memo) in records_memos {
        let open_record = &record.open_asset_record;
        let (low, high) = u64_to_u32_pair(open_record.amount);
        if let XfrAmount::Confidential((com_low, com_high)) = open_record.blind_asset_record.amount
        {
            let (lock_amount_low, lock_amount_high) = memo
                .lock_amount
                .as_ref()
                .c(d!(NoahError::InconsistentStructureError))?;
            m.push(RistrettoScalar::from(low));
            r.push(open_record.amount_blinds.0);
            ctexts.push(lock_amount_low.clone());
            commitments.push(
                com_low
                    .decompress()
                    .c(d!(NoahError::DecompressElementError))?,
            );
            m.push(RistrettoScalar::from(high));
            r.push(open_record.amount_blinds.1);
            ctexts.push(lock_amount_high.clone());
            commitments.push(
                com_high
                    .decompress()
                    .c(d!(NoahError::DecompressElementError))?,
            );
        }
        if let XfrAssetType::Confidential(com) = open_record.blind_asset_record.asset_type {
            let lock_asset_type = memo
                .lock_asset_type
                .as_ref()
                .c(d!(NoahError::InconsistentStructureError))?;
            m.push(open_record.asset_type.as_scalar());
            r.push(open_record.type_blind);
            ctexts.push(lock_asset_type.clone());
            commitments.push(com.decompress().c(d!(NoahError::DecompressElementError))?);
        }
    }
    Ok(pedersen_elgamal_aggregate_eq_proof(
        transcript,
        prng,
        m.as_slice(),
        r.as_slice(),
        &pub_key,
        ctexts.as_slice(),
        commitments.as_slice(),
    ))
}

fn collect_records_and_memos_by_keys<'a>(
    map: &mut LinearMap<RecordDataEncKey, Vec<(&'a AssetRecord, &'a TracerMemo)>>,
    inputs: &'a [AssetRecord],
    outputs: &'a [AssetRecord],
) {
    for record in inputs.iter().chain(outputs) {
        for (policy, memo) in record
            .tracing_policies
            .get_policies()
            .iter()
            .zip(record.asset_tracers_memos.iter())
        {
            if policy.asset_tracing
                && record
                    .open_asset_record
                    .blind_asset_record
                    .get_record_type()
                    != AssetRecordType::NonConfidentialAmount_NonConfidentialAssetType
            {
                let tracer_pub_key = policy.enc_keys.record_data_enc_key.clone();
                map.entry(tracer_pub_key)
                    .or_insert(vec![])
                    .push((record, memo))
            }
        }
    }
}

type BarMemosPoliciesCollectionIterator<'a> = core::iter::Zip<
    core::iter::Zip<
        core::slice::Iter<'a, &'a TracingPolicies>,
        core::slice::Iter<'a, BlindAssetRecord>,
    >,
    core::slice::Iter<'a, Vec<TracerMemo>>,
>;

#[derive(Clone)]
/// A collection of blind asset records, memos, and policies, for ease of programming.
pub struct BarMemosPoliciesCollection<'a> {
    bars: &'a [BlindAssetRecord],
    memos: &'a [Vec<TracerMemo>],
    policies: &'a [&'a TracingPolicies],
}

impl<'a> BarMemosPoliciesCollection<'a> {
    /// Create a new collection.
    pub fn new(
        bars: &'a [BlindAssetRecord],
        memos: &'a [Vec<TracerMemo>],
        policies: &'a [&'a TracingPolicies],
    ) -> Self {
        BarMemosPoliciesCollection {
            bars,
            memos,
            policies,
        }
    }

    /// Obtain an iterator of this collection.
    pub fn range_over(&self) -> BarMemosPoliciesCollectionIterator<'a> {
        self.policies.iter().zip(self.bars.iter()).zip(self.memos)
    }

    /// Check if the collection is well-constructed.
    pub fn check(&self) -> Result<()> {
        if self.policies.len() != self.bars.len() || self.bars.len() != self.memos.len() {
            Err(eg!(NoahError::ParameterError))
        } else {
            Ok(())
        }
    }
}

fn collect_bars_and_memos_by_keys<'a>(
    map: &mut LinearMap<RecordDataEncKey, BarMemoVec<'a>>,
    bmp: &BarMemosPoliciesCollection<'a>,
) -> Result<()> {
    bmp.check().c(d!())?;

    for ((tracing_policies_i, bar_i), memos_i) in bmp.range_over() {
        // If the bar is non-confidential skip memo and bar, since there is no tracing proof.
        if bar_i.get_record_type()
            == AssetRecordType::NonConfidentialAmount_NonConfidentialAssetType
        {
            continue;
        }

        let tracing_policies_i = tracing_policies_i.get_policies();
        for (policy_i_j, memo_i_j) in tracing_policies_i.iter().zip(memos_i.iter()) {
            if policy_i_j.asset_tracing {
                let key = policy_i_j.enc_keys.record_data_enc_key.clone();
                map.entry(key)
                    .or_insert(Default::default())
                    .push(bar_i, memo_i_j); // insert ith record with j-th memo
            }
        }
    }
    Ok(())
}

pub(crate) fn batch_verify_tracer_tracing_proof<R: CryptoRng + RngCore>(
    prng: &mut R,
    xfr_bodies: &[&XfrBody],
    instances_policies: &[&XfrNotePoliciesRef<'_>],
) -> Result<()> {
    if xfr_bodies.len() != instances_policies.len() {
        return Err(eg!(NoahError::ParameterError));
    }

    // 1. Batch asset_type and amount tracing.
    let input_reveal_policies: Result<Vec<&[&TracingPolicies]>> = instances_policies
        .iter()
        .map(|policies| {
            if policies.valid {
                Ok(policies.inputs_tracing_policies.as_slice())
            } else {
                Err(eg!(NoahError::ParameterError))
            }
        })
        .collect();
    let output_reveal_policies: Result<Vec<&[&TracingPolicies]>> = instances_policies
        .iter()
        .map(|policies| {
            if policies.valid {
                Ok(policies.outputs_tracing_policies.as_slice())
            } else {
                Err(eg!(NoahError::ParameterError))
            }
        })
        .collect();
    batch_verify_asset_tracing_proofs(
        prng,
        xfr_bodies,
        &input_reveal_policies.c(d!())?,
        &output_reveal_policies.c(d!())?,
    )
    .c(d!(NoahError::XfrVerifyAssetTracingAssetAmountError))?;

    // 2. Check the identity proof individually for now.
    for (xfr_body, policies) in xfr_bodies.iter().zip(instances_policies.iter()) {
        // 2. do identity tracing proof
        let inputs_len = xfr_body.inputs.len();
        verify_identity_proofs(
            &policies.inputs_tracing_policies,
            &xfr_body.asset_tracing_memos[..inputs_len],
            &xfr_body.proofs.asset_tracing_proof.inputs_identity_proofs,
            &policies.inputs_sig_commitments,
        )
        .c(d!())?;
        verify_identity_proofs(
            &policies.outputs_tracing_policies,
            &xfr_body.asset_tracing_memos[inputs_len..],
            &xfr_body.proofs.asset_tracing_proof.outputs_identity_proofs,
            &policies.outputs_sig_commitments,
        )
        .c(d!())?;
    }

    Ok(())
}

fn batch_verify_asset_tracing_proofs<R: CryptoRng + RngCore>(
    prng: &mut R,
    xfr_bodies: &[&XfrBody],
    input_reveal_policies: &[&[&TracingPolicies]],
    output_reveal_policies: &[&[&TracingPolicies]],
) -> Result<()> {
    // Idea: collect all instances of perdersen_elgamal_equality proofs and call a single
    // batch verification for all of them.

    // Each asset record can be associated with several tracing policies.
    // Also, each tracing key in a policy can be associated with several records.
    // Proofs for same tracing key records can be aggregated into a single short proof in an XfrBody.

    // Strategy:
    // 1. For each XfrBody collect a mapping of tracing key <-> Vec<BlindAssetRecords, Memos>, and all the associated proofs.
    // 2. On each XfrBody: for each (key, Vec<BlindAssetRecord, Memo>, proof) tuple, build an instance of a pedersen_elgamal_aggregated verify proof
    // 3. Call a single batch verification proof for all the tuples collected in 2.
    let mut instances = vec![];
    let mut all_records_map = Vec::with_capacity(xfr_bodies.len());
    let mut all_proofs = Vec::with_capacity(xfr_bodies.len());
    for (xfr_body, (input_policies, output_policies)) in xfr_bodies.iter().zip(
        input_reveal_policies
            .iter()
            .zip(output_reveal_policies.iter()),
    ) {
        let records_map =
            collect_records_memos_by_key(xfr_body, input_policies, output_policies).c(d!())?;
        let m = records_map.len();
        if m != xfr_body
            .proofs
            .asset_tracing_proof
            .asset_type_and_amount_proofs
            .len()
        {
            return Err(eg!(NoahError::XfrVerifyAssetTracingAssetAmountError));
        }
        all_records_map.push(records_map);
        all_proofs.push(
            &xfr_body
                .proofs
                .asset_tracing_proof
                .asset_type_and_amount_proofs,
        );
    }

    for (records_map, proofs) in all_records_map.iter().zip(all_proofs.iter()) {
        for ((key, records_and_memos), proof) in records_map.iter().zip(proofs.iter()) {
            let (ctexts, commitments) =
                extract_ciphertext_and_commitments(&records_and_memos.0).c(d!())?;
            let peg_eq_instance = PedersenElGamalProofInstance {
                public_key: key,
                cts: ctexts,
                commitments,
                proof,
            };
            instances.push(peg_eq_instance);
        }
    }
    let mut transcript = Transcript::new(b"AssetTracingProofs");
    pedersen_elgamal_batch_verify(&mut transcript, prng, &instances).c(d!())
}

#[derive(Default)]
struct BarMemoVec<'a>(Vec<(&'a BlindAssetRecord, &'a TracerMemo)>);

impl<'a> BarMemoVec<'a> {
    fn push(&mut self, record: &'a BlindAssetRecord, memo: &'a TracerMemo) {
        self.0.push((record, memo))
    }
}

fn collect_records_memos_by_key<'a>(
    xfr_body: &'a XfrBody,
    input_reveal_policies: &'a [&TracingPolicies],
    output_reveal_policies: &'a [&TracingPolicies],
) -> Result<LinearMap<RecordDataEncKey, BarMemoVec<'a>>> {
    let mut map: LinearMap<RecordDataEncKey, BarMemoVec<'a>> = LinearMap::new();
    let inputs_len = xfr_body.inputs.len();
    let bars_memo_policies_input = BarMemosPoliciesCollection::new(
        &xfr_body.inputs,
        &xfr_body.asset_tracing_memos[..inputs_len], // only inputs
        input_reveal_policies,
    );
    collect_bars_and_memos_by_keys(&mut map, &bars_memo_policies_input)
        .c(d!(NoahError::XfrVerifyAssetTracingIdentityError))?;

    let bars_memo_policies_output = BarMemosPoliciesCollection::new(
        &xfr_body.outputs,
        &xfr_body.asset_tracing_memos[inputs_len..], // only outputs
        output_reveal_policies,
    );
    collect_bars_and_memos_by_keys(&mut map, &bars_memo_policies_output)
        .c(d!(NoahError::XfrVerifyAssetTracingIdentityError))?;
    Ok(map)
}

fn verify_identity_proofs(
    reveal_policies: &[&TracingPolicies],
    memos: &[Vec<TracerMemo>],
    proofs: &[Vec<Option<ACConfidentialRevealProof>>],
    sig_commitments: &[Option<&ACCommitment>],
) -> Result<()> {
    // 1. Check structures.
    let n = reveal_policies.len();

    if memos.len() != proofs.len() || n != sig_commitments.len() {
        return Err(eg!(NoahError::XfrVerifyAssetTracingIdentityError));
    }
    // if no policies, memos and proofs should be empty
    if n == 0 {
        // all memos must be empty
        if !memos.iter().all(|vec| vec.is_empty()) || !proofs.iter().all(|vec| vec.is_empty()) {
            return Err(eg!(NoahError::XfrVerifyAssetTracingIdentityError));
        }
    } else if n != memos.len() {
        return Err(eg!(NoahError::XfrVerifyAssetTracingIdentityError));
    }

    // 2. Check proofs.
    for (policies, (memos, (proofs, sig_commitment))) in reveal_policies
        .iter()
        .zip(memos.iter().zip(proofs.iter().zip(sig_commitments.iter())))
    {
        let m = policies.len();
        if m != memos.len() || m != proofs.len() {
            return Err(eg!(NoahError::XfrVerifyAssetTracingIdentityError));
        }
        // for each policy memo and proof
        let policies = policies.get_policies();
        for (policy, (memo, proof)) in policies.iter().zip(memos.iter().zip(proofs)) {
            let enc_keys = &policy.enc_keys.attrs_enc_key;
            match (&policy.identity_tracing, proof) {
                (Some(policy), Some(proof)) => {
                    let sig_com =
                        sig_commitment.c(d!(NoahError::XfrVerifyAssetTracingIdentityError))?;
                    ac_confidential_verify(
                        &policy.cred_issuer_pub_key,
                        enc_keys,
                        &policy.reveal_map.as_slice(),
                        sig_com,
                        &memo.lock_attributes[..],
                        proof,
                        &[],
                    )
                    .c(d!(NoahError::XfrVerifyAssetTracingIdentityError))?
                }
                (None, None) => {}
                _ => {
                    return Err(eg!(NoahError::XfrVerifyAssetTracingIdentityError));
                }
            }
        }
    }
    Ok(())
}

fn extract_ciphertext_and_commitments(
    records_and_memos: &[(&BlindAssetRecord, &TracerMemo)],
) -> Result<(Vec<ElGamalCiphertext<RistrettoPoint>>, Vec<RistrettoPoint>)> {
    let mut ctexts = vec![];
    let mut coms = vec![];
    for record_and_memo in records_and_memos {
        let record = record_and_memo.0;
        let asset_tracer_memo = record_and_memo.1;

        if asset_tracer_memo.lock_amount.is_none() && record.amount.is_confidential() {
            return Err(eg!(NoahError::InconsistentStructureError)); // There should be a lock for the amount
        }
        if let Some(lock_amount) = &asset_tracer_memo.lock_amount {
            ctexts.push(lock_amount.0.clone());
            ctexts.push(lock_amount.1.clone());
            let commitments = record
                .amount
                .get_commitments()
                .c(d!(NoahError::InconsistentStructureError))?;
            coms.push(
                (commitments.0)
                    .decompress()
                    .c(d!(NoahError::DecompressElementError))?,
            );
            coms.push(
                (commitments.1)
                    .decompress()
                    .c(d!(NoahError::DecompressElementError))?,
            );
        }

        if asset_tracer_memo.lock_asset_type.is_none() && record.asset_type.is_confidential() {
            return Err(eg!(NoahError::InconsistentStructureError)); // There should be a lock for the asset type
        }
        if let Some(lock_type) = &asset_tracer_memo.lock_asset_type {
            ctexts.push(lock_type.clone());
            coms.push(
                record
                    .asset_type
                    .get_commitment()
                    .c(d!(NoahError::InconsistentStructureError))?
                    .decompress()
                    .c(d!(NoahError::DecompressElementError))?,
            );
        }
    }
    Ok((ctexts, coms))
}

/// Compute a range proof for confidential amount non-confidential asset type transfers.
/// The proof guarantees that output amounts and difference between total input,
/// and total output are in the range [0,2^{64} - 1].
pub(crate) fn gen_range_proof(
    inputs: &[&OpenAssetRecord],
    outputs: &[&OpenAssetRecord],
) -> Result<XfrRangeProof> {
    let num_output = outputs.len();
    let upper_power2 = min_greater_equal_power_of_two((2 * (num_output + 1)) as u32) as usize;
    if upper_power2 > MAX_CONFIDENTIAL_RECORD_NUMBER {
        return Err(eg!(NoahError::RangeProofProveError));
    }

    let params = BulletproofParams::default();

    // Build values vector (out amounts + amount difference).
    let in_total = inputs.iter().fold(0u64, |accum, x| accum + x.amount);
    let out_amounts: Vec<u64> = outputs.iter().map(|x| x.amount).collect();
    let out_total = out_amounts.iter().sum::<u64>();
    let xfr_diff = if in_total >= out_total {
        in_total - out_total
    } else {
        return Err(eg!(NoahError::RangeProofProveError));
    };
    let mut values = Vec::with_capacity(upper_power2);
    for x in out_amounts {
        let (lower, higher) = u64_to_u32_pair(x);
        values.push(lower as u64);
        values.push(higher as u64);
    }
    let (diff_low, diff_high) = u64_to_u32_pair(xfr_diff);
    values.push(diff_low as u64);
    values.push(diff_high as u64);
    values.resize(upper_power2, 0u64);

    // Build blinding vectors (out blindings + blindings difference).
    let (total_blind_input_low, total_blind_input_high) = add_blindings(inputs);
    let (total_blind_output_low, total_blind_output_high) = add_blindings(outputs);

    let xfr_blind_diff_low = total_blind_input_low.sub(&total_blind_output_low);
    let xfr_blind_diff_high = total_blind_input_high.sub(&total_blind_output_high);

    let mut range_proof_blinds = Vec::with_capacity(upper_power2);
    for output in outputs.iter() {
        range_proof_blinds.push(output.amount_blinds.0); // low
        range_proof_blinds.push(output.amount_blinds.1); // high
    }
    range_proof_blinds.push(xfr_blind_diff_low);
    range_proof_blinds.push(xfr_blind_diff_high);
    for _ in range_proof_blinds.len()..upper_power2 {
        range_proof_blinds.push(RistrettoScalar::default());
    }

    // The transcript header is unchanged for compatibility.
    let mut transcript = Transcript::new(b"Zei Range Proof");
    let (range_proof, coms) = prove_ranges(
        &params.bp_gens,
        &mut transcript,
        values.as_slice(),
        range_proof_blinds.as_slice(),
        BULLET_PROOF_RANGE,
    )
    .c(d!(NoahError::RangeProofProveError))?;

    let diff_com_low = coms[2 * num_output];
    let diff_com_high = coms[2 * num_output + 1];
    Ok(XfrRangeProof {
        range_proof,
        xfr_diff_commitment_low: diff_com_low,
        xfr_diff_commitment_high: diff_com_high,
    })
}

fn add_blindings(oar: &[&OpenAssetRecord]) -> (RistrettoScalar, RistrettoScalar) {
    oar.iter().fold(
        (RistrettoScalar::zero(), RistrettoScalar::zero()),
        |(low, high), x| (low.add(&x.amount_blinds.0), high.add(&x.amount_blinds.1)),
    )
}

pub(crate) fn batch_verify_confidential_amount<R: CryptoRng + RngCore>(
    prng: &mut R,
    params: &BulletproofParams,
    instances: &[(
        &Vec<BlindAssetRecord>,
        &Vec<BlindAssetRecord>,
        &XfrRangeProof,
    )],
) -> Result<()> {
    // The transcript header is unchanged for compatibility.
    let mut transcripts = vec![Transcript::new(b"Zei Range Proof"); instances.len()];
    let proofs: Vec<&RangeProof> = instances.iter().map(|(_, _, pf)| &pf.range_proof).collect();
    let mut commitments = vec![];
    for (input, output, proof) in instances {
        commitments
            .push(extract_value_commitments(input.as_slice(), output.as_slice(), proof).c(d!())?);
    }
    let value_commitments = commitments.iter().map(|c| c.as_slice()).collect_vec();
    batch_verify_ranges(
        prng,
        &params.bp_gens,
        proofs.as_slice(),
        &mut transcripts,
        &value_commitments,
        BULLET_PROOF_RANGE,
    )
    .c(d!(NoahError::XfrVerifyConfidentialAmountError))
}

fn extract_value_commitments(
    inputs: &[BlindAssetRecord],
    outputs: &[BlindAssetRecord],
    proof: &XfrRangeProof,
) -> Result<Vec<CompressedRistretto>> {
    let num_output = outputs.len();
    let upper_power2 = min_greater_equal_power_of_two((2 * num_output + 2) as u32) as usize;
    let pow2_32 = RistrettoScalar::from(POW_2_32);

    let mut commitments = Vec::with_capacity(upper_power2);

    // 1. Verify proof commitment to transfer's input - output amounts match proof commitments.
    let mut total_input_com_low = RistrettoPoint::get_identity();
    let mut total_input_com_high = RistrettoPoint::get_identity();
    for input in inputs.iter() {
        let (com_low, com_high) = match input.amount {
            XfrAmount::Confidential((com_low, com_high)) => (
                com_low
                    .decompress()
                    .c(d!(NoahError::XfrVerifyConfidentialAmountError))?,
                com_high
                    .decompress()
                    .c(d!(NoahError::XfrVerifyConfidentialAmountError))?,
            ),
            XfrAmount::NonConfidential(amount) => {
                let (low, high) = u64_to_u32_pair(amount);
                let pc_gens = PedersenCommitmentRistretto::default();
                let com_low = pc_gens.commit(RistrettoScalar::from(low), RistrettoScalar::zero());
                let com_high = pc_gens.commit(RistrettoScalar::from(high), RistrettoScalar::zero());
                (com_low, com_high)
            }
        };
        total_input_com_low = total_input_com_low.add(&com_low);
        total_input_com_high = total_input_com_high.add(&com_high);
    }
    let mut total_output_com_low = RistrettoPoint::get_identity();
    let mut total_output_com_high = RistrettoPoint::get_identity();
    for output in outputs.iter() {
        let (com_low, com_high) = match output.amount {
            XfrAmount::Confidential((com_low, com_high)) => (
                com_low.decompress().c(d!(NoahError::ParameterError))?,
                com_high.decompress().c(d!(NoahError::ParameterError))?,
            ),
            XfrAmount::NonConfidential(amount) => {
                let (low, high) = u64_to_u32_pair(amount);
                let pc_gens = PedersenCommitmentRistretto::default();
                let com_low = pc_gens.commit(RistrettoScalar::from(low), RistrettoScalar::zero());
                let com_high = pc_gens.commit(RistrettoScalar::from(high), RistrettoScalar::zero());
                (com_low, com_high)
            }
        };
        total_output_com_low = total_output_com_low.add(&com_low);
        total_output_com_high = total_output_com_high.add(&com_high);

        commitments.push(com_low.compress());
        commitments.push(com_high.compress());
    }

    // 2. Derive input - output commitment, compare with proof struct low and high commitments
    let derived_xfr_diff_com = total_input_com_low.sub(&total_output_com_low).add(
        &total_input_com_high
            .sub(&total_output_com_high)
            .mul(&pow2_32),
    );
    let proof_xfr_com_low = proof
        .xfr_diff_commitment_low
        .decompress()
        .c(d!(NoahError::DecompressElementError))?;
    let proof_xfr_com_high = proof
        .xfr_diff_commitment_high
        .decompress()
        .c(d!(NoahError::DecompressElementError))?;
    let proof_xfr_com_diff = proof_xfr_com_low.add(&proof_xfr_com_high.mul(&pow2_32));

    if derived_xfr_diff_com.compress() != proof_xfr_com_diff.compress() {
        return Err(eg!(NoahError::XfrVerifyConfidentialAmountError));
    }

    // 3. Push diff commitments.
    commitments.push(proof.xfr_diff_commitment_low);
    commitments.push(proof.xfr_diff_commitment_high);

    // 4. Pad with commitments to 0.
    for _ in commitments.len()..upper_power2 {
        commitments.push(CompressedRistretto::identity());
    }

    Ok(commitments)
}

/// Compute an asset proof for confidential asset transfers
pub(crate) fn asset_proof<R: CryptoRng + RngCore>(
    prng: &mut R,
    pc_gens: &PedersenCommitmentRistretto,
    open_inputs: &[&OpenAssetRecord],
    open_outputs: &[&OpenAssetRecord],
) -> Result<ChaumPedersenProofX> {
    let mut asset_coms = vec![];
    let mut asset_blinds = vec![];

    for x in open_inputs.iter().chain(open_outputs) {
        let commitment = match x.blind_asset_record.asset_type {
            XfrAssetType::Confidential(com) => com.decompress().c(d!(NoahError::ParameterError))?,
            XfrAssetType::NonConfidential(asset_type) => {
                pc_gens.commit(asset_type.as_scalar(), x.type_blind)
            }
        };
        asset_coms.push(commitment);
        asset_blinds.push(x.type_blind);
    }
    let mut transcript = Transcript::new(b"AssetEquality");

    chaum_pedersen_prove_multiple_eq(
        &mut transcript,
        prng,
        &open_inputs[0].asset_type.as_scalar(),
        asset_coms.as_slice(),
        asset_blinds.as_slice(),
    )
    .c(d!())
}

pub(crate) fn batch_verify_confidential_asset<R: CryptoRng + RngCore>(
    prng: &mut R,
    instances: &[(
        &Vec<BlindAssetRecord>,
        &Vec<BlindAssetRecord>,
        &ChaumPedersenProofX,
    )],
) -> Result<()> {
    let pc_gens = PedersenCommitmentRistretto::default();
    let mut transcript = Transcript::new(b"AssetEquality");
    let mut proof_instances = Vec::with_capacity(instances.len());
    for (inputs, outputs, proof) in instances {
        let instance_commitments: Result<Vec<RistrettoPoint>> = inputs
            .iter()
            .chain(outputs.iter())
            .map(|x| match x.asset_type {
                XfrAssetType::Confidential(com) => {
                    com.decompress().c(d!(NoahError::ParameterError))
                }
                XfrAssetType::NonConfidential(asset_type) => {
                    Ok(pc_gens.commit(asset_type.as_scalar(), RistrettoScalar::zero()))
                }
            })
            .collect();
        proof_instances.push((instance_commitments.c(d!())?, *proof));
    }
    chaum_pedersen_batch_verify_multiple_eq(&mut transcript, prng, &proof_instances)
        .c(d!(NoahError::XfrVerifyConfidentialAssetError))
}

#[cfg(test)]
mod tests {
    use crate::xfr::{
        proofs::verify_identity_proofs,
        structs::{AssetTracerKeyPair, TracerMemo, TracingPolicies, TracingPolicy},
    };
    use noah_algebra::prelude::*;

    #[test]
    fn verify_identity_proofs_structure() {
        let mut prng = test_rng();

        // Case where the number of asset tracing policies is 0
        let reveal_policies = vec![];
        let memos = vec![];
        let proofs = vec![];
        let sig_commitments = vec![];

        // 1. no policies => correct verification
        let res = verify_identity_proofs(
            reveal_policies.as_slice(),
            memos.as_slice(),
            proofs.as_slice(),
            sig_commitments.as_slice(),
        );
        pnk!(res);

        // fake sig commitment
        let sig_commitment = crate::anon_creds::ACCommitment::default(); // { 0: ACSignature { sigma1: BLSG1::get_identity(),
                                                                         //sigma2: BLSG1::get_identity() } };

        // 2. sig commitments length doesn't match memos length
        let sig_commitments = vec![Some(&sig_commitment)];
        let res = verify_identity_proofs(
            reveal_policies.as_slice(),
            memos.as_slice(),
            proofs.as_slice(),
            sig_commitments.as_slice(),
        );

        msg_eq!(
            NoahError::XfrVerifyAssetTracingIdentityError,
            res.unwrap_err()
        );

        // 2. if policy, then there must be memos and proofs
        let policy = TracingPolicy {
            enc_keys: AssetTracerKeyPair::generate(&mut prng).enc_key,
            asset_tracing: true,    // do asset tracing
            identity_tracing: None, // do not trace identity
        };

        let asset_tracing_policies = TracingPolicies(vec![policy]);
        let reveal_policies = vec![&asset_tracing_policies];

        let res = verify_identity_proofs(
            reveal_policies.as_slice(),
            memos.as_slice(),
            proofs.as_slice(),
            sig_commitments.as_slice(),
        );

        msg_eq!(
            NoahError::XfrVerifyAssetTracingIdentityError,
            res.unwrap_err()
        );

        // fake memo
        let tracer_key = AssetTracerKeyPair::generate(&mut prng).enc_key;
        let memos = vec![vec![TracerMemo::new(
            &mut prng,
            &tracer_key,
            None,
            None,
            &[],
        )]];
        let reveal_policies = vec![&asset_tracing_policies];

        let res = verify_identity_proofs(
            reveal_policies.as_slice(),
            memos.as_slice(),
            proofs.as_slice(),
            sig_commitments.as_slice(),
        );

        msg_eq!(
            NoahError::XfrVerifyAssetTracingIdentityError,
            res.unwrap_err()
        );
    }
}
