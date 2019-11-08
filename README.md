![alt text](https://github.com/eianio/zei/raw/master/zei_logo.png)

**Zei: Eian Cryptographic Library**

Zei is a library that provide tools to create and verify public transaction
with confidential data.

Support:
 - Basic Cryptographic tools:
    - ElGamalEncryption in the exponent over generic groups.
    - A Naive Multisignature (concatenation of ed25519 signatures)
       - Future version will support BLS multisignatures
    - Hybrid Encryption using signature key
 - Advanced Cryptographic tools:
   - Anonymous Credentials based on David Pointcheval and Olivier Sanders.
   Short Randomizable Signatures. CT RSA 2015. https://eprint.iacr.org/2015/525.pdf.
   It currently uses BLS12_381 as the underlying pairing
   - Confidential Anonymous Credential Reveal: Allows to encrypt credential attributes
   so that a verifier can check a credential signature without learning the attributes.
   This functionality allows for identity attributes tracking over a public ledger.
   - Chaum Pedersen proofs: Allows to prove in zero-knowledge that a set of Pedersen
   commitments open to the same value. Used in transfers to prove that the input confidential asset
   is the same as the output asset type.
   - Pedersen-ElGamal Equality Proofs: Allows to prove in zero-knowledge that the
   decryption of an Elgamal ciphertexts correctly opens a pedersen commitment.
   Use in transfers that allow tracking amounts and asset type without publicly
   revealing these values.
   - Dlog: Simple proof of knowlege of discrete logarithms over generic groups.
 - Xfr multi-input multi-output UTXO transfers
   - Plain: XfrNote reveal amount and asset type
   - Confidential amount and/or asset type: XfrNote hides amount and/or asset type
   - AssetType mixing: Allows for multiple asset types in a confidential transaction
    Implemented via the Cloak protocol. Currently using Interstellar spacesuite prototype
   - Tracking policies: Allow tracking of amount, asset type, and/or identity
   of asset holders. That is, confidential Xfrs need to provide ciphertexts of
   amount/asset_type and/or identity and prove that this are correctly formed.

# Benchmarks

# Development environment setup

## Install RUST

Run the following script and select option 1)
```
> curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

We need to use rust nightly.
```
> rustup default nightly
```
## Tests

### Run the tests

Run all the tests.

```
> cargo test
```

Run the documentation tests.

```
> cargo test --doc
```

### Test coverage

[Tarpaulin](https://github.com/xd009642/tarpaulin) is a test coverage tool for Rust.
Note that Tarpaulin only supports x86_64 processors running Linux.

Install Tarpaulin:

```
> cargo install cargo-tarpaulin
```

Run Tarpaulin, using a timeout of 120 seconds.

```
> cargo tarpaulin --timeout 120
...
[INFO tarpaulin] Coverage Results:
|| Tested/Total Lines:
|| src/algebra/bls12_381.rs: 186/226
|| src/algebra/bn.rs: 92/124
|| src/algebra/groups.rs: 21/21
|| src/algebra/ristretto.rs: 127/165
|| src/algebra/utils.rs: 7/8
|| src/basic_crypto/elgamal.rs: 112/114
...
```

## Generate and read the documentation

### Standard

```
> cargo doc --open
```

### Visualize dependencies

#### Cargo tree

This tool allows to visualizes crates' dependencies as a tree.

To install:

```
> cargo install cargo-tree
```

To run:

```
> cargo tree

zei v0.0.1 (/home/philippe/repositories/findora/zei)
├── aes-ctr v0.3.0
│   ├── aes-soft v0.3.3
│   │   ├── block-cipher-trait v0.6.2
│   │   │   └── generic-array v0.12.3
│   │   │       └── typenum v1.11.2
│   │   ├── byteorder v1.3.2
│   │   └── opaque-debug v0.2.3
│   │   [dev-dependencies]
│   │   └── block-cipher-trait v0.6.2 (*)
│   ├── ctr v0.3.2
...
```

#### Cargo deps

This tool allows to visualizes crates' dependencies as a graph.

First you need to install graphviz.

For ubuntu:

```
> sudo apt install graphviz
```

Then install cargo-deps:

```
> cargo install cargo-deps
```

Generate the graph of dependencies as an image:

```
> cargo deps --no-transitive-deps | dot -Tpng > graph.png
```

![graph-dependencies](doc/dependencies.png)

## Code formatting

Use the following command to install rustfmt, the tool that allows to format the code
according to some agreed standard defined in `rustfmt.toml`.

```
> rustup component add rustfmt --toolchain nightly
> rustup self update
```

Then to format your code run
```
> cargo fmt
```

# Use of zei library

To install, add the following to your project's `Cargo.toml`:

```toml
[dependencies.zei]
version = "0.0.1"
```

Then, in your library or executable source, add:

```rust
extern crate zei;
```

By default, several `zei`'s tools uses `curve25519-dalek`'s `u64_backend`
feature, which uses Rust's `i128` feature to achieve roughly double the speed as
the `u32_backend` feature.  When targetting 32-bit systems, however, you'll
likely want to compile with
 `cargo build --no-default-features --features="u32_backend"`.
If you're building for a machine with avx2 instructions, there's also the
experimental `avx2_backend`.  To use it, compile with
`RUSTFLAGS="-C target_cpu=native" cargo build --no-default-features --features="avx2_backend"`
