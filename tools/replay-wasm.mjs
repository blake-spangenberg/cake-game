// Prints the scripted round's determinism checkpoints from the WASM build, one per line.
import { readFileSync } from "node:fs";

const wasmDir = new URL("../build/wasm/", import.meta.url);
const { initSync, replay_script } = await import(new URL("cake_wasm.js", wasmDir));
initSync({ module: readFileSync(new URL("cake_wasm_bg.wasm", wasmDir)) });
for (const checkpoint of replay_script()) {
  console.log(checkpoint.toString(16).padStart(16, "0"));
}
