import * as THREE from "three";
import init, { Game, get_layout } from "@wasm/cake_wasm.js";
import { Controls } from "./controls";
import { Orbit } from "./orbit";
import { Scene } from "./scene";

/** Ticks a slow frame may catch up on, so a stalled tab does not spiral. */
const MAX_TICKS_PER_FRAME = 5;
/** Frames beyond the simulation's tick rate would show nothing new, so they are not drawn. */
const MAX_FRAMES_PER_S = 60;
const CUT_DESCEND_S = 0.4;
const CUT_ROCK_S = 0.4;
/** How far the knife rocks either way through the cut, a gentle mezzaluna rock. */
const CUT_ROCK_RAD = 0.18;

await init();
const layout = get_layout();
/** Development rounds have no time limit. */
const roundTicks = import.meta.env.DEV ? undefined : layout.round_ticks;
const tickMs = 1000 / layout.ticks_per_s;
const frameMs = 1000 / MAX_FRAMES_PER_S;
const canvas = document.querySelector<HTMLCanvasElement>("#view")!;
const clock = document.querySelector<HTMLDivElement>("#clock")!;
const cutButton = document.querySelector<HTMLButtonElement>("#cut")!;
const result = document.querySelector<HTMLDivElement>("#result")!;
const grams = document.querySelector<HTMLDivElement>("#grams")!;
const score = document.querySelector<HTMLDivElement>("#score")!;

const scene = new Scene(canvas, layout);
const orbit = new Orbit(scene.camera, new THREE.Vector3(0, layout.stand_top_y_m, 0));
let game = new Game(roundTicks);
const controls = new Controls(scene, orbit, layout, game.knife_pose()[0]);
/** The cut in progress: when it started, the knife's travel, and the result to show once it lands. */
let cutting: { startMs: number; fromLowerM: number; toLowerM: number; split: Float64Array } | null = null;
let lagMs = 0;
let lastFrameMs = performance.now();
let nextFrameMs = lastFrameMs;
clock.hidden = roundTicks === undefined;

function startRound(): void {
  game.free();
  game = new Game(roundTicks);
  controls.reset(game.knife_pose()[0]);
  scene.resetCake(game);
  cutting = null;
  result.hidden = true;
}

function startCut(split: Float64Array, nowMs: number): void {
  cutting = { startMs: nowMs, fromLowerM: game.knife_pose()[1], toLowerM: game.cut_lower_m(), split };
  scene.showHalves(game);
}

/** Sweeps the knife down through the cake, rocks it once, then parts the halves and shows the result. */
function animateCut(nowMs: number): void {
  const cut = cutting!;
  const elapsedS = (nowMs - cut.startMs) / 1000;
  const descend = Math.min(elapsedS / CUT_DESCEND_S, 1);
  const rock = THREE.MathUtils.clamp((elapsedS - CUT_DESCEND_S) / CUT_ROCK_S, 0, 1);
  const lowerM = cut.fromLowerM + (cut.toLowerM - cut.fromLowerM) * descend * descend;
  scene.placeKnife(game, lowerM, CUT_ROCK_RAD * Math.sin(2 * Math.PI * rock));
  if (rock < 1 || !result.hidden) return;
  scene.partHalves();
  const [above, below, points] = cut.split;
  const [aboveG, belowG] = [above, below].map((m3) => Math.round(m3 * layout.density_g_per_m3));
  grams.textContent = `${aboveG} g | ${belowG} g`;
  score.textContent = `${(Math.floor(points * 10) / 10).toFixed(1)}`;
  result.hidden = false;
}

function stepSimulation(elapsedMs: number, nowMs: number): void {
  lagMs += elapsedMs;
  let ticks = 0;
  while (lagMs >= tickMs && ticks < MAX_TICKS_PER_FRAME && !cutting) {
    game.step(controls.stepInput(game.knife_pose()[1]));
    lagMs -= tickMs;
    ticks += 1;
    const split = game.find_split();
    if (split) startCut(split, nowMs);
  }
  if (ticks === MAX_TICKS_PER_FRAME || cutting) lagMs = 0;
}

function onFrame(nowMs: number): void {
  requestAnimationFrame(onFrame);
  if (nowMs + 1 < nextFrameMs) return;
  nextFrameMs = Math.max(nextFrameMs + frameMs, nowMs);
  const elapsedMs = nowMs - lastFrameMs;
  lastFrameMs = nowMs;
  stepSimulation(elapsedMs, nowMs);
  orbit.update(elapsedMs / 1000);
  if (cutting) {
    animateCut(nowMs);
  } else {
    scene.sync(game);
  }
  cutButton.disabled = cutting !== null || !game.is_cut_through();
  if (roundTicks !== undefined) clock.textContent = `${Math.ceil((roundTicks - game.tick()) / layout.ticks_per_s)}`;
  scene.render(elapsedMs / 1000);
}

cutButton.addEventListener("click", () => startCut(game.cut(), performance.now()));
document.querySelector("#again")!.addEventListener("click", startRound);
scene.resetCake(game);
requestAnimationFrame(onFrame);
