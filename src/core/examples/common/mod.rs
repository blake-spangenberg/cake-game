use std::{env, fs, process};

use cake_core::cakes::{CakeModel, build_slice_missing_cake};

/// The baked cake file named by the first argument, or the placeholder cake with none.
pub fn fetch_model() -> CakeModel {
    let Some(path) = env::args().nth(1) else {
        return build_slice_missing_cake();
    };
    let bytes = fs::read(&path).unwrap_or_else(|error| exit_with(&format!("cannot read {path}: {error}")));
    CakeModel::decode(&bytes).unwrap_or_else(|error| exit_with(&format!("{path} is not a playable cake: {error:?}")))
}

fn exit_with(message: &str) -> ! {
    eprintln!("{message}");
    process::exit(1);
}
