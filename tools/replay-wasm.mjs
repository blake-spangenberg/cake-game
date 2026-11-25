// Prints the scripted round's determinism checkpoints from the WASM build, one per line: on the
// placeholder cake, then on each packed cake named by id (`assets/cakes/<id>.cake.br`), as the
// core's `replay` example does natively.
// Usage: node replay-wasm.mjs <wasm-bindgen output dir> [id...]
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { brotliDecompressSync } from "node:zlib";

if (process.argv.length < 3) throw new Error("usage: node replay-wasm.mjs <wasm-bindgen output dir> [id...]");
const wasmDir = pathToFileURL(`${process.argv[2]}/`);
const { initSync, replay_script, replay_cake_script } = await import(new URL("cake_wasm.js", wasmDir));
initSync({ module: readFileSync(new URL("cake_wasm_bg.wasm", wasmDir)) });
const checkpoints = [...replay_script()];
for (const id of process.argv.slice(3)) {
  const cakeFile = brotliDecompressSync(readFileSync(new URL(`../assets/cakes/${id}.cake.br`, import.meta.url)));
  checkpoints.push(...replay_cake_script(cakeFile));
}
for (const checkpoint of checkpoints) {
  console.log(checkpoint.toString(16).padStart(16, "0"));
}
