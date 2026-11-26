/**
 * The client's end of the server's WebSocket, whose messages `src/server/src/protocol.rs` defines
 * and which change together with it. Control messages are JSON text; inputs go up and poses come
 * down as little-endian binary frames.
 */

import { type DebugSettings, IS_DEBUG_PAGE } from "./debug";

/** Floats per pose record: the cake's translation and rotation quaternion, the knife's tilt, lowering, and rest, the stand's angle. */
const POSE_FLOATS = 11;
/** Bytes per pose record: id, tick, and flags as u32, then the floats as f32. */
const POSE_RECORD_SIZE = 4 * 3 + 4 * POSE_FLOATS;
const POSE_FLAG_HOLDING = 1 << 0;
const POSE_FLAG_CUT = 1 << 1;

/** The server's `MAX_NAME_LEN`, the most characters a name may have; a server test keeps the two equal. */
export const MAX_NAME_LEN = 24;
/** The cake a host picks for one the server settles at random. */
export const RANDOM_CAKE = "random";

type Phase = "lobby" | "loading" | "playing" | "judging" | "results";

export interface Member {
  id: number;
  name: string;
}

export type RoomMessage = { type: "room"; phase: Phase; host: number | null; members: Member[] };

export interface Standing {
  id: number;
  name: string;
  rank: number;
  score: number;
  above_m3: number;
  below_m3: number;
  tick: number;
  has_touched: boolean;
}

/** The next round's cake, always a real one, its length, and whether the host picked Random, which the server settled on it. */
export type PickMessage = { type: "pick"; cake: string; is_random: boolean; round_ticks: number };

/** A round's standings, best first, and everyone in the room in the order they give their standup update. */
export type ResultsMessage = { type: "results"; standings: Standing[]; standup: Member[] };

/** A player's cut, frozen at `tick` with its pose, the knife's lowering at its bottom, and whether it divided the cake. Never the score. */
export type CutMessage = {
  type: "cut";
  id: number;
  tick: number;
  pose: { cake: number[]; knife: [number, number, number]; stand_rad: number };
  cut_lower_m: number;
  cake_touch_lower_m: number;
  is_cut_through: boolean;
};

/**
 * The judging reveal begun `elapsed_ms` ago: each round player in `order`, judged by their
 * standing, in a scene of `hold_ms`, then their count's matching `count_ms`, then `after_ms`.
 */
export type JudgingMessage = {
  type: "judging";
  order: number[];
  hold_ms: number;
  count_ms: number[];
  after_ms: number;
  elapsed_ms: number;
  standings: Standing[];
};

/** To a page reopened into its seat in a round: every tick its world stepped, as runs of one input in the core's wire format, and the tick it cut at, if it has. */
export type ResumeMessage = { type: "resume"; runs: { ticks: number; input: number[] }[]; cut_tick: number | null };

export type ServerMessage =
  | { type: "welcome"; id: number; seat: string }
  | ResumeMessage
  | RoomMessage
  | PickMessage
  | { type: "claim_refused" }
  /** A join refused because a player whose page is connected has the same name, ignoring case and surrounding spaces. The server closes the connection after it. */
  | { type: "name_taken" }
  /** The round's players named as they started or joined it, so a page that joins after one left can still label them. */
  | { type: "round"; cake: string; round_ticks: number; players: Member[] }
  /** The round's players whose page has yet to load the cake, who play once it has, their views showing from `now_playing` on. */
  | { type: "still_loading"; players: number[] }
  /** A round player who loaded the cake after the round started, or joined it in play, now plays it. */
  | { type: "now_playing"; player: Member }
  | { type: "start"; elapsed_ms: number }
  | CutMessage
  /** Every round player has cut; the judging follows once the opponents' tiles have played the last cut. */
  | { type: "play_over" }
  | JudgingMessage
  | ResultsMessage
  | { type: "desync"; tick: number }
  | { type: "error"; message: string }
  /** The room's debug settings, to `?debug` pages only. */
  | ({ type: "debug" } & DebugSettings);

type ClientMessage =
  | { type: "join"; name: string; standup_preference: number; has_scrum_update: boolean; seat: string | null; is_debug?: true }
  | { type: "add_bot" }
  | { type: "remove_bot" }
  | { type: "set_round_length"; round_s: number }
  | { type: "set_bot_latency"; latency_ms: number }
  | { type: "claim_host"; password: string }
  | { type: "pick"; cake: string }
  | { type: "start" }
  | { type: "return_to_lobby" }
  | { type: "ready" }
  | { type: "cut"; tick: number }
  | { type: "hash"; tick: number; hash: string };

/** A round player's world as the server last replayed it. */
export interface PlayerPose {
  tick: number;
  isHolding: boolean;
  isCut: boolean;
  /** The cake's translation then rotation quaternion, the knife's tilt, lowering, and rest, the stand's angle. */
  values: Float32Array;
}

/**
 * The server's WebSocket URL when this page should play online: always for a production build,
 * which the server serves, and in development when `?server=<host:port>` names a running server.
 */
export function findServerUrl(): URL | null {
  const server = new URLSearchParams(location.search).get("server");
  if (server) return new URL(`ws://${server}/ws`);
  if (import.meta.env.DEV) return null;
  const url = new URL("ws", location.href);
  url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
  return url;
}

/** What a player gives on joining: the name others see, and their standup order preference, hidden from others until the results. */
export interface Profile {
  name: string;
  /** From 0, first to give their update, to 1, last. */
  standupPreference: number;
  /** Whether they have a scrum update to give, without which they get no standup slot and the preference goes unused. */
  hasScrumUpdate: boolean;
}

/** How long this page's messages take to and from the server beyond the network's own time, each way, which only a `?debug` page sets. */
let simulatedLatencyMs = 0;
/** How far each message's simulated latency strays at random either side of `simulatedLatencyMs`. */
let simulatedJitterMs = 0;
/** Until when, on `performance.now()`'s clock, every message this page sends waits, as a TCP stream waits on a lost packet's resend. */
let sendsHeldUntilMs = 0;

/** Delays every message this page sends or receives by `latencyMs` from now on, each way, to see the game on a slower network. */
export function setSimulatedLatencyMs(latencyMs: number): void {
  simulatedLatencyMs = latencyMs;
  console.info(`simulating ${latencyMs} ms of latency each way`);
}

/** Strays each message's simulated latency from now on by up to `jitterMs` either side, at random, keeping the messages in order as TCP does. */
function setSimulatedJitterMs(jitterMs: number): void {
  simulatedJitterMs = jitterMs;
  console.info(`simulating ${jitterMs} ms of jitter each way`);
}

/** Holds every message this page sends for the next `holdMs`, then lets them go on together, as a lost packet's resend holds a TCP stream. */
function holdSimulatedSends(holdMs: number): void {
  sendsHeldUntilMs = performance.now() + holdMs;
  console.info(`holding sends for ${holdMs} ms`);
}

if (IS_DEBUG_PAGE) Object.assign(window, { setSimulatedJitterMs, holdSimulatedSends });

declare global {
  interface Window {
    /** On a `?debug` page, for step files: strays this page's simulated latency at random, each way. */
    setSimulatedJitterMs?: typeof setSimulatedJitterMs;
    /** On a `?debug` page, for step files: holds this page's sends, as a lost packet does. */
    holdSimulatedSends?: typeof holdSimulatedSends;
  }
}

/**
 * Hands each item on `simulatedLatencyMs`, strayed by up to `simulatedJitterMs`, after it was
 * pushed, or after `heldUntilMs` if that is later, in the order pushed, so a latency lowered
 * meanwhile never lets a later item overtake an earlier one. With no latency and nothing
 * waiting, an item goes on at once.
 */
class DelayLine<T> {
  private readonly waiting: { dueMs: number; item: T }[] = [];
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly onDue: (item: T) => void,
    private readonly getHeldUntilMs: () => number = () => 0,
  ) {}

  push(item: T): void {
    const nowMs = performance.now();
    const strayMs = simulatedJitterMs * (2 * Math.random() - 1);
    const dueMs = Math.max(nowMs + simulatedLatencyMs + strayMs, this.getHeldUntilMs() + simulatedLatencyMs, this.waiting.at(-1)?.dueMs ?? 0);
    if (dueMs <= nowMs && this.waiting.length === 0) {
      this.onDue(item);
      return;
    }
    this.waiting.push({ dueMs, item });
    this.schedule();
  }

  private schedule(): void {
    if (this.timer !== null || this.waiting.length === 0) return;
    this.timer = setTimeout(() => this.release(), Math.max(this.waiting[0].dueMs - performance.now(), 0));
  }

  private release(): void {
    this.timer = null;
    const nowMs = performance.now();
    while (this.waiting.length > 0 && this.waiting[0].dueMs <= nowMs) this.onDue(this.waiting.shift()!.item);
    this.schedule();
  }
}

export class Net {
  /** Every player's latest pose in the newest round, by id. */
  readonly poses = new Map<number, PlayerPose>();
  private readonly socket: WebSocket;
  private readonly outgoing: DelayLine<string | Int32Array<ArrayBuffer>>;
  private readonly incoming: DelayLine<MessageEvent | CloseEvent>;
  private isLeaving = false;

  /**
   * Joins with `profile`, whose name has at most `MAX_NAME_LEN` characters and no control
   * characters, which the server would refuse, into the `seat` an earlier page of this player's
   * was welcomed with, if it is still held. `onClose` runs when the connection is lost, but not after `disconnect`.
   */
  constructor(url: URL, profile: Profile, seat: string | null, onMessage: (message: ServerMessage) => void, onClose: () => void) {
    this.socket = new WebSocket(url);
    this.socket.binaryType = "arraybuffer";
    this.outgoing = new DelayLine((data) => this.socket.send(data), () => sendsHeldUntilMs);
    this.incoming = new DelayLine((event) => {
      if (this.isLeaving) return;
      if (event instanceof CloseEvent) {
        console.error(`server connection closed: ${event.code} ${event.reason}`);
        onClose();
        return;
      }
      this.onMessage(event, onMessage);
    });
    this.socket.addEventListener("open", () => {
      const join = { type: "join", name: profile.name, standup_preference: profile.standupPreference, has_scrum_update: profile.hasScrumUpdate, seat } as const;
      this.sendJson(IS_DEBUG_PAGE ? { ...join, is_debug: true } : join);
    });
    this.socket.addEventListener("close", (event) => this.incoming.push(event));
    this.socket.addEventListener("message", (event) => this.incoming.push(event));
  }

  /** The input `Controls.stepInput` built, stamped with the tick it steps the world from. */
  sendInput(tick: number, encoded: Int32Array): void {
    const frame = new Int32Array(1 + encoded.length);
    frame[0] = tick;
    frame.set(encoded, 1);
    this.outgoing.push(frame);
  }

  /** From a `?debug` page: adds a bot to the room. */
  sendAddBot(): void {
    this.sendJson({ type: "add_bot" });
  }

  /** From a `?debug` page: removes the room's newest bot. */
  sendRemoveBot(): void {
    this.sendJson({ type: "remove_bot" });
  }

  /** From a `?debug` page: sets the length of the rounds started from now on. */
  sendSetRoundLength(roundS: number): void {
    this.sendJson({ type: "set_round_length", round_s: roundS });
  }

  /** From a `?debug` page: sets the bots' latency each way. */
  sendSetBotLatency(latencyMs: number): void {
    this.sendJson({ type: "set_bot_latency", latency_ms: latencyMs });
  }

  private onMessage(event: MessageEvent, onMessage: (message: ServerMessage) => void): void {
    if (event.data instanceof ArrayBuffer) {
      this.readPoses(event.data);
      return;
    }
    const message = JSON.parse(event.data as string) as ServerMessage;
    if (message.type === "round") this.poses.clear();
    onMessage(message);
  }

  /** Leaves the room for good, with no `onClose` and no more messages. */
  disconnect(): void {
    this.isLeaving = true;
    this.socket.close();
  }

  sendClaimHost(password: string): void {
    this.sendJson({ type: "claim_host", password });
  }

  sendPick(cake: string): void {
    this.sendJson({ type: "pick", cake });
  }

  sendStart(): void {
    this.sendJson({ type: "start" });
  }

  sendReturnToLobby(): void {
    this.sendJson({ type: "return_to_lobby" });
  }

  sendReady(): void {
    this.sendJson({ type: "ready" });
  }

  sendCut(tick: number): void {
    this.sendJson({ type: "cut", tick });
  }

  sendHash(tick: number, hash: bigint): void {
    this.sendJson({ type: "hash", tick, hash: hash.toString() });
  }

  private sendJson(message: ClientMessage): void {
    this.outgoing.push(JSON.stringify(message));
  }

  private readPoses(frame: ArrayBuffer): void {
    const view = new DataView(frame);
    for (let offset = 0; offset + POSE_RECORD_SIZE <= frame.byteLength; offset += POSE_RECORD_SIZE) {
      const flags = view.getUint32(offset + 8, true);
      const values = new Float32Array(POSE_FLOATS);
      for (let i = 0; i < POSE_FLOATS; i++) values[i] = view.getFloat32(offset + 12 + 4 * i, true);
      const pose = { tick: view.getUint32(offset + 4, true), isHolding: (flags & POSE_FLAG_HOLDING) !== 0, isCut: (flags & POSE_FLAG_CUT) !== 0, values };
      this.poses.set(view.getUint32(offset, true), pose);
    }
  }
}
