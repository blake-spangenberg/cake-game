//! The deterministic cake-cutting simulation shared by the browser client (as WASM) and the
//! server (natively). Determinism across both is a hard requirement: transcendental math only
//! through `libm`, no iteration over hashed collections, no threads.

pub mod cakes;
pub mod cut;
pub mod input;
pub mod knife;
pub mod mesh;
pub mod replay;
pub mod sim;
pub mod table;
pub mod triangle_tree;
