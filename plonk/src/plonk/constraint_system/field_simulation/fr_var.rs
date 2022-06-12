use crate::plonk::constraint_system::{field_simulation::SimFrMulVar, TurboCS, VarIndex};
use zei_algebra::{
    bls12_381::BLSScalar,
    cmp::{max, min},
    prelude::*,
};
use zei_crypto::field_simulation::{
    ristretto_scalar_field_sub_pad_in_limbs, SimFr, SimReducibility, BIT_IN_TOP_LIMB, BIT_PER_LIMB,
    NUM_OF_LIMBS, NUM_OF_LIMBS_MUL,
};

/// `SimFrVar` is the variable for `SimFr` in
/// `TurboConstraintSystem<BLSScalar>`
#[derive(Clone)]
pub struct SimFrVar {
    /// the `SimFr` value.
    pub val: SimFr,
    /// the `SimFr` variables.
    pub var: [VarIndex; NUM_OF_LIMBS],
}

impl SimFrVar {
    /// Create a zero `SimFr`.
    pub fn new(cs: &mut TurboCS<BLSScalar>) -> Self {
        Self {
            val: SimFr::default(),
            var: [cs.zero_var(); NUM_OF_LIMBS],
        }
    }

    /// the Sub operation.
    pub fn sub(&self, cs: &mut TurboCS<BLSScalar>, other: &SimFrVar) -> SimFrVar {
        let mut res = SimFrVar::new(cs);
        res.val = &self.val - &other.val;

        let zero = BLSScalar::zero();
        let one = BLSScalar::one();
        let minus_one = one.neg();

        let zero_var = cs.zero_var();

        let r_limbs = ristretto_scalar_field_sub_pad_in_limbs();

        // The following gate represents
        // res.var[i] := self.var[i] - other.var[i] + r_limbs[i]
        for i in 0..NUM_OF_LIMBS {
            res.var[i] = cs.new_variable(res.val.limbs[i]);

            cs.push_add_selectors(one, zero, minus_one, zero);
            cs.push_mul_selectors(zero, zero);
            cs.push_constant_selector(r_limbs[i]);
            cs.push_ecc_selector(zero);
            cs.push_rescue_selectors(zero, zero, zero, zero);
            cs.push_out_selector(one);

            cs.wiring[0].push(self.var[i]);
            cs.wiring[1].push(zero_var);
            cs.wiring[2].push(other.var[i]);
            cs.wiring[3].push(zero_var);
            cs.wiring[4].push(res.var[i]);
            cs.size += 1;
        }

        res
    }

    /// the Mul operation.
    pub fn mul(&self, cs: &mut TurboCS<BLSScalar>, other: &SimFrVar) -> SimFrMulVar {
        let mut res = SimFrMulVar::new(cs);
        res.val = &self.val * &other.val;

        let zero = BLSScalar::zero();
        let one = BLSScalar::one();

        let zero_var = cs.zero_var();

        for i in 0..NUM_OF_LIMBS_MUL {
            let smallest_left = max(NUM_OF_LIMBS, i + 1) - NUM_OF_LIMBS;
            let largest_left = min(i, NUM_OF_LIMBS - 1);

            let left_array = (smallest_left..=largest_left).collect::<Vec<_>>();

            let mut prior_res_val = BLSScalar::zero();
            let mut prior_res = cs.zero_var();
            for left in left_array {
                let res_val =
                    prior_res_val.add(&self.val.limbs[left].mul(&other.val.limbs[i - left]));
                let res = cs.new_variable(res_val);

                // The following gate represents
                // res := prior_res + self.var[left] * other.var[i - left]

                cs.push_add_selectors(one, zero, zero, zero);
                cs.push_mul_selectors(zero, one);
                cs.push_constant_selector(zero);
                cs.push_ecc_selector(zero);
                cs.push_rescue_selectors(zero, zero, zero, zero);
                cs.push_out_selector(one);

                cs.wiring[0].push(prior_res);
                cs.wiring[1].push(zero_var);
                cs.wiring[2].push(self.var[left]);
                cs.wiring[3].push(other.var[i - left]);
                cs.wiring[4].push(res);
                cs.size += 1;

                prior_res = res;
                prior_res_val = res_val;
            }

            res.var[i] = prior_res;
        }
        res
    }

    /// Alloc a constant gate.
    pub fn alloc_constant(cs: &mut TurboCS<BLSScalar>, val: &SimFr) -> Self {
        let mut res = Self::new(cs);
        res.val = val.clone();
        for i in 0..NUM_OF_LIMBS {
            res.var[i] = cs.new_variable(val.limbs[i]);
            cs.insert_constant_gate(res.var[i], val.limbs[i]);
        }
        res
    }

    /// Alloc an input variable.
    pub fn alloc_input(cs: &mut TurboCS<BLSScalar>, val: &SimFr) -> Self {
        let mut res = Self::new(cs);
        res.val = val.clone();
        for i in 0..NUM_OF_LIMBS {
            res.var[i] = cs.new_variable(val.limbs[i]);
        }
        res
    }

    /// Alloc a witness variable and range check gate.
    pub fn alloc_witness(cs: &mut TurboCS<BLSScalar>, val: &SimFr) -> Self {
        assert!(val.num_of_additions_over_normal_form == SimReducibility::StrictlyNotReducible);

        let mut res = Self::new(cs);
        res.val = val.clone();
        res.val.num_of_additions_over_normal_form = SimReducibility::AtMostReducibleByOne;
        for i in 0..NUM_OF_LIMBS {
            res.var[i] = cs.new_variable(val.limbs[i]);

            if i == NUM_OF_LIMBS - 1 {
                cs.range_check(res.var[i], BIT_IN_TOP_LIMB);
            } else {
                cs.range_check(res.var[i], BIT_PER_LIMB);
            }
        }
        res
    }

    /// Alloc a witness variable and range check gate with bounded.
    pub fn alloc_witness_bounded_total_bits(
        cs: &mut TurboCS<BLSScalar>,
        val: &SimFr,
        total_bits: usize,
    ) -> Self {
        assert!(val.num_of_additions_over_normal_form == SimReducibility::StrictlyNotReducible);

        let mut res = Self::new(cs);
        res.val = val.clone();
        if total_bits == 253 {
            res.val.num_of_additions_over_normal_form = SimReducibility::AtMostReducibleByOne;
        }

        let mut remaining_bits = total_bits;

        for i in 0..NUM_OF_LIMBS {
            if remaining_bits != 0 {
                res.var[i] = cs.new_variable(val.limbs[i]);
                let bit_limit = min(remaining_bits, BIT_PER_LIMB);
                cs.range_check(res.var[i], bit_limit);
                remaining_bits -= bit_limit;
            } else {
                res.var[i] = cs.zero_var();
            }
        }
        res
    }
}

#[cfg(test)]
mod test {
    use crate::plonk::constraint_system::{
        field_simulation::{SimFrMulVar, SimFrVar},
        TurboCS,
    };
    use num_bigint::{BigUint, RandBigInt};
    use rand_chacha::ChaCha20Rng;
    use std::ops::Shl;
    use zei_algebra::{bls12_381::BLSScalar, prelude::*};
    use zei_crypto::field_simulation::{
        ristretto_scalar_field_in_biguint, SimFr, NUM_OF_LIMBS, NUM_OF_LIMBS_MUL,
    };

    fn test_sim_fr_equality(cs: TurboCS<BLSScalar>, val: &SimFrVar) {
        let mut cs = cs;
        for i in 0..NUM_OF_LIMBS {
            cs.insert_constant_gate(val.var[i], val.val.limbs[i]);
        }

        let witness = cs.get_and_clear_witness();
        assert!(cs.verify_witness(&witness[..], &[]).is_ok());
    }

    fn test_sim_fr_mul_equality(cs: TurboCS<BLSScalar>, val: &SimFrMulVar) {
        let mut cs = cs;
        for i in 0..NUM_OF_LIMBS_MUL {
            cs.insert_constant_gate(val.var[i], val.val.limbs[i]);
        }

        let witness = cs.get_and_clear_witness();
        assert!(cs.verify_witness(&witness[..], &[]).is_ok());
    }

    #[test]
    fn test_alloc_constant() {
        let mut rng = ChaCha20Rng::from_entropy();
        let p_biguint = ristretto_scalar_field_in_biguint();

        for _ in 0..100 {
            let a = rng.gen_biguint_range(&BigUint::zero(), &p_biguint);
            let a_sim_fr = SimFr::from(&a);

            {
                let mut cs = TurboCS::<BLSScalar>::new();
                let a_sim_fr_var = SimFrVar::alloc_constant(&mut cs, &a_sim_fr);
                test_sim_fr_equality(cs, &a_sim_fr_var);
            }
        }
    }

    #[test]
    fn test_alloc_witness() {
        let mut rng = ChaCha20Rng::from_entropy();
        let p_biguint = ristretto_scalar_field_in_biguint();

        for _ in 0..100 {
            let a = rng.gen_biguint_range(&BigUint::zero(), &p_biguint);
            let a_sim_fr = SimFr::from(&a);

            {
                let mut cs = TurboCS::<BLSScalar>::new();
                let a_sim_fr_var = SimFrVar::alloc_witness(&mut cs, &a_sim_fr);
                test_sim_fr_equality(cs, &a_sim_fr_var);
            }
        }
    }

    #[test]
    fn test_sub() {
        let mut rng = ChaCha20Rng::from_entropy();
        let p_biguint = ristretto_scalar_field_in_biguint();

        for _ in 0..100 {
            let a = rng.gen_biguint_range(&BigUint::zero(), &p_biguint);
            let b = rng.gen_biguint_range(&BigUint::zero(), &p_biguint);

            let a_sim_fr = SimFr::from(&a);
            let b_sim_fr = SimFr::from(&b);

            {
                let mut cs = TurboCS::<BLSScalar>::new();

                let a_sim_fr_var = SimFrVar::alloc_witness(&mut cs, &a_sim_fr);
                let b_sim_fr_var = SimFrVar::alloc_witness(&mut cs, &b_sim_fr);

                let c_sim_fr_var = a_sim_fr_var.sub(&mut cs, &b_sim_fr_var);
                test_sim_fr_equality(cs, &c_sim_fr_var);
            }
        }
    }

    #[test]
    fn test_mul() {
        let mut rng = ChaCha20Rng::from_entropy();
        let p_biguint = ristretto_scalar_field_in_biguint();

        for _ in 0..100 {
            let a = rng.gen_biguint_range(&BigUint::zero(), &p_biguint);
            let b = rng.gen_biguint_range(&BigUint::zero(), &p_biguint);

            let a_sim_fr = SimFr::from(&a);
            let b_sim_fr = SimFr::from(&b);

            {
                let mut cs = TurboCS::<BLSScalar>::new();

                let a_sim_fr_var = SimFrVar::alloc_witness(&mut cs, &a_sim_fr);
                let b_sim_fr_var = SimFrVar::alloc_witness(&mut cs, &b_sim_fr);

                let c_sim_fr_mul_var = a_sim_fr_var.mul(&mut cs, &b_sim_fr_var);
                test_sim_fr_mul_equality(cs, &c_sim_fr_mul_var);
            }
        }
    }

    #[test]
    fn test_bounded_allocated_witness() {
        let mut rng = ChaCha20Rng::from_entropy();

        for _ in 0..100 {
            let a = rng.gen_biguint(240);
            let a_sim_fr = SimFr::from(&a);

            {
                let mut cs = TurboCS::<BLSScalar>::new();

                let a_sim_fr_var =
                    SimFrVar::alloc_witness_bounded_total_bits(&mut cs, &a_sim_fr, 240);
                test_sim_fr_equality(cs, &a_sim_fr_var);
            }
        }
    }

    #[test]
    #[should_panic]
    fn test_bounded_allocated_witness_bad() {
        let a = BigUint::from(1u32).shl(240);
        let a_sim_fr = SimFr::from(&a);

        {
            let mut cs = TurboCS::<BLSScalar>::new();

            let a_sim_fr_var = SimFrVar::alloc_witness_bounded_total_bits(&mut cs, &a_sim_fr, 240);

            test_sim_fr_equality(cs, &a_sim_fr_var);
        }
    }
}
