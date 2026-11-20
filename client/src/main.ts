import init, { Game, get_layout } from "@wasm/cake_wasm.js";
import { Controls } from "./controls";
import { Scene } from "./scene";

const CAMERA_TARGET = [0, 0.06, 0] as const;
const CAMERAS: Record<string, readonly [number, number, number]> = {
  front: [0, 0.55, 1.0],
  side: [1.0, 0.5, 0],
  low: [0, 0.12, 0.9],
  top: [0, 1.3, 0.001],
};
/** Ticks a slow frame may catch up on, so a stalled tab does not spiral. */
const MAX_TICKS_PER_FRAME = 5;

await init();
const layout = get_layout();
const tickMs = 1000 / layout.ticks_per_s;
const canvas = document.querySelector<HTMLCanvasElement>("#view")!;
const clock = document.querySelector<HTMLDivElement>("#clock")!;
const cutButton = document.querySelector<HTMLButtonElement>("#cut")!;
const result = document.querySelector<HTMLDivElement>("#result")!;
const grams = document.querySelector<HTMLDivElement>("#grams")!;

const scene = new Scene(canvas, layout);
let game = new Game();
const controls = new Controls(scene, layout, game.knife_pose());
let lagMs = 0;
let lastFrameMs = performance.now();

function startRound(): void {
  game.free();
  game = new Game();
  controls.reset(game.knife_pose());
  scene.resetCake(game);
  result.hidden = true;
  cutButton.disabled = false;
}

function showCut(split: Float64Array): void {
  scene.showHalves(game);
  const [above, below] = [...split].map((m3) => m3 * layout.density_g_per_m3);
  grams.textContent = `${above.toFixed(2)} g | ${below.toFixed(2)} g - off by ${Math.abs(above - below).toFixed(2)} g`;
  result.hidden = false;
  cutButton.disabled = true;
}

function setCamera(name: string): void {
  const position = CAMERAS[name];
  if (!position) return;
  scene.camera.position.set(...position);
  scene.camera.lookAt(...CAMERA_TARGET);
}

function onFrame(nowMs: number): void {
  lagMs += nowMs - lastFrameMs;
  lastFrameMs = nowMs;
  let ticks = 0;
  while (lagMs >= tickMs && ticks < MAX_TICKS_PER_FRAME && !cutButton.disabled) {
    game.step(controls.encodeInput());
    lagMs -= tickMs;
    ticks += 1;
    const split = game.find_split();
    if (split) showCut(split);
  }
  if (ticks === MAX_TICKS_PER_FRAME) lagMs = 0;
  scene.sync(game);
  const remainingS = Math.ceil((layout.round_ticks - game.tick()) / layout.ticks_per_s);
  clock.textContent = `${remainingS}`;
  scene.render();
  requestAnimationFrame(onFrame);
}

cutButton.addEventListener("click", () => showCut(game.cut()));
document.querySelector("#again")!.addEventListener("click", startRound);
for (const button of document.querySelectorAll<HTMLButtonElement>("#cameras button")) {
  button.addEventListener("click", () => setCamera(button.dataset.camera!));
}
setCamera("front");
scene.resetCake(game);
requestAnimationFrame(onFrame);
