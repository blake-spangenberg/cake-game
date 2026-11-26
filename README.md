# Cake Game - Scrum Edition
3D multiplayer browser game inspired by German game show *Schlag den Henssler*. Each player grabs and places a 3D cake, aims a knife, and cuts, and the cut closest to two equal halves by weight wins.

On Windows, run `tools/install-bash.ps1` once in PowerShell to get Git bash and GNU make. Then:
```sh
make install   # Rust toolchain, Node, wasm-bindgen, client packages
make run       # offline practice: Vite's dev server at http://localhost:5173 with hot reload, and no multiplayer server
make start     # the online game: build the production client and run cake-server (lobby and multiplayer) at http://127.0.0.1:8080, or at SERVE_LISTEN=<addr:port>
make stop      # stop the Linux service
make test      # format check, lints, tests, and the native-vs-WASM determinism check
make upgrade   # bump the toolchain and every dependency to its latest release, past semver; run make test after
```

`make run` and `make start` differ on every OS, not only in the service. `make run` is the dev loop: the page reloads as you edit, and it connects to no server, so you play alone. `make start` is what guests get, the lobby and the multiplayer loop. Off Linux it runs `cake-server` in the foreground until you stop it. On Linux it (re)starts `cake-server` as a systemd user service (`tools/service.sh`) that outlives the shell and reboots, and needs a `deploy.env` copied from `deploy.env.example`.
