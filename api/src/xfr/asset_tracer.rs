use crate::anon_creds::{Attr, AttributeCiphertext};
use crate::xfr::structs::{
    AssetTracerDecKeys, AssetTracerEncKeys, AssetType, TracerMemo, ASSET_TYPE_LENGTH,
};
use noah_algebra::{
    bls12_381::{BLSScalar, BLSG1},
    prelude::*,
    ristretto::{RistrettoPoint, RistrettoScalar},
};
use noah_crypto::basic::{
    elgamal::{
        elgamal_encrypt, elgamal_partial_decrypt, ElGamalCiphertext, ElGamalDecKey, ElGamalEncKey,
    },
    hybrid_encryption::{hybrid_decrypt_with_x25519_secret_key, hybrid_encrypt_x25519},
};

/// The encryption key for the record data.
pub type RecordDataEncKey = ElGamalEncKey<RistrettoPoint>;
/// The decryption key for the record data.
pub type RecordDataDecKey = ElGamalDecKey<RistrettoScalar>;
/// The ciphertext of the record data.
pub type RecordDataCiphertext = ElGamalCiphertext<RistrettoPoint>;
type DecryptedAssetMemo = (Option<u64>, Option<AssetType>, Vec<Attr>);

const U32_BYTES: usize = 4;

impl TracerMemo {
    /// Sample a new TracerMemo.
    /// amount_info is (amount_low, amount_high, amount_blind_low, amount_blind_high) tuple
    /// asset_type_info is (asset_type, asset_type_blind) tuple
    pub fn new<R: CryptoRng + RngCore>(
        prng: &mut R,
        tracer_enc_key: &AssetTracerEncKeys,
        amount_info: Option<(u32, u32, &RistrettoScalar, &RistrettoScalar)>,
        asset_type_info: Option<(&AssetType, &RistrettoScalar)>,
        attrs_info: &[(Attr, AttributeCiphertext)],
    ) -> Self {
        let mut plaintext = vec![];
        let lock_amount = amount_info.map(|(amount_low, amount_high, blind_low, blind_high)| {
            plaintext.extend_from_slice(&amount_low.to_be_bytes());
            plaintext.extend_from_slice(&amount_high.to_be_bytes());
            let ctext_amount_low = elgamal_encrypt(
                &RistrettoScalar::from(amount_low),
                blind_low,
                &tracer_enc_key.record_data_enc_key,
            );
            let ctext_amount_high = elgamal_encrypt(
                &RistrettoScalar::from(amount_high),
                blind_high,
                &tracer_enc_key.record_data_enc_key,
            );
            (ctext_amount_low, ctext_amount_high)
        });

        let lock_asset_type = asset_type_info.map(|(asset_type, blind)| {
            plaintext.extend_from_slice(&asset_type.0);
            elgamal_encrypt(
                &asset_type.as_scalar(),
                blind,
                &tracer_enc_key.record_data_enc_key,
            )
        });

        for (attr, _) in attrs_info.iter() {
            plaintext.extend_from_slice(&attr.to_be_bytes())
        }
        let lock_info = hybrid_encrypt_x25519(prng, &tracer_enc_key.lock_info_enc_key, &plaintext);

        TracerMemo {
            enc_key: tracer_enc_key.clone(),
            lock_amount,
            lock_asset_type,
            lock_attributes: attrs_info.iter().map(|(_, ctext)| ctext.clone()).collect(),
            lock_info,
        }
    }

    /// Decrypts the asset tracer memo:
    /// Returns NoahError:BogusAssetTracerMemo in case decrypted values are inconsistents
    pub fn decrypt(&self, dec_key: &AssetTracerDecKeys) -> Result<DecryptedAssetMemo> {
        let mut plaintext =
            hybrid_decrypt_with_x25519_secret_key(&self.lock_info, &dec_key.lock_info_dec_key);

        // decrypt and sanitize amount
        let amount = if self.lock_amount.is_some() {
            if plaintext.len() < 2 * U32_BYTES {
                return Err(eg!(NoahError::BogusAssetTracerMemo));
            }
            let amount_low = u8_be_slice_to_u32(&plaintext[0..U32_BYTES]);
            let amount_high = u8_be_slice_to_u32(&plaintext[U32_BYTES..2 * U32_BYTES]);
            let amount = (amount_low as u64) + ((amount_high as u64) << 32);
            self.verify_amount(&dec_key.record_data_dec_key, amount)
                .c(d!(NoahError::BogusAssetTracerMemo))?;
            plaintext = plaintext.split_off(2 * U32_BYTES);
            Some(amount)
        } else {
            None
        };

        // decrypt and sanitize asset type
        let asset_type = if self.lock_asset_type.is_some() {
            if plaintext.len() < ASSET_TYPE_LENGTH {
                return Err(eg!(NoahError::BogusAssetTracerMemo));
            }
            let mut asset_type = [0u8; ASSET_TYPE_LENGTH];
            asset_type.copy_from_slice(&plaintext[0..ASSET_TYPE_LENGTH]);
            let asset_type = AssetType(asset_type);

            self.verify_asset_type(&dec_key.record_data_dec_key, &asset_type)
                .c(d!(NoahError::BogusAssetTracerMemo))?;
            plaintext = plaintext.split_off(ASSET_TYPE_LENGTH);
            Some(asset_type)
        } else {
            None
        };

        if plaintext.len() < self.lock_attributes.len() * U32_BYTES {
            return Err(eg!(NoahError::BogusAssetTracerMemo));
        }
        let mut attrs = vec![];
        for attr_byte in plaintext.chunks(U32_BYTES) {
            attrs.push(u8_be_slice_to_u32(attr_byte));
        }

        if !self
            .verify_identity_attributes(&dec_key.attrs_dec_key, &attrs)
            .c(d!(NoahError::BogusAssetTracerMemo))?
            .iter()
            .all(|&x| x)
        {
            return Err(eg!(NoahError::BogusAssetTracerMemo));
        }
        Ok((amount, asset_type, attrs))
    }

    /// Check if the amount encrypted in self.lock_amount is expected.
    /// If self.lock_amount is None, return Err(NoahError::ParameterError),
    /// Otherwise, if decrypted amount is not expected amount, return Err(NoahError::AssetTracingExtractionError), else Ok(()).
    pub fn verify_amount(
        &self,
        dec_key: &ElGamalDecKey<RistrettoScalar>,
        expected: u64,
    ) -> Result<()> {
        let (low, high) = u64_to_u32_pair(expected);
        if let Some((ctext_low, ctext_high)) = self.lock_amount.as_ref() {
            let decrypted_low = elgamal_partial_decrypt(ctext_low, dec_key);
            let decrypted_high = elgamal_partial_decrypt(ctext_high, dec_key);
            let base = RistrettoPoint::get_base();
            if base.mul(&RistrettoScalar::from(low)) != decrypted_low
                || base.mul(&RistrettoScalar::from(high)) != decrypted_high
            {
                Err(eg!(NoahError::AssetTracingExtractionError))
            } else {
                Ok(())
            }
        } else {
            Err(eg!(NoahError::ParameterError)) // nothing to decrypt
        }
    }

    /// Check if the asset type encrypted in self.lock_asset_type is expected.
    /// return Err if lock_asset_type is None or the decrypted is not as expected, else returns Ok.
    pub fn verify_asset_type(
        &self,
        dec_key: &ElGamalDecKey<RistrettoScalar>,
        expected: &AssetType,
    ) -> Result<()> {
        if let Some(ctext) = self.lock_asset_type.as_ref() {
            let decrypted = elgamal_partial_decrypt(ctext, dec_key);
            if decrypted == RistrettoPoint::get_base().mul(&expected.as_scalar()) {
                return Ok(());
            }
            Err(eg!(NoahError::AssetTracingExtractionError))
        } else {
            Err(eg!(NoahError::ParameterError)) // nothing to decrypt
        }
    }

    /// Decrypt asset_type in self.lock_asset_type via a linear scan over candidate_asset_types.
    /// If self.lock_asset_type is None, return Err(NoahError::ParameterError),
    /// Otherwise, if decrypted asset_type is not in the candidate list return Err(NoahError::AssetTracingExtractionError),
    /// else return the decrypted asset_type.
    pub fn extract_asset_type(
        &self,
        dec_key: &ElGamalDecKey<RistrettoScalar>,
        candidate_asset_types: &[AssetType],
    ) -> Result<AssetType> {
        if candidate_asset_types.is_empty() {
            return Err(eg!(NoahError::ParameterError));
        }
        for candidate in candidate_asset_types.iter() {
            if self.verify_asset_type(&dec_key, &candidate).is_ok() {
                return Ok(*candidate);
            }
        }
        Err(eg!(NoahError::AssetTracingExtractionError))
    }

    /// Check is the attributes encrypted in self.lock_attrs are the same as in expected_attributes,
    /// If self.lock_attrs is None or if attribute length doesn't match expected list, return Err(NoahError::ParameterError),
    /// Otherwise, it returns a boolean vector indicating true for every positive match and false otherwise.
    pub fn verify_identity_attributes(
        &self,
        dec_key: &ElGamalDecKey<BLSScalar>,
        expected_attributes: &[u32],
    ) -> Result<Vec<bool>> {
        if self.lock_attributes.len() != expected_attributes.len() {
            return Err(eg!(NoahError::ParameterError));
        }
        let mut result = vec![];
        for (ctext, expected) in self.lock_attributes.iter().zip(expected_attributes.iter()) {
            let scalar_attr = BLSScalar::from(*expected);
            let elem = elgamal_partial_decrypt(ctext, dec_key);
            if elem != BLSG1::get_base().mul(&scalar_attr) {
                result.push(false);
            } else {
                result.push(true);
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use crate::xfr::structs::{AssetTracerKeyPair, AssetType, TracerMemo};
    use noah_algebra::{bls12_381::BLSScalar, prelude::*, ristretto::RistrettoScalar};
    use noah_crypto::basic::elgamal::elgamal_encrypt;

    #[test]
    fn extract_amount_from_tracer_memo() {
        let mut prng = test_rng();
        let tracer_keys = AssetTracerKeyPair::generate(&mut prng);
        let memo = TracerMemo::new(&mut prng, &tracer_keys.enc_key, None, None, &[]);
        assert!(memo
            .verify_amount(&tracer_keys.dec_key.record_data_dec_key, 10)
            .is_err());

        let amount = (1u64 << 40) + 500; // low and high are small u32 numbers
        let (low, high) = u64_to_u32_pair(amount);
        let memo = TracerMemo::new(
            &mut prng,
            &tracer_keys.enc_key,
            Some((
                low,
                high,
                &RistrettoScalar::from(191919u32),
                &RistrettoScalar::from(2222u32),
            )),
            None,
            &[],
        );
        assert!(memo
            .verify_amount(&tracer_keys.dec_key.record_data_dec_key, amount)
            .is_ok());
    }

    #[test]
    fn extract_asset_type_from_tracer_memo() {
        let mut prng = test_rng();
        let tracer_keys = AssetTracerKeyPair::generate(&mut prng);
        let memo = TracerMemo::new(&mut prng, &tracer_keys.enc_key, None, None, &[]);
        assert!(memo
            .extract_asset_type(&tracer_keys.dec_key.record_data_dec_key, &[])
            .is_err());

        let asset_type = AssetType::from_identical_byte(2u8);
        let memo = TracerMemo::new(
            &mut prng,
            &tracer_keys.enc_key,
            None,
            Some((&asset_type, &RistrettoScalar::from(191919u32))),
            &[],
        );

        msg_eq!(
            NoahError::ParameterError,
            memo.extract_asset_type(&tracer_keys.dec_key.record_data_dec_key, &[])
                .unwrap_err(),
        );
        msg_eq!(
            NoahError::AssetTracingExtractionError,
            memo.extract_asset_type(
                &tracer_keys.dec_key.record_data_dec_key,
                &[AssetType::from_identical_byte(0u8)]
            )
            .unwrap_err(),
        );
        msg_eq!(
            NoahError::AssetTracingExtractionError,
            memo.extract_asset_type(
                &tracer_keys.dec_key.record_data_dec_key,
                &[
                    AssetType::from_identical_byte(0u8),
                    AssetType::from_identical_byte(1u8)
                ]
            )
            .unwrap_err(),
        );
        assert!(memo
            .extract_asset_type(
                &tracer_keys.dec_key.record_data_dec_key,
                &[
                    AssetType::from_identical_byte(0u8),
                    AssetType::from_identical_byte(1u8),
                    asset_type
                ]
            )
            .is_ok());
        assert!(memo
            .extract_asset_type(
                &tracer_keys.dec_key.record_data_dec_key,
                &[
                    asset_type,
                    AssetType::from_identical_byte(0u8),
                    AssetType::from_identical_byte(1u8)
                ]
            )
            .is_ok());
        assert!(memo
            .extract_asset_type(
                &tracer_keys.dec_key.record_data_dec_key,
                &[
                    AssetType::from_identical_byte(0u8),
                    asset_type,
                    AssetType::from_identical_byte(1u8)
                ]
            )
            .is_ok());
    }

    #[test]
    fn extract_identity_attributed_from_tracer_memo() {
        let mut prng = test_rng();
        let tracer_keys = AssetTracerKeyPair::generate(&mut prng);
        let attrs = [1u32, 2, 3];

        let attrs_and_ctexts = attrs
            .iter()
            .map(|x| {
                let scalar = BLSScalar::from(*x);
                (
                    *x,
                    elgamal_encrypt(
                        &scalar,
                        &BLSScalar::from(1000u32),
                        &tracer_keys.enc_key.attrs_enc_key,
                    ),
                )
            })
            .collect_vec();

        let memo = TracerMemo::new(
            &mut prng,
            &tracer_keys.enc_key,
            None,
            None,
            &attrs_and_ctexts,
        );

        msg_eq!(
            NoahError::ParameterError,
            memo.verify_identity_attributes(&tracer_keys.dec_key.attrs_dec_key, &[1u32])
                .unwrap_err(),
        );
        msg_eq!(
            NoahError::ParameterError,
            memo.verify_identity_attributes(&tracer_keys.dec_key.attrs_dec_key, &[1u32, 2, 3, 4])
                .unwrap_err(),
        );
        assert_eq!(
            memo.verify_identity_attributes(&tracer_keys.dec_key.attrs_dec_key, &[1u32, 2, 4])
                .unwrap(),
            vec![true, true, false]
        );
        assert_eq!(
            memo.verify_identity_attributes(&tracer_keys.dec_key.attrs_dec_key, &[4u32, 2, 3])
                .unwrap(),
            vec![false, true, true]
        );
        assert_eq!(
            memo.verify_identity_attributes(&tracer_keys.dec_key.attrs_dec_key, &[1u32, 2, 3])
                .unwrap(),
            vec![true, true, true]
        );
        assert_eq!(
            memo.verify_identity_attributes(&tracer_keys.dec_key.attrs_dec_key, &[3u32, 1, 2])
                .unwrap(),
            vec![false, false, false]
        );
    }
}
