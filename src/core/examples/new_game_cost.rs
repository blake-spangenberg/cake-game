//! Times loading a round's cake by stage: decoding its file, which validates it, building the
//! `Cake` every world on it shares, and building one world on that. Usage:
//! `new_game_cost <file.cake>...`. Prints the least of several runs in ms, since other work on the
//! machine only ever adds time.

use std::sync::Arc;
use std::time::Instant;

use cake_core::cakes::CakeModel;
use cake_core::knife::START_TILT_RAD;
use cake_core::sim::{Cake, World};
use cake_core::triangle_tree::TriangleTree;

const RUNS: u32 = 7;

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    assert!(!paths.is_empty(), "usage: new_game_cost <file.cake>...");
    for path in &paths {
        let bytes = std::fs::read(path).expect("cake file readable");
        let model = CakeModel::decode(&bytes).expect("cake file playable");
        let mesh = &model.mesh;
        let tree = TriangleTree::build(&mesh.positions, &mesh.triangles);
        let shared = Arc::new(Cake::build(model.clone()));
        println!("{path}: {} vertices, {} triangles", mesh.positions.len(), mesh.triangles.len());
        println!("  decode {:.2}", time_ms(|| CakeModel::decode(&bytes)));
        println!("    validate {:.2}", time_ms(|| model.validate()));
        println!("      closed mesh {:.2}", time_ms(|| mesh.validate_closed()));
        println!("  shared cake {:.2}", time_ms(|| Cake::build(model.clone())));
        println!("    tree {:.2}", time_ms(|| TriangleTree::build(&mesh.positions, &mesh.triangles)));
        println!("    width range {:.2}", time_ms(|| mesh.calc_width_range_m(Some(&tree))));
        println!("  world {:.2}", time_ms(|| World::create(shared.clone(), START_TILT_RAD, None)));
    }
}

fn time_ms<T>(mut run: impl FnMut() -> T) -> f64 {
    (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            std::hint::black_box(run());
            start.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::INFINITY, f64::min)
}
