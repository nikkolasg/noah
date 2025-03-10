#[cfg(test)]
mod smoke_axfr {
    use digest::Digest;
    use mem_db::MemoryDB;
    use noah::keys::SecretKey;
    use noah::parameters::params::{ProverParams, VerifierParams};
    use noah::parameters::AddressFormat::{ED25519, SECP256K1};
    use noah::{
        anon_xfr::{
            abar_to_abar::*,
            abar_to_ar::*,
            abar_to_bar::*,
            ar_to_abar::*,
            bar_to_abar::*,
            structs::{
                AnonAssetRecord, MTLeafInfo, MTNode, MTPath, OpenAnonAssetRecord,
                OpenAnonAssetRecordBuilder,
            },
            FEE_TYPE,
        },
        keys::{KeyPair, KeyType, PublicKey},
        xfr::{
            asset_record::{build_blind_asset_record, open_blind_asset_record, AssetRecordType},
            structs::{
                AssetRecordTemplate, AssetType, BlindAssetRecord, OwnerMemo, ASSET_TYPE_LENGTH,
            },
        },
    };
    use noah_accumulators::merkle_tree::{PersistentMerkleTree, Proof, TreePath};
    use noah_algebra::{bls12_381::BLSScalar, prelude::*, ristretto::PedersenCommitmentRistretto};
    use noah_crypto::basic::anemoi_jive::{AnemoiJive, AnemoiJive381};
    use parking_lot::RwLock;
    use sha2::Sha512;
    use std::sync::Arc;
    use storage::{
        state::{ChainState, State},
        store::PrefixedStore,
    };

    const AMOUNT: u64 = 10u64;
    const ASSET: AssetType = AssetType([1u8; ASSET_TYPE_LENGTH]);
    const ASSET3: AssetType = AssetType([2u8; ASSET_TYPE_LENGTH]);
    const ASSET4: AssetType = AssetType([2u8; ASSET_TYPE_LENGTH]);
    const ASSET5: AssetType = AssetType([2u8; ASSET_TYPE_LENGTH]);
    const ASSET6: AssetType = AssetType([2u8; ASSET_TYPE_LENGTH]);

    fn build_bar<R: RngCore + CryptoRng>(
        pubkey: &PublicKey,
        prng: &mut R,
        pc_gens: &PedersenCommitmentRistretto,
        amt: u64,
        asset_type: AssetType,
        ar_type: AssetRecordType,
    ) -> (BlindAssetRecord, Option<OwnerMemo>) {
        let ar = AssetRecordTemplate::with_no_asset_tracing(amt, asset_type, ar_type, *pubkey);
        let (bar, _, memo) = build_blind_asset_record(prng, &pc_gens, &ar, vec![]);
        (bar, memo)
    }

    fn build_oabar<R: CryptoRng + RngCore>(
        prng: &mut R,
        amount: u64,
        asset_type: AssetType,
        keypair: &KeyPair,
    ) -> OpenAnonAssetRecord {
        OpenAnonAssetRecordBuilder::new()
            .amount(amount)
            .asset_type(asset_type)
            .pub_key(&keypair.get_pk())
            .finalize(prng)
            .unwrap()
            .build()
            .unwrap()
    }

    fn hash_abar(uid: u64, abar: &AnonAssetRecord) -> BLSScalar {
        AnemoiJive381::eval_variable_length_hash(&[BLSScalar::from(uid), abar.commitment])
    }

    fn build_mt_leaf_info_from_proof(proof: Proof, uid: u64) -> MTLeafInfo {
        return MTLeafInfo {
            path: MTPath {
                nodes: proof
                    .nodes
                    .iter()
                    .map(|e| MTNode {
                        left: e.left,
                        mid: e.mid,
                        right: e.right,
                        is_left_child: (e.path == TreePath::Left) as u8,
                        is_mid_child: (e.path == TreePath::Middle) as u8,
                        is_right_child: (e.path == TreePath::Right) as u8,
                    })
                    .collect(),
            },
            root: proof.root,
            root_version: proof.root_version,
            uid,
        };
    }

    fn random_hasher<R: CryptoRng + RngCore>(prng: &mut R) -> Sha512 {
        let mut hasher = Sha512::new();
        let mut random_bytes = [0u8; 32];
        prng.fill_bytes(&mut random_bytes);
        hasher.update(&random_bytes);
        hasher
    }

    fn mock_fee(x: usize, y: usize) -> u32 {
        5 + (x as u32) + 2 * (y as u32)
    }

    #[test]
    fn ar_to_abar_secp256k1() {
        let mut prng = test_rng();
        let sender = KeyPair::sample(&mut prng, SECP256K1);
        let receiver = KeyPair::sample(&mut prng, SECP256K1);
        ar_to_abar(sender, receiver);
    }

    #[test]
    fn ar_to_abar_ed25519() {
        let mut prng = test_rng();
        let sender = KeyPair::sample(&mut prng, ED25519);
        let receiver = KeyPair::sample(&mut prng, ED25519);
        ar_to_abar(sender, receiver);
    }

    fn ar_to_abar(sender: KeyPair, receiver: KeyPair) {
        let mut prng = test_rng();
        let pc_gens = PedersenCommitmentRistretto::default();
        let params = ProverParams::gen_ar_to_abar().unwrap();
        let verify_params = VerifierParams::get_ar_to_abar().unwrap();

        let (bar, memo) = build_bar(
            &sender.get_pk(),
            &mut prng,
            &pc_gens,
            AMOUNT,
            ASSET,
            AssetRecordType::NonConfidentialAmount_NonConfidentialAssetType,
        );
        let obar = open_blind_asset_record(&bar, &memo, &sender).unwrap();

        let note =
            gen_ar_to_abar_note(&mut prng, &params, &obar, &sender, &receiver.get_pk()).unwrap();
        assert!(verify_ar_to_abar_note(&verify_params, &note).is_ok());

        #[cfg(feature = "parallel")]
        {
            let notes = vec![&note; 6];
            assert!(batch_verify_ar_to_abar_note(&verify_params, &notes).is_ok());
        }

        // check open abar
        let oabar =
            OpenAnonAssetRecordBuilder::from_abar(&note.body.output, note.body.memo, &receiver)
                .unwrap()
                .build()
                .unwrap();
        assert_eq!(oabar.get_amount(), AMOUNT);
        assert_eq!(oabar.get_asset_type(), ASSET);
    }

    #[test]
    fn bar_to_abar_secp256k1() {
        let mut prng = test_rng();
        let sender = KeyPair::sample(&mut prng, SECP256K1);
        let receiver = KeyPair::sample(&mut prng, SECP256K1);
        bar_to_abar(sender, receiver);
    }

    #[test]
    fn bar_to_abar_ed25519() {
        let mut prng = test_rng();
        let sender = KeyPair::sample(&mut prng, ED25519);
        let receiver = KeyPair::sample(&mut prng, ED25519);
        bar_to_abar(sender, receiver);
    }

    fn bar_to_abar(sender: KeyPair, receiver: KeyPair) {
        let mut prng = test_rng();
        let pc_gens = PedersenCommitmentRistretto::default();
        let params = ProverParams::gen_bar_to_abar().unwrap();
        let verify_params = VerifierParams::get_bar_to_abar().unwrap();

        let (bar, memo) = build_bar(
            &sender.get_pk(),
            &mut prng,
            &pc_gens,
            AMOUNT,
            ASSET,
            AssetRecordType::ConfidentialAmount_ConfidentialAssetType,
        );
        let obar = open_blind_asset_record(&bar, &memo, &sender).unwrap();

        let note =
            gen_bar_to_abar_note(&mut prng, &params, &obar, &sender, &receiver.get_pk()).unwrap();
        assert!(verify_bar_to_abar_note(&verify_params, &note, &sender.get_pk()).is_ok());

        let mut err_note = note.clone();
        let message = b"error_message";
        let bad_sig = sender.sign(message).unwrap();
        err_note.signature = bad_sig;
        assert!(verify_bar_to_abar_note(&verify_params, &err_note, &sender.get_pk()).is_err());

        #[cfg(feature = "parallel")]
        {
            let mut notes = vec![&note; 6];
            let pub_keys = vec![sender.get_pk_ref(); 6];
            assert!(batch_verify_bar_to_abar_note(&verify_params, &notes, &pub_keys).is_ok());

            notes[5] = &err_note;
            assert!(batch_verify_bar_to_abar_note(&verify_params, &notes, &pub_keys).is_err());
        }

        // check open ABAR
        let oabar =
            OpenAnonAssetRecordBuilder::from_abar(&note.body.output, note.body.memo, &receiver)
                .unwrap()
                .build()
                .unwrap();
        assert_eq!(oabar.get_amount(), AMOUNT);
        assert_eq!(oabar.get_asset_type(), ASSET);
    }

    #[test]
    fn abar_to_ar_secp256k1() {
        let mut prng = test_rng();
        let sender = KeyPair::sample(&mut prng, SECP256K1);
        let receiver = KeyPair::sample(&mut prng, SECP256K1);
        abar_to_ar(sender, receiver);
    }

    #[test]
    fn abar_to_ar_ed25519() {
        let mut prng = test_rng();
        let sender = KeyPair::sample(&mut prng, ED25519);
        let receiver = KeyPair::sample(&mut prng, ED25519);
        abar_to_ar(sender, receiver);
    }

    fn abar_to_ar(sender: KeyPair, receiver: KeyPair) {
        let mut prng = test_rng();

        let address_format = match sender.get_sk_ref() {
            SecretKey::Ed25519(_) => ED25519,
            SecretKey::Secp256k1(_) => SECP256K1,
        };

        let params = ProverParams::gen_abar_to_ar(address_format).unwrap();
        let verify_params = VerifierParams::get_abar_to_ar(address_format).unwrap();

        let fdb = MemoryDB::new();
        let cs = Arc::new(RwLock::new(ChainState::new(fdb, "abar_ar".to_owned(), 0)));
        let mut state = State::new(cs, false);
        let store = PrefixedStore::new("my_store", &mut state);
        let mut mt = PersistentMerkleTree::new(store).unwrap();

        let mut oabar = build_oabar(&mut prng, AMOUNT, ASSET, &sender);
        let abar = AnonAssetRecord::from_oabar(&oabar);
        mt.add_commitment_hash(hash_abar(0, &abar)).unwrap();
        mt.commit().unwrap();
        let proof = mt.generate_proof(0).unwrap();
        oabar.update_mt_leaf_info(build_mt_leaf_info_from_proof(proof.clone(), 0));

        let pre_note =
            init_abar_to_ar_note(&mut prng, &oabar, &sender, &receiver.get_pk()).unwrap();
        let hash = random_hasher(&mut prng);
        let note = finish_abar_to_ar_note(&mut prng, &params, pre_note, hash.clone()).unwrap();
        verify_abar_to_ar_note(&verify_params, &note, &proof.root, hash.clone()).unwrap();

        let err_root = BLSScalar::random(&mut prng);
        assert!(verify_abar_to_ar_note(&verify_params, &note, &err_root, hash.clone()).is_err());

        let err_hash = random_hasher(&mut prng);
        assert!(
            verify_abar_to_ar_note(&verify_params, &note, &proof.root, err_hash.clone()).is_err()
        );

        let mut err_nullifier = note.clone();
        err_nullifier.body.input = BLSScalar::random(&mut prng);
        assert!(
            verify_abar_to_ar_note(&verify_params, &err_nullifier, &proof.root, hash.clone())
                .is_err()
        );

        #[cfg(feature = "parallel")]
        {
            let mut notes = vec![&note; 6];
            let mut merkle_roots = vec![&proof.root; 6];
            let mut hashes = vec![hash.clone(); 6];
            batch_verify_abar_to_ar_note(&verify_params, &notes, &merkle_roots, hashes.clone())
                .unwrap();

            merkle_roots[5] = &err_root;
            assert!(batch_verify_abar_to_ar_note(
                &verify_params,
                &notes,
                &merkle_roots,
                hashes.clone(),
            )
            .is_err());

            merkle_roots[5] = &proof.root;
            hashes[5] = err_hash;
            assert!(batch_verify_abar_to_ar_note(
                &verify_params,
                &notes,
                &merkle_roots,
                hashes.clone(),
            )
            .is_err());

            hashes[5] = hash.clone();
            notes[5] = &err_nullifier;
            assert!(batch_verify_abar_to_ar_note(
                &verify_params,
                &notes,
                &merkle_roots,
                hashes.clone(),
            )
            .is_err());
            notes[5] = &note;
            assert!(batch_verify_abar_to_ar_note(
                &verify_params,
                &notes,
                &merkle_roots,
                hashes.clone(),
            )
            .is_ok());
        }

        // check open AR
        let obar = open_blind_asset_record(&note.body.output, &note.body.memo, &receiver).unwrap();
        assert_eq!(*obar.get_amount(), AMOUNT);
        assert_eq!(*obar.get_asset_type(), ASSET);
    }

    #[test]
    fn abar_to_bar_secp256k1() {
        let mut prng = test_rng();
        let sender = KeyPair::sample(&mut prng, SECP256K1);
        let receiver = KeyPair::sample(&mut prng, SECP256K1);
        abar_to_bar(sender, receiver);
    }

    #[test]
    fn abar_to_bar_ed25519() {
        let mut prng = test_rng();
        let sender = KeyPair::sample(&mut prng, ED25519);
        let receiver = KeyPair::sample(&mut prng, ED25519);
        abar_to_bar(sender, receiver);
    }

    fn abar_to_bar(sender: KeyPair, receiver: KeyPair) {
        let mut prng = test_rng();

        let address_format = match sender.get_sk_ref() {
            SecretKey::Ed25519(_) => ED25519,
            SecretKey::Secp256k1(_) => SECP256K1,
        };

        let params = ProverParams::gen_abar_to_bar(address_format).unwrap();
        let verify_params = VerifierParams::get_abar_to_bar(address_format).unwrap();

        let fdb = MemoryDB::new();
        let cs = Arc::new(RwLock::new(ChainState::new(fdb, "abar_bar".to_owned(), 0)));
        let mut state = State::new(cs, false);
        let store = PrefixedStore::new("my_store", &mut state);
        let mut mt = PersistentMerkleTree::new(store).unwrap();

        let mut oabar = build_oabar(&mut prng, AMOUNT, ASSET, &sender);
        let abar = AnonAssetRecord::from_oabar(&oabar);
        mt.add_commitment_hash(hash_abar(0, &abar)).unwrap(); // mock
        mt.add_commitment_hash(hash_abar(1, &abar)).unwrap();
        mt.commit().unwrap();
        let proof = mt.generate_proof(1).unwrap();
        oabar.update_mt_leaf_info(build_mt_leaf_info_from_proof(proof.clone(), 1));

        let pre_note = init_abar_to_bar_note(
            &mut prng,
            &oabar,
            &sender,
            &receiver.get_pk(),
            AssetRecordType::ConfidentialAmount_ConfidentialAssetType,
        )
        .unwrap();
        let hash = random_hasher(&mut prng);
        let note = finish_abar_to_bar_note(&mut prng, &params, pre_note, hash.clone()).unwrap();
        verify_abar_to_bar_note(&verify_params, &note, &proof.root, hash.clone()).unwrap();

        let err_root = BLSScalar::random(&mut prng);
        assert!(verify_abar_to_bar_note(&verify_params, &note, &err_root, hash.clone()).is_err());

        let err_hash = random_hasher(&mut prng);
        assert!(
            verify_abar_to_bar_note(&verify_params, &note, &proof.root, err_hash.clone()).is_err()
        );

        let mut err_nullifier = note.clone();
        err_nullifier.body.input = BLSScalar::random(&mut prng);
        assert!(
            verify_abar_to_bar_note(&verify_params, &err_nullifier, &proof.root, hash.clone())
                .is_err()
        );

        #[cfg(feature = "parallel")]
        {
            let mut notes = vec![&note; 6];
            let mut merkle_roots = vec![&proof.root; 6];
            let mut hashes = vec![hash.clone(); 6];
            batch_verify_abar_to_bar_note(&verify_params, &notes, &merkle_roots, hashes.clone())
                .unwrap();

            merkle_roots[5] = &err_root;
            assert!(batch_verify_abar_to_bar_note(
                &verify_params,
                &notes,
                &merkle_roots,
                hashes.clone(),
            )
            .is_err());

            merkle_roots[5] = &proof.root;
            hashes[5] = err_hash;
            assert!(batch_verify_abar_to_bar_note(
                &verify_params,
                &notes,
                &merkle_roots,
                hashes.clone(),
            )
            .is_err());

            hashes[5] = hash;
            notes[5] = &err_nullifier;
            assert!(batch_verify_abar_to_bar_note(
                &verify_params,
                &notes,
                &merkle_roots,
                hashes.clone(),
            )
            .is_err());
            notes[5] = &note;
            assert!(
                batch_verify_abar_to_bar_note(&verify_params, &notes, &merkle_roots, hashes)
                    .is_ok()
            );
        }

        // check open BAR
        let obar = open_blind_asset_record(&note.body.output, &note.body.memo, &receiver).unwrap();
        assert_eq!(*obar.get_amount(), AMOUNT);
        assert_eq!(*obar.get_asset_type(), ASSET);
    }

    #[test]
    fn abar_1in_1out_1asset() {
        let fee_amount = mock_fee(1, 1);
        let outputs = vec![(1, FEE_TYPE)];
        let inputs = vec![(fee_amount as u64 + 1, FEE_TYPE)];
        test_abar(
            inputs,
            outputs,
            fee_amount,
            "abar-1-1-1",
            Some(KeyType::Ed25519),
        );
    }

    #[test]
    fn abar_1in_2out_1asset() {
        let fee_amount = mock_fee(1, 2);
        let outputs = vec![(1, FEE_TYPE), (0, FEE_TYPE)];
        let inputs = vec![(fee_amount as u64 + 1, FEE_TYPE)];
        test_abar(
            inputs,
            outputs,
            fee_amount,
            "abar-1-2-1",
            Some(KeyType::Ed25519),
        );
    }

    #[test]
    fn abar_2in_1out_1asset() {
        let fee_amount = mock_fee(2, 1);
        let outputs = vec![(1, FEE_TYPE)];
        let inputs = vec![(1, FEE_TYPE), (fee_amount as u64, FEE_TYPE)];
        test_abar(inputs, outputs, fee_amount, "abar-2-1-1", None);
    }

    #[test]
    fn abar_6in_6out_1asset() {
        // supported max inputs and outputs
        let fee_amount = mock_fee(6, 6);
        let outputs = vec![
            (1, FEE_TYPE),
            (22, FEE_TYPE),
            (333, FEE_TYPE),
            (4444, FEE_TYPE),
            (55555, FEE_TYPE),
            (666666, FEE_TYPE),
        ];
        let inputs = vec![
            (1, FEE_TYPE),
            (22, FEE_TYPE),
            (333, FEE_TYPE),
            (4444, FEE_TYPE),
            (55555, FEE_TYPE),
            (666666 + fee_amount as u64, FEE_TYPE),
        ];
        test_abar(inputs, outputs, fee_amount, "abar-6-6-1", None);
    }

    #[test]
    fn abar_2in_3out_2asset() {
        let fee_amount = mock_fee(2, 3);
        let outputs = vec![(5, FEE_TYPE), (15, FEE_TYPE), (30, ASSET)];
        let inputs = vec![(20 + fee_amount as u64, FEE_TYPE), (30, ASSET)];
        test_abar(inputs, outputs, fee_amount, "abar-2-3-2", None);
    }

    #[test]
    fn abar_6in_6out_6asset() {
        // supported max inputs and outputs and assets
        let fee_amount = mock_fee(6, 6);
        let outputs = vec![
            (1, FEE_TYPE),
            (22, ASSET),
            (333, ASSET3),
            (4444, ASSET4),
            (55555, ASSET5),
            (666666, ASSET6),
        ];
        let inputs = vec![
            (1 + fee_amount as u64, FEE_TYPE),
            (22, ASSET),
            (333, ASSET3),
            (4444, ASSET4),
            (55555, ASSET5),
            (666666, ASSET6),
        ];
        test_abar(inputs, outputs, fee_amount, "abar-6-6-6", None);
    }

    #[test]
    fn abar_1in_xout_1asset() {
        for output_len in 1..=20 {
            let fee_amount = mock_fee(1, output_len);

            let fee_t = FEE_TYPE;
            let inputs = vec![(100 + fee_amount as u64, fee_t)];

            let mut outputs = Vec::with_capacity(output_len);
            outputs.push(((100 - output_len + 1) as u64, fee_t));
            for _ in 0..output_len - 1 {
                outputs.push((1, fee_t));
            }

            assert_eq!(outputs.len(), output_len);

            test_abar(
                inputs,
                outputs,
                fee_amount,
                format!("abar-1-{}-1", output_len).as_str(),
                Some(KeyType::Secp256k1),
            );
        }
    }

    fn test_abar(
        inputs: Vec<(u64, AssetType)>,
        outputs: Vec<(u64, AssetType)>,
        fee: u32,
        name: &str,
        input_key_type: Option<KeyType>,
    ) {
        let mut prng = test_rng();

        let address_format = if input_key_type.is_none() {
            if prng.gen() {
                SECP256K1
            } else {
                ED25519
            }
        } else {
            match input_key_type.unwrap() {
                KeyType::Ed25519 => ED25519,
                KeyType::Secp256k1 => SECP256K1,
                _ => unimplemented!(),
            }
        };

        let params =
            ProverParams::gen_abar_to_abar(inputs.len(), outputs.len(), address_format).unwrap();
        let verifier_params =
            VerifierParams::load_abar_to_abar(inputs.len(), outputs.len(), address_format).unwrap();

        let sender = KeyPair::sample(&mut prng, address_format);

        let receivers: Vec<KeyPair> = (0..outputs.len())
            .map(|_| {
                if prng.gen() {
                    KeyPair::sample(&mut prng, SECP256K1)
                } else {
                    KeyPair::sample(&mut prng, ED25519)
                }
            })
            .collect();

        let mut oabars: Vec<OpenAnonAssetRecord> = inputs
            .iter()
            .map(|input| build_oabar(&mut prng, input.0, input.1, &sender))
            .collect();
        let abars: Vec<_> = oabars.iter().map(AnonAssetRecord::from_oabar).collect();

        let fdb = MemoryDB::new();
        let cs = Arc::new(RwLock::new(ChainState::new(fdb, name.to_owned(), 0)));
        let mut state = State::new(cs, false);
        let store = PrefixedStore::new("my_store", &mut state);
        let mut mt = PersistentMerkleTree::new(store).unwrap();
        let mut uids = vec![];
        for i in 0..abars.len() {
            let abar_comm = hash_abar(mt.entry_count(), &abars[i]);
            uids.push(mt.add_commitment_hash(abar_comm).unwrap());
        }
        mt.commit().unwrap();
        let root = mt.get_root().unwrap();
        for (i, uid) in uids.iter().enumerate() {
            let proof = mt.generate_proof(*uid).unwrap();
            oabars[i].update_mt_leaf_info(build_mt_leaf_info_from_proof(proof, *uid));
        }

        let oabars_out: Vec<OpenAnonAssetRecord> = outputs
            .iter()
            .enumerate()
            .map(|(i, output)| build_oabar(&mut prng, output.0, output.1, &receivers[i]))
            .collect();

        let pre_note = init_anon_xfr_note(&oabars, &oabars_out, fee, &sender).unwrap();
        let hash = random_hasher(&mut prng);
        let note = finish_anon_xfr_note(&mut prng, &params, pre_note, hash.clone()).unwrap();

        verify_anon_xfr_note(&verifier_params, &note, &root, hash.clone()).unwrap();

        #[cfg(feature = "parallel")]
        {
            let verifiers_params = vec![&verifier_params; 6];
            let notes = vec![&note; 6];
            let merkle_roots = vec![&root; 6];
            let hashes = vec![hash.clone(); 6];
            assert!(
                batch_verify_anon_xfr_note(&verifiers_params, &notes, &merkle_roots, hashes)
                    .is_ok()
            );
        }

        // check abar
        for i in 0..note.body.outputs.len() {
            let oabar = OpenAnonAssetRecordBuilder::from_abar(
                &note.body.outputs[i],
                note.body.owner_memos[i].clone(),
                &receivers[i],
            )
            .unwrap()
            .build()
            .unwrap();
            assert_eq!(oabars_out[i].get_amount(), oabar.get_amount());
            assert_eq!(oabars_out[i].get_asset_type(), oabar.get_asset_type());
        }
    }
}
