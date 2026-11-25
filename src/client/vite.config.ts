import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
// The Makefile's BUILD_DIR, relative to the repo root, where this agent's WASM build lives.
const buildDir = path.resolve(repoRoot, process.env.BUILD_DIR ?? "build");
const wasmDir = path.join(buildDir, "wasm");

export default defineConfig({
  // Relative asset paths, so the build works behind a reverse proxy at a subpath as at the root.
  base: "./",
  publicDir: `${repoRoot}build/public`,
  resolve: { alias: { "@wasm": wasmDir } },
  server: { fs: { allow: [".", wasmDir] } },
  build: { outDir: path.join(buildDir, "client"), emptyOutDir: true },
});
