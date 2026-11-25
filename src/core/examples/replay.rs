//! Prints the scripted round's determinism checkpoints, one per line, to diff against WASM: on
//! the placeholder cake, then on each packed cake named by id (`assets/cakes/<id>.cake.br`).
//! Usage: `replay [id...]`.

use std::io::Read;
use std::path::Path;

use cake_core::cakes::CakeModel;
use cake_core::replay::{build_scripted_inputs, replay, replay_script_on};

mod common;

fn main() {
    let mut checkpoints = replay(&build_scripted_inputs());
    for id in std::env::args().skip(1) {
        checkpoints.extend(replay_script_on(fetch_packed_cake(&id)));
    }
    for checkpoint in checkpoints {
        println!("{checkpoint:016x}");
    }
}

fn fetch_packed_cake(id: &str) -> CakeModel {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../assets/cakes/{id}.cake.br"));
    let packed = std::fs::read(&path)
        .unwrap_or_else(|error| common::exit_with(&format!("cannot read {}: {error}", path.display())));
    let mut bytes = Vec::new();
    if let Err(error) = brotli_decompressor::Decompressor::new(packed.as_slice(), 4096).read_to_end(&mut bytes) {
        common::exit_with(&format!("cannot unpack {}: {error}", path.display()));
    }
    CakeModel::decode(&bytes)
        .unwrap_or_else(|error| common::exit_with(&format!("{} is not a playable cake: {error:?}", path.display())))
}
