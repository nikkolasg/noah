use digest::Digest;
use merlin::Transcript;
use noah_algebra::prelude::*;

/// The transcript methods for the Sigma protocol.
pub trait SigmaTranscript {
    /// Initialize the transcript for the Sigma protocol.
    fn init_sigma<G: Group>(
        &mut self,
        instance_name: &'static [u8],
        public_scalars: &[&G::ScalarType],
        public_elems: &[G],
    );
    /// Append group elements to the transcript.
    fn append_group_element<G: Group>(&mut self, label: &'static [u8], elem: &G);
    /// Append field elements to the transcript.
    fn append_field_element<S: Scalar>(&mut self, label: &'static [u8], scalar: &S);
    /// Append the proof commitment to the transcript.
    fn append_proof_commitment<G: Group>(&mut self, elem: &G);
    /// Compute a challenge from the transcript.
    fn get_challenge<S: Scalar>(&mut self) -> S;
}

impl SigmaTranscript for Transcript {
    fn init_sigma<G: Group>(
        &mut self,
        instance_name: &'static [u8],
        public_scalars: &[&G::ScalarType],
        public_elems: &[G],
    ) {
        self.append_message(
            b"Sigma Protocol domain",
            b"Sigma protocol single group v.0.1",
        );
        self.append_message(b"Sigma Protocol instance", instance_name);
        for scalar in public_scalars {
            self.append_message(b"public scalar", scalar.to_bytes().as_slice())
        }
        for elem in public_elems {
            self.append_message(b"public elem", elem.to_compressed_bytes().as_slice())
        }
    }
    fn append_group_element<G: Group>(&mut self, label: &'static [u8], elem: &G) {
        self.append_message(label, elem.to_compressed_bytes().as_slice());
    }
    fn append_field_element<S: Scalar>(&mut self, label: &'static [u8], scalar: &S) {
        self.append_message(label, scalar.to_bytes().as_slice());
    }
    fn append_proof_commitment<G: Group>(&mut self, elem: &G) {
        self.append_group_element(b"proof_commitment", elem);
    }
    fn get_challenge<S: Scalar>(&mut self) -> S {
        let mut buffer = vec![0u8; 32];
        self.challenge_bytes(b"Sigma challenge", &mut buffer);
        let mut hash = sha2::Sha512::new();
        hash.update(&buffer[..]);
        S::from_hash(hash)
    }
}

fn init_sigma_protocol<G: Group>(transcript: &mut Transcript, elems: &[G]) {
    transcript.init_sigma(b"New Sigma Protocol", &[], elems);
}

fn sample_blindings<R: CryptoRng + RngCore, S: Scalar>(prng: &mut R, n: usize) -> Vec<S> {
    let mut r = vec![];
    for _ in 0..n {
        r.push(S::random(prng));
    }
    r
}

fn compute_proof_commitments<G: Group>(
    transcript: &mut Transcript,
    blindings: &[G::ScalarType],
    elems: &[G],
    lhs_matrix: &[Vec<usize>],
) -> Vec<G> {
    let mut pf_commitments = vec![];

    for row in lhs_matrix.iter() {
        let mut pf_commitment = G::get_identity();
        assert_eq!(row.len(), blindings.len());
        for (elem_index, blind) in (*row).iter().zip(blindings) {
            pf_commitment = pf_commitment.add(&elems[*elem_index].mul(blind));
        }
        transcript.append_proof_commitment(&pf_commitment);
        pf_commitments.push(pf_commitment);
    }
    pf_commitments
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// The Sigma proof.
pub struct SigmaProof<S, G> {
    pub(crate) commitments: Vec<G>,
    pub(crate) responses: Vec<S>,
}

/// Simple Sigma protocol PoK for the statement `lhs_matrix` * `secrets_scalars` = `rhs_vec`
/// Elements in `lhs_matrix` and `rhs_vec` must be in `elems` slice
pub fn sigma_prove<R: CryptoRng + RngCore, G: Group>(
    transcript: &mut Transcript,
    prng: &mut R,
    elems: &[G],               // public elements of the proofs
    lhs_matrix: &[Vec<usize>], // each row defines a lhs of a constraint
    secret_scalars: &[&G::ScalarType],
) -> SigmaProof<G::ScalarType, G> {
    init_sigma_protocol::<G>(transcript, elems);
    let blindings = sample_blindings::<_, G::ScalarType>(prng, secret_scalars.len());
    let proof_commitments =
        compute_proof_commitments::<G>(transcript, blindings.as_slice(), elems, lhs_matrix);

    let challenge = transcript.get_challenge::<G::ScalarType>();

    let mut responses = vec![];

    for (secret, blind) in secret_scalars.iter().zip(blindings.iter()) {
        responses.push(secret.mul(&challenge).add(blind))
    }
    SigmaProof {
        commitments: proof_commitments,
        responses,
    }
}

#[allow(non_snake_case)]
fn collect_multi_exp_scalars<R: CryptoRng + RngCore, S: Scalar>(
    prng: &mut R,
    n_elems: usize,        // all public group elements
    matrix: &[Vec<usize>], // matrix defining LHS of constrains
    rhs: &[usize],         // RHS of constrant
    responses: &[S],       // proof challenge responses
    challenge: &S,         // challenge
) -> Vec<S> {
    // verifier needs to check that `matrix * responses = challenge * rhs + proof_commitment`
    // rows are merged using a random linear combination
    // this functions collects the scalars factors for each element in order to apply a single
    // multi-exponentiation to verify all equations
    let mut s = vec![S::from(0u32); n_elems + rhs.len()]; // n elements + m proof commitments
    let mut alphas = vec![]; // linear combination scalars
                             // find in the matrix each element and multiply corresponding response by alpha
    for (j, row) in matrix.iter().enumerate() {
        let alpha = S::random(prng);
        for (i, s_i) in s[0..n_elems].iter_mut().enumerate() {
            for (elem_index, r) in row.iter().zip(responses) {
                if i == *elem_index {
                    *s_i = s_i.add(&alpha.mul(r))
                }
            }
        }
        s[n_elems + j] = s[n_elems + j].sub(&alpha);
        alphas.push(alpha);
    }
    for (elem_index, alpha) in rhs.iter().zip(alphas.iter()) {
        for (i, s_i) in s[0..n_elems].iter_mut().enumerate() {
            if i == *elem_index {
                *s_i = s_i.sub(&alpha.mul(challenge));
            }
        }
    }
    s
}

/// Returns a scalar vector for a sigma protocol proof verification. The scalars can then be used
/// in a single multi-exponentiation to verify the proof. The associated elements are elems
/// concatenated wit proof.commitments.
pub fn sigma_verify_scalars<R: CryptoRng + RngCore, G: Group>(
    transcript: &mut Transcript,
    prng: &mut R, //use of for linear combination multiexp
    elems: &[G],
    lhs_matrix: &[Vec<usize>],
    rhs_vec: &[usize],
    proof: &SigmaProof<G::ScalarType, G>,
) -> Vec<G::ScalarType> {
    assert_eq!(lhs_matrix.len(), rhs_vec.len());
    assert_eq!(rhs_vec.len(), proof.commitments.len());

    init_sigma_protocol::<G>(transcript, elems);
    for c in proof.commitments.iter() {
        transcript.append_proof_commitment(c);
    }
    let challenge = transcript.get_challenge::<G::ScalarType>();
    collect_multi_exp_scalars(
        prng,
        elems.len(),
        lhs_matrix,
        rhs_vec,
        &proof.responses,
        &challenge,
    )
}

/// Simple Sigma protocol PoK verification for the statement `lhs_matrix` * `secrets_scalars` = `rhs_vec`
/// Elements in `lhs_matrix` and `rhs_vec` must be in `elems` slice
pub fn sigma_verify<R: CryptoRng + RngCore, G: Group>(
    transcript: &mut Transcript,
    prng: &mut R, //use of for linear combination multiexp
    elems: &[G],
    lhs_matrix: &[Vec<usize>],
    rhs_vec: &[usize],
    proof: &SigmaProof<G::ScalarType, G>,
) -> Result<()> {
    let multi_exp_scalars =
        sigma_verify_scalars(transcript, prng, elems, lhs_matrix, rhs_vec, proof);

    let scalars_as_ref = multi_exp_scalars.iter().collect_vec();
    let mut me_elems = vec![];
    for e in elems {
        me_elems.push(e);
    }
    for e in proof.commitments.iter() {
        me_elems.push(e);
    }
    let result = G::multi_exp(scalars_as_ref.as_slice(), me_elems.as_slice());
    if result != G::get_identity() {
        Err(eg!(NoahError::ZKProofVerificationError))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use merlin::Transcript;
    use noah_algebra::{
        prelude::*,
        ristretto::{RistrettoPoint, RistrettoScalar as Scalar},
    };

    #[test]
    #[allow(non_snake_case)]
    fn test_sigma() {
        let G = RistrettoPoint::get_base();
        let secret = Scalar::from(10u32);
        let H = G.mul(&secret);

        let mut prover_transcript = Transcript::new(b"Test");
        let mut verifier_transcript = Transcript::new(b"Test");
        let mut prng = test_rng();

        //test 1 simple dlog
        let elems = [G, H];
        let lhs_matrix = vec![vec![0]];
        let rhs_vec = vec![1];
        let dlog_proof = super::sigma_prove(
            &mut prover_transcript,
            &mut prng,
            &elems,
            lhs_matrix.as_slice(),
            &[&secret],
        );
        assert!(super::sigma_verify(
            &mut verifier_transcript,
            &mut prng,
            &elems,
            lhs_matrix.as_slice(),
            rhs_vec.as_slice(),
            &dlog_proof
        )
        .is_ok());

        let bad_matrix = vec![vec![1]];
        let dlog_proof = super::sigma_prove(
            &mut prover_transcript,
            &mut prng,
            &elems,
            bad_matrix.as_slice(),
            &[&secret],
        );
        assert!(super::sigma_verify(
            &mut verifier_transcript,
            &mut prng,
            &elems,
            bad_matrix.as_slice(),
            rhs_vec.as_slice(),
            &dlog_proof
        )
        .is_err());

        // test2: two contrains, two secrets
        // 1) H = secret * G, 2) H2 = secret2 * G
        let secret2 = Scalar::from(20u32);
        let H2 = G.mul(&secret2);
        let zero = RistrettoPoint::get_identity();
        let elems = [zero, G, H, H2];
        let lhs_matrix: &[Vec<usize>] = &[vec![1, 0], vec![0, 1]];
        let rhs_vec: &[usize] = &[2, 3];
        let dlog_proof = super::sigma_prove(
            &mut prover_transcript,
            &mut prng,
            &elems,
            lhs_matrix,
            &[&secret, &secret2],
        );
        assert!(super::sigma_verify(
            &mut verifier_transcript,
            &mut prng,
            &elems,
            lhs_matrix,
            rhs_vec,
            &dlog_proof
        )
        .is_ok());

        let lhs_matrix: &[Vec<usize>] = &[vec![1, 1], vec![0, 1]]; // bad row 1
        let dlog_proof = super::sigma_prove(
            &mut prover_transcript,
            &mut prng,
            &elems,
            lhs_matrix,
            &[&secret, &secret2],
        );
        assert!(super::sigma_verify(
            &mut verifier_transcript,
            &mut prng,
            &elems,
            lhs_matrix,
            rhs_vec,
            &dlog_proof
        )
        .is_err());

        let lhs_matrix: &[Vec<usize>] = &[vec![1, 0], vec![0, 0]]; // bad row 2
        let dlog_proof = super::sigma_prove(
            &mut prover_transcript,
            &mut prng,
            &elems,
            lhs_matrix,
            &[&secret, &secret2],
        );
        assert!(super::sigma_verify(
            &mut verifier_transcript,
            &mut prng,
            &elems,
            lhs_matrix,
            rhs_vec,
            &dlog_proof
        )
        .is_err());

        // test3: two constarains, 5 secrets
        let secret3 = Scalar::from(30u32);
        let secret4 = Scalar::from(40u32);
        let secret5 = Scalar::from(50u32);
        let Z1 = G.mul(&secret).add(&H.mul(&secret2));
        let Z2 = G.mul(&secret3).add(&H.mul(&secret4)).add(&H2.mul(&secret5));

        let elems = [zero, G, H, H2, Z1, Z2];
        let matrix: &[Vec<usize>] = &[vec![1, 2, 0, 0, 0], vec![0, 0, 1, 2, 3]];
        let rhs_vec = &[4, 5];
        let secrets: &[&Scalar] = &[&secret, &secret2, &secret3, &secret4, &secret5];
        let proof = super::sigma_prove(&mut prover_transcript, &mut prng, &elems, matrix, secrets);
        assert!(super::sigma_verify(
            &mut verifier_transcript,
            &mut prng,
            &elems,
            matrix,
            rhs_vec,
            &proof
        )
        .is_ok());

        let secrets: &[&Scalar] = &[&secret, &secret2, &secret3, &secret4, &Scalar::zero()]; // bad secrets
        let proof = super::sigma_prove(&mut prover_transcript, &mut prng, &elems, matrix, secrets);
        assert!(super::sigma_verify(
            &mut verifier_transcript,
            &mut prng,
            &elems,
            matrix,
            rhs_vec,
            &proof
        )
        .is_err());
    }
}
