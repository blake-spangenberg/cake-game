// Prints the scripted round's determinism checkpoints from the WASM build, one per line.
// Usage: node replay-wasm.mjs <wasm-bindgen output dir>
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

if (process.argv.length !== 3) throw new Error("usage: node replay-wasm.mjs <wasm-bindgen output dir>");
const wasmDir = pathToFileURL(`${process.argv[2]}/`);
const { initSync, replay_script } = await import(new URL("cake_wasm.js", wasmDir));
initSync({ module: readFileSync(new URL("cake_wasm_bg.wasm", wasmDir)) });
for (const checkpoint of replay_script()) {
  console.log(checkpoint.toString(16).padStart(16, "0"));
}
