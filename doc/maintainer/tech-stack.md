# Tech stack
The settled stack for the browser proof of concept and beyond.

## Stack
| Layer | Pick |
|---|---|
| Core | Rust crate holding the simulation (`rapier3d` with `enhanced-determinism`, the grab, the knife) and the cut, compiled to WASM for the client and natively for the server. Toolchain pinned in `rust-toolchain.toml` |
| Client | TypeScript, Vite, Three.js (WebGL2, shadow maps), with the core loaded through `wasm-bindgen` |
| Server | Rust, `axum` with its WebSocket support on `tokio`, linking the core natively. Serves the client build and the WebSocket on one plain-HTTP port |
| HTTPS | The host's existing reverse proxy terminates TLS and routes to the server. Guests already reach the home machine |
| Assets | glTF 2.0 (`.glb`) |
| Tests | `cargo test` for the core, plus the determinism test below |
| Dev environment | Windows, `rustup` with the `wasm32-unknown-unknown` target, Node LTS for Vite, VS Code with `rust-analyzer` |

## Server authority
Clients are untrusted. The server replays every player's inputs in its own copy of that player's world and scores the cut it computes, so no one can fake a cut. Every player's cake and knife are independent of everyone else's, so no rollback is needed: the local player simulates instantly, and the server runs a round trip behind.
- The simulation steps at a fixed 60 Hz and counts ticks. Clients send tick-stamped inputs, never poses.
- Inputs are quantized before they enter the simulation: the client casts the mouse ray in TypeScript, then sends and simulates the same fixed-point target. Nothing computed with JS `Math` functions reaches the core unquantized.
- `client/src/controls.ts` hand-encodes the core's input wire format, so it changes together with `core/src/input.rs`.
- Sim code must stay deterministic across native and WASM: transcendental math only through `libm` or nalgebra's own functions, no iteration over `HashMap` or `HashSet` (randomly seeded), no threads, no Rapier `parallel` or `simd` features. One exact Rapier version.
- `Cut!` carries its tick. The server replays to it, freezes, and cuts. Timeout is the server cutting at the round's final tick, and inputs stamped after it are dropped.
- Spectators receive poses streamed from the server's replay. A late joiner gets the current poses and the round state, with no input history.
- Desync is the one new failure mode: a divergent replay would score a cut the player never saw. The client sends a world-state hash about once a second, and the server logs any mismatch.
- Determinism test: one recorded input log replayed natively and in WASM must give the same world-snapshot hash at every checkpoint. `make test` runs it, so it reruns on every Rapier or Rust toolchain upgrade.

## Cut and weigh
Each model has uniform density, so grams are volume times a constant. Scoring clips the mesh's triangles against the cut plane and sums the signed tetrahedron volumes to a point on that plane, so the cap contributes nothing and needs no triangulation. Drawing the halves does need the cap, so the proof of concept chooses between building it in the core and building it in the client with `manifold-3d`. Both depend on the mesh being closed.

## Asset pipeline
Most free models are not watertight, so a scripted step repairs and validates each one, and models it cannot fix are set aside to repair by hand or drop. Per model, the script:
- Repairs it to a closed mesh, likely through headless Blender (`blender --background --python`), which reads and writes glTF
- Rejects it unless every edge borders exactly two consistently wound triangles, and checks its bounds against the cutting region and blade span
- Precomputes the physics collider as a convex decomposition (`parry3d`'s V-HACD), since a dynamic triangle-mesh collider is unstable, and sets mass and center of mass from the render mesh so the cake hangs where its shape says it should

## Ruled out
- Bevy and Godot's web export: download size works against near-instant late joins, and Godot's physics promises no cross-platform determinism
- Babylon.js: more built in, but heavier, and its bundled Havok physics would go unused
- Jolt: its WASM build is deterministic only when compiled with `CROSS_PLATFORM_DETERMINISTIC`, which the prebuilt packages lack
- Trusted clients reporting their own poses: simpler and immune to desync, but open to cheating from dev tools
