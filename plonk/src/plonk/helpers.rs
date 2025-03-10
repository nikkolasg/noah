use crate::plonk::{
    constraint_system::ConstraintSystem,
    errors::PlonkError,
    indexer::{PlonkPK, PlonkPf, PlonkVK},
};
use crate::poly_commit::{
    field_polynomial::FpPolynomial,
    pcs::{HomomorphicPolyComElem, PolyComScheme},
};
use ark_ff::{batch_inversion, Field};
use ark_poly::EvaluationDomain;
use noah_algebra::cfg_into_iter;
use noah_algebra::prelude::*;
use noah_algebra::{cmp::min, traits::Domain};

#[cfg(feature = "parallel")]
use rayon::{
    iter::IntoParallelIterator,
    prelude::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator},
};

/// The data structure for challenges in Plonk.
#[derive(Default)]
pub(super) struct PlonkChallenges<F> {
    challenges: Vec<F>,
}

impl<F: Scalar> PlonkChallenges<F> {
    /// Create a challenges with capacity 4.
    pub(super) fn new() -> PlonkChallenges<F> {
        PlonkChallenges {
            challenges: Vec::with_capacity(4),
        }
    }

    /// Insert beta and gamma.
    pub(super) fn insert_beta_gamma(&mut self, beta: F, gamma: F) -> Result<()> {
        if self.challenges.is_empty() {
            self.challenges.push(beta);
            self.challenges.push(gamma);
            Ok(())
        } else {
            Err(eg!())
        }
    }

    /// Insert alpha.
    pub(super) fn insert_alpha(&mut self, alpha: F) -> Result<()> {
        if self.challenges.len() == 2 {
            self.challenges.push(alpha);
            Ok(())
        } else {
            Err(eg!())
        }
    }

    /// Insert zeta.
    pub(super) fn insert_zeta(&mut self, zeta: F) -> Result<()> {
        if self.challenges.len() == 3 {
            self.challenges.push(zeta);
            Ok(())
        } else {
            Err(eg!())
        }
    }

    /// Insert u.
    pub(super) fn insert_u(&mut self, u: F) -> Result<()> {
        if self.challenges.len() == 4 {
            self.challenges.push(u);
            Ok(())
        } else {
            Err(eg!())
        }
    }

    /// Return beta and gamma.
    pub(super) fn get_beta_gamma(&self) -> Result<(&F, &F)> {
        if self.challenges.len() > 1 {
            Ok((&self.challenges[0], &self.challenges[1]))
        } else {
            Err(eg!())
        }
    }

    /// Return alpha.
    pub(super) fn get_alpha(&self) -> Result<&F> {
        if self.challenges.len() > 2 {
            Ok(&self.challenges[2])
        } else {
            Err(eg!())
        }
    }

    /// Return zeta.
    pub(super) fn get_zeta(&self) -> Result<&F> {
        if self.challenges.len() > 3 {
            Ok(&self.challenges[3])
        } else {
            Err(eg!())
        }
    }

    /// Return u.
    pub(super) fn get_u(&self) -> Result<&F> {
        if self.challenges.len() > 4 {
            Ok(&self.challenges[4])
        } else {
            Err(eg!())
        }
    }
}

/// Return the PI polynomial.
pub(super) fn pi_poly<PCS: PolyComScheme, E: EvaluationDomain<<PCS::Field as Domain>::Field>>(
    prover_params: &PlonkPK<PCS>,
    pi: &[PCS::Field],
    domain: &E,
) -> FpPolynomial<PCS::Field> {
    let mut evals = Vec::with_capacity(prover_params.verifier_params.cs_size);
    for (i, _) in prover_params.group.iter().enumerate() {
        if let Some((pos, _)) = prover_params
            .verifier_params
            .public_vars_constraint_indices
            .iter()
            .find_position(|&&x| x == i)
        {
            evals.push(pi[pos])
        } else {
            evals.push(PCS::Field::zero());
        }
    }

    FpPolynomial::ifft_with_domain(domain, &evals)
}

/// Add a random degree `num_hide_points`+`zeroing_degree` polynomial
/// that vanishes on X^{zeroing_degree} -1. Goal is to randomize
/// `polynomial` maintaining output values for elements in a sub group
/// of order N. Eg, when num_hide_points is 1, then it adds
/// (r1 + r2*X) * (X^zeroing_degree - 1) to `polynomial.
pub(super) fn hide_polynomial<R: CryptoRng + RngCore, F: Domain>(
    prng: &mut R,
    polynomial: &mut FpPolynomial<F>,
    hiding_degree: usize,
    zeroing_degree: usize,
) -> Vec<F> {
    let mut blinds = Vec::new();
    for i in 0..hiding_degree {
        let mut blind = F::random(prng);
        blinds.push(blind);
        polynomial.add_coef_assign(&blind, i);
        blind = blind.neg();
        polynomial.add_coef_assign(&blind, zeroing_degree + i);
    }
    blinds
}

/// Build the z polynomial, by interpolating
/// z(\omega^{i+1}) = z(\omega^i)\prod_{j=1}^{n_wires_per_gate}(fj(\omega^i)
/// + \beta * k_j * \omega^i +\gamma)/(fj(\omega^i) + \beta * perm_j(\omega^i) +\gamma)
/// and setting z(1) = 1 for the base case
pub(super) fn z_poly<PCS: PolyComScheme, CS: ConstraintSystem<Field = PCS::Field>>(
    prover_params: &PlonkPK<PCS>,
    w: &[PCS::Field],
    challenges: &PlonkChallenges<PCS::Field>,
) -> FpPolynomial<PCS::Field> {
    let n_wires_per_gate = CS::n_wires_per_gate();
    let (beta, gamma) = challenges.get_beta_gamma().unwrap();
    let perm = &prover_params.permutation;
    let n_constraints = w.len() / n_wires_per_gate;
    let group = &prover_params.group[..];

    // computes permutation values
    let p_of_x =
        |perm_value: usize, n: usize, group: &[PCS::Field], k: &[PCS::Field]| -> PCS::Field {
            for (i, ki) in k.iter().enumerate().skip(1) {
                if perm_value < (i + 1) * n && perm_value >= i * n {
                    return ki.mul(&group[perm_value % n]);
                }
            }
            k[0].mul(&group[perm_value])
        };

    let k = &prover_params.verifier_params.k;

    let res = cfg_into_iter!(0..n_constraints - 1)
        .map(|i| {
            // 1. numerator = prod_{j=1..n_wires_per_gate}(fj(\omega^i) + \beta * k_j * \omega^i + \gamma)
            // 2. denominator = prod_{j=1..n_wires_per_gate}(fj(\omega^i) + \beta * permj(\omega^i) +\gamma)
            let mut numerator = PCS::Field::one();
            let mut denominator = PCS::Field::one();
            for j in 0..n_wires_per_gate {
                let k_x = k[j].mul(&group[i]);
                let f_x = &w[j * n_constraints + i];
                let f_plus_beta_id_plus_gamma = &f_x.add(gamma).add(&beta.mul(&k_x));
                numerator.mul_assign(&f_plus_beta_id_plus_gamma);

                let p_x = p_of_x(perm[j * n_constraints + i], n_constraints, group, k);
                let f_plus_beta_perm_plus_gamma = f_x.add(gamma).add(&beta.mul(&p_x));
                denominator.mul_assign(&f_plus_beta_perm_plus_gamma);
            }

            (numerator, denominator)
        })
        .collect::<Vec<(PCS::Field, PCS::Field)>>();

    let (numerators, denominators): (Vec<PCS::Field>, Vec<PCS::Field>) =
        res.iter().cloned().unzip();

    let mut denominators = denominators
        .iter()
        .map(|x| x.get_field())
        .collect::<Vec<<PCS::Field as Domain>::Field>>();
    batch_inversion(&mut denominators);

    let mut prev = PCS::Field::one();
    let mut z_evals = vec![];
    z_evals.push(prev);
    for (x, y) in denominators.iter().zip(numerators.iter()) {
        let x = <PCS::Field as Domain>::from_field(*x);
        prev.mul_assign(&y.mul(&x));
        z_evals.push(prev);
    }

    // interpolate the polynomial
    FpPolynomial::from_coefs(z_evals)
}

/// Compute the t polynomial.
pub(super) fn t_poly<PCS: PolyComScheme, CS: ConstraintSystem<Field = PCS::Field>>(
    cs: &CS,
    prover_params: &PlonkPK<PCS>,
    w_polys: &[FpPolynomial<PCS::Field>],
    z: &FpPolynomial<PCS::Field>,
    challenges: &PlonkChallenges<PCS::Field>,
    pi: &FpPolynomial<PCS::Field>,
) -> Result<FpPolynomial<PCS::Field>> {
    let n = cs.size();
    let m = cs.quot_eval_dom_size();
    let factor = m / n;
    if n * factor != m {
        return Err(eg!(PlonkError::SetupError));
    }

    let domain_m = FpPolynomial::<PCS::Field>::quotient_evaluation_domain(m)
        .c(d!(PlonkError::GroupNotFound(n)))?;
    let k = &prover_params.verifier_params.k;

    let mut z_h_inv_coset_evals: Vec<<PCS::Field as Domain>::Field> = Vec::with_capacity(factor);
    let group_gen_pow_n = domain_m.group_gen.pow(&[n as u64]);
    let mut multiplier = k[1].get_field().pow(&[n as u64]);
    for _ in 0..factor {
        let eval = multiplier.sub(&<PCS::Field as Domain>::Field::one());
        z_h_inv_coset_evals.push(eval);
        multiplier.mul_assign(&group_gen_pow_n);
    }
    batch_inversion(&mut z_h_inv_coset_evals);
    let z_h_inv_coset_evals = z_h_inv_coset_evals
        .iter()
        .map(|x| PCS::Field::from_field(*x))
        .collect::<Vec<_>>();

    // Compute the evaluations of w/pi/z polynomials on the coset k[1] * <root_m>.
    let w_polys_coset_evals: Vec<Vec<PCS::Field>> = w_polys
        .iter()
        .map(|poly| poly.coset_fft_with_domain(&domain_m, &k[1]))
        .collect();
    let pi_coset_evals = pi.coset_fft_with_domain(&domain_m, &k[1]);
    let z_coset_evals = z.coset_fft_with_domain(&domain_m, &k[1]);

    // Compute the evaluations of the quotient polynomial on the coset.
    let (beta, gamma) = challenges.get_beta_gamma().unwrap();

    let alpha = challenges.get_alpha().unwrap();
    let alpha_pow_2 = alpha.mul(alpha);
    let alpha_pow_3 = alpha_pow_2.mul(alpha);
    let alpha_pow_4 = alpha_pow_3.mul(alpha);
    let alpha_pow_5 = alpha_pow_4.mul(alpha);
    let alpha_pow_6 = alpha_pow_5.mul(alpha);
    let alpha_pow_7 = alpha_pow_6.mul(alpha);
    let alpha_pow_8 = alpha_pow_7.mul(alpha);
    let alpha_pow_9 = alpha_pow_8.mul(alpha);

    let t_coset_evals = cfg_into_iter!(0..m)
        .map(|point| {
            let w_vals: Vec<&PCS::Field> = w_polys_coset_evals
                .iter()
                .map(|poly_coset_evals| &poly_coset_evals[point])
                .collect();
            let q_vals: Vec<&PCS::Field> = prover_params
                .q_coset_evals
                .iter()
                .map(|poly_coset_evals| &poly_coset_evals[point])
                .collect();
            // q * w
            let term1 = CS::eval_gate_func(&w_vals, &q_vals, &pi_coset_evals[point]).unwrap();

            // alpha * [z(X)\prod_j (fj(X) + beta * kj * X + gamma)]
            let mut term2 = alpha.mul(&z_coset_evals[point]);
            for j in 0..CS::n_wires_per_gate() {
                let tmp = w_polys_coset_evals[j][point]
                    .add(gamma)
                    .add(&beta.mul(&k[j].mul(&prover_params.coset_quotient[point])));
                term2.mul_assign(&tmp);
            } // alpha * [z(\omega * X)\prod_j (fj(X) + beta * perm_j(X) + gamma)]
            let mut term3 = alpha.mul(&z_coset_evals[(point + factor) % m]);
            for (w_poly_coset_evals, s_coset_evals) in w_polys_coset_evals
                .iter()
                .zip(prover_params.s_coset_evals.iter())
            {
                let tmp = &w_poly_coset_evals[point]
                    .add(gamma)
                    .add(&beta.mul(&s_coset_evals[point]));
                term3.mul_assign(&tmp);
            }

            // alpha^2 * (z(X) - 1) * L_1(X)
            let term4 = alpha_pow_2
                .mul(&prover_params.l1_coset_evals[point])
                .mul(&z_coset_evals[point].sub(&PCS::Field::one()));

            let qb_eval_point = prover_params.qb_coset_eval[point];

            // alpha^3 * qb(X) (w[1] (w[1] - 1))
            let w1_eval_point = w_polys_coset_evals[1][point];
            let term5 = alpha_pow_3
                .mul(&qb_eval_point)
                .mul(&w1_eval_point)
                .mul(&w1_eval_point.sub(&PCS::Field::one()));

            // alpha^4 * qb(X) (w[2] (w[2] - 1))
            let w2_eval_point = w_polys_coset_evals[2][point];
            let term6 = alpha_pow_4
                .mul(&qb_eval_point)
                .mul(&w2_eval_point)
                .mul(&w2_eval_point.sub(&PCS::Field::one()));

            // alpha^5 * qb(X) (w[3] (w[3] - 1))
            let w3_eval_point = w_polys_coset_evals[3][point];
            let term7 = alpha_pow_5
                .mul(&qb_eval_point)
                .mul(&w3_eval_point)
                .mul(&w3_eval_point.sub(&PCS::Field::one()));

            let w0_eval_point = w_polys_coset_evals[0][point];
            let wo_eval_point = w_polys_coset_evals[4][point];
            let w0_eval_point_next = w_polys_coset_evals[0][(point + factor) % m];
            let w1_eval_point_next = w_polys_coset_evals[1][(point + factor) % m];
            let w2_eval_point_next = w_polys_coset_evals[2][(point + factor) % m];
            let q_prk1_eval_point = prover_params.q_prk_coset_evals[0][point];
            let q_prk2_eval_point = prover_params.q_prk_coset_evals[1][point];
            let q_prk3_eval_point = prover_params.q_prk_coset_evals[2][point];
            let q_prk4_eval_point = prover_params.q_prk_coset_evals[3][point];
            let g = prover_params.verifier_params.anemoi_generator;
            let g_square_plus_one = g.square().add(PCS::Field::one());
            let g_inv = prover_params.verifier_params.anemoi_generator_inv;
            let five = &[5u64];

            let tmp = w3_eval_point + &(g * &w2_eval_point) + &q_prk3_eval_point;

            // - alpha^6 * q_{prk3} *
            //  (
            //    (w[3] + g * w[2] + q_{prk3} - w_next[2]) ^ 5
            //    + g * (w[3] + g * w[2] + q_{prk3}) ^ 2
            //    - (w[0] + g * w[1] + q_{prk1})
            //  )
            let term8 = alpha_pow_6.mul(&q_prk3_eval_point).mul(
                (tmp - &w2_eval_point_next).pow(five) + &(g * tmp.square())
                    - &(w0_eval_point + g * w1_eval_point + &q_prk1_eval_point),
            );
            // - alpha^8 * q_{prk3} *
            //  (
            //    (w[3] + g * w[2] + q_{prk3} - w_next[2]) ^ 5
            //    + g * w_next[2] ^ 2 + g^-1
            //    - w_next[0]
            //  )
            let term10 = alpha_pow_8.mul(&q_prk3_eval_point).mul(
                (tmp - &w2_eval_point_next).pow(five) + &(g * w2_eval_point_next.square()) + g_inv
                    - &w0_eval_point_next,
            );

            // - alpha^7 * q_{prk3} *
            //  (
            //    (g * w[3] + (g^2 + 1) * w[2] + q_{prk4} - w[4]) ^ 5
            //    + g * (g * w[3] + (g^2 + 1) * w[2] + q_{prk4}) ^ 2
            //    - (g * w[0] + (g^2 + 1) * w[1] + q_{prk2})
            //  )
            let tmp =
                g * &w3_eval_point + &(g_square_plus_one * &w2_eval_point) + &q_prk4_eval_point;
            let term9 = alpha_pow_7.mul(&q_prk3_eval_point).mul(
                (tmp - &wo_eval_point).pow(five) + &(g * tmp.square())
                    - &(g * &w0_eval_point
                        + g_square_plus_one * w1_eval_point
                        + &q_prk2_eval_point),
            );

            // - alpha^9 * q_{prk3} *
            //  (
            //    (g * w[3] + (g^2 + 1) * w[2] + q_{prk4} - w[4]) ^ 5
            //    + g * w[4] ^ 2 + g^-1
            //    - w_next[1]
            //  )
            let term11 = alpha_pow_9.mul(&q_prk3_eval_point).mul(
                (tmp - &wo_eval_point).pow(five) + &(g * wo_eval_point.square()) + g_inv
                    - &w1_eval_point_next,
            );

            let numerator = term1
                .add(&term2)
                .add(&term4.sub(&term3))
                .add(&term5)
                .add(&term6)
                .add(&term7)
                .sub(&term8)
                .sub(&term9)
                .sub(&term10)
                .sub(&term11);
            numerator.mul(&z_h_inv_coset_evals[point % factor])
        })
        .collect::<Vec<PCS::Field>>();

    let k_inv = k[1].inv().c(d!(PlonkError::DivisionByZero))?;

    Ok(FpPolynomial::coset_ifft_with_domain(
        &domain_m,
        &t_coset_evals,
        &k_inv,
    ))
}

/// Compute r polynomial or commitment.
#[cfg(not(feature = "parallel"))]
fn r_poly_or_comm<F: Scalar, PCSType: HomomorphicPolyComElem<Scalar = F>>(
    w: &[F],
    q_polys_or_comms: &[PCSType],
    qb_poly_or_comm: &PCSType,
    q_prk1_poly_or_comm: &PCSType,
    q_prk2_poly_or_comm: &PCSType,
    k: &[F],
    last_s_poly_or_comm: &PCSType,
    z_poly_or_comm: &PCSType,
    w_polys_eval_zeta: &[&F],
    s_polys_eval_zeta: &[&F],
    q_prk3_eval_zeta: &F,
    z_eval_zeta_omega: &F,
    challenges: &PlonkChallenges<F>,
    t_polys_or_comms: &[PCSType],
    first_lagrange_eval_zeta: &F,
    z_h_eval_zeta: &F,
    n_t_polys: usize,
) -> PCSType {
    let (beta, gamma) = challenges.get_beta_gamma().unwrap();
    let alpha = challenges.get_alpha().unwrap();
    let zeta = challenges.get_zeta().unwrap();

    let alpha_pow_2 = alpha.mul(alpha);
    let alpha_pow_3 = alpha_pow_2.mul(alpha);
    let alpha_pow_4 = alpha_pow_3.mul(alpha);
    let alpha_pow_5 = alpha_pow_4.mul(alpha);
    let alpha_pow_6 = alpha_pow_5.mul(alpha);
    let alpha_pow_7 = alpha_pow_6.mul(alpha);

    // 1. sum_{i=1..n_selectors} wi * qi(X)
    let mut l = q_polys_or_comms[0].mul(&w[0]);
    for i in 1..q_polys_or_comms.len() {
        l.add_assign(&q_polys_or_comms[i].mul(&w[i]));
    }

    // 2. z(X) [ alpha * prod_{j=1..n_wires_per_gate} (fj(zeta) + beta * kj * zeta + gamma)
    //              + alpha^2 * L1(zeta)]
    let z_scalar =
        compute_z_scalar_in_r(w_polys_eval_zeta, k, challenges, first_lagrange_eval_zeta);
    l.add_assign(&z_poly_or_comm.mul(&z_scalar));

    // 3. - perm_{n_wires_per_gate}(X) [alpha * z(zeta * omega) * beta
    //    * prod_{j=1..n_wires_per_gate-1}(fj(zeta) + beta * perm_j(zeta) + gamma)]
    let mut s_last_poly_scalar = alpha.mul(&z_eval_zeta_omega.mul(beta));
    for i in 0..w_polys_eval_zeta.len() - 1 {
        let tmp = w_polys_eval_zeta[i]
            .add(&beta.mul(s_polys_eval_zeta[i]))
            .add(gamma);
        s_last_poly_scalar.mul_assign(&tmp);
    }
    l.sub_assign(&last_s_poly_or_comm.mul(&s_last_poly_scalar));

    // 4. + qb(X) * (w[1] (w[1] - 1) * alpha^3 + w[2] (w[2] - 1) * alpha^4 + w[3] (w[3] - 1) * alpha^5)
    let w1_part = w[1].mul(&(w[1] - &F::one())).mul(&alpha_pow_3);
    let w2_part = w[2].mul(&(w[2] - &F::one())).mul(&alpha_pow_4);
    let w3_part = w[3].mul(&(w[3] - &F::one())).mul(&alpha_pow_5);
    l.add_assign(&qb_poly_or_comm.mul(&w1_part.add(w2_part).add(w3_part)));

    // 5. + q_{prk3}(eval zeta) * (q_{prk1}(X) * alpha^6 + q_{prk2}(X) * alpha ^ 7)
    l.add_assign(&q_prk1_poly_or_comm.mul(&q_prk3_eval_zeta.mul(alpha_pow_6)));
    l.add_assign(&q_prk2_poly_or_comm.mul(&q_prk3_eval_zeta.mul(alpha_pow_7)));

    let factor = zeta.pow(&[n_t_polys as u64]);
    let mut exponent = z_h_eval_zeta.mul(factor);
    let mut t_poly_combined = t_polys_or_comms[0].clone().mul(&z_h_eval_zeta);
    for t_poly in t_polys_or_comms.iter().skip(1) {
        t_poly_combined.add_assign(&t_poly.mul(&exponent));
        exponent.mul_assign(&factor);
    }
    l.sub_assign(&t_poly_combined);
    l
}

/// Compute r polynomial or commitment.
#[cfg(feature = "parallel")]
fn r_poly_or_comm<F: Scalar, PCSType: HomomorphicPolyComElem<Scalar = F>>(
    w: &[F],
    q_polys_or_comms: &[PCSType],
    qb_poly_or_comm: &PCSType,
    q_prk1_poly_or_comm: &PCSType,
    q_prk2_poly_or_comm: &PCSType,
    k: &[F],
    last_s_poly_or_comm: &PCSType,
    z_poly_or_comm: &PCSType,
    w_polys_eval_zeta: &[&F],
    s_polys_eval_zeta: &[&F],
    q_prk3_eval_zeta: &F,
    z_eval_zeta_omega: &F,
    challenges: &PlonkChallenges<F>,
    t_polys_or_comms: &[PCSType],
    first_lagrange_eval_zeta: &F,
    z_h_eval_zeta: &F,
    n_t_polys: usize,
) -> PCSType {
    let (beta, gamma) = challenges.get_beta_gamma().unwrap();
    let zeta = challenges.get_zeta().unwrap();
    let alpha = challenges.get_alpha().unwrap();
    let alpha_neg = alpha.neg();
    let beta_zeta = beta.mul(zeta);
    let one = F::one();
    let zero = F::zero();
    let z_h_eval_zeta_neg = z_h_eval_zeta.neg();

    let alpha_pow_2 = alpha.mul(alpha);
    let alpha_pow_3 = alpha_pow_2.mul(alpha);
    let alpha_pow_4 = alpha_pow_3.mul(alpha);
    let alpha_pow_5 = alpha_pow_4.mul(alpha);
    let alpha_pow_6 = alpha_pow_5.mul(alpha);
    let alpha_pow = vec![&zero, &alpha_pow_3, &alpha_pow_4, &alpha_pow_5];

    let mut polys_or_comms = q_polys_or_comms.iter().collect::<Vec<&PCSType>>();
    let mut challenges = w.iter().collect::<Vec<&F>>();

    // res.0 = prod_{j=1..n_wires_per_gate-1} (wj(zeta) + beta * kj * zeta + gamma)
    // res.1 = prod_{j=1..n_wires_per_gate-1} (wj(zeta) + beta * perm_j(zeta) + gamma)
    // res.2 = prod_{j=2..n_wires_per_gate-1} (wj(zeta) * (wj(zeta)-1) * alpha ^ j)
    let mut res = w_polys_eval_zeta
        .par_iter()
        .take(w_polys_eval_zeta.len() - 1)
        .zip(k)
        .zip(s_polys_eval_zeta)
        .zip(alpha_pow)
        .map(|(((wj, kj), sj), alpha_pow)| {
            let term1 = wj.add(kj.mul(&beta_zeta)).add(gamma);
            let term2 = wj.add(beta.mul(*sj)).add(gamma);
            let term3 = wj.mul(alpha_pow).mul(wj.sub(&one));

            (term1, term2, term3)
        })
        .reduce(
            || (one, one, zero),
            |x, y| ((x.0.mul(&y.0)), (x.1.mul(&y.1)), (x.2.add(&y.2))),
        );

    // res.0 * (w_{n_wires_per_gate}(zeta) + beta * k_{n_wires_per_gate} * zeta + gamma)
    //  = prod_{j=1..n_wires_per_gate} (wj(zeta) + beta * kj * zeta + gamma)
    res.0.mul_assign(
        &w_polys_eval_zeta[w_polys_eval_zeta.len() - 1]
            .add(k[k.len() - 1].mul(&beta_zeta))
            .add(gamma),
    );

    // (res.0 + (L1(zeta) * alpha)) * alpha * z(x)
    //  = res.0 * alpha * z(x) + L1(zeta) * alpha ^ 2 * z(x)
    res.0.add_assign(&first_lagrange_eval_zeta.mul(alpha));
    res.0.mul_assign(alpha);
    polys_or_comms.push(&z_poly_or_comm);
    challenges.push(&res.0);

    // res.1 * z(zeta * omega) * beta * perm_{n_wires_per_gate}(X)
    polys_or_comms.push(last_s_poly_or_comm);
    res.1
        .mul_assign(&z_eval_zeta_omega.mul(beta).mul(&alpha_neg));
    challenges.push(&res.1);

    // res.2 * qb(X)
    polys_or_comms.push(&qb_poly_or_comm);
    challenges.push(&res.2);

    // q_{prk1}(X) * q_{prk3}(eval zeta) * alpha ^ 6
    polys_or_comms.push(&q_prk1_poly_or_comm);
    let q_prk3_pow_6 = q_prk3_eval_zeta.mul(alpha_pow_6);
    challenges.push(&q_prk3_pow_6);

    // q_{prk2}(X) * q_{prk3}(eval zeta) * alpha ^ 7
    polys_or_comms.push(&q_prk2_poly_or_comm);
    let q_prk3_pow_7 = q_prk3_pow_6.mul(alpha);
    challenges.push(&q_prk3_pow_7);

    // - z_h(zeta) * t_0(x) - \sum_{j=1..t_polys_or_comms.len()-1} (t_j(x) * (zeta) ^ (n_t_polys * j) * z_h(zeta))
    let mut exponents = Vec::new();
    exponents.push(z_h_eval_zeta_neg);
    let factor = zeta.pow(&[n_t_polys as u64]);
    let mut exponent = factor.mul(&z_h_eval_zeta_neg);
    for _ in 0..t_polys_or_comms.len() - 1 {
        exponents.push(exponent);
        exponent.mul_assign(&factor);
    }
    for (t_poly_or_comm, exp) in t_polys_or_comms.iter().zip(&exponents) {
        polys_or_comms.push(t_poly_or_comm);
        challenges.push(exp);
    }

    // sum_{j=0..polys_or_comms.len()} (polys_or_comms[j] * challenges[j])
    polys_or_comms
        .par_iter()
        .zip(challenges)
        .map(|(polys_or_comm, challenge)| polys_or_comm.mul(challenge))
        .reduce(|| PCSType::default(), |x, y| x.add(&y))
}

/// compute the scalar factor of z(X) in the r poly.
/// prod(fi(\zeta) + \beta * k_i * \zeta + \gamma) * \alpha
///       + (\zeta^n - 1) / (\zeta-1) * \alpha^2
#[cfg(not(feature = "parallel"))]
fn compute_z_scalar_in_r<F: Scalar>(
    w_polys_eval_zeta: &[&F],
    k: &[F],
    challenges: &PlonkChallenges<F>,
    first_lagrange_eval_zeta: &F,
) -> F {
    let n_wires_per_gate = w_polys_eval_zeta.len();
    let (beta, gamma) = challenges.get_beta_gamma().unwrap();
    let alpha = challenges.get_alpha().unwrap();
    let alpha_square = alpha.mul(alpha);
    let zeta = challenges.get_zeta().unwrap();

    // 1. alpha * prod_{i=1..n_wires_per_gate}(fi(\zeta) + \beta * k_i * \zeta + \gamma)
    let beta_zeta = beta.mul(zeta);
    let mut z_scalar = *alpha;
    for i in 0..n_wires_per_gate {
        let tmp = w_polys_eval_zeta[i].add(&k[i].mul(&beta_zeta)).add(gamma);
        z_scalar.mul_assign(&tmp);
    }

    // 2. alpha^2 * (beta^n - 1) / (beta - 1)
    z_scalar.add_assign(&first_lagrange_eval_zeta.mul(alpha_square));
    z_scalar
}

/// Compute the r polynomial.
pub(super) fn r_poly<PCS: PolyComScheme, CS: ConstraintSystem<Field = PCS::Field>>(
    prover_params: &PlonkPK<PCS>,
    z: &FpPolynomial<PCS::Field>,
    w_polys_eval_zeta: &[&PCS::Field],
    s_polys_eval_zeta: &[&PCS::Field],
    q_prk3_eval_zeta: &PCS::Field,
    z_eval_zeta_omega: &PCS::Field,
    challenges: &PlonkChallenges<PCS::Field>,
    t_polys: &[FpPolynomial<PCS::Field>],
    first_lagrange_eval_zeta: &PCS::Field,
    z_h_eval_zeta: &PCS::Field,
    n_t_polys: usize,
) -> FpPolynomial<PCS::Field> {
    let w = CS::eval_selector_multipliers(w_polys_eval_zeta).unwrap(); // safe unwrap
    r_poly_or_comm::<PCS::Field, FpPolynomial<PCS::Field>>(
        &w,
        &prover_params.q_polys,
        &prover_params.qb_poly,
        &prover_params.q_prk_polys[0],
        &prover_params.q_prk_polys[1],
        &prover_params.verifier_params.k,
        &prover_params.s_polys[CS::n_wires_per_gate() - 1],
        z,
        w_polys_eval_zeta,
        s_polys_eval_zeta,
        q_prk3_eval_zeta,
        z_eval_zeta_omega,
        challenges,
        t_polys,
        first_lagrange_eval_zeta,
        z_h_eval_zeta,
        n_t_polys,
    )
}

/// Commit the r commitment.
pub(super) fn r_commitment<PCS: PolyComScheme, CS: ConstraintSystem<Field = PCS::Field>>(
    verifier_params: &PlonkVK<PCS>,
    cm_z: &PCS::Commitment,
    w_polys_eval_zeta: &[&PCS::Field],
    s_polys_eval_zeta: &[&PCS::Field],
    q_prk3_eval_zeta: &PCS::Field,
    z_eval_zeta_omega: &PCS::Field,
    challenges: &PlonkChallenges<PCS::Field>,
    t_polys: &[PCS::Commitment],
    first_lagrange_eval_zeta: &PCS::Field,
    z_h_eval_zeta: &PCS::Field,
    n_t_polys: usize,
) -> PCS::Commitment {
    let w = CS::eval_selector_multipliers(w_polys_eval_zeta).unwrap(); // safe unwrap
    r_poly_or_comm::<PCS::Field, PCS::Commitment>(
        &w,
        &verifier_params.cm_q_vec,
        &verifier_params.cm_qb,
        &verifier_params.cm_prk_vec[0],
        &verifier_params.cm_prk_vec[1],
        &verifier_params.k,
        &verifier_params.cm_s_vec[CS::n_wires_per_gate() - 1],
        cm_z,
        w_polys_eval_zeta,
        s_polys_eval_zeta,
        q_prk3_eval_zeta,
        z_eval_zeta_omega,
        challenges,
        t_polys,
        first_lagrange_eval_zeta,
        z_h_eval_zeta,
        n_t_polys,
    )
}

/// Compute sum_{i=1}^\ell w_i L_j(X), where j is the constraint
/// index for the i-th public value. L_j(X) = (X^n-1) / (X - \omega^j) is
/// the j-th lagrange base (zero for every X = \omega^i, except when i == j)
#[cfg(not(feature = "parallel"))]
pub(super) fn eval_pi_poly<PCS: PolyComScheme>(
    verifier_params: &PlonkVK<PCS>,
    public_inputs: &[PCS::Field],
    z_h_eval_zeta: &PCS::Field,
    eval_point: &PCS::Field,
    root: &PCS::Field,
) -> PCS::Field {
    let mut eval = PCS::Field::zero();

    for ((constraint_index, public_value), lagrange_constant) in verifier_params
        .public_vars_constraint_indices
        .iter()
        .zip(public_inputs)
        .zip(verifier_params.lagrange_constants.iter())
    {
        // X - \omega^j j-th Lagrange denominator
        let root_to_j = root.pow(&[*constraint_index as u64]);
        let denominator = eval_point.sub(&root_to_j);
        let denominator_inv = denominator.inv().unwrap();
        let lagrange_i = lagrange_constant.mul(&denominator_inv);
        eval.add_assign(&lagrange_i.mul(public_value));
    }

    eval.mul(z_h_eval_zeta)
}

/// Compute sum_{i=1}^\ell w_i L_j(X), where j is the constraint
/// index for the i-th public value. L_j(X) = (X^n-1) / (X - \omega^j) is
/// the j-th lagrange base (zero for every X = \omega^i, except when i == j)
#[cfg(feature = "parallel")]
pub(super) fn eval_pi_poly<PCS: PolyComScheme>(
    verifier_params: &PlonkVK<PCS>,
    public_inputs: &[PCS::Field],
    z_h_eval_zeta: &PCS::Field,
    eval_point: &PCS::Field,
    root: &PCS::Field,
) -> PCS::Field {
    verifier_params
        .public_vars_constraint_indices
        .par_iter()
        .zip(public_inputs)
        .zip(&verifier_params.lagrange_constants)
        .map(|((constraint_index, public_value), lagrange_constant)| {
            let root_to_j = root.pow(&[*constraint_index as u64]);
            let denominator = eval_point.sub(&root_to_j);
            let denominator_inv = denominator.inv().unwrap();
            let lagrange_i = lagrange_constant.mul(&denominator_inv);
            lagrange_i.mul(public_value)
        })
        .reduce(|| PCS::Field::zero(), |x, y| x.add(y))
        .mul(z_h_eval_zeta)
}

/// Compute constant c_j such that 1 = c_j * prod_{i != j} (\omega^j - \omega^i).
/// In such case, j-th lagrange base can be represented
/// by L_j(X) = c_j (X^n-1) / (X- \omega^j)
pub(super) fn compute_lagrange_constant<F: Scalar>(group: &[F], base_index: usize) -> F {
    let mut constant_inv = F::one();
    for (i, elem) in group.iter().enumerate() {
        if i == base_index {
            continue;
        }
        constant_inv.mul_assign(&group[base_index].sub(elem));
    }
    constant_inv.inv().unwrap()
}

/// Evaluate the r polynomial at point \zeta.
pub(super) fn r_eval_zeta<PCS: PolyComScheme>(
    proof: &PlonkPf<PCS>,
    challenges: &PlonkChallenges<PCS::Field>,
    pi_eval_zeta: &PCS::Field,
    first_lagrange_eval_zeta: &PCS::Field,
    anemoi_generator: PCS::Field,
    anemoi_generator_inv: PCS::Field,
) -> PCS::Field {
    let alpha = challenges.get_alpha().unwrap();
    let alpha_pow_2 = alpha.mul(alpha);
    let alpha_pow_3 = alpha_pow_2.mul(alpha);
    let alpha_pow_4 = alpha_pow_3.mul(alpha);
    let alpha_pow_5 = alpha_pow_4.mul(alpha);
    let alpha_pow_6 = alpha_pow_5.mul(alpha);
    let alpha_pow_7 = alpha_pow_6.mul(alpha);
    let alpha_pow_8 = alpha_pow_7.mul(alpha);
    let alpha_pow_9 = alpha_pow_8.mul(alpha);

    let (beta, gamma) = challenges.get_beta_gamma().unwrap();

    let term0 = pi_eval_zeta;
    let mut term1 = alpha.mul(&proof.z_eval_zeta_omega);
    let n_wires_per_gate = &proof.w_polys_eval_zeta.len();
    for i in 0..n_wires_per_gate - 1 {
        let b = proof.w_polys_eval_zeta[i]
            .add(&beta.mul(&proof.s_polys_eval_zeta[i]))
            .add(gamma);
        term1.mul_assign(&b);
    }
    term1.mul_assign(&proof.w_polys_eval_zeta[n_wires_per_gate - 1].add(gamma));

    let term2 = first_lagrange_eval_zeta.mul(alpha_pow_2);

    let five = &[5u64];
    let tmp = proof.w_polys_eval_zeta[3]
        + &(anemoi_generator * &proof.w_polys_eval_zeta[2])
        + &proof.prk_3_poly_eval_zeta;
    let term3 = alpha_pow_6.mul(&proof.prk_3_poly_eval_zeta).mul(
        (tmp - &proof.w_polys_eval_zeta_omega[2]).pow(five) + anemoi_generator * &tmp.square()
            - &(proof.w_polys_eval_zeta[0] + &(anemoi_generator * &proof.w_polys_eval_zeta[1])),
    );
    let term5 = alpha_pow_8.mul(&proof.prk_3_poly_eval_zeta).mul(
        (tmp - &proof.w_polys_eval_zeta_omega[2]).pow(five)
            + anemoi_generator * &proof.w_polys_eval_zeta_omega[2].square()
            + anemoi_generator_inv
            - &proof.w_polys_eval_zeta_omega[0],
    );

    let anemoi_generator_square_plus_one = anemoi_generator.square().add(PCS::Field::one());
    let tmp = anemoi_generator * &proof.w_polys_eval_zeta[3]
        + &(anemoi_generator_square_plus_one * &proof.w_polys_eval_zeta[2])
        + &proof.prk_4_poly_eval_zeta;
    let term4 = alpha_pow_7.mul(&proof.prk_3_poly_eval_zeta).mul(
        (tmp - &proof.w_polys_eval_zeta[4]).pow(five) + anemoi_generator * &tmp.square()
            - &(anemoi_generator * &proof.w_polys_eval_zeta[0]
                + &(anemoi_generator_square_plus_one * &proof.w_polys_eval_zeta[1])),
    );
    let term6 = alpha_pow_9.mul(&proof.prk_3_poly_eval_zeta).mul(
        (tmp - &proof.w_polys_eval_zeta[4]).pow(five)
            + anemoi_generator * &proof.w_polys_eval_zeta[4].square()
            + anemoi_generator_inv
            - &proof.w_polys_eval_zeta_omega[1],
    );

    let term1_plus_term2 = term1.add(&term2);
    term1_plus_term2
        .sub(&term0)
        .add(&term3)
        .add(&term4)
        .add(&term5)
        .add(&term6)
}

/// Split the t polynomial into `n_wires_per_gate` degree-`n` polynomials and commit.
pub(crate) fn split_t_and_commit<R: CryptoRng + RngCore, PCS: PolyComScheme>(
    prng: &mut R,
    pcs: &PCS,
    lagrange_pcs: Option<&PCS>,
    t: &FpPolynomial<PCS::Field>,
    n_wires_per_gate: usize,
    n: usize,
) -> Result<(Vec<PCS::Commitment>, Vec<FpPolynomial<PCS::Field>>)> {
    let mut cm_t_vec = vec![];
    let mut t_polys = vec![];
    let coefs_len = t.get_coefs_ref().len();

    let zero = PCS::Field::zero();
    let mut prev_coef = zero;

    for i in 0..n_wires_per_gate {
        let coefs_start = i * n;
        let coefs_end = if i == n_wires_per_gate - 1 {
            coefs_len
        } else {
            (i + 1) * n
        };

        let mut coefs = if coefs_start < coefs_len {
            t.get_coefs_ref()[coefs_start..min(coefs_len, coefs_end)].to_vec()
        } else {
            vec![]
        };

        let rand = PCS::Field::random(prng);
        if i != n_wires_per_gate - 1 {
            coefs.resize(n + 1, zero);
            coefs[n].add_assign(&rand);
            coefs[0].sub_assign(&prev_coef);
        } else {
            if coefs.len() == 0 {
                coefs = vec![prev_coef.neg()];
            } else {
                coefs[0].sub_assign(&prev_coef);
            }
        }
        prev_coef = rand;

        let (cm_t, t_poly) = if let Some(lagrange_pcs) = lagrange_pcs {
            let degree = coefs.len();
            let mut max_power_of_2 = degree;
            for i in (0..=degree).rev() {
                if (i & (i - 1)) == 0 {
                    max_power_of_2 = i;
                    break;
                }
            }

            let mut blinds = vec![];
            for i in &coefs[max_power_of_2..] {
                blinds.push(i.neg());
            }

            let mut new_coefs = coefs[..max_power_of_2].to_vec();
            for (i, v) in blinds.iter().enumerate() {
                new_coefs[i] = new_coefs[i] - v;
            }

            let sub_q = FpPolynomial::from_coefs(new_coefs);
            let q_eval = FpPolynomial::fft(&sub_q, max_power_of_2).c(d!())?;
            let q_eval = FpPolynomial::from_coefs(q_eval);

            let cm = lagrange_pcs.commit(&q_eval).c(d!())?;
            let cm_t = pcs.apply_blind_factors(&cm, &blinds, max_power_of_2);
            (cm_t, FpPolynomial::from_coefs(coefs))
        } else {
            let t_poly = FpPolynomial::from_coefs(coefs);
            let cm_t = pcs.commit(&t_poly).c(d!(PlonkError::CommitmentError))?;
            (cm_t, t_poly)
        };

        cm_t_vec.push(cm_t);
        t_polys.push(t_poly);
    }

    Ok((cm_t_vec, t_polys))
}

/// for a evaluation domain H, when x = 1, L_1(x) = (x^n-1) / (x-1) != 0,
/// when x = a and a \in H different from 1, L_1(x) = 0.
pub(super) fn first_lagrange_poly<PCS: PolyComScheme>(
    challenges: &PlonkChallenges<PCS::Field>,
    group_order: u64,
) -> (PCS::Field, PCS::Field) {
    let zeta = challenges.get_zeta().unwrap();
    let one = PCS::Field::one();
    let zeta_n = zeta.pow(&[group_order]);
    let z_h_eval_zeta = zeta_n.sub(&one);
    let zeta_minus_one = zeta.sub(&one);
    let l1_eval_zeta = z_h_eval_zeta.mul(zeta_minus_one.inv().unwrap());
    (z_h_eval_zeta, l1_eval_zeta)
}
#[cfg(test)]
mod test {
    use crate::plonk::{
        constraint_system::TurboCS,
        helpers::{z_poly, PlonkChallenges},
        indexer::indexer,
    };
    use crate::poly_commit::kzg_poly_com::{KZGCommitmentScheme, KZGCommitmentSchemeBLS};
    use noah_algebra::{bls12_381::BLSScalar, prelude::*};

    type F = BLSScalar;

    #[test]
    fn test_z_polynomial() {
        let mut cs = TurboCS::new();

        let zero = F::zero();
        let one = F::one();
        let two = one.add(&one);
        let three = two.add(&one);
        let four = three.add(&one);
        let five = four.add(&one);
        let six = five.add(&one);
        let seven = six.add(&one);

        let witness = [one, three, five, four, two, two, seven, six];
        cs.add_variables(&witness);

        cs.insert_add_gate(0 + 2, 4 + 2, 1 + 2);
        cs.insert_add_gate(1 + 2, 4 + 2, 2 + 2);
        cs.insert_add_gate(2 + 2, 4 + 2, 6 + 2);
        cs.insert_add_gate(3 + 2, 5 + 2, 7 + 2);
        cs.pad();

        let mut prng = test_rng();
        let pcs = KZGCommitmentScheme::new(20, &mut prng);
        let params = indexer(&cs, &pcs).unwrap();

        let mut challenges = PlonkChallenges::<F>::new();
        challenges.insert_beta_gamma(one, zero).unwrap();
        let q = z_poly::<KZGCommitmentSchemeBLS, TurboCS<F>>(&params, &witness[..], &challenges);

        let q0 = q.coefs[0];
        assert_eq!(q0, one);
    }
}
