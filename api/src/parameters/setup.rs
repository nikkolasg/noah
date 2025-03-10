#![allow(clippy::upper_case_acronyms)]
#![allow(non_camel_case_types)]
#![cfg_attr(
    any(feature = "no_urs", feature = "no_srs", feature = "no_vk"),
    allow(unused)
)]

use ark_bulletproofs::BulletproofGens as BulletproofGensOverSecq256k1;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize, Compress, Validate};
use bulletproofs::BulletproofGens;
use noah::parameters::{AddressFormat, SRS};
use noah_algebra::bls12_381::BLSG1;
use noah_algebra::secq256k1::{PedersenCommitmentSecq256k1, Secq256k1BulletproofGens};
use noah_algebra::utils::save_to_file;
use noah_plonk::poly_commit::kzg_poly_com::KZGCommitmentSchemeBLS;
use rand_chacha::ChaChaRng;
use rand_core::SeedableRng;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::{collections::HashMap, path::PathBuf};
use structopt::StructOpt;

use noah::anon_xfr::TREE_DEPTH;
use noah::parameters::bulletproofs::{BulletproofParams, BulletproofURS};
use noah::parameters::params::{
    ProverParams, VerifierParams, VerifierParamsSplitCommon, ANON_XFR_BP_GENS_LEN,
    MAX_ANONYMOUS_RECORD_NUMBER_CONSOLIDATION_RECEIVER,
    MAX_ANONYMOUS_RECORD_NUMBER_CONSOLIDATION_SENDER, MAX_ANONYMOUS_RECORD_NUMBER_ONE_INPUT,
    MAX_ANONYMOUS_RECORD_NUMBER_STANDARD,
};
use noah::parameters::AddressFormat::{ED25519, SECP256K1};
use noah_algebra::zorro::ZorroBulletproofGens;
use rayon::prelude::*;
use serde::Serialize;

#[derive(StructOpt, Debug)]
#[structopt(
    about = "Noah tool to generate necessary zero-knowledge proof parameters.",
    rename_all = "kebab-case"
)]
enum Actions {
    /// Generates the verifying key for anonymous transfer
    TRANSFER { directory: PathBuf },

    /// Generates the verifying key for ABAR to BAR transform
    ABAR_TO_BAR { directory: PathBuf },

    /// Generates the verifying key for BAR to ABAR transform
    BAR_TO_ABAR { directory: PathBuf },

    /// Generates the verifying key for AR to ABAR transform
    AR_TO_ABAR { directory: PathBuf },

    /// Generates the verifying key for ABAR to AR transform
    ABAR_TO_AR { directory: PathBuf },

    /// Generates the uniform reference string for Bulletproof (over the Curve25519 curve).
    BULLETPROOF_OVER_CURVE25519 { directory: PathBuf },

    /// Generates the uniform reference string for Bulletproof (over the Secq256k1 curve).
    BULLETPROOF_OVER_SECQ256K1 { directory: PathBuf },

    /// Generates the uniform reference string for Bulletproof (over the Zorro curve),
    BULLETPROOF_OVER_ZORRO { directory: PathBuf },

    /// Cut the SRS, adapt to Lagrange, and only save the minimum 2^11, 2^12, and 2^13 padding
    CUT_SRS { directory: PathBuf },

    /// Generates all necessary parameters
    ALL { directory: PathBuf },
}

fn main() {
    use Actions::*;
    let action = Actions::from_args();
    match action {
        TRANSFER { directory } => {
            gen_transfer_vk(directory.clone(), SECP256K1);
            gen_transfer_vk(directory, ED25519);
        }

        ABAR_TO_BAR { directory } => {
            gen_abar_to_bar_vk(directory);
        }

        BAR_TO_ABAR { directory } => {
            gen_bar_to_abar_vk(directory);
        }

        AR_TO_ABAR { directory } => {
            gen_ar_to_abar_vk(directory);
        }

        ABAR_TO_AR { directory } => {
            gen_abar_to_ar_vk(directory);
        }

        BULLETPROOF_OVER_CURVE25519 { directory } => gen_bulletproof_curve25519_urs(directory),

        BULLETPROOF_OVER_SECQ256K1 { directory } => gen_bulletproof_secq256k1_urs(directory),

        BULLETPROOF_OVER_ZORRO { directory } => gen_bulletproof_zorro_urs(directory),

        CUT_SRS { directory } => cut_srs(directory),

        ALL { directory } => gen_all(directory),
    };
}

// cargo run --release --features="gen no_vk" --bin gen-params transfer "./parameters"
fn gen_transfer_vk(directory: PathBuf, address_format: AddressFormat) {
    println!(
        "Generating verifying keys for anonymous transfer for 1..{} payers, 1..{} payees ...",
        MAX_ANONYMOUS_RECORD_NUMBER_STANDARD, MAX_ANONYMOUS_RECORD_NUMBER_STANDARD
    );

    match address_format {
        AddressFormat::SECP256K1 => println!("... for secp256k1"),
        AddressFormat::ED25519 => println!("... for ed25519"),
    }

    let transfer_params = VerifierParams::get_abar_to_abar(1, 1, address_format).unwrap();
    let (common, _) = transfer_params.split().unwrap();
    let common_ser = bincode::serialize(&common).unwrap();

    let mut common_path = directory.clone();
    common_path.push("transfer-vk-common.bin");
    save_to_file(&common_ser, common_path);

    let specials_sync = Arc::new(Mutex::new(BTreeMap::<(usize, usize), Vec<u8>>::new()));

    let is: Vec<usize> = (1..=MAX_ANONYMOUS_RECORD_NUMBER_STANDARD)
        .map(|i| i)
        .collect();
    is.par_iter().for_each(|i| {
        let max_receiver = if *i == 1 {
            MAX_ANONYMOUS_RECORD_NUMBER_ONE_INPUT
        } else {
            MAX_ANONYMOUS_RECORD_NUMBER_STANDARD
        };

        let js: Vec<usize> = (1..=max_receiver).map(|j| j).collect();
        js.par_iter().for_each(|j| {
            println!("generating {} payers & {} payees", i, j);
            let node_params = VerifierParams::get_abar_to_abar(*i, *j, address_format).unwrap();
            println!(
                "the size of the constraint system for {} payers & {} payees: {}",
                i, j, node_params.shrunk_cs.size
            );
            let (_, special) = node_params.split().unwrap();
            specials_sync
                .lock()
                .unwrap()
                .insert((*i, *j), bincode::serialize(&special).unwrap());
        });
    });

    let is: Vec<usize> = (MAX_ANONYMOUS_RECORD_NUMBER_STANDARD + 1
        ..=MAX_ANONYMOUS_RECORD_NUMBER_CONSOLIDATION_SENDER)
        .map(|i| i)
        .collect();
    is.par_iter().for_each(|i| {
        let js: Vec<usize> = (1..=MAX_ANONYMOUS_RECORD_NUMBER_CONSOLIDATION_RECEIVER)
            .map(|j| j)
            .collect();
        js.par_iter().for_each(|j| {
            println!("generating {} payers & {} payees", i, j);
            let node_params = VerifierParams::get_abar_to_abar(*i, *j, address_format).unwrap();
            println!(
                "the size of the constraint system for {} payers & {} payees: {}",
                i, j, node_params.shrunk_cs.size
            );
            let (_, special) = node_params.split().unwrap();
            specials_sync
                .lock()
                .unwrap()
                .insert((*i, *j), bincode::serialize(&special).unwrap());
        });
    });

    let mut specials = BTreeMap::<(usize, usize), Vec<u8>>::new();
    for (idx, v) in specials_sync.lock().unwrap().iter() {
        specials.insert(*idx, v.clone());
    }

    let specials_ser = bincode::serialize(&specials).unwrap();
    let mut specials_path = directory.clone();
    match address_format {
        SECP256K1 => specials_path.push("transfer-vk-secp256k1-specific.bin"),
        ED25519 => specials_path.push("transfer-vk-ed25519-specific.bin"),
    }
    save_to_file(&specials_ser, specials_path);
}

// cargo run --release --features="gen no_vk" --bin gen-params abar-to-bar "./parameters"
fn gen_abar_to_bar_vk(path: PathBuf) {
    println!("Generating the verifying key for ABAR TO BAR for secp256k1 ...");
    let mut new_path = path.clone();
    let user_params = ProverParams::gen_abar_to_bar(SECP256K1).unwrap();
    let node_params = VerifierParams::from(user_params);
    println!(
        "the size of the constraint system for ABAR TO BAR for secp256k1: {}",
        node_params.shrunk_cs.size
    );
    let bytes = bincode::serialize(&node_params).unwrap();
    new_path.push("abar-to-bar-vk-secp256k1.bin");
    save_to_file(&bytes, new_path);

    let start = std::time::Instant::now();
    let _n: VerifierParams = bincode::deserialize(&bytes).unwrap();
    let elapsed = start.elapsed();
    println!("Deserialize time: {:.2?}", elapsed);

    println!("Generating the verifying key for ABAR TO BAR for ed25519 ...");
    let mut new_path = path.clone();
    let user_params = ProverParams::gen_abar_to_bar(ED25519).unwrap();
    let node_params = VerifierParams::from(user_params);
    println!(
        "the size of the constraint system for ABAR TO BAR for ed25519: {}",
        node_params.shrunk_cs.size
    );
    let bytes = bincode::serialize(&node_params).unwrap();
    new_path.push("abar-to-bar-vk-ed25519.bin");
    save_to_file(&bytes, new_path);

    let start = std::time::Instant::now();
    let _n: VerifierParams = bincode::deserialize(&bytes).unwrap();
    let elapsed = start.elapsed();
    println!("Deserialize time: {:.2?}", elapsed);
}

// cargo run --release --features="gen no_vk" --bin gen-params bar-to-abar "./parameters"
fn gen_bar_to_abar_vk(mut path: PathBuf) {
    println!("Generating the verifying key for BAR TO ABAR ...");

    let user_params = ProverParams::gen_bar_to_abar().unwrap();
    let node_params = VerifierParams::from(user_params);
    println!(
        "the size of the constraint system for BAR TO ABAR: {}",
        node_params.shrunk_cs.size
    );
    let bytes = bincode::serialize(&node_params).unwrap();
    path.push("bar-to-abar-vk.bin");
    save_to_file(&bytes, path);

    let start = std::time::Instant::now();
    let _n: VerifierParams = bincode::deserialize(&bytes).unwrap();
    let elapsed = start.elapsed();
    println!("Deserialize time: {:.2?}", elapsed);
}

// cargo run --release --features="gen no_vk" --bin gen-params ar-to-abar "./parameters"
fn gen_ar_to_abar_vk(mut path: PathBuf) {
    println!("Generating the verifying key for AR TO ABAR ...");

    let user_params = ProverParams::gen_ar_to_abar().unwrap();
    let node_params = VerifierParams::from(user_params);
    println!(
        "the size of the constraint system for AR TO ABAR: {}",
        node_params.shrunk_cs.size
    );
    let bytes = bincode::serialize(&node_params).unwrap();
    path.push("ar-to-abar-vk.bin");
    save_to_file(&bytes, path);

    let start = std::time::Instant::now();
    let _n: VerifierParams = bincode::deserialize(&bytes).unwrap();
    let elapsed = start.elapsed();
    println!("Deserialize time: {:.2?}", elapsed);
}

// cargo run --release --features="gen no_vk" --bin gen-params abar-to-ar "./parameters"
fn gen_abar_to_ar_vk(path: PathBuf) {
    println!("Generating the verifying key for ABAR TO AR for secp256k1 ...");
    let mut new_path = path.clone();
    let user_params = ProverParams::gen_abar_to_ar(SECP256K1).unwrap();
    let node_params = VerifierParams::from(user_params);
    println!(
        "the size of the constraint system for ABAR TO AR for secp256k1: {}",
        node_params.shrunk_cs.size
    );
    let bytes = bincode::serialize(&node_params).unwrap();
    new_path.push("abar-to-ar-vk-secp256k1.bin");
    save_to_file(&bytes, new_path);

    let start = std::time::Instant::now();
    let _n: VerifierParams = bincode::deserialize(&bytes).unwrap();
    let elapsed = start.elapsed();
    println!("Deserialize time: {:.2?}", elapsed);

    println!("Generating the verifying key for ABAR TO AR for ed25519 ...");
    let mut new_path = path.clone();
    let user_params = ProverParams::gen_abar_to_ar(ED25519).unwrap();
    let node_params = VerifierParams::from(user_params);
    println!(
        "the size of the constraint system for ABAR TO AR for ed25519: {}",
        node_params.shrunk_cs.size
    );
    let bytes = bincode::serialize(&node_params).unwrap();
    new_path.push("abar-to-ar-vk-ed25519.bin");
    save_to_file(&bytes, new_path);

    let start = std::time::Instant::now();
    let _n: VerifierParams = bincode::deserialize(&bytes).unwrap();
    let elapsed = start.elapsed();
    println!("Deserialize time: {:.2?}", elapsed);
}

// cargo run --release --features="gen no_urs no_srs no_vk" --bin gen-params bulletproof-over-curve25519 "./parameters"
fn gen_bulletproof_curve25519_urs(mut path: PathBuf) {
    println!("Generating Bulletproof(over the Curve25519 curve) uniform reference string ...");

    let pp = BulletproofParams::default();
    let bytes = bincode::serialize(&pp).unwrap();
    path.push("bulletproof-curve25519-urs.bin");
    save_to_file(&bytes, path);

    let start = std::time::Instant::now();
    let _n: BulletproofParams = bincode::deserialize(&bytes).unwrap();
    let elapsed = start.elapsed();
    println!("Deserialize time: {:.2?}", elapsed);
}

// cargo run --release --features="gen no_urs no_srs no_vk" --bin gen-params bulletproof-over-secq256k1 "./parameters"
fn gen_bulletproof_secq256k1_urs(mut path: PathBuf) {
    println!("Generating Bulletproof(over the Secq256k1 curve) uniform reference string ...");

    let bp_gens = Secq256k1BulletproofGens::new(ANON_XFR_BP_GENS_LEN, 1);
    let mut bytes = Vec::new();
    bp_gens
        .serialize_with_mode(&mut bytes, Compress::No)
        .unwrap();
    path.push("bulletproof-secq256k1-urs.bin");
    save_to_file(&bytes, path);

    let start = std::time::Instant::now();
    let reader = ark_std::io::BufReader::new(bytes.as_slice());
    let _bp_gens =
        Secq256k1BulletproofGens::deserialize_with_mode(reader, Compress::No, Validate::No)
            .unwrap();
    println!("Deserialize time: {:.2?}", start.elapsed());
}

// cargo run --release --features="gen no_urs no_srs no_vk" --bin gen-params bulletproof-over-zorro "./parameters"
fn gen_bulletproof_zorro_urs(mut path: PathBuf) {
    println!("Generating Bulletproof(over the Secq256k1 curve) uniform reference string ...");

    let bp_gens = ZorroBulletproofGens::new(ANON_XFR_BP_GENS_LEN, 1);
    let mut bytes = Vec::new();
    bp_gens
        .serialize_with_mode(&mut bytes, Compress::No)
        .unwrap();
    path.push("bulletproof-zorro-urs.bin");
    save_to_file(&bytes, path);

    let start = std::time::Instant::now();
    let reader = ark_std::io::BufReader::new(bytes.as_slice());
    let _bp_gens =
        ZorroBulletproofGens::deserialize_with_mode(reader, Compress::No, Validate::No).unwrap();
    println!("Deserialize time: {:.2?}", start.elapsed());
}

// cargo run --release --features="gen no_vk" --bin gen-params cut-srs "./parameters"
fn cut_srs(mut path: PathBuf) {
    let srs = SRS.unwrap();
    let KZGCommitmentSchemeBLS {
        public_parameter_group_1,
        public_parameter_group_2,
    } = KZGCommitmentSchemeBLS::from_unchecked_bytes(&srs).unwrap();

    if public_parameter_group_1.len() == 2057 {
        println!("Already complete");
        return;
    }

    let mut new_group_1 = vec![BLSG1::default(); 2057];
    new_group_1[0..2051].copy_from_slice(&public_parameter_group_1[0..2051]);
    new_group_1[2051..2054].copy_from_slice(&public_parameter_group_1[4096..4099]);
    new_group_1[2054..2057].copy_from_slice(&public_parameter_group_1[8192..8195]);

    let new_srs = KZGCommitmentSchemeBLS {
        public_parameter_group_2,
        public_parameter_group_1: new_group_1,
    };

    let bytes = new_srs.to_unchecked_bytes().unwrap();
    path.push("srs-padding.bin");
    save_to_file(&bytes, path);
}

// cargo run --release --features="gen no_vk" --bin gen-params all "./parameters"
fn gen_all(directory: PathBuf) {
    gen_transfer_vk(directory.clone(), SECP256K1);
    gen_transfer_vk(directory.clone(), ED25519);
    gen_abar_to_bar_vk(directory.clone());
    gen_bar_to_abar_vk(directory.clone());
    gen_ar_to_abar_vk(directory.clone());
    gen_abar_to_ar_vk(directory.clone());
    gen_bulletproof_curve25519_urs(directory.clone());
    gen_bulletproof_secq256k1_urs(directory.clone());
    gen_bulletproof_zorro_urs(directory.clone());
    cut_srs(directory)
}
