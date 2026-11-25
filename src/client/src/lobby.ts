import { CAKES, CAKE_KINDS, type CakeEntry, type CakeKind } from "./cakes";
import { MAX_NAME_LEN, type Member, type RoomMessage, type Standing } from "./net";

/** The cake id the server reads as a random cake. */
export const RANDOM_CAKE = "random";
/** Unicode control characters, which the server refuses in a name. */
const CONTROL_CHARS = /\p{Cc}/gu;

/**
 * What an online page shows outside a round: the name entry before joining, then the players,
 * the how-to-play card, the credits link, and the host's cake picker and Start, or the results
 * of the round just played.
 */
export class Lobby {
  private readonly panel = document.querySelector<HTMLElement>("#lobby")!;
  private readonly players = document.querySelector<HTMLUListElement>("#players")!;
  private readonly status = document.querySelector<HTMLParagraphElement>("#lobby-status")!;
  private readonly standingsCard = document.querySelector<HTMLElement>("#standings-card")!;
  private readonly standingsList = document.querySelector<HTMLOListElement>("#standings")!;
  private readonly pick = document.querySelector<HTMLElement>("#cake-pick")!;
  private readonly tiles = document.querySelector<HTMLDivElement>("#cake-tiles")!;
  private readonly startButton = document.querySelector<HTMLButtonElement>("#start")!;
  private room: RoomMessage | null = null;
  private myId = 0;
  private pickedCake = RANDOM_CAKE;
  private isDisconnected = false;

  constructor(onStart: (cake: string) => void) {
    this.startButton.addEventListener("click", () => {
      this.startButton.disabled = true;
      onStart(this.pickedCake);
    });
  }

  /** Asks for the player's name until they give one the server accepts, trimmed and without control characters. */
  waitName(): Promise<string> {
    const dialog = document.querySelector<HTMLDialogElement>("#join")!;
    const input = document.querySelector<HTMLInputElement>("#name")!;
    input.maxLength = MAX_NAME_LEN;
    // Typing must not reach the camera's arrow-key and WASD swing, which listens on the window.
    for (const type of ["keydown", "keyup"]) dialog.addEventListener(type, (event) => event.stopPropagation());
    dialog.addEventListener("cancel", (event) => event.preventDefault());
    dialog.showModal();
    return new Promise((resolve) => {
      dialog.querySelector("form")!.addEventListener("submit", (event) => {
        event.preventDefault();
        const name = input.value.replace(CONTROL_CHARS, "").trim();
        if (name.length === 0) return;
        dialog.close();
        resolve(name);
      });
    });
  }

  /** Shows the panel, filled in from the last room message, or hides it during this page's round. */
  setVisible(isVisible: boolean): void {
    if (this.panel.hidden === !isVisible) return;
    this.panel.hidden = !isVisible;
  }

  /** Follows the room: its players and host, and whether this page now picks the cake. */
  showRoom(room: RoomMessage, myId: number): void {
    const wasPicking = this.isPicking();
    this.room = room;
    this.myId = myId;
    const isPicking = this.isPicking();
    if (room.phase === "lobby") this.standingsCard.hidden = true;
    this.players.replaceChildren(...room.members.map((member) => this.buildPlayer(member, room.host)));
    if (isPicking) this.ensureTiles();
    this.pick.hidden = !isPicking;
    // After unhiding, since a hidden picker keeps the scroll it had.
    if (isPicking && !wasPicking) {
      this.selectCake(RANDOM_CAKE);
      this.tiles.scrollTop = 0;
    }
    this.startButton.disabled = false;
    this.syncStatus();
  }

  /** The round's standings, while the room shows its results. */
  showStandings(standings: Standing[]): void {
    if (this.room?.phase !== "results") return;
    this.standingsList.replaceChildren(...standings.map(buildStanding));
    this.standingsCard.hidden = false;
  }

  showDisconnected(): void {
    this.isDisconnected = true;
    this.pick.hidden = true;
    this.syncStatus();
  }

  private isHost(): boolean {
    return this.room?.host === this.myId;
  }

  private isPicking(): boolean {
    return this.isHost() && this.room?.phase === "lobby";
  }

  private syncStatus(): void {
    this.status.textContent = this.buildStatus();
    this.status.hidden = this.status.textContent === "";
  }

  private buildStatus(): string {
    if (this.isDisconnected) return "Lost the connection to the server. Reload the page to rejoin.";
    if (!this.room) return "Joining...";
    if (this.room.phase === "results") return "Back to the lobby in a moment.";
    if (this.room.phase !== "lobby") return "A round is on. You play the next one.";
    if (this.isHost()) return "";
    const host = this.room.members.find((member) => member.id === this.room!.host);
    return host ? `${host.name} is picking the cake.` : "";
  }

  private buildPlayer(member: Member, hostId: number | null): HTMLLIElement {
    const item = document.createElement("li");
    item.textContent = member.id === this.myId ? `${member.name} (you)` : member.name;
    if (member.id === hostId) item.append(buildSpan("badge", "host"));
    return item;
  }

  /** Builds the picker's tiles the first time this page picks, so a page that never hosts never downloads a preview. */
  private ensureTiles(): void {
    if (this.tiles.childElementCount > 0) return;
    const random = this.buildTile(RANDOM_CAKE, "Random", buildSpan("cake-tile-mark", "?"));
    const groups = CAKE_KINDS.map((kind) => this.buildGroup(kind)).filter((group) => group !== null);
    this.tiles.append(buildGroupElement(null, [random]), ...groups);
  }

  private buildGroup(kind: CakeKind): HTMLElement | null {
    const cakes = CAKES.filter((cake) => cake.kind === kind).sort((a, b) => a.name.localeCompare(b.name));
    if (cakes.length === 0) return null;
    return buildGroupElement(kind, cakes.map((cake) => this.buildTile(cake.id, cake.name, buildPreview(cake))));
  }

  private buildTile(id: string, name: string, art: HTMLElement): HTMLButtonElement {
    const tile = document.createElement("button");
    tile.className = "cake-tile";
    tile.dataset.cake = id;
    tile.append(art, buildSpan("cake-tile-name", name));
    tile.addEventListener("click", () => this.selectCake(id));
    return tile;
  }

  private selectCake(id: string): void {
    this.pickedCake = id;
    for (const tile of this.tiles.querySelectorAll<HTMLButtonElement>(".cake-tile")) tile.ariaPressed = String(tile.dataset.cake === id);
  }
}

function buildPreview(cake: CakeEntry): HTMLImageElement {
  const image = document.createElement("img");
  image.src = `${import.meta.env.BASE_URL}previews/${cake.id}.webp`;
  image.alt = "";
  image.loading = "lazy";
  return image;
}

function buildGroupElement(kind: CakeKind | null, tiles: HTMLElement[]): HTMLElement {
  const group = document.createElement("div");
  group.className = "cake-group";
  if (kind) group.append(Object.assign(document.createElement("h3"), { textContent: kind }));
  const grid = document.createElement("div");
  grid.className = "cake-grid";
  grid.append(...tiles);
  group.append(grid);
  return group;
}

function buildStanding(standing: Standing): HTMLLIElement {
  const item = document.createElement("li");
  item.append(
    buildSpan("standing-rank", `${standing.rank}.`),
    buildSpan("standing-name", standing.name),
    buildSpan("standing-score", `${standing.score.toFixed(1)}%`),
  );
  return item;
}

function buildSpan(className: string, text: string): HTMLSpanElement {
  return Object.assign(document.createElement("span"), { className, textContent: text });
}
