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
- Inputs are quantized before they enter the simulation: the client casts rays and eases controls in TypeScript, then sends and simulates the same fixed-point values. Nothing computed with JS `Math` functions reaches the core unquantized.
- `src/client/src/controls.ts` hand-encodes the core's input wire format, so it changes together with `src/core/src/input.rs`.
- Sim code must stay deterministic across native and WASM: transcendental math only through `libm` or nalgebra's own functions, no iteration over `HashMap` or `HashSet` (randomly seeded), no threads, no Rapier `parallel` or `simd` features. One exact Rapier version.
- `Cut!` carries its tick. The server replays to it, freezes, and cuts. Timeout is the server cutting at the round's final tick, and inputs stamped after it are dropped.
- Spectators receive poses streamed from the server's replay. A late joiner gets the current poses and the round state, with no input history.
- Desync is the one new failure mode: a divergent replay would score a cut the player never saw. The client sends a world-state hash about once a second, and the server logs any mismatch.
- Determinism test: one recorded input log replayed natively and in WASM must give the same world-snapshot hash at every checkpoint. `make test` runs it, so it reruns on every Rapier or Rust toolchain upgrade.

## Cut and weigh
Each model has uniform density, so grams are volume times a constant. Scoring clips the mesh's triangles against the cut plane and sums the signed tetrahedron volumes to a point on that plane, so the cap contributes nothing and needs no triangulation. Drawing the halves caps each cut face through the stencil buffer, which needs no cap geometry but, like scoring, depends on the mesh being closed. Real cap geometry (core-built or `manifold-3d`) waits until cut faces need textured layers.

## Asset pipeline
Most free models are not watertight or are built of overlapping parts, so `tools/bake-cake.sh <model.glb> <out.cake>` repairs and validates each one, and rejects what it cannot fix, to repair by hand or drop. Two steps:
- `tools/repair-model.py` (Python through `uv`, with `trimesh`, `pymeshfix`, and `manifold3d`) welds vertices, closes holes only on pieces that are not already manifold, unions the pieces into one solid so overlapping boxes become their outer surface, drops specks under 1% of the volume, and stops where several large pieces remain (`--keep-largest` keeps one, as for a pair of shoes). It decimates a model over `--max-triangles` (40000 by default) by quadric edge collapse, since the bake's collider decomposition takes minutes on a quarter-million triangles. It then scales the longest side to `--size-m` (0.24 by default), centers the model over the stand, rests it on y = 0, and carries its coloring as per-vertex sRGB colors sampled at each output vertex from the nearest point of the source's textures, vertex colors, or material colors. A texture is first blurred to the output's vertex spacing, so a pattern finer than the vertices averages out instead of aliasing into blotches, and `--smooth-colors` (2 passes by default) then smooths colors across similar-colored neighbors. `--delight` (0 to 1) divides out the slow brightness trend a photogrammetry scan baked in, keeping each region's detail and stopping at sharp brightness steps. Headless Blender was the first plan, but `winget` could not download it, and these libraries need no install beyond `uv`
- `src/bake` (`cake-bake`) reads that glb, splits the mesh into convex hulls with `parry3d`'s V-HACD, cuts each hull to at most `MAX_HULL_VERTICES` vertices by dropping the vertices whose loss costs the least volume, since a hull face with many vertices jitters on the table and never sleeps, and writes the `.cake` file. The core's `CakeModel::validate` is the gate: closed and outward-wound mesh, every side within `MAX_CAKE_EXTENT_M`, resting on y = 0 within the stand's plate, usable hulls

Mass and center of mass come from the render mesh when the world is built, so the hulls only carry collision. Baked cakes are tracked as brotli-packed `assets/cakes/<id>.cake.br` (`tools/cake-pack.mjs`, about 38% of the raw size, since float positions barely compress), and `make cakes`, which `make run` runs, unpacks the ones that changed into `build/public/cakes/`, the client's public directory, and deletes unpacked cakes whose source is gone. `make test` checks that `src/client/src/cakes.ts` lists exactly the packed cakes. `tools/bake-cake.sh` packs an output named `*.cake.br`. The `.cake` file (`CakeModel::encode` and `decode`) is little-endian: `CAKE`, a version, the vertex, triangle, and hull counts, each hull's vertex count, then the mesh's positions, each hull's vertices, the triangle indices, and one sRGB color of three bytes per mesh vertex.

## Ruled out
- Bevy and Godot's web export: download size works against near-instant late joins, and Godot's physics promises no cross-platform determinism
- Babylon.js: more built in, but heavier, and its bundled Havok physics would go unused
- Jolt: its WASM build is deterministic only when compiled with `CROSS_PLATFORM_DETERMINISTIC`, which the prebuilt packages lack
- Trusted clients reporting their own poses: simpler and immune to desync, but open to cheating from dev tools
