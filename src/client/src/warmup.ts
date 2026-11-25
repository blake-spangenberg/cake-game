import { Game } from "@wasm/cake_wasm.js";
import { waitFrames } from "./frames";

/**
 * Builds a throwaway world on `cakeFile` and steps it once with the input `buildInput` gives it,
 * each a frame apart, then frees it, so the WASM a tick runs is compiled before a round needs it:
 * a browser compiles WASM lazily on a function's first call, which cost a fresh browser's first
 * round frame 17-74 ms. Every world is independent, so no other world's state or hash changes.
 */
export async function stepThrowawayWorld(cakeFile: Uint8Array, buildInput: (game: Game) => Int32Array): Promise<void> {
  await waitFrames(1);
  const game = new Game(undefined, cakeFile);
  try {
    await waitFrames(1);
    game.step(buildInput(game));
  } finally {
    game.free();
  }
}
