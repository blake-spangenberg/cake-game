import { URGENT_S } from "./countdown";
import { DESCEND_S } from "./cutMotion";
import chopUrl from "./sounds/knife-chop.mp3?url";
import dingUrl from "./sounds/ding.mp3?url";
import whistleUrl from "./sounds/whistle.mp3?url";

/**
 * The game's sounds: the owner's recordings of a knife chopping onto a board, a referee's
 * whistle, and a dinner bell's ding, encoded by `tools/encode-sounds.py`, and a clock's tick
 * synthesized as the page loads.
 * Browsers start audio only after a user gesture, so nothing sounds before the first input after
 * `enableSound`, and every sound is silently skipped where audio is unavailable. A round's ticks
 * and whistle, and a judging reveal's count ticks and dings, are scheduled ahead on the audio
 * clock, which runs on in a background tab, where the page draws no frames and its timers fire late.
 */

type Voice = "chop" | "tick" | "whistle" | "ding";
type Bus = "ownChop" | "opponentChop" | "tick" | "countTick" | "whistle" | "ding";
/** Whose cut a chop is for: an opponent's plays quieter than the player's own. */
export type ChopSource = "own" | "opponent";

/** A voice ready to play, and when its loudest moment comes, such as the chop's knife meeting the board. */
interface LoadedVoice {
  buffer: AudioBuffer;
  peakS: number;
}

/** The voices decode and render at this rate whatever the output's, which resamples them. */
const VOICE_RATE_HZ = 48000;
const VOICE_PEAK = 0.89;
/**
 * Each bus's gain on its voice, which peaks at `VOICE_PEAK`. Laptop speakers play these to a whole
 * Zoom room. The recordings' gains give them the loudness (K-weighted, over 400 ms) the synthesized
 * chop and whistle had at 0.6, 0.2, and 0.22, which the ticks were balanced against.
 */
const BUS_GAINS: Record<Bus, number> = { ownChop: 0.48, opponentChop: 0.16, tick: 0.35, countTick: 0.12, whistle: 0.25, ding: 0.48 };
const RECORDING_URLS: Record<Exclude<Voice, "tick">, string> = { chop: chopUrl, whistle: whistleUrl, ding: dingUrl };
/** At most this many opponents' chops sound at once, so a dozen cuts in the same instant do not blare. */
const MAX_OPPONENT_CHOPS = 3;
/** The whistle waits this long after the round's last cut, so the last chop is heard first. */
const WHISTLE_AFTER_LAST_CUT_S = 0.8;
const TICK_LENGTH_S = 0.08;

/** The page's audio output: locked until the first input, unavailable where the browser has none or refused it. */
type Output = { kind: "locked" } | { kind: "unavailable" } | { kind: "open"; context: AudioContext; buses: Record<Bus, GainNode> };

/**
 * A sound due at `atMs` on `performance.now()`'s clock, and its source while scheduled on the
 * output's clock, which a stopped output drops, since a sound scheduled there would sound late.
 */
interface Cue {
  voice: Voice;
  bus: Bus;
  atMs: number;
  source: AudioBufferSourceNode | null;
}

let output: Output = { kind: "locked" };
/** The voices, none until they have loaded, or ever where audio is unavailable. */
let voices: Record<Voice, LoadedVoice> | null = null;
/** When each opponent chop voice in use ends, on the output's clock. */
const opponentChopEndsS: number[] = new Array<number>(MAX_OPPONENT_CHOPS).fill(-Infinity);
const cues = new Set<Cue>();
/** Asks for a click where the browser stopped the page's audio, as some do in a background tab. */
let soundNote: HTMLElement | null = null;

/**
 * Loads the voices and lets them play from the first pointer or key input on, the name entry
 * included, as browsers require.
 */
export function enableSound(): void {
  if (typeof AudioContext === "undefined" || typeof OfflineAudioContext === "undefined") return;
  soundNote = document.querySelector<HTMLElement>("#sound-note");
  for (const type of ["pointerdown", "keydown"]) window.addEventListener(type, onInput, { capture: true, passive: true });
  loadVoices().then(
    (loaded) => {
      voices = loaded;
      syncCues();
    },
    (error: unknown) => console.error("loading the sounds failed, so the game stays silent:", error),
  );
  // Starts the browser's audio service now, which the first AudioContext otherwise starts while
  // blocking the page for about 100 ms, at an input that may come mid-round. It asks no permission.
  navigator.mediaDevices?.enumerateDevices().catch(() => {});
}

/** Plays a chop timed to land with a cut's knife, given as the cut's motion starts. */
export function playChop(source: ChopSource): void {
  if (!voices) return;
  play("chop", source === "own" ? "ownChop" : "opponentChop", DESCEND_S - voices.chop.peakS);
}

/** Blows the whistle for the last cut of a round, as an offline round's only cut is. */
export function playLastCutWhistle(): void {
  play("whistle", "whistle", WHISTLE_AFTER_LAST_CUT_S);
}

/**
 * An online round's sounds by the server's clock: the countdown's tick each second of its last
 * `URGENT_S`, and the whistle once, at its 0 s or after its last cut if that comes first.
 */
export class RoundSounds {
  private roundMs = 0;
  private cues: Cue[] = [];

  /** Readies for a round `roundMs` long. */
  open(roundMs: number): void {
    this.close();
    this.roundMs = roundMs;
  }

  /** Starts the round opened last at `startMs` on `performance.now()`'s clock, skipping what is already past. */
  start(startMs: number): void {
    this.close();
    const endMs = startMs + this.roundMs;
    const tickAtMs = Array.from({ length: URGENT_S }, (_, idx) => endMs - (URGENT_S - idx) * 1000);
    this.cues = addCues([...buildCues("tick", "tick", tickAtMs), ...buildCues("whistle", "whistle", [endMs])]);
  }

  /** Blows for the round's last cut, ending its ticks, unless the whistle already blew at 0 s. */
  blowForLastCut(): void {
    const whistle = this.cues.at(-1);
    if (!whistle || whistle.atMs <= performance.now()) return;
    this.close();
    play("whistle", "whistle", WHISTLE_AFTER_LAST_CUT_S);
  }

  /** Silences the round, as leaving it does. */
  close(): void {
    for (const cue of this.cues) removeCue(cue);
    this.cues = [];
  }
}

/**
 * A judging reveal's sounds, all scheduled when it starts: the gram counters' ticks, quieter than
 * the countdown's, since they rattle while the count runs fast, and a bell's ding as each judged
 * player joins the leaderboard.
 */
export class RevealSounds {
  private cues: Cue[] = [];

  /**
   * Ticks at each of `tickAtMs` and dings at each of `dingAtMs` on `performance.now()`'s clock,
   * skipping those already past.
   */
  start(tickAtMs: readonly number[], dingAtMs: readonly number[]): void {
    this.close();
    this.cues = addCues([...buildCues("tick", "countTick", tickAtMs), ...buildCues("ding", "ding", dingAtMs)]);
  }

  /** Silences the reveal, as its end or leaving the room does. */
  close(): void {
    for (const cue of this.cues) removeCue(cue);
    this.cues = [];
  }
}

/** Plays `voice` through `bus` in `delayS`, unless the output is not running or an opponent's chop finds every voice in use. */
function play(voice: Voice, bus: Bus, delayS: number): void {
  if (output.kind !== "open" || output.context.state !== "running" || !voices) return;
  schedule(output, voices, voice, bus, output.context.currentTime + delayS);
}

/** Starts `voice` through `bus` at `startS` on `open`'s clock, returning its source, or none for an opponent's chop finding every voice in use. */
function schedule(open: Output & { kind: "open" }, loaded: Record<Voice, LoadedVoice>, voice: Voice, bus: Bus, startS: number): AudioBufferSourceNode | null {
  if (bus === "opponentChop" && !takeOpponentChop(startS, loaded.chop.buffer.duration)) return null;
  const source = new AudioBufferSourceNode(open.context, { buffer: loaded[voice].buffer });
  source.connect(open.buses[bus]);
  source.start(startS);
  return source;
}

/** Claims an opponent chop voice from `startS` for `durationS`, returning false while all are in use. */
function takeOpponentChop(startS: number, durationS: number): boolean {
  const idx = opponentChopEndsS.findIndex((endS) => endS <= startS);
  if (idx < 0) return false;
  opponentChopEndsS[idx] = startS + durationS;
  return true;
}

function buildCues(voice: Voice, bus: Bus, atMs: readonly number[]): Cue[] {
  return atMs.map((ms) => ({ voice, bus, atMs: ms, source: null }));
}

/** Adds `added` to the cues to play, scheduling them now where the output runs, and returns them. */
function addCues(added: Cue[]): Cue[] {
  for (const cue of added) cues.add(cue);
  syncCues();
  return added;
}

function removeCue(cue: Cue): void {
  cue.source?.stop();
  cues.delete(cue);
}

/**
 * Schedules every cue still to come on the output's clock while it runs, and drops their sources
 * while it does not, to schedule them again once it runs: a stopped clock would play them late.
 */
function syncCues(): void {
  const nowMs = performance.now();
  const running = output.kind === "open" && output.context.state === "running" ? output : null;
  for (const cue of cues) {
    if (cue.atMs <= nowMs) {
      cues.delete(cue);
      continue;
    }
    if (!running || !voices) {
      cue.source?.stop();
      cue.source = null;
      continue;
    }
    cue.source ??= schedule(running, voices, cue.voice, cue.bus, running.context.currentTime + (cue.atMs - nowMs) / 1000);
  }
}

/** Opens the output on the first input, and resumes it on any later one if the browser stopped it. */
function onInput(): void {
  if (output.kind === "locked") output = openOutput();
  if (output.kind !== "open" || output.context.state === "running") return;
  const stopped = output;
  stopped.context.resume().catch((error: unknown) => {
    console.error("resuming the page's audio failed, so the game stays silent:", error);
    if (output !== stopped) return;
    output = { kind: "unavailable" };
    showSoundNote(false);
  });
}

function openOutput(): Exclude<Output, { kind: "locked" }> {
  let context: AudioContext;
  try {
    context = new AudioContext();
  } catch (error) {
    console.error("opening the page's audio failed, so the game stays silent:", error);
    return { kind: "unavailable" };
  }
  let hasRun = context.state === "running";
  context.addEventListener("statechange", () => {
    // The page never stops its own output, so a running output that stops was stopped by the
    // browser, as some do in a background tab, until an input resumes it.
    hasRun ||= context.state === "running";
    showSoundNote(hasRun && context.state !== "running" && context.state !== "closed");
    syncCues();
  });
  // Runs from the next statechange, since an output running already fires none.
  queueMicrotask(syncCues);
  return { kind: "open", context, buses: buildBuses(context) };
}

function showSoundNote(isShown: boolean): void {
  if (soundNote && soundNote.hidden === isShown) soundNote.hidden = !isShown;
}

function buildBuses(context: AudioContext): Record<Bus, GainNode> {
  const buses = Object.entries(BUS_GAINS).map(([bus, gain]) => [bus, new GainNode(context, { gain })] as const);
  for (const [, gain] of buses) gain.connect(context.destination);
  return Object.fromEntries(buses) as Record<Bus, GainNode>;
}

async function loadVoices(): Promise<Record<Voice, LoadedVoice>> {
  const [chop, tick, whistle, ding] = await Promise.all([
    fetchRecording(RECORDING_URLS.chop),
    renderTick(),
    fetchRecording(RECORDING_URLS.whistle),
    fetchRecording(RECORDING_URLS.ding),
  ]);
  return { chop, tick, whistle, ding };
}

/**
 * Downloads and decodes a recording in an offline context, which, unlike the page's first
 * AudioContext, holds up no frame while the browser starts its audio service.
 */
async function fetchRecording(url: string): Promise<LoadedVoice> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`fetching ${url} failed: HTTP ${response.status}`);
  const encoded = await response.arrayBuffer();
  const buffer = await new OfflineAudioContext(1, 1, VOICE_RATE_HZ).decodeAudioData(encoded);
  return normalize(buffer);
}

/** Renders the tick once, for every later play to reuse. */
async function renderTick(): Promise<LoadedVoice> {
  const context = new OfflineAudioContext(1, Math.ceil(TICK_LENGTH_S * VOICE_RATE_HZ), VOICE_RATE_HZ);
  buildTick(context);
  return normalize(await context.startRendering());
}

/** Scales `buffer` to peak at `VOICE_PEAK`, finding when it peaks. */
function normalize(buffer: AudioBuffer): LoadedVoice {
  const samples = buffer.getChannelData(0);
  let peakIdx = 0;
  for (let i = 1; i < samples.length; i += 1) if (Math.abs(samples[i]) > Math.abs(samples[peakIdx])) peakIdx = i;
  const peak = Math.abs(samples[peakIdx]);
  if (peak > 0) for (let i = 0; i < samples.length; i += 1) samples[i] *= VOICE_PEAK / peak;
  return { buffer, peakS: peakIdx / buffer.sampleRate };
}

/** A mechanical clock's tick: a short bright click over a woody tock. */
function buildTick(context: OfflineAudioContext): void {
  const atS = 0.002;
  strike(connectNoise(context, 0x71c1, new BiquadFilterNode(context, { type: "bandpass", frequency: 2800, Q: 2 })), atS, 0.6, 0.003);
  strike(connectTone(context, 1900, atS), atS, 0.25, 0.008);
  strike(connectTone(context, 850, atS), atS, 0.35, 0.012);
}

/** Sets `gain` to rise to `peak` at `atS` within a millisecond, then decay with time constant `decayS`. */
function strike(gain: GainNode, atS: number, peak: number, decayS: number): void {
  gain.gain.setValueAtTime(0, atS);
  gain.gain.linearRampToValueAtTime(peak, atS + 0.001);
  gain.gain.setTargetAtTime(0, atS + 0.001, decayS);
}

/** Plays seeded white noise from 0 through `filter` into a gain, silent until set, then into the destination. */
function connectNoise(context: OfflineAudioContext, seed: number, filter: BiquadFilterNode): GainNode {
  const buffer = context.createBuffer(1, context.length, context.sampleRate);
  const samples = buffer.getChannelData(0);
  let state = seed;
  for (let i = 0; i < samples.length; i += 1) {
    // xorshift32: the same noise every render, so every page and the exported WAVs sound alike.
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    samples[i] = (state >>> 0) / 0x80000000 - 1;
  }
  const noise = new AudioBufferSourceNode(context, { buffer });
  const gain = new GainNode(context, { gain: 0 });
  noise.connect(filter).connect(gain).connect(context.destination);
  noise.start(0);
  return gain;
}

/** Plays a sine at `frequencyHz` from `atS` into a gain, silent until set, then into the destination. */
function connectTone(context: OfflineAudioContext, frequencyHz: number, atS: number): GainNode {
  const oscillator = new OscillatorNode(context, { frequency: frequencyHz });
  const gain = new GainNode(context, { gain: 0 });
  oscillator.connect(gain).connect(context.destination);
  oscillator.start(atS);
  return gain;
}
