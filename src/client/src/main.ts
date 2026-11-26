import * as THREE from "three";
import init, { Game, get_layout } from "@wasm/cake_wasm.js";
import { CAKES, CAKE_KINDS, type CakeEntry, type CakeKind, buildCreditsList, buildRoundCreditHtml, fetchCakeFile } from "./cakes";
import { Controls } from "./controls";
import { Countdown } from "./countdown";
import { calcCutMotion } from "./cutMotion";
import { IS_DEBUG_PAGE } from "./debug";
import { FpsMeter } from "./fpsMeter";
import { waitFrames } from "./frames";
import { Judging } from "./judging";
import { type Screen, setLoadingShown } from "./lobby";
import { type JudgingMessage, type Member, type Net, RANDOM_CAKE, type ResultsMessage, type ResumeMessage, type ServerMessage } from "./net";
import { buildLibrariesList } from "./libraries";
import { Opponents } from "./opponents";
import { Orbit } from "./orbit";
import { watchActivity } from "./profile";
import { fetchPropAssets } from "./props";
import { buildOwnScene, calcRevealFrame } from "./reveal";
import { CakePreloader, freeRoundCake } from "./roundCakes";
import { type PickKind, Scene } from "./scene";
import { RoundSounds, playChop, playLastCutWhistle } from "./sound";
import { formatScore } from "./standings";
import { debugPanel, lobby, room, roundTimer, serverUrl } from "./startup";
import { stepThrowawayWorld } from "./warmup";

/**
 * Ticks a slow frame may step with the live controls. Practice drops any lag past them, so a
 * stalled tab does not spiral. An online round steps the older ticks with the held input instead.
 */
const MAX_TICKS_PER_FRAME = 5;
/** How long a frame may spend stepping the ticks an online round missed, so the page stays responsive while it catches up. */
const CATCH_UP_BUDGET_MS = 40;
/** Frames beyond the simulation's tick rate would show nothing new, so they are not drawn. */
const MAX_FRAMES_PER_S = 60;
/** How long a round's cake that failed to load waits before the page tries it again. */
const LOAD_RETRY_MS = 1000;
/**
 * Frames the scene's build waits for once the room is named: the first shows the room, after which
 * a browser that never drew such a page spends up to 75 ms compiling its raster pipelines, which a
 * WebGL context created meanwhile waits on.
 */
const ROOM_SHOWN_FRAMES = 3;

await init();
const layout = get_layout();
// Started at once, so it is ready by the join.
const propAssets = fetchPropAssets(layout);
/** Development rounds and practice while waiting for an online round have no time limit. An online round takes the server's. */
const offlineRoundTicks = import.meta.env.DEV || serverUrl ? undefined : layout.round_ticks;
let roundTicks = offlineRoundTicks;
const tickMs = 1000 / layout.ticks_per_s;
const frameMs = 1000 / MAX_FRAMES_PER_S;
const canvas = document.querySelector<HTMLCanvasElement>("#view")!;
const clock = document.querySelector<HTMLDivElement>("#clock")!;
const cutButton = document.querySelector<HTMLButtonElement>("#cut")!;
const result = document.querySelector<HTMLDivElement>("#result")!;
const score = document.querySelector<HTMLDivElement>("#score")!;
const again = document.querySelector<HTMLButtonElement>("#again")!;
const cakeMenu = document.querySelector<HTMLDivElement>("#cake-menu")!;
const picker = document.querySelector<HTMLSelectElement>("#cake-picker")!;
const credit = document.querySelector<HTMLDivElement>("#credit")!;
picker.append(new Option("Random", RANDOM_CAKE), ...CAKE_KINDS.map(buildPickerGroup).filter((group) => group.childElementCount > 0));
// Online, the host picks each round's cake in the lobby, and practice takes a random one.
picker.hidden = lobby !== null;
document.querySelector("#cake-credits")!.replaceWith(buildCreditsList());
document.querySelector("#library-credits")!.replaceWith(buildLibrariesList());
const credits = document.querySelector<HTMLDialogElement>("#credits")!;
credits.addEventListener("close", () => history.replaceState(null, "", location.pathname + location.search));
document.querySelector("#libraries-link")!.addEventListener("click", (event) => {
  event.preventDefault();
  document.querySelector("#libraries")!.scrollIntoView({ behavior: "smooth" });
});
window.addEventListener("hashchange", showCreditsForHash);
showCreditsForHash();

// Online, the page builds nothing more before the room names the cake its first game needs, which
// comes after the join, so building the scene and renderers holds no frame while the player types,
// and then waits for the frame that shows the room, which the build would hold up for 0.1 s.
if (room) {
  await room.firstCakeId;
  await waitFrames(ROOM_SHOWN_FRAMES);
}
const assets = await propAssets;
const scene = new Scene(canvas, layout, assets);
const preloader = new CakePreloader(scene);
// The second renderer gets a frame of its own, since a first visit's first WebGL context alone took up to 84 ms.
if (lobby) await waitFrames(1);
// Offline there are no opponents, so no second WebGL context.
const opponents = lobby ? new Opponents(layout, layout.ticks_per_s, assets.studio) : null;
const judging = lobby ? new Judging(scene, layout) : null;
const countdown = new Countdown(clock, layout.ticks_per_s);
const roundSounds = new RoundSounds();
const orbit = new Orbit(scene.camera, layout.stand_top_y_m);
const fpsMeter = IS_DEBUG_PAGE ? new FpsMeter() : null;
/**
 * The cut in progress: when it started, the knife's travel, the lowering where the edge enters the
 * cake and the halves split open (none once they have, or for a cut that misses), and the result.
 * Offline, the knife's settling starts the cut's own reveal at `revealStartMs`, counting up the
 * grams of `halvesG`, the whole cake's alone for a miss. An online cut's halves stay together
 * until the judging reveal.
 */
let cutting: {
  startMs: number;
  fromLowerM: number;
  toLowerM: number;
  splitLowerM: number | null;
  split: Float64Array;
  halvesG: number[];
  isOnline: boolean;
  revealStartMs: number | null;
} | null = null;
/** Counts round starts, so a round that finished loading after a later one began is dropped. */
let roundStarts = 0;
let lagMs = 0;
let lastFrameMs = performance.now();
let nextFrameMs = lastFrameMs;
let myId = 0;
/** The steps of this player's own world, when this page reopened into its seat mid-round, for the round's load to play back. */
let resumed: ResumeMessage | null = null;
/** Whether the online round has players besides this page, whose tiles need its cake. */
let hasOpponents = false;
/** Whether the scene holds the online round's cake, so the round can show rather than "Loading...". */
let isRoundCakeShown = false;
/** The tiles' load of the round's cake from `game`, which keeps that game alive until it ends. */
let opponentsCakeLoad: Promise<void> = Promise.resolve();
/** Counts resyncs, so a rebuild a later `resume` overtook is dropped. */
let resyncs = 0;
/** When the online round began stepping ticks this page missed, until it has caught up with the round's clock. */
let catchUpStartMs: number | null = null;
/** The online round's players, and those whose cut this page has heard of, so it knows when the last has cut. */
const roundPlayerIds = new Set<number>();
const cutPlayerIds = new Set<number>();

/** An online round the server started: this page's time at its tick 0. */
interface StartedRound {
  startMs: number;
}

/** The ticks of a reopened page's world to step again, each with the input the server stepped it with, and the tick it cut at, if it has. */
interface Replayed {
  inputs: Int32Array[];
  cutTick: number | null;
}

/**
 * Where this page is in an online round: outside one (offline, practicing), loading the round's
 * cake (which the server may start meanwhile), waiting for the start, playing, cut and waiting for
 * the judging, spectating a round it is not in, or showing the judging reveal, which every page in
 * the room does once play ends, until the results. While playing, every input goes to the server,
 * `heldInput` is the one sent last, which the server holds for any tick it has not heard of, and
 * `stepped` holds the input each tick of the world stepped with, by tick.
 */
type Online =
  | { kind: "practice" }
  | { kind: "loading"; started: StartedRound | null; ticks: number }
  | { kind: "waiting" }
  | { kind: "playing"; started: StartedRound; heldInput: Int32Array; replayed: Replayed | null; stepped: Int32Array[] }
  | { kind: "cut"; started: StartedRound }
  | { kind: "spectating"; started: StartedRound | null; ticks: number }
  | { kind: "judging" };
let online: Online = { kind: "practice" };
const firstCake = room ? findCake(await room.firstCakeId) : pickCake();
credit.replaceChildren(buildRoundCreditHtml(firstCake));
const firstCakeFile = await fetchCakeFile(firstCake);
let game = new Game(roundTicks, firstCakeFile);
/** The cake `game` plays, so a spectator already showing the round's cake keeps it. */
let gameCake = firstCake;
const controls = new Controls(scene, orbit, layout, game);
if (IS_DEBUG_PAGE) {
  window.readKnife = readKnife;
  window.readWorld = readWorld;
  window.readHalves = () => scene.getHalfSlidesM();
  window.readTiles = () => opponents?.readTiles() ?? [];
  window.pickAt = pickAt;
  window.readAnchors = readAnchors;
}

declare global {
  interface Window {
    /** On a `?debug` page, for step files: how far each cut half has slid apart, as the reveal labels them. */
    readHalves?: () => number[];
    /** On a `?debug` page, for step files: the knife's pose, whether `Cut!` would divide the cake, and the view's cursor. */
    readKnife?: typeof readKnife;
    /** On a `?debug` page, for step files: the world's tick and the cake's pose, to compare with the server's poses of it. */
    readWorld?: typeof readWorld;
    /** On a `?debug` page, for step files: what each opponent's tile shows this frame. */
    readTiles?: () => ReturnType<Opponents["readTiles"]>;
    /** On a `?debug` page, for step files: what a press at the client point would grab in the play view, if anything. */
    pickAt?: typeof pickAt;
    /** On a `?debug` page, for step files: where the cake, the plate, its rim, and the knife's edge are on screen, in client pixels. */
    readAnchors?: typeof readAnchors;
  }
}

/** Points on the plate's rim `readAnchors` gives, evenly round it from the +x axis. */
const RIM_ANCHOR_LEN = 16;
/** Points across the blade `readAnchors` gives, from one end of its span to the other, at each of two heights above its edge. */
const BLADE_ANCHOR_LEN = 9;

function readAnchors(): { cake: number[]; plate: number[]; rim: number[][]; knife: number[][] } {
  const [x, y, z] = game.get_cake_pose();
  const rim = Array.from({ length: RIM_ANCHOR_LEN }, (_, idx) => {
    const angleRad = (2 * Math.PI * idx) / RIM_ANCHOR_LEN;
    return projectToClient(layout.stand_radius_m * Math.cos(angleRad), layout.stand_top_y_m, layout.stand_radius_m * Math.sin(angleRad));
  });
  // The frame's edge and down are directions; the edge's lowest point hangs below the pivot by the lowering.
  const frame = game.get_knife_frame();
  const [edge, down] = [0, 3].map((start) => new THREE.Vector3().fromArray(frame, start));
  const edgeMiddle = new THREE.Vector3(0, layout.pivot_y_m, 0).addScaledVector(down, game.get_knife_pose()[1]);
  const knife = [0.5, 0.25].flatMap((backShare) =>
    Array.from({ length: BLADE_ANCHOR_LEN }, (_, idx) => {
      const alongM = layout.blade_half_span_m * 0.6 * ((2 * idx) / (BLADE_ANCHOR_LEN - 1) - 1);
      const point = edgeMiddle.clone().addScaledVector(down, -layout.blade_back_m * backShare).addScaledVector(edge, alongM);
      return projectToClient(point.x, point.y, point.z);
    }),
  );
  return { cake: projectToClient(x, y, z), plate: projectToClient(0, layout.stand_top_y_m, 0), rim, knife };
}

function projectToClient(x: number, y: number, z: number): number[] {
  const rect = canvas.getBoundingClientRect();
  const ndc = new THREE.Vector3(x, y, z).project(scene.camera);
  return [rect.left + ((ndc.x + 1) / 2) * rect.width, rect.top + ((1 - ndc.y) / 2) * rect.height];
}

/** The world's tick, the cake's pose, and the stand's angle, and in an online round the page's own round clock's start and tick. */
function readWorld(): { tick: number; cakePose: number[]; standRad: number; startMs: number | null; clockTick: number | null } {
  const started = online.kind === "playing" || online.kind === "cut" ? online.started : null;
  const clockTick = started && calcClockTick(started, performance.now());
  return { tick: game.get_tick(), cakePose: [...game.get_cake_pose()], standRad: game.get_stand_angle_rad(), startMs: started?.startMs ?? null, clockTick };
}

function pickAt(clientX: number, clientY: number): PickKind | null {
  const rect = canvas.getBoundingClientRect();
  const pointer = new THREE.Vector2(((clientX - rect.left) / rect.width) * 2 - 1, -((clientY - rect.top) / rect.height) * 2 + 1);
  const raycaster = new THREE.Raycaster();
  raycaster.setFromCamera(pointer, scene.camera);
  return scene.pick(raycaster)?.kind ?? null;
}

function readKnife(): { tiltRad: number; loweredM: number; isCutThrough: boolean; cursor: string } {
  const [tiltRad, lowerM, restM] = game.get_knife_pose();
  return { tiltRad, loweredM: lowerM - restM, isCutThrough: game.is_cut_through(), cursor: canvas.style.cursor };
}

/** The picker's group for one kind of cake, its cakes by name. */
function buildPickerGroup(kind: CakeKind): HTMLOptGroupElement {
  const group = document.createElement("optgroup");
  group.label = kind;
  const cakes = CAKES.filter((cake) => cake.kind === kind).sort((a, b) => a.name.localeCompare(b.name));
  group.append(...cakes.map((cake) => new Option(cake.name, cake.id)));
  return group;
}

/** Opens the credits page when the URL fragment asks for it, as the lobby's link does. */
function showCreditsForHash(): void {
  if (location.hash === "#credits" && !credits.open) credits.showModal();
}

/** The cake the picker names, a random one for Random. */
function pickCake(): CakeEntry {
  if (picker.value === RANDOM_CAKE) return CAKES[Math.floor(Math.random() * CAKES.length)];
  return CAKES.find((cake) => cake.id === picker.value)!;
}

/**
 * Loads the next round while this one plays on, or takes it as the lobby preloaded it, then swaps
 * every part over at once, returning whether a later round did not start first.
 */
async function startRound(cake = pickCake(), ticks = offlineRoundTicks): Promise<boolean> {
  const start = ++roundStarts;
  const next = await preloader.take(cake, ticks);
  if (start !== roundStarts) {
    freeRoundCake(next);
    return false;
  }
  roundTicks = ticks;
  game.free();
  game = next.game;
  gameCake = cake;
  credit.replaceChildren(buildRoundCreditHtml(cake));
  controls.reset(game);
  scene.resetCake(next.loaded);
  orbit.resetForCake(scene.getCakeHeightM());
  cutting = null;
  result.hidden = true;
  return true;
}

function startCut(split: Float64Array, nowMs: number): void {
  const toLowerM = game.calc_cut_lower_m();
  const isCutThrough = game.is_cut_through();
  const splitLowerM = isCutThrough ? Math.min(game.calc_cake_touch_lower_m(), toLowerM) : null;
  // A cut that missed skips the count.
  const halvesG = isCutThrough ? [split[0], split[1]].map((m3) => m3 * layout.density_g_per_m3) : [];
  cutting = { startMs: nowMs, fromLowerM: game.get_knife_pose()[1], toLowerM, splitLowerM, split, halvesG, isOnline: online.kind === "playing", revealStartMs: null };
  playChop("own");
  // Offline, this player's cut is the round's last.
  if (!lobby) playLastCutWhistle();
  if (online.kind !== "playing") return;
  console.info(`cut at tick ${game.get_tick()}: score ${split[2]}`);
  online = { kind: "cut", started: online.started };
  blowIfLastCut();
}

/**
 * Blows the whistle once this page has cut and heard of every other round player's cut, so the
 * last cut's whistle comes as this player sees it rather than a round trip later with `play_over`,
 * which blows it otherwise, for a spectator or a page still playing.
 */
function blowIfLastCut(): void {
  if (online.kind !== "cut") return;
  for (const id of roundPlayerIds) {
    if (id !== myId && !cutPlayerIds.has(id)) return;
  }
  roundSounds.blowForLastCut();
}

/**
 * Sweeps the knife down through the cake, rocks it and lets it settle. Offline, the halves then
 * part as their gram counters count up, and the score shows once the counting ends.
 */
function animateCut(nowMs: number): void {
  const cut = cutting!;
  const { lowerM, rockRad, isDone } = calcCutMotion((nowMs - cut.startMs) / 1000, cut.fromLowerM, cut.toLowerM);
  scene.placeKnife(game, lowerM, rockRad);
  if (cut.splitLowerM !== null && lowerM >= cut.splitLowerM) {
    scene.showHalves(game);
    cut.splitLowerM = null;
  }
  if (!isDone || cut.isOnline || !scene.hasHalfRests()) return;
  cut.revealStartMs ??= nowMs;
  const frame = calcRevealFrame((nowMs - cut.revealStartMs) / 1000, buildOwnScene(cut.halvesG), cut.halvesG);
  scene.setHalfPartShare(frame.partShare);
  if (frame.shownG.length > 0) scene.showHalfGrams(frame.shownG);
  if (!frame.isCounted || !result.hidden) return;
  score.textContent = formatScore(cut.split[2], cut.halvesG.length === 0);
  score.classList.toggle("is-miss", cut.halvesG.length === 0);
  result.hidden = false;
}

function stepSimulation(elapsedMs: number, nowMs: number): void {
  if (online.kind === "playing") {
    stepOnline(online, nowMs);
    return;
  }
  // Online, the page steps only its rounds: the lobby covers it between them, and a spectator only watches.
  lagMs = online.kind === "practice" && !lobby ? lagMs + elapsedMs : 0;
  let ticks = 0;
  while (lagMs >= tickMs && ticks < MAX_TICKS_PER_FRAME && !cutting) {
    game.step(controls.stepInput(game));
    lagMs -= tickMs;
    ticks += 1;
    startTimeoutCut(nowMs);
  }
  if (ticks === MAX_TICKS_PER_FRAME || cutting) lagMs = 0;
}

/**
 * Steps an online round up to the server's clock. A reopened page first steps again what the
 * server already stepped for it. The last frame's worth of ticks step with the live controls.
 * Older ones, missed while the page was hidden or slow, step with the held input, as the server
 * replays a tick it has no input for, as many as the frame's budget allows.
 */
function stepOnline(playing: Online & { kind: "playing" }, nowMs: number): void {
  const clockTick = calcClockTick(playing.started, nowMs);
  const budgetEndMs = performance.now() + CATCH_UP_BUDGET_MS;
  while (!cutting && game.get_tick() < clockTick) {
    const isMissed = game.get_tick() < clockTick - MAX_TICKS_PER_FRAME;
    if (isMissed) catchUpStartMs ??= nowMs;
    if (isMissed && performance.now() >= budgetEndMs) return;
    if (playing.replayed && stepReplayed(playing, playing.replayed, nowMs)) continue;
    stepOnlineTick(playing, isMissed ? playing.heldInput : controls.stepInput(game), nowMs);
  }
  if (catchUpStartMs === null) return;
  console.info(`caught up to the round's clock at tick ${game.get_tick()} in ${Math.round(nowMs - catchUpStartMs)} ms`);
  catchUpStartMs = null;
}

/** Steps one tick of the round with `input`, which the server hears of. */
function stepOnlineTick(playing: Online & { kind: "playing" }, input: Int32Array, nowMs: number): void {
  playing.heldInput = input;
  playing.stepped[game.get_tick()] = input;
  getNet().sendInput(game.get_tick(), input);
  game.step(input);
  if (game.get_tick() % layout.ticks_per_s === 0) getNet().sendHash(game.get_tick(), game.calc_hash());
  startTimeoutCut(nowMs);
}

/**
 * Steps a reopened page's next tick as the server already stepped it, without sending it again,
 * or cuts where the player had cut, returning whether there was such a tick.
 */
function stepReplayed(playing: Online & { kind: "playing" }, replayed: Replayed, nowMs: number): boolean {
  const tick = game.get_tick();
  if (tick === replayed.cutTick) {
    playing.replayed = null;
    startCut(game.cut(), nowMs);
    return true;
  }
  const input = replayed.inputs[tick];
  if (!input) {
    playing.replayed = null;
    return false;
  }
  playing.heldInput = input;
  playing.stepped[tick] = input;
  game.step(input);
  startTimeoutCut(nowMs);
  return true;
}

/**
 * Rebuilds this page's world from the server's steps of it, which parted from the page's own
 * when the page's inputs reached the server past its grace, then steps on with what this page
 * stepped since, which the server took as sent, and swaps the rebuilt world in to play on. The
 * rebuild spreads over frames while the page plays on, and is dropped if the page cuts first.
 */
async function resync(playing: Online & { kind: "playing" }, resume: ResumeMessage): Promise<void> {
  const serverInputs = expandReplayed(resume).inputs;
  const resyncCount = ++resyncs;
  const isCurrent = () => online === playing && !cutting && resyncCount === resyncs;
  const cakeFile = await fetchCakeFile(gameCake);
  await opponentsCakeLoad;
  if (!isCurrent()) return;
  const rebuilt = new Game(roundTicks, cakeFile);
  while (rebuilt.get_tick() < game.get_tick()) {
    const budgetEndMs = performance.now() + CATCH_UP_BUDGET_MS;
    while (rebuilt.get_tick() < game.get_tick() && performance.now() < budgetEndMs) {
      const tick = rebuilt.get_tick();
      rebuilt.step(serverInputs[tick] ?? playing.stepped[tick]);
      // A world stops at its timeout cut, which the page's own world reaches first.
      if (rebuilt.get_tick() === tick) break;
    }
    if (rebuilt.get_tick() < game.get_tick()) await waitFrames(1);
    if (!isCurrent() || rebuilt.find_split()) {
      rebuilt.free();
      return;
    }
  }
  console.info(`resynced to the server's ${serverInputs.length} ticks at tick ${rebuilt.get_tick()}`);
  game.free();
  game = rebuilt;
}

/** A hidden tab lets go of the cake and knife as if the mouse were released, and the server holds that until the tab returns. */
function letGo(nowMs: number): void {
  controls.letGo();
  if (online.kind !== "playing" || cutting || online.replayed) return;
  stepOnlineTick(online, controls.buildReleasedInput(online.heldInput), nowMs);
}

function expandReplayed(resume: ResumeMessage): Replayed {
  const inputs = resume.runs.flatMap((run) => Array<Int32Array>(run.ticks).fill(Int32Array.from(run.input)));
  return { inputs, cutTick: resume.cut_tick };
}

/** The tick a started online round's clock has reached at `nowMs`, past its end once it is over. */
function calcClockTick(started: StartedRound, nowMs: number): number {
  return Math.floor((nowMs - started.startMs) / tickMs);
}

/**
 * Whether an online round is still stepping ticks this page missed. A cut then could land after
 * the server's deadline, which scores the round's timeout cut instead.
 */
function isCatchingUp(nowMs: number): boolean {
  return online.kind === "playing" && game.get_tick() < calcClockTick(online.started, nowMs) - MAX_TICKS_PER_FRAME;
}

/** Whether a drag now moves the cake, knife, or stand: offline until the cut, online in a round this page plays until its cut. */
function isInputLive(): boolean {
  if (cutting) return false;
  return online.kind === "playing" || (online.kind === "practice" && !lobby);
}

/** Starts the cut the world makes itself at the round's last tick, once it has. */
function startTimeoutCut(nowMs: number): void {
  const split = game.find_split();
  if (split) startCut(split, nowMs);
}

function onServerMessage(message: ServerMessage): void {
  switch (message.type) {
    case "welcome":
      myId = message.id;
      break;
    case "room":
      onRoom(message);
      break;
    case "pick":
      lobby!.showPick(message);
      // Built hidden with the round's length, which the round's own message repeats.
      preloader.preload(findCake(message.cake), message.round_ticks);
      break;
    case "claim_refused":
      lobby!.showClaimRefused();
      break;
    case "round":
      resumed = null;
      roundTimer?.start(performance.now());
      roundSounds.open(message.round_ticks * tickMs);
      onRound(findCake(message.cake), message.round_ticks, message.players);
      break;
    case "resume":
      // Mid-play, the server's world parted from this page's; otherwise this page reopened into its seat.
      if (online.kind === "playing" && !cutting) void resync(online, message);
      else resumed = message;
      break;
    case "start": {
      const startMs = performance.now() - message.elapsed_ms;
      onStart({ startMs });
      roundSounds.start(startMs);
      break;
    }
    case "still_loading":
      opponents!.removeOpponents(message.players);
      break;
    case "now_playing":
      roundPlayerIds.add(message.player.id);
      if (message.player.id !== myId) opponents!.addOpponent(message.player);
      break;
    case "cut":
      opponents!.addCut(message.id, message.tick, message.pose, message.cut_lower_m);
      judging!.addCut(message);
      cutPlayerIds.add(message.id);
      blowIfLastCut();
      break;
    case "play_over":
      roundSounds.blowForLastCut();
      break;
    case "judging":
      // A page that joined after the last cut heard no whistle and should not hear one now.
      roundSounds.close();
      showJudging(message);
      break;
    case "results":
      roundTimer?.stop(performance.now());
      showResults(message);
      break;
    case "debug":
      debugPanel?.showSettings(message);
      break;
    case "desync":
      console.warn(`server replay differs at tick ${message.tick}`);
      break;
    case "error":
      console.error(`server: ${message.message}`);
      break;
  }
}

function findCake(id: string): CakeEntry {
  return CAKES.find((cake) => cake.id === id)!;
}

function onRoom(room: Extract<ServerMessage, { type: "room" }>): void {
  lobby!.showRoom(room, myId);
  debugPanel?.showHost(room.host === myId);
  opponents!.syncMembers(room.members);
  if (room.phase !== "lobby") return;
  opponents!.close();
  // A round no page loaded in time is called off, leaving its sat-out pages in the lobby.
  if (online.kind === "spectating") online = { kind: "practice" };
}

/** Shows the round's other players beside this page's own game, and plays the round if this page is in it, or spectates it. */
function onRound(cake: CakeEntry, ticks: number, players: readonly Member[]): void {
  roundPlayerIds.clear();
  cutPlayerIds.clear();
  for (const player of players) roundPlayerIds.add(player.id);
  const others = players.filter((player) => player.id !== myId);
  opponents!.open(others);
  judging!.open();
  hasOpponents = others.length > 0;
  isRoundCakeShown = false;
  // Until the round's cake loads, the stand stands empty rather than showing an earlier cake or its credit.
  if (!isShowingUntouched(cake)) {
    scene.hideCake();
    credit.replaceChildren();
  }
  if (players.some((player) => player.id === myId)) {
    void joinRound(cake, ticks);
    return;
  }
  void spectate(cake, ticks);
}

/** Whether the page shows `cake` untouched where the players' start, as a round on it begins. */
function isShowingUntouched(cake: CakeEntry): boolean {
  return gameCake.id === cake.id && game.get_tick() === 0 && !cutting;
}

/**
 * Shows a round this page is not in: its cake where the players' start, untouched, beside the
 * opponents' tiles, with the rules and the countdown. A page that loaded the cake already keeps it.
 */
async function spectate(cake: CakeEntry, ticks: number): Promise<void> {
  online = { kind: "spectating", started: null, ticks };
  try {
    if (!isShowingUntouched(cake) && !(await startRound(cake, ticks))) return;
    setRoundCakeShown();
    await loadOpponentsCake();
  } catch (error) {
    console.error(`loading cake ${cake.id} to spectate failed:`, error);
  }
}

/** Notes that the scene holds the online round's cake, whole, for the round and its judging to show. */
function setRoundCakeShown(): void {
  isRoundCakeShown = true;
  judging!.setCakeReady();
}

/** Gives the opponents' tiles the round's cake, unless the round has no one else to show. */
async function loadOpponentsCake(): Promise<void> {
  if (!hasOpponents) return;
  opponentsCakeLoad = opponents!.loadCake(game);
  await opponentsCakeLoad;
}

/** Whether this page is loading an online round's cake, read fresh so a state change during an await shows. */
function isLoading(): boolean {
  return online.kind === "loading";
}

/**
 * Loads an online round's cake, then plays it if the server started it meanwhile, with the time
 * left, or waits for the start. A load that fails is tried again until the round ends, since the
 * round never waits for this page and its world idles until the page plays.
 */
async function joinRound(cake: CakeEntry, ticks: number): Promise<void> {
  online = { kind: "loading", started: null, ticks };
  let isLatest = false;
  while (isLoading()) {
    try {
      isLatest = await startRound(cake, ticks);
      break;
    } catch (error) {
      console.error(`loading cake ${cake.id} failed, trying again:`, error);
      await new Promise((resolve) => setTimeout(resolve, LOAD_RETRY_MS));
    }
  }
  if (!isLatest) return;
  setRoundCakeShown();
  if (!isLoading()) return;
  getNet().sendReady();
  void loadOpponentsCake();
  const { started } = online;
  if (started) console.info(`loaded the cake at the round's tick ${calcClockTick(started, performance.now())}, playing the time left`);
  online = started ? startPlaying(started) : { kind: "waiting" };
}

/** This page's round in play from `started`, first stepping again what the server stepped for it if it reopened into its seat. */
function startPlaying(started: StartedRound): Online {
  const replayed = resumed && expandReplayed(resumed);
  resumed = null;
  catchUpStartMs = null;
  return { kind: "playing", started, heldInput: controls.buildIdleInput(game), replayed, stepped: [] };
}

/** Starts the round this page waits for, or notes the start for when its cake has loaded, or for a spectator's countdown. */
function onStart(started: StartedRound): void {
  if (online.kind === "loading") online = { ...online, started };
  else if (online.kind === "waiting") online = startPlaying(started);
  else if (online.kind === "spectating") online = { ...online, started };
}

/**
 * Shows the judging reveal in place of this page's own view and the opponents' tiles, in step
 * with every other page, whatever this page was doing: a page still catching up to its own cut
 * sees it there, as the server scored it.
 */
function showJudging(message: JudgingMessage): void {
  console.info(`judging: ${JSON.stringify(message.order)}, counting ${JSON.stringify(message.count_ms)} ms, ${message.elapsed_ms} ms in`);
  opponents!.close();
  cutting = null;
  result.hidden = true;
  online = { kind: "judging" };
  judging!.start(message, performance.now() - message.elapsed_ms);
}

/** Shows an online round's results over the whole screen, until the host returns everyone to the lobby. */
function showResults(results: ResultsMessage): void {
  console.info(`results: ${JSON.stringify(results.standings)}`);
  console.info(`standup: ${JSON.stringify(results.standup)}`);
  opponents!.close();
  judging!.stop();
  lobby!.showResults(results);
  online = { kind: "practice" };
}

/**
 * Shows the lobby whenever this page is not in a round, as the rules alone while it spectates,
 * "Loading..." while the round it is in or watches has no cake yet, Again for offline practice,
 * `Cut!` to anyone but a spectator, and the countdown and the cake's credit only with the round.
 */
function syncOverlays(nowMs: number): void {
  const screen = findScreen();
  if (lobby) lobby.setScreen(screen);
  else setLoadingShown(screen === "loading");
  const isRoundShown = screen === "round" || screen === "rules";
  setHidden(cakeMenu, !isRoundShown);
  setHidden(again, lobby !== null || online.kind !== "practice");
  setHidden(cutButton, online.kind === "spectating" || online.kind === "judging");
  const ticks = isRoundShown ? findCountdownTicks(nowMs) : null;
  setHidden(clock, ticks === null);
  if (ticks) countdown.show(Math.max(ticks.left, 0), ticks.total);
}

/** What the page shows for where it is in the room. Offline, it shows its practice round from the first frame. */
function findScreen(): Screen {
  if (!lobby) return "round";
  switch (online.kind) {
    case "practice":
      return lobby.hasRoomToShow() ? "lobby" : "loading";
    case "loading":
      return "loading";
    case "spectating":
      return isRoundCakeShown ? "rules" : "loading";
    case "judging":
      return isRoundCakeShown ? "round" : "loading";
    default:
      return "round";
  }
}

/**
 * The round's ticks in all and left to play: a playing page's by its world, and a spectator's or
 * a page that has cut's by the server's clock, which runs to the round's end. None before a start
 * or once the judging starts.
 */
function findCountdownTicks(nowMs: number): { left: number; total: number } | null {
  if (online.kind === "judging") return null;
  if (online.kind === "spectating") {
    return online.started && { left: online.ticks - calcClockTick(online.started, nowMs), total: online.ticks };
  }
  if (online.kind === "cut") return { left: roundTicks! - calcClockTick(online.started, nowMs), total: roundTicks! };
  return roundTicks === undefined ? null : { left: roundTicks - game.get_tick(), total: roundTicks };
}

function setHidden(element: HTMLElement, isHidden: boolean): void {
  if (element.hidden !== isHidden) element.hidden = isHidden;
}

/** The room's connection, which an online page has from its join until it leaves for inactivity, when it is in no round. */
function getNet(): Net {
  return room!.getNet()!;
}

/** Leaves the room behind once the connection is lost, which `Room` shows as a fatal error. */
function onLost(): void {
  leaveRoom();
}

/**
 * `message` as if it had arrived `heldMs` later, so a round start or judging reveal held while the
 * page built its first game still starts when it did for every other page.
 */
function ageMessage(message: ServerMessage, heldMs: number): ServerMessage {
  if (message.type !== "start" && message.type !== "judging") return message;
  return { ...message, elapsed_ms: message.elapsed_ms + heldMs };
}

/** Leaves the online round and the lobby's pick behind, back to practice, which only a later join leaves. */
function leaveRoom(): void {
  opponents!.close();
  judging!.stop();
  roundSounds.close();
  preloader.drop();
  online = { kind: "practice" };
}

/** Leaves the room once the player has gone an hour without input, and asks for their name and preference again to rejoin. */
async function rejoinAfterInactivity(): Promise<void> {
  if (!room!.getNet()) return;
  room!.disconnect();
  leaveRoom();
  room!.connect(await lobby!.askProfile());
}

function onFrame(nowMs: number): void {
  requestAnimationFrame(onFrame);
  if (nowMs + 1 < nextFrameMs) return;
  nextFrameMs = Math.max(nextFrameMs + frameMs, nowMs);
  const elapsedMs = nowMs - lastFrameMs;
  lastFrameMs = nowMs;
  stepSimulation(elapsedMs, nowMs);
  // The judging poses the scene and its camera itself, and prepares its scenes as cuts come in.
  judging?.update(nowMs);
  if (judging?.isShowing()) {
    scene.setViewShiftPx(judging.getViewShiftPx());
  } else {
    orbit.update(elapsedMs / 1000);
    if (cutting) animateCut(nowMs);
    else scene.sync(game);
    scene.setViewShiftPx(opponents?.getViewShiftPx() ?? 0);
  }
  cutButton.disabled = cutting !== null || !game.is_cut_through() || isCatchingUp(nowMs);
  scene.setHighlight(isInputLive() ? controls.findHighlightKind() : null);
  syncOverlays(nowMs);
  scene.render();
  const net = room?.getNet();
  if (net) opponents!.render(nowMs, net.poses);
  fpsMeter?.record(nowMs, elapsedMs);
  roundTimer?.update(nowMs);
}

scene.resetCake(await scene.loadCake(game));
orbit.resetForCake(scene.getCakeHeightM());
// Drawing first would compile every shader in one frame, which held the lobby 150-200 ms.
await scene.compile();
cutButton.addEventListener("click", () => {
  const nowMs = performance.now();
  if (isCatchingUp(nowMs)) return;
  if (online.kind === "playing") getNet().sendCut(game.get_tick());
  startCut(game.cut(), nowMs);
});
again.addEventListener("click", () => void startRound());
picker.addEventListener("change", () => {
  picker.blur();
  if (online.kind === "practice") void startRound();
});
document.addEventListener("visibilitychange", () => {
  if (document.hidden) letGo(performance.now());
});
requestAnimationFrame(onFrame);
room?.handOver({ onMessage: (message, heldMs) => onServerMessage(ageMessage(message, heldMs)), onLost });
if (room) watchActivity(() => void rejoinAfterInactivity());
// Online, nothing steps a world before the first round, so the lobby's wait compiles the tick.
if (lobby) stepThrowawayWorld(firstCakeFile, (world) => controls.buildIdleInput(world)).catch((error: unknown) => console.error("stepping a throwaway world failed:", error));
opponents?.ensureWarm().catch((error: unknown) => console.error("warming up the opponents' tiles failed:", error));
