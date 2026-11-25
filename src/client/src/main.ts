import * as THREE from "three";
import init, { Game, get_layout } from "@wasm/cake_wasm.js";
import { CAKES, CAKE_KINDS, type CakeEntry, type CakeKind, buildCreditHtml, buildCreditsList, fetchCakeFile } from "./cakes";
import { Controls } from "./controls";
import { Countdown } from "./countdown";
import { calcCutMotion } from "./cutMotion";
import { Net, type ServerMessage, type Standing, findServerUrl } from "./net";
import { buildLibrariesList } from "./libraries";
import { Lobby, RANDOM_CAKE } from "./lobby";
import { Opponents } from "./opponents";
import { Orbit } from "./orbit";
import { Scene } from "./scene";

/**
 * Ticks a slow frame may step with the live controls. Practice drops any lag past them, so a
 * stalled tab does not spiral. An online round steps the older ticks with the held input instead.
 */
const MAX_TICKS_PER_FRAME = 5;
/** How long a frame may spend stepping the ticks an online round missed, so the page stays responsive while it catches up. */
const CATCH_UP_BUDGET_MS = 40;
/** Frames beyond the simulation's tick rate would show nothing new, so they are not drawn. */
const MAX_FRAMES_PER_S = 60;
/** The camera orbits this far above the stand's plate, among a cake set on it and the knife resting over it. */
const ORBIT_ABOVE_STAND_M = 0.13;

const serverUrl = findServerUrl();
const lobby = serverUrl ? new Lobby((cake) => net!.sendStart(cake)) : null;
// Asked before the WASM and the practice cake load, so the player types while they do.
const typedName = lobby?.waitName() ?? null;
await init();
const layout = get_layout();
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

const scene = new Scene(canvas, layout);
// Offline there are no opponents, so no second WebGL context.
const opponents = lobby ? new Opponents(layout, layout.ticks_per_s) : null;
const countdown = new Countdown(clock, layout.ticks_per_s);
const orbit = new Orbit(scene.camera, new THREE.Vector3(0, layout.stand_top_y_m + ORBIT_ABOVE_STAND_M, 0));
picker.append(new Option("Random", RANDOM_CAKE), ...CAKE_KINDS.map(buildPickerGroup).filter((group) => group.childElementCount > 0));
// Online, the host picks each round's cake in the lobby, and practice takes a random one.
picker.hidden = lobby !== null;
document.querySelector("#cake-credits")!.replaceWith(buildCreditsList());
document.querySelector("#library-credits")!.replaceWith(buildLibrariesList());
const credits = document.querySelector<HTMLDialogElement>("#credits")!;
credits.addEventListener("close", () => history.replaceState(null, "", location.pathname + location.search));
window.addEventListener("hashchange", showCreditsForHash);
showCreditsForHash();
const firstCake = pickCake();
credit.replaceChildren(buildCreditHtml(firstCake));
let game = new Game(roundTicks, await fetchCakeFile(firstCake));
const controls = new Controls(scene, orbit, layout, game);
/**
 * The cut in progress: when it started, the knife's travel, the lowering where the edge enters the
 * cake and the halves split open (none once they have, or for a cut that misses), and the result
 * to show once it lands, with no score for an online round's, which the server's results give.
 */
let cutting: { startMs: number; fromLowerM: number; toLowerM: number; splitLowerM: number | null; split: Float64Array; isOnline: boolean } | null = null;
/** Counts round starts, so a round that finished loading after a later one began is dropped. */
let roundStarts = 0;
let lagMs = 0;
let lastFrameMs = performance.now();
let nextFrameMs = lastFrameMs;
let net: Net | null = null;
let myId = 0;

/** An online round the server started: this page's time at its tick 0, and the results once they came, which show once this player's own cut does. */
interface StartedRound {
  startMs: number;
  standings: Standing[] | null;
}

/**
 * Where this page is in an online round: practicing between rounds, loading the round's cake
 * (which the server may start meanwhile), waiting for the start, playing, or cut and waiting for
 * the results. Only practice shows the lobby. While playing, every input goes to the server, and
 * `heldInput` is the one sent last, which the server holds for any tick it has not heard of.
 */
type Online =
  | { kind: "practice" }
  | { kind: "loading"; started: StartedRound | null }
  | { kind: "waiting" }
  | { kind: "playing"; started: StartedRound; heldInput: Int32Array }
  | { kind: "cut" };
let online: Online = { kind: "practice" };

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

/** Loads the next round while this one plays on, then swaps every part over at once, returning whether a later round did not start first. */
async function startRound(cake = pickCake(), ticks = offlineRoundTicks): Promise<boolean> {
  const start = ++roundStarts;
  const next = new Game(ticks, await fetchCakeFile(cake));
  const texture = await scene.loadCakeTexture(next);
  if (start !== roundStarts) {
    next.free();
    texture?.dispose();
    return false;
  }
  roundTicks = ticks;
  game.free();
  game = next;
  credit.replaceChildren(buildCreditHtml(cake));
  controls.reset(game);
  scene.resetCake(game, texture);
  cutting = null;
  result.hidden = true;
  return true;
}

function startCut(split: Float64Array, nowMs: number): void {
  const toLowerM = game.calc_cut_lower_m();
  const splitLowerM = game.is_cut_through() ? Math.min(game.calc_cake_touch_lower_m(), toLowerM) : null;
  cutting = { startMs: nowMs, fromLowerM: game.get_knife_pose()[1], toLowerM, splitLowerM, split, isOnline: online.kind === "playing" };
  if (online.kind !== "playing") return;
  console.info(`cut at tick ${game.get_tick()}: score ${split[2]}`);
  const { standings } = online.started;
  if (standings) {
    showResults(standings);
    return;
  }
  online = { kind: "cut" };
}

/** Sweeps the knife down through the cake, rocks it and lets it settle, then parts the halves and shows the result. */
function animateCut(nowMs: number): void {
  const cut = cutting!;
  const { lowerM, rockRad, isDone } = calcCutMotion((nowMs - cut.startMs) / 1000, cut.fromLowerM, cut.toLowerM);
  scene.placeKnife(game, lowerM, rockRad);
  if (cut.splitLowerM !== null && lowerM >= cut.splitLowerM) {
    scene.showHalves(game);
    cut.splitLowerM = null;
  }
  if (!isDone || !result.hidden) return;
  scene.partHalves();
  const [above, below, points] = cut.split;
  const [aboveG, belowG] = [above, below].map((m3) => Math.round(m3 * layout.density_g_per_m3));
  if (!cut.isOnline) scene.showHalfGrams([aboveG, belowG]);
  score.textContent = cut.isOnline ? "" : `${points.toFixed(1)}%`;
  result.hidden = false;
}

function stepSimulation(elapsedMs: number, nowMs: number): void {
  if (online.kind === "playing") {
    stepOnline(online, nowMs);
    return;
  }
  lagMs = online.kind === "practice" ? lagMs + elapsedMs : 0;
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
 * Steps an online round up to the server's clock. The last frame's worth of ticks step with the
 * live controls. Older ones, missed while the page was hidden or slow, step with the held input,
 * as the server replays a tick it has no input for, as many as the frame's budget allows.
 */
function stepOnline(playing: Online & { kind: "playing" }, nowMs: number): void {
  const clockTick = calcClockTick(playing.started, nowMs);
  const budgetEndMs = performance.now() + CATCH_UP_BUDGET_MS;
  while (!cutting && game.get_tick() < clockTick) {
    const isMissed = game.get_tick() < clockTick - MAX_TICKS_PER_FRAME;
    if (isMissed && performance.now() >= budgetEndMs) return;
    const input = isMissed ? playing.heldInput : controls.stepInput(game);
    playing.heldInput = input;
    net!.sendInput(game.get_tick(), input);
    game.step(input);
    if (game.get_tick() % layout.ticks_per_s === 0) net!.sendHash(game.get_tick(), game.calc_hash());
    startTimeoutCut(nowMs);
  }
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

/** Starts the cut the world makes itself at the round's last tick, once it has. */
function startTimeoutCut(nowMs: number): void {
  const split = game.find_split();
  if (split) startCut(split, nowMs);
}

function onServerMessage(message: ServerMessage): void {
  if (message.type === "welcome") myId = message.id;
  if (message.type === "room") onRoom(message);
  if (message.type === "round") onRound(CAKES.find((cake) => cake.id === message.cake)!, message.round_ticks, message.players);
  if (message.type === "start") onStart({ startMs: performance.now() - message.elapsed_ms, standings: null });
  if (message.type === "cut") opponents!.addCut(message.id, message.tick, message.pose, message.cut_lower_m);
  if (message.type === "results") onResults(message.standings);
  if (message.type === "desync") console.warn(`server replay differs at tick ${message.tick}`);
  if (message.type === "error") console.error(`server: ${message.message}`);
}

function onRoom(room: Extract<ServerMessage, { type: "room" }>): void {
  lobby!.showRoom(room, myId);
  opponents!.syncMembers(room.members);
  if (room.phase === "lobby") opponents!.close();
}

/** Shows the round's other players beside this page's own game, and plays the round if this page is in it, or spectates it. */
function onRound(cake: CakeEntry, ticks: number, players: readonly number[]): void {
  opponents!.open(players.filter((id) => id !== myId));
  if (players.includes(myId)) {
    void joinRound(cake, ticks);
    return;
  }
  void spectate(cake);
}

/** Loads the round's cake for the opponents' tiles alone, since a spectator plays none of it. */
async function spectate(cake: CakeEntry): Promise<void> {
  try {
    const spectated = new Game(undefined, await fetchCakeFile(cake));
    try {
      await opponents!.loadCake(spectated);
    } finally {
      spectated.free();
    }
  } catch (error) {
    console.error(`loading cake ${cake.id} to spectate failed:`, error);
  }
}

/** Whether this page is loading an online round's cake, read fresh so a state change during an await shows. */
function isLoading(): boolean {
  return online.kind === "loading";
}

/**
 * Loads an online round's cake, then plays it if the server started it meanwhile, or waits for the
 * start. A cake that fails to load returns the page to practice, so the lobby shows again.
 */
async function joinRound(cake: CakeEntry, ticks: number): Promise<void> {
  online = { kind: "loading", started: null };
  let isLatest: boolean;
  try {
    isLatest = await startRound(cake, ticks);
  } catch (error) {
    console.error(`loading cake ${cake.id} failed:`, error);
    if (isLoading()) online = { kind: "practice" };
    return;
  }
  if (!isLatest || !isLoading()) return;
  net!.sendReady();
  void opponents!.loadCake(game);
  const { started } = online;
  online = started ? { kind: "playing", started, heldInput: controls.buildIdleInput(game) } : { kind: "waiting" };
}

/** Starts the round this page waits for, or notes the start for when its cake has loaded. A spectator ignores it. */
function onStart(started: StartedRound): void {
  if (online.kind === "loading") online = { kind: "loading", started };
  else if (online.kind === "waiting") online = { kind: "playing", started, heldInput: controls.buildIdleInput(game) };
}

/** Shows the results, or keeps them until this page reaches its own cut, which it may still be catching up to. */
function onResults(standings: Standing[]): void {
  console.info(`results: ${JSON.stringify(standings)}`);
  const started = online.kind === "playing" || online.kind === "loading" ? online.started : null;
  if (started) {
    started.standings = standings;
    return;
  }
  showResults(standings);
}

/** Shows an online round's results in the lobby, where this page practices until the next round. */
function showResults(standings: Standing[]): void {
  opponents!.close();
  lobby!.showStandings(standings);
  online = { kind: "practice" };
}

/**
 * Shows the lobby whenever this page has joined and is not in a round, with Again for practice,
 * and the clock and the cake's credit only when the lobby does not cover them.
 */
function syncOverlays(): void {
  const isLobbyShown = lobby !== null && net !== null && online.kind === "practice";
  lobby?.setVisible(isLobbyShown);
  setHidden(cakeMenu, isLobbyShown);
  setHidden(clock, isLobbyShown || roundTicks === undefined);
  setHidden(again, online.kind !== "practice");
}

function setHidden(element: HTMLElement, isHidden: boolean): void {
  if (element.hidden !== isHidden) element.hidden = isHidden;
}

function connect(name: string): void {
  net = new Net(serverUrl!, name, onServerMessage, () => {
    lobby!.showDisconnected();
    opponents!.close();
    online = { kind: "practice" };
  });
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
  cutButton.disabled = cutting !== null || !game.is_cut_through() || isCatchingUp(nowMs);
  if (roundTicks !== undefined) countdown.show(roundTicks - game.get_tick(), roundTicks);
  syncOverlays();
  scene.render(elapsedMs / 1000);
  if (net) opponents!.render(nowMs, net.poses);
}

scene.resetCake(game, await scene.loadCakeTexture(game));
cutButton.addEventListener("click", () => {
  const nowMs = performance.now();
  if (isCatchingUp(nowMs)) return;
  if (online.kind === "playing") net!.sendCut(game.get_tick());
  startCut(game.cut(), nowMs);
});
again.addEventListener("click", () => void startRound());
picker.addEventListener("change", () => {
  picker.blur();
  if (online.kind === "practice") void startRound();
});
requestAnimationFrame(onFrame);
if (typedName) connect(await typedName);
