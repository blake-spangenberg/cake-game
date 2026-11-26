/**
 * The page's entry, which loads without three.js, the scene, or the WASM: an online page shows
 * the lobby, asks for the name at once, and joins the room as soon as the player answers, while
 * the game (`main.ts`) loads behind the dialog, so the lobby's warm-ups still run before any round
 * starts and a player who joins at once waits on neither the game nor the WASM to see the room.
 */
import { DebugPanel, IS_DEBUG_PAGE, RoundTimer } from "./debug";
import { Lobby, showFatal } from "./lobby";
import { Net, type Profile, type ServerMessage, findServerUrl, setSimulatedLatencyMs } from "./net";
import { loadSeat, recordInput, saveSeat } from "./profile";
import { enableSound } from "./sound";

/** What the game does with the room's messages and with a lost connection, once it has loaded. */
export interface RoomHandlers {
  /** Takes `message`, which waited `heldMs` for the game to load, or 0 once the game has. */
  onMessage(message: ServerMessage, heldMs: number): void;
  onLost(): void;
}

/** A message from the room, with when it arrived, or the connection's loss, waiting for the game. */
type Held = { kind: "message"; message: ServerMessage; receivedMs: number } | { kind: "lost" };

/**
 * This page's connection to the room, which each join opens. Until the game hands over its
 * handlers, the room's messages and a lost connection wait in order, while the lobby already
 * lists the room's players and pick.
 */
export class Room {
  /**
   * The first cake the room names, its pick or its round's, which the game builds its first world
   * on, so it downloads no cake the room has no use for.
   */
  readonly firstCakeId: Promise<string>;
  private resolveFirstCakeId: (id: string) => void = () => {};
  private net: Net | null = null;
  private handlers: RoomHandlers | null = null;
  private held: Held[] = [];
  private myId = 0;
  /** Whether the server welcomed the latest join into a seat, rather than refusing it with an error before closing. */
  private isWelcomed = false;
  private isRefused = false;

  constructor(
    private readonly url: URL,
    private readonly lobby: Lobby,
  ) {
    this.firstCakeId = new Promise((resolve) => (this.resolveFirstCakeId = resolve));
  }

  /** The connection of the latest join, or none before the first and after `disconnect`. */
  getNet(): Net | null {
    return this.net;
  }

  /** Joins the room as `profile`, into this browser's held seat if the server still holds it. */
  connect(profile: Profile): void {
    this.isWelcomed = false;
    this.isRefused = false;
    this.net = new Net(this.url, profile, loadSeat(), (message) => this.onMessage(message), () => this.onLost());
  }

  /** Leaves the room for good, with no more messages or loss from that connection. */
  disconnect(): void {
    this.net?.disconnect();
    this.net = null;
  }

  /** Gives the room's messages and a lost connection to `handlers` from now on, those held so far first, in order. */
  handOver(handlers: RoomHandlers): void {
    const nowMs = performance.now();
    for (const held of this.held) {
      if (held.kind === "lost") handlers.onLost();
      else handlers.onMessage(held.message, nowMs - held.receivedMs);
    }
    this.held = [];
    this.handlers = handlers;
  }

  private onMessage(message: ServerMessage): void {
    if (message.type === "name_taken") {
      this.askAgainForTakenName();
      return;
    }
    if (message.type === "welcome") {
      saveSeat(message.seat);
      this.isWelcomed = true;
    }
    if (message.type === "error" && !this.isWelcomed) this.isRefused = true;
    if (message.type === "pick" || message.type === "round") this.resolveFirstCakeId(message.cake);
    if (this.handlers) {
      this.handlers.onMessage(message, 0);
      return;
    }
    this.held.push({ kind: "message", message, receivedMs: performance.now() });
    this.showEarly(message);
  }

  /**
   * Shows the room's lobby, players, and pick before the game has loaded, which waits on the
   * WASM, the cake's download, and the scene. A room in a round or its results shows only once
   * the game has loaded, with "Loading..." until then.
   */
  private showEarly(message: ServerMessage): void {
    if (message.type === "welcome") this.myId = message.id;
    if (message.type === "room" && message.phase === "lobby") {
      this.lobby.showRoom(message, this.myId);
      debugPanel?.showHost(message.host === this.myId);
      this.lobby.setScreen("lobby");
    }
    if (message.type === "pick") this.lobby.showPick(message);
    if (message.type === "debug") debugPanel?.showSettings(message);
  }

  /** Drops the refused connection, which the server closes, and joins again with the name the player picks next. */
  private askAgainForTakenName(): void {
    this.disconnect();
    this.lobby
      .askProfile(true)
      .then((profile) => this.connect(profile))
      .catch((error: unknown) => console.error("joining the room again failed:", error));
  }

  private onLost(): void {
    this.lobby.showDisconnected(this.isRefused ? "refused" : "lost");
    if (this.handlers) {
      this.handlers.onLost();
      return;
    }
    this.held.push({ kind: "lost" });
  }
}

enableSound();
export const serverUrl = findServerUrl();
export const lobby = serverUrl ? new Lobby() : null;
export const room = serverUrl && lobby ? new Room(serverUrl, lobby) : null;
/** A `?debug` page's timer from a round's Start to its results, online only. */
export const roundTimer = IS_DEBUG_PAGE && room ? new RoundTimer() : null;
/** A `?debug` page's lobby controls, online only. */
export const debugPanel =
  IS_DEBUG_PAGE && room
    ? new DebugPanel({
        onAddBot: () => room.getNet()?.sendAddBot(),
        onRemoveBot: () => room.getNet()?.sendRemoveBot(),
        onSetRoundLength: (roundS) => room.getNet()?.sendSetRoundLength(roundS),
        onSetBotLatency: (latencyMs) => room.getNet()?.sendSetBotLatency(latencyMs),
        onSetOwnLatency: setSimulatedLatencyMs,
      })
    : null;
if (lobby && room) {
  recordInput();
  lobby.registerActions({
    onClaimHost: (password) => room.getNet()?.sendClaimHost(password),
    onPick: (cake) => room.getNet()?.sendPick(cake),
    onStart: () => {
      roundTimer?.start(performance.now());
      room.getNet()?.sendStart();
    },
    onReturnToLobby: () => room.getNet()?.sendReturnToLobby(),
  });
  lobby
    .waitProfile()
    .then((profile) => room.connect(profile))
    .catch((error: unknown) => console.error("joining the room failed:", error));
}
import("./main").catch((error: unknown) => {
  console.error("loading the game failed:", error);
  showFatal("failed");
});
