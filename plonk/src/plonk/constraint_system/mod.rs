use noah_algebra::prelude::*;

/// Module for Field Simulation Constrain System.
pub mod field_simulation;

/// Module for Turbo PLONK Constrain System.
pub mod turbo;

/// Module for ECC.
pub mod ecc;

/// Module for the Anemoi-Jive hash function.
pub mod anemoi_jive;

/// Default used constraint system.
#[doc(hidden)]
pub use turbo::TurboCS;

/// Variable index
pub type VarIndex = usize;

/// Constraint index
pub type CsIndex = usize;

/// Trait for PLONK constraint systems.
pub trait ConstraintSystem: Sized {
    /// Type of scalar field.
    type Field: Scalar;

    /// Return the number of constraints in the system.
    /// `size` should divide q-1 where q is the size of the prime field.
    /// This enables finding a multiplicative subgroup with order `size`.
    fn size(&self) -> usize;

    /// Return number of variables in the constrain system
    fn num_vars(&self) -> usize;

    /// Return the wiring of the constrain system
    fn wiring(&self) -> &[Vec<usize>];

    /// Return the size of the evaluation domain for computing the quotient polynomial.
    /// `quot_eval_dom_size divides q-1 where q is the size of the prime field.
    /// `quot_eval_dom_size is larger than the degree of the quotient polynomial.
    /// `quot_eval_dom_size is a multiple of 'size'.
    fn quot_eval_dom_size(&self) -> usize;

    /// Return the number of wires in a single gate.
    fn n_wires_per_gate() -> usize;

    /// Return the number of selectors.
    fn num_selectors(&self) -> usize;

    /// Compute the permutation implied by the copy constraints.
    fn compute_permutation(&self) -> Vec<usize> {
        let n = self.size();
        let n_wires_per_gate = Self::n_wires_per_gate();
        let mut perm = vec![0usize; n_wires_per_gate * n];
        let mut marked = vec![false; self.num_vars()];
        let mut v = Vec::with_capacity(n_wires_per_gate * n);
        for wire_slice in self.wiring().iter() {
            v.extend_from_slice(wire_slice);
        }
        // form a cycle for each variable value
        // marked variables already processd
        // for each unmarked variable, find all position where this variable occurs to form a cycle.
        for (i, value) in v.iter().enumerate() {
            if marked[*value] {
                continue;
            }
            let first = i;
            let mut prev = i;
            for (j, current_value) in v[i + 1..].iter().enumerate() {
                if current_value == value {
                    perm[prev] = i + 1 + j; //current index in v
                    prev = i + 1 + j;
                }
            }
            perm[prev] = first;
            marked[*value] = true
        }
        perm
    }

    /// Compute the indices of the constraints related to public inputs.
    fn public_vars_constraint_indices(&self) -> &[usize];

    /// Compute the indices of the witnesses related to public inputs.
    fn public_vars_witness_indices(&self) -> &[usize];

    /// Compute the indices of the constraints that need a boolean constraint of the second, third, and fourth inputs.
    fn boolean_constraint_indices(&self) -> &[CsIndex];

    /// Compute the Anemoi selectors.
    fn compute_anemoi_jive_selectors(&self) -> [Vec<Self::Field>; 4];

    /// Map the witnesses into the wires of the circuit.
    /// The (i * size + j)-th output element is the value of the i-th wire on the j-th gate.
    fn extend_witness(&self, witness: &[Self::Field]) -> Vec<Self::Field> {
        let mut extended = Vec::with_capacity(Self::n_wires_per_gate() * self.size());
        for wire_slice in self.wiring().iter() {
            for index in wire_slice.iter() {
                extended.push(witness[*index].clone());
            }
        }
        extended
    }

    /// Borrow the (index)-th selector vector.
    fn selector(&self, index: usize) -> Result<&[Self::Field]>;

    /// Evaluate the constraint equation given public input and the
    /// values of the wires and the selectors.
    fn eval_gate_func(
        wire_vals: &[&Self::Field],
        sel_vals: &[&Self::Field],
        pub_input: &Self::Field,
    ) -> Result<Self::Field>;

    /// Given the wires values of a gate, evaluate the coefficients
    /// of the selectors in the constraint equation.
    fn eval_selector_multipliers(wire_vals: &[&Self::Field]) -> Result<Vec<Self::Field>>;

    /// is only for verifier use.
    fn is_verifier_only(&self) -> bool {
        false
    }

    /// Shrink to only verifier use.
    fn shrink_to_verifier_only(&self) -> Self;

    /// Get the Anemoi generator and generator inverse.
    fn get_anemoi_parameters(&self) -> Result<(Self::Field, Self::Field)>;

    /// Get the hiding degree for each witness polynomial.
    fn get_hiding_degree(&self, idx: usize) -> usize;
}
