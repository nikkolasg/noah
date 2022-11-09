/// The number of bytes for a scalar value over the curve25519 curve
pub const CURVE25519_SCALAR_LEN: usize = 32;

mod fr;
pub use fr::*;

mod fq;
pub use fq::*;

mod g1;
pub use g1::*;

#[cfg(test)]
mod curve25519_groups_test {
    use crate::{
        curve25519::{Curve25519Point, Curve25519Scalar},
        prelude::*,
        traits::group_tests::{test_scalar_operations, test_scalar_serialization},
    };
    use ark_bulletproofs::curve::curve25519::G1Affine;
    use ark_ec::ProjectiveCurve;

    #[test]
    fn test_scalar_ops() {
        test_scalar_operations::<Curve25519Scalar>();
    }

    #[test]
    fn scalar_deser() {
        test_scalar_serialization::<Curve25519Scalar>();
    }

    #[test]
    fn scalar_from_to_bytes() {
        let small_value = Curve25519Scalar::from(165747u32);
        let small_value_bytes = small_value.to_bytes();
        let expected_small_value_bytes: [u8; 32] = [
            115, 135, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ];
        assert_eq!(small_value_bytes, expected_small_value_bytes);

        let small_value_from_bytes = Curve25519Scalar::from_bytes(&small_value_bytes).unwrap();
        assert_eq!(small_value_from_bytes, small_value);
    }

    #[test]
    fn curve_points_respresentation_of_g1() {
        let mut prng = test_rng();

        let g1 = Curve25519Point::get_base();
        let s1 = Curve25519Scalar::from(50 + prng.next_u32() % 50);

        let g1 = g1.mul(&s1);

        let g1_prime = Curve25519Point::random(&mut prng);

        // This is the projective representation of g1
        let g1_projective = g1.0;
        let g1_prime_projective = g1_prime.0;

        // This is the affine representation of g1_prime
        let g1_prime_affine = G1Affine::from(g1_prime_projective);

        let g1_pr_plus_g1_prime_pr = g1_projective.add(&g1_prime_projective);

        // These two operations correspond to summation of points,
        // one in projective form and the other in affine form
        let g1_pr_plus_g1_prime_af = g1_projective.add_mixed(&g1_prime_affine);
        assert_eq!(g1_pr_plus_g1_prime_pr, g1_pr_plus_g1_prime_af);

        let g1_pr_plus_g1_prime_af = g1_projective.add_mixed(&g1_prime_projective.into_affine());
        assert_eq!(g1_pr_plus_g1_prime_pr, g1_pr_plus_g1_prime_af);
    }

    #[test]
    fn test_serialization_of_points() {
        let mut prng = test_rng();

        let g1 = Curve25519Point::random(&mut prng);
        let g1_bytes = g1.to_compressed_bytes();
        let g1_recovered = Curve25519Point::from_compressed_bytes(&g1_bytes).unwrap();
        assert_eq!(g1, g1_recovered);
    }
}
