/**
 * The page's entry, which loads without three.js, the scene, or the WASM: an online page shows
 * the lobby, asks for the name at once, and joins the room as soon as the player answers, while
 * the game (`main.ts`) loads behind the dialog, so the lobby's warm-ups still run before any round
 * starts and a player who joins at once waits on neither the game nor the WASM to see the room.
 */
import { Lobby } from "./lobby";
import { Net, type Profile, type RoomMessage, type ServerMessage, findServerUrl } from "./net";
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
  /** The room as last described before the game loaded, shown once the round's message says whether this page plays in it. */
  private earlyRoom: RoomMessage | null = null;

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
    if (message.type === "welcome") saveSeat(message.seat);
    if (message.type === "pick" || message.type === "round") this.resolveFirstCakeId(message.cake);
    if (this.handlers) {
      this.handlers.onMessage(message, 0);
      return;
    }
    this.held.push({ kind: "message", message, receivedMs: performance.now() });
    this.showEarly(message);
  }

  /**
   * Shows the room's players and pick before the game has loaded, which waits on the WASM, the
   * cake's download, and the scene. A room in a round shows once the round's message says this
   * page spectates it, shrunk to the rules as it will be beside the round, and a round this page
   * plays in shows only once the game has loaded.
   */
  private showEarly(message: ServerMessage): void {
    if (message.type === "welcome") this.myId = message.id;
    if (message.type === "room") this.earlyRoom = message;
    if (message.type === "room" && message.phase === "lobby") this.lobby.showRoom(message, this.myId);
    if (message.type === "pick") this.lobby.showPick(message);
    if (message.type === "round" && this.earlyRoom && !message.players.some((player) => player.id === this.myId)) {
      this.lobby.showRoom(this.earlyRoom, this.myId);
      this.lobby.setVisible(true);
    }
  }

  private onLost(): void {
    if (this.handlers) {
      this.handlers.onLost();
      return;
    }
    this.held.push({ kind: "lost" });
    this.lobby.showDisconnected();
  }
}

enableSound();
export const serverUrl = findServerUrl();
export const lobby = serverUrl ? new Lobby() : null;
export const room = serverUrl && lobby ? new Room(serverUrl, lobby) : null;
if (lobby && room) {
  recordInput();
  lobby.registerActions({
    onClaimHost: (password) => room.getNet()?.sendClaimHost(password),
    onPick: (cake) => room.getNet()?.sendPick(cake),
    onStart: () => room.getNet()?.sendStart(),
    onReturnToLobby: () => room.getNet()?.sendReturnToLobby(),
  });
  lobby
    .waitProfile()
    .then((profile) => room.connect(profile))
    .catch((error: unknown) => console.error("joining the room failed:", error));
}
import("./main").catch((error: unknown) => console.error("loading the game failed:", error));
