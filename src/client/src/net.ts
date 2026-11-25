/**
 * The client's end of the server's WebSocket, whose messages `src/server/src/protocol.rs` defines
 * and which change together with it. Control messages are JSON text; inputs go up and poses come
 * down as little-endian binary frames.
 */

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

/** The next round's cake, always a real one, and whether the host picked Random, which the server settled on it. */
export type PickMessage = { type: "pick"; cake: string; is_random: boolean };

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

/** The judging reveal begun `elapsed_ms` ago: each round player in `order`, a scene of `scene_ms` apiece, judged by their standing. */
export type JudgingMessage = { type: "judging"; order: number[]; scene_ms: number; elapsed_ms: number; standings: Standing[] };

/** To a page reopened into its seat in a round: every tick its world stepped, as runs of one input in the core's wire format, and the tick it cut at, if it has. */
export type ResumeMessage = { type: "resume"; runs: { ticks: number; input: number[] }[]; cut_tick: number | null };

export type ServerMessage =
  | { type: "welcome"; id: number; seat: string }
  | ResumeMessage
  | RoomMessage
  | PickMessage
  | { type: "claim_refused" }
  /** The round's players named as it started, so a page that joins after one left can still label them. */
  | { type: "round"; cake: string; round_ticks: number; players: Member[] }
  /** The round's players whose page had not loaded the cake when it started, who spectate it instead. */
  | { type: "sit_out"; players: number[] }
  | { type: "start"; elapsed_ms: number }
  | CutMessage
  /** Every round player has cut; the judging follows once the opponents' tiles have played the last cut. */
  | { type: "play_over" }
  | JudgingMessage
  | ResultsMessage
  | { type: "desync"; tick: number }
  | { type: "error"; message: string };

type ClientMessage =
  | { type: "join"; name: string; standup_preference: number; seat: string | null }
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
}

export class Net {
  /** Every player's latest pose in the newest round, by id. */
  readonly poses = new Map<number, PlayerPose>();
  private readonly socket: WebSocket;
  private isLeaving = false;

  /**
   * Joins with `profile`, whose name has at most `MAX_NAME_LEN` characters and no control
   * characters, which the server would refuse, into the `seat` an earlier page of this player's
   * was welcomed with, if it is still held. `onClose` runs when the connection is lost, but not after `disconnect`.
   */
  constructor(url: URL, profile: Profile, seat: string | null, onMessage: (message: ServerMessage) => void, onClose: () => void) {
    this.socket = new WebSocket(url);
    this.socket.binaryType = "arraybuffer";
    this.socket.addEventListener("open", () => this.sendJson({ type: "join", name: profile.name, standup_preference: profile.standupPreference, seat }));
    this.socket.addEventListener("close", (event) => {
      if (this.isLeaving) return;
      console.error(`server connection closed: ${event.code} ${event.reason}`);
      onClose();
    });
    this.socket.addEventListener("message", (event) => {
      if (this.isLeaving) return;
      if (event.data instanceof ArrayBuffer) {
        this.readPoses(event.data);
        return;
      }
      const message = JSON.parse(event.data as string) as ServerMessage;
      if (message.type === "round") this.poses.clear();
      onMessage(message);
    });
  }

  /** The input `Controls.stepInput` built, stamped with the tick it steps the world from. */
  sendInput(tick: number, encoded: Int32Array): void {
    const frame = new Int32Array(1 + encoded.length);
    frame[0] = tick;
    frame.set(encoded, 1);
    this.socket.send(frame);
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
    this.socket.send(JSON.stringify(message));
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
