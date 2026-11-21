//! Prints the scripted round's determinism checkpoints, one per line, to diff against WASM.

use cake_core::replay::{build_scripted_inputs, replay};

fn main() {
    for checkpoint in replay(&build_scripted_inputs()) {
        println!("{checkpoint:016x}");
    }
}
