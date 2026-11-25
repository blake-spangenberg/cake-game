import { DESCEND_S } from "./cutMotion";

/**
 * The game's sounds, synthesized as the page loads rather than shipped as files: a knife chopping
 * through a cake onto the board, a clock's tick, and a referee's whistle. Browsers start audio
 * only after a user gesture, so nothing sounds before the first input after `enableSound`, and
 * every sound is silently skipped where audio is unavailable.
 */

type Voice = "chop" | "tick" | "whistle";
type Bus = "ownChop" | "opponentChop" | "tick" | "countTick" | "whistle";
/** Whose cut a chop is for: an opponent's plays quieter than the player's own. */
export type ChopSource = "own" | "opponent";

const VOICES: readonly Voice[] = ["chop", "tick", "whistle"];
/** The voices render at this rate whatever the output's, which resamples them. */
const VOICE_RATE_HZ = 48000;
const VOICE_PEAK = 0.89;
/** Each bus's gain on its voice, whose rendering peaks at `VOICE_PEAK`. Laptop speakers play these to a whole Zoom room. */
const BUS_GAINS: Record<Bus, number> = { ownChop: 0.6, opponentChop: 0.2, tick: 0.35, countTick: 0.12, whistle: 0.22 };
/** At most this many opponents' chops sound at once, so a dozen cuts in the same instant do not blare. */
const MAX_OPPONENT_CHOPS = 3;
/** The whistle waits this long after the round's last cut, so the last chop is heard first. */
const WHISTLE_AFTER_LAST_CUT_S = 0.8;
/** Where the knife lands on the board in the chop's rendering, after the blade's slice through the cake. */
const CHOP_LAND_S = 0.05;

/** The page's audio output: locked until the first input, unavailable where the browser has none or refused it. */
type Output = { kind: "locked" } | { kind: "unavailable" } | { kind: "open"; context: AudioContext; buses: Record<Bus, GainNode> };

let output: Output = { kind: "locked" };
/** The voices, none until they have rendered, or ever where audio is unavailable. */
let voices: Record<Voice, AudioBuffer> | null = null;
/** When each opponent chop voice in use ends, on the output's clock. */
const opponentChopEndsS: number[] = new Array<number>(MAX_OPPONENT_CHOPS).fill(-Infinity);

/**
 * Renders the voices and lets them play from the first pointer or key input on, the name entry
 * included, as browsers require.
 */
export function enableSound(): void {
  if (typeof AudioContext === "undefined" || typeof OfflineAudioContext === "undefined") return;
  for (const type of ["pointerdown", "keydown"]) window.addEventListener(type, onInput, { capture: true, passive: true });
  // Voices that fail to render leave the game silent, as where audio is unavailable.
  renderVoices().then((rendered) => (voices = rendered), () => {});
  // Starts the browser's audio service now, which the first AudioContext otherwise starts while
  // blocking the page for about 100 ms, at an input that may come mid-round. It asks no permission.
  navigator.mediaDevices?.enumerateDevices().catch(() => {});
}

/** Plays a chop timed to land with a cut's knife, given as the cut's motion starts. */
export function playChop(source: ChopSource): void {
  play("chop", source === "own" ? "ownChop" : "opponentChop", DESCEND_S - CHOP_LAND_S);
}

/** Plays the countdown's tick. */
export function playTick(): void {
  play("tick", "tick", 0);
}

/** Plays the tick of a reveal's gram counter, quieter than the countdown's, since it rattles while the count runs fast. */
export function playCountTick(): void {
  play("tick", "countTick", 0);
}

/** Blows the whistle for the last cut of a round, as an offline round's only cut is. */
export function playLastCutWhistle(): void {
  play("whistle", "whistle", WHISTLE_AFTER_LAST_CUT_S);
}

/** Blows the whistle once per online round: at its 0 s, or after its last cut if that comes first. */
export class RoundWhistle {
  private roundMs = 0;
  private deadlineTimer: number | null = null;

  /** Readies for a round `roundMs` long. */
  open(roundMs: number): void {
    this.close();
    this.roundMs = roundMs;
  }

  /** Starts the round opened last at `startMs` on `performance.now()`'s clock. A round already past its 0 s stays silent. */
  start(startMs: number): void {
    this.close();
    const untilEndMs = startMs + this.roundMs - performance.now();
    if (untilEndMs <= 0) return;
    this.deadlineTimer = window.setTimeout(() => this.blow(0), untilEndMs);
  }

  /** Blows for the round's last cut, unless the whistle already blew at 0 s. */
  blowForLastCut(): void {
    this.blow(WHISTLE_AFTER_LAST_CUT_S);
  }

  /** Silences the round, as leaving it does. */
  close(): void {
    if (this.deadlineTimer !== null) window.clearTimeout(this.deadlineTimer);
    this.deadlineTimer = null;
  }

  private blow(delayS: number): void {
    if (this.deadlineTimer === null) return;
    this.close();
    play("whistle", "whistle", delayS);
  }
}

/** Plays `voice` through `bus` in `delayS`, unless the output is not running or an opponent's chop finds every voice in use. */
function play(voice: Voice, bus: Bus, delayS: number): void {
  if (output.kind !== "open" || output.context.state !== "running" || !voices) return;
  const startS = output.context.currentTime + delayS;
  if (bus === "opponentChop" && !takeOpponentChop(startS, voices.chop.duration)) return;
  const source = new AudioBufferSourceNode(output.context, { buffer: voices[voice] });
  source.connect(output.buses[bus]);
  source.start(startS);
}

/** Claims an opponent chop voice from `startS` for `durationS`, returning false while all are in use. */
function takeOpponentChop(startS: number, durationS: number): boolean {
  const idx = opponentChopEndsS.findIndex((endS) => endS <= startS);
  if (idx < 0) return false;
  opponentChopEndsS[idx] = startS + durationS;
  return true;
}

/** Opens the output on the first input, and resumes it on any later one if the browser suspended it. */
function onInput(): void {
  if (output.kind === "locked") output = openOutput();
  if (output.kind !== "open" || output.context.state === "running") return;
  const suspended = output;
  suspended.context.resume().catch(() => {
    if (output === suspended) output = { kind: "unavailable" };
  });
}

function openOutput(): Exclude<Output, { kind: "locked" }> {
  try {
    const context = new AudioContext();
    return { kind: "open", context, buses: buildBuses(context) };
  } catch {
    return { kind: "unavailable" };
  }
}

function buildBuses(context: AudioContext): Record<Bus, GainNode> {
  const buildBus = (bus: Bus) => {
    const gain = new GainNode(context, { gain: BUS_GAINS[bus] });
    gain.connect(context.destination);
    return gain;
  };
  return { ownChop: buildBus("ownChop"), opponentChop: buildBus("opponentChop"), tick: buildBus("tick"), countTick: buildBus("countTick"), whistle: buildBus("whistle") };
}

async function renderVoices(): Promise<Record<Voice, AudioBuffer>> {
  const [chop, tick, whistle] = await Promise.all(VOICES.map(renderVoice));
  return { chop, tick, whistle };
}

/** Renders `voice` once, peaking at `VOICE_PEAK`, for every later play to reuse. */
async function renderVoice(voice: Voice): Promise<AudioBuffer> {
  const recipe = RECIPES[voice];
  const context = new OfflineAudioContext(1, Math.ceil(recipe.lengthS * VOICE_RATE_HZ), VOICE_RATE_HZ);
  recipe.build(context);
  const buffer = await context.startRendering();
  const samples = buffer.getChannelData(0);
  const peak = samples.reduce((max, sample) => Math.max(max, Math.abs(sample)), 0);
  if (peak > 0) for (let i = 0; i < samples.length; i += 1) samples[i] *= VOICE_PEAK / peak;
  return buffer;
}

interface Recipe {
  lengthS: number;
  build(context: OfflineAudioContext): void;
}

const RECIPES: Record<Voice, Recipe> = {
  chop: { lengthS: 0.3, build: buildChop },
  tick: { lengthS: 0.08, build: buildTick },
  whistle: { lengthS: 0.8, build: buildWhistle },
};

/**
 * A knife through cake onto a wooden board: a soft slice swelling as the blade passes through the
 * cake, then the knock of the edge meeting the board, its crisp snap over a short woody body.
 * The body sits above 400 Hz as well as below, since laptop speakers drop the low end.
 */
function buildChop(context: OfflineAudioContext): void {
  const landS = CHOP_LAND_S;
  const slice = connectNoise(context, 0x5eed1, new BiquadFilterNode(context, { type: "lowpass", frequency: 1800, Q: 0.7 }));
  slice.gain.setValueAtTime(0, 0);
  slice.gain.linearRampToValueAtTime(0.12, landS);
  slice.gain.setTargetAtTime(0, landS, 0.008);
  strike(connectNoise(context, 0x5eed2, new BiquadFilterNode(context, { type: "bandpass", frequency: 3000, Q: 0.9 })), landS, 0.5, 0.006);
  strike(connectNoise(context, 0x5eed3, new BiquadFilterNode(context, { type: "bandpass", frequency: 900, Q: 1.5 })), landS, 0.9, 0.02);
  const body = connectTone(context, 260, landS);
  body.oscillator.frequency.setValueAtTime(260, landS);
  body.oscillator.frequency.exponentialRampToValueAtTime(170, landS + 0.06);
  strike(body.gain, landS, 0.8, 0.035);
  strike(connectTone(context, 560, landS).gain, landS, 0.35, 0.018);
}

/** A mechanical clock's tick: a short bright click over a woody tock. */
function buildTick(context: OfflineAudioContext): void {
  const atS = 0.002;
  strike(connectNoise(context, 0x71c1, new BiquadFilterNode(context, { type: "bandpass", frequency: 2800, Q: 2 })), atS, 0.6, 0.003);
  strike(connectTone(context, 1900, atS).gain, atS, 0.25, 0.008);
  strike(connectTone(context, 850, atS).gain, atS, 0.35, 0.012);
}

/**
 * A referee's pea whistle in one long blast, pitched lower than a real one's 3-4 kHz so laptop
 * speakers do not make it shrill: a tone that rises into its pitch, warbled in pitch and loudness
 * by the pea's spin, with breath noise around it, and a slight fall as the breath runs out.
 */
function buildWhistle(context: OfflineAudioContext): void {
  const [pitchHz, endS, releaseS, warbleHz] = [2150, 0.78, 0.07, 27];
  const envelope = new GainNode(context, { gain: 0 });
  envelope.connect(context.destination);
  envelope.gain.setValueAtTime(0, 0);
  envelope.gain.linearRampToValueAtTime(1, 0.03);
  envelope.gain.setValueAtTime(1, endS - releaseS);
  envelope.gain.linearRampToValueAtTime(0, endS);
  const tone = new OscillatorNode(context, { frequency: pitchHz * 0.9 });
  tone.frequency.setValueAtTime(pitchHz * 0.9, 0);
  tone.frequency.exponentialRampToValueAtTime(pitchHz, 0.05);
  tone.frequency.setValueAtTime(pitchHz, endS - releaseS);
  tone.frequency.exponentialRampToValueAtTime(pitchHz * 0.95, endS);
  const warble = new OscillatorNode(context, { frequency: warbleHz });
  warble.connect(new GainNode(context, { gain: 45 })).connect(tone.frequency);
  const loudness = new GainNode(context, { gain: 0.75 });
  warble.connect(new GainNode(context, { gain: 0.25 })).connect(loudness.gain);
  tone.connect(loudness).connect(envelope);
  const breath = connectNoise(context, 0xb1e5, new BiquadFilterNode(context, { type: "bandpass", frequency: pitchHz, Q: 6 }), envelope);
  breath.gain.value = 0.15;
  const hiss = connectNoise(context, 0xb1e6, new BiquadFilterNode(context, { type: "highpass", frequency: 1000 }), envelope);
  hiss.gain.value = 0.03;
  tone.start(0);
  warble.start(0);
}

/** Sets `gain` to rise to `peak` at `atS` within a millisecond, then decay with time constant `decayS`. */
function strike(gain: GainNode, atS: number, peak: number, decayS: number): void {
  gain.gain.setValueAtTime(0, atS);
  gain.gain.linearRampToValueAtTime(peak, atS + 0.001);
  gain.gain.setTargetAtTime(0, atS + 0.001, decayS);
}

/** Plays seeded white noise from 0 through `filter` into a gain, silent until set, then into `output`. */
function connectNoise(context: OfflineAudioContext, seed: number, filter: BiquadFilterNode, output: AudioNode = context.destination): GainNode {
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
  noise.connect(filter).connect(gain).connect(output);
  noise.start(0);
  return gain;
}

/** Plays a sine at `frequencyHz` from `atS` into a gain, silent until set, then into the destination. */
function connectTone(context: OfflineAudioContext, frequencyHz: number, atS: number): { oscillator: OscillatorNode; gain: GainNode } {
  const oscillator = new OscillatorNode(context, { frequency: frequencyHz });
  const gain = new GainNode(context, { gain: 0 });
  oscillator.connect(gain).connect(context.destination);
  oscillator.start(atS);
  return { oscillator, gain };
}
