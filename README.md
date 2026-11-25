# Zoom Cake Game
A browser game for a Zoom room: each player grabs and places a 3D cake, aims a knife, and cuts, and the cut closest to two equal halves by weight wins.

On Windows, run `tools/install-bash.ps1` once in PowerShell to get Git bash and GNU make. Then:
```sh
make install   # Rust toolchain, Node, wasm-bindgen, client packages
make run       # build the simulation to WASM and serve the client at http://localhost:5173
make serve     # build the client and run the multiplayer server at http://127.0.0.1:8080, or at SERVE_LISTEN=<addr:port>; on Linux, as a service
make stop      # stop the Linux service
make test      # format check, lints, tests, and the native-vs-WASM determinism check
make upgrade   # bump the toolchain and every dependency to its latest release, past semver, then make test
```

The stack and the determinism rules the simulation must follow are in `doc/maintainer/tech-stack.md`.
