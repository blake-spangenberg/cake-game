import * as THREE from "three";
import init, { Game, get_layout } from "@wasm/cake_wasm.js";
import { CAKES, type CakeEntry, buildCreditHtml, fetchCakeFile } from "./cakes";
import { Controls } from "./controls";
import { Orbit } from "./orbit";
import { Scene } from "./scene";

/** Ticks a slow frame may catch up on, so a stalled tab does not spiral. */
const MAX_TICKS_PER_FRAME = 5;
/** Frames beyond the simulation's tick rate would show nothing new, so they are not drawn. */
const MAX_FRAMES_PER_S = 60;
const CUT_DESCEND_S = 0.4;
const CUT_ROCK_S = 1.2;
/** How far the knife rocks either way through the cut, a gentle mezzaluna rock. */
const CUT_ROCK_RAD = 0.36;
/** Extra rock time, as a share of `CUT_ROCK_S`, for the knife to swing a little past center and settle. */
const CUT_SETTLE_SHARE = 0.5;
/** The picker's value for a random cake each round. */
const RANDOM_CAKE = "random";

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
const picker = document.querySelector<HTMLSelectElement>("#cake-picker")!;
const credit = document.querySelector<HTMLDivElement>("#credit")!;

const scene = new Scene(canvas, layout);
/** The camera orbits halfway between the resting knife's edge and the stand's plate. */
const orbit = new Orbit(scene.camera, new THREE.Vector3(0, (layout.pivot_y_m + layout.stand_top_y_m) / 2, 0));
picker.append(new Option("Random", RANDOM_CAKE), ...CAKES.map((cake) => new Option(cake.title, cake.id)));
const firstCake = pickCake();
credit.replaceChildren(buildCreditHtml(firstCake));
let game = new Game(roundTicks, await fetchCakeFile(firstCake));
const controls = new Controls(scene, orbit, layout, game.get_knife_pose()[0]);
/** The cut in progress: when it started, the knife's travel, and the result to show once it lands. */
let cutting: { startMs: number; fromLowerM: number; toLowerM: number; split: Float64Array } | null = null;
let lagMs = 0;
let lastFrameMs = performance.now();
let nextFrameMs = lastFrameMs;
clock.hidden = roundTicks === undefined;

/** The cake the picker names, a random one for Random. */
function pickCake(): CakeEntry {
  if (picker.value === RANDOM_CAKE) return CAKES[Math.floor(Math.random() * CAKES.length)];
  return CAKES.find((cake) => cake.id === picker.value)!;
}

async function startRound(): Promise<void> {
  const cake = pickCake();
  const file = await fetchCakeFile(cake);
  credit.replaceChildren(buildCreditHtml(cake));
  game.free();
  game = new Game(roundTicks, file);
  controls.reset(game.get_knife_pose()[0]);
  await scene.resetCake(game);
  cutting = null;
  result.hidden = true;
}

function startCut(split: Float64Array, nowMs: number): void {
  cutting = { startMs: nowMs, fromLowerM: game.get_knife_pose()[1], toLowerM: game.calc_cut_lower_m(), split };
  scene.showHalves(game);
}

/**
 * The knife's tilt in radians, given how far through its rock it is: 0 to 1 for one full
 * back-and-forth, then up to `1 + CUT_SETTLE_SHARE` for the swing past center and back. The settle
 * continues the same sine under a fade from 1 to 0 with no slope at either end, so it neither
 * jumps nor stops abruptly, and swings out to about an eighth of the rock's extent.
 */
function calcRockRad(rock: number): number {
  const settle = Math.max(rock - 1, 0) / CUT_SETTLE_SHARE;
  const fade = settle >= 1 ? 0 : ((1 + Math.cos(Math.PI * settle)) / 2) ** 3;
  return CUT_ROCK_RAD * Math.sin(2 * Math.PI * rock) * fade;
}

/** Sweeps the knife down through the cake, rocks it and lets it settle, then parts the halves and shows the result. */
function animateCut(nowMs: number): void {
  const cut = cutting!;
  const elapsedS = (nowMs - cut.startMs) / 1000;
  const descend = Math.min(elapsedS / CUT_DESCEND_S, 1);
  const rockEnd = 1 + CUT_SETTLE_SHARE;
  const rock = THREE.MathUtils.clamp((elapsedS - CUT_DESCEND_S) / CUT_ROCK_S, 0, rockEnd);
  const lowerM = cut.fromLowerM + (cut.toLowerM - cut.fromLowerM) * descend * descend;
  scene.placeKnife(game, lowerM, calcRockRad(rock));
  if (rock < rockEnd || !result.hidden) return;
  scene.partHalves();
  const [above, below, points] = cut.split;
  const [aboveG, belowG] = [above, below].map((m3) => Math.round(m3 * layout.density_g_per_m3));
  grams.textContent = `${aboveG} g | ${belowG} g`;
  score.textContent = `${points.toFixed(1)}%`;
  result.hidden = false;
}

function stepSimulation(elapsedMs: number, nowMs: number): void {
  lagMs += elapsedMs;
  let ticks = 0;
  while (lagMs >= tickMs && ticks < MAX_TICKS_PER_FRAME && !cutting) {
    game.step(controls.stepInput(game));
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
  if (roundTicks !== undefined) clock.textContent = `${Math.ceil((roundTicks - game.get_tick()) / layout.ticks_per_s)}`;
  scene.render(elapsedMs / 1000);
}

cutButton.addEventListener("click", () => startCut(game.cut(), performance.now()));
document.querySelector("#again")!.addEventListener("click", () => void startRound());
picker.addEventListener("change", () => {
  picker.blur();
  void startRound();
});
await scene.resetCake(game);
requestAnimationFrame(onFrame);
