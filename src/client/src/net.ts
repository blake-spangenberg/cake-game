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

type Phase = "lobby" | "loading" | "playing" | "results";

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

export type ServerMessage =
  | { type: "welcome"; id: number }
  | RoomMessage
  | { type: "round"; cake: string; round_ticks: number; players: number[] }
  | { type: "start"; elapsed_ms: number }
  | {
      type: "cut";
      id: number;
      tick: number;
      pose: { cake: number[]; knife: [number, number, number]; stand_rad: number };
      cut_lower_m: number;
      cake_touch_lower_m: number;
      is_cut_through: boolean;
    }
  | { type: "results"; standings: Standing[] }
  | { type: "desync"; tick: number }
  | { type: "error"; message: string };

type ClientMessage =
  | { type: "join"; name: string }
  | { type: "start"; cake: string }
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

export class Net {
  /** Every player's latest pose in the newest round, by id. */
  readonly poses = new Map<number, PlayerPose>();
  private readonly socket: WebSocket;

  /** Joins as `name`, at most `MAX_NAME_LEN` characters with no control characters, which the server would refuse. */
  constructor(url: URL, name: string, onMessage: (message: ServerMessage) => void, onClose: () => void) {
    this.socket = new WebSocket(url);
    this.socket.binaryType = "arraybuffer";
    this.socket.addEventListener("open", () => this.sendJson({ type: "join", name }));
    this.socket.addEventListener("close", (event) => {
      console.error(`server connection closed: ${event.code} ${event.reason}`);
      onClose();
    });
    this.socket.addEventListener("message", (event) => {
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

  sendStart(cake: string): void {
    this.sendJson({ type: "start", cake });
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
