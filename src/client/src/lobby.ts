import { CAKES, CAKE_KINDS, type CakeEntry } from "./cakes";
import { MAX_NAME_LEN, type Member, type PickMessage, type Profile, RANDOM_CAKE, type ResultsMessage, type RoomMessage, type Standing } from "./net";
import { DEFAULT_STANDUP_PREFERENCE, isActive, loadProfile, saveProfile } from "./profile";
import { buildRules } from "./rules";

/** Unicode control characters, which the server refuses in a name. */
const CONTROL_CHARS = /\p{Cc}/gu;
/** Every cake the picker offers, Random first, then grouped by kind, each kind's by name, with no group titles. */
const PICKER_CAKES: readonly CakeEntry[] = CAKE_KINDS.flatMap((kind) => CAKES.filter((cake) => cake.kind === kind).sort((a, b) => a.name.localeCompare(b.name)));

/** What the lobby's buttons ask the server for. */
export interface LobbyActions {
  onClaimHost(password: string): void;
  onPick(cake: string): void;
  onStart(): void;
  onReturnToLobby(): void;
}

/**
 * What an online page shows outside a round, over the whole screen: the name and standup entry
 * before joining, then the players, the rules, the credits link, and for the host the cake
 * picker and Start, or the round's leaderboard and standup order until the host returns everyone
 * to the lobby. Anyone not hosting can claim host with the password, which only the server checks.
 */
export class Lobby {
  private readonly panel = document.querySelector<HTMLElement>("#lobby")!;
  private readonly lobbyScreen = document.querySelector<HTMLElement>("#lobby-screen")!;
  private readonly tally = document.querySelector<HTMLElement>("#tally")!;
  private readonly players = document.querySelector<HTMLUListElement>("#players")!;
  private readonly leaderboard = document.querySelector<HTMLOListElement>("#leaderboard")!;
  private readonly standupOrder = document.querySelector<HTMLOListElement>("#standup-order")!;
  private readonly status = document.querySelector<HTMLParagraphElement>("#lobby-status")!;
  private readonly pickButton = document.querySelector<HTMLButtonElement>("#pick-cake")!;
  private readonly startButton = document.querySelector<HTMLButtonElement>("#start")!;
  private readonly returnButton = document.querySelector<HTMLButtonElement>("#return-to-lobby")!;
  private readonly claimButton = document.querySelector<HTMLButtonElement>("#claim-host")!;
  private readonly creditsLink = document.querySelector<HTMLAnchorElement>("#credits-link")!;
  private readonly picker = document.querySelector<HTMLDialogElement>("#picker")!;
  private readonly tiles = document.querySelector<HTMLDivElement>("#cake-tiles")!;
  private readonly claim = document.querySelector<HTMLDialogElement>("#claim")!;
  private readonly password = document.querySelector<HTMLInputElement>("#password")!;
  private readonly claimError = document.querySelector<HTMLParagraphElement>("#claim-error")!;
  private readonly joinDialog = document.querySelector<HTMLDialogElement>("#join")!;
  private readonly nameInput = document.querySelector<HTMLInputElement>("#name")!;
  private readonly standupSlider = document.querySelector<HTMLInputElement>("#standup")!;
  private room: RoomMessage | null = null;
  private pick: PickMessage | null = null;
  private hasResults = false;
  /** Whether this page sits out the round on for loading its cake too slowly, until the room's next phase. */
  private isSatOut = false;
  private myId = 0;
  private isDisconnected = false;

  constructor() {
    this.panel.hidden = false;
    document.querySelector("#how-to-play")!.append(buildRules());
    this.claimButton.addEventListener("click", () => this.openClaim());
    document.querySelector("#claim-cancel")!.addEventListener("click", () => this.claim.close());
    this.joinDialog.addEventListener("cancel", (event) => event.preventDefault());
    for (const dialog of [this.claim, this.joinDialog]) stopKeys(dialog);
    this.nameInput.maxLength = MAX_NAME_LEN;
  }

  /** Has the lobby's buttons ask the server through `actions`. The buttons show only in a room. */
  registerActions(actions: LobbyActions): void {
    this.pickButton.addEventListener("click", () => this.openPicker());
    this.startButton.addEventListener("click", () => {
      this.startButton.disabled = true;
      actions.onStart();
    });
    this.returnButton.addEventListener("click", () => {
      this.returnButton.disabled = true;
      actions.onReturnToLobby();
    });
    this.tiles.addEventListener("click", (event) => {
      const tile = (event.target as Element).closest<HTMLButtonElement>(".cake-tile");
      if (!tile) return;
      actions.onPick(tile.dataset.cake!);
      this.picker.close();
    });
    this.claim.querySelector("form")!.addEventListener("submit", (event) => {
      event.preventDefault();
      actions.onClaimHost(this.password.value);
    });
  }

  /**
   * The name and standup preference to join with: those remembered on this browser if the player
   * has been active on the site within the hour, or else whatever they enter, filled in as last set.
   */
  waitProfile(): Promise<Profile> {
    const remembered = loadProfile();
    if (remembered && isActive()) return Promise.resolve(remembered);
    return this.askProfile();
  }

  /** Asks for the name and standup preference, filled in as last set, until the player gives a name the server accepts. */
  askProfile(): Promise<Profile> {
    const remembered = loadProfile();
    this.nameInput.value = remembered?.name ?? "";
    this.standupSlider.valueAsNumber = remembered?.standupPreference ?? DEFAULT_STANDUP_PREFERENCE;
    // The page joins anew once the player answers, even after a lost connection.
    this.isDisconnected = false;
    this.room = null;
    this.players.replaceChildren();
    this.claim.close();
    this.syncControls();
    if (!this.joinDialog.open) this.joinDialog.showModal();
    return new Promise((resolve) => {
      const form = this.joinDialog.querySelector("form")!;
      const onSubmit = (event: SubmitEvent) => {
        event.preventDefault();
        const name = this.nameInput.value.replace(CONTROL_CHARS, "").trim();
        if (name.length === 0) return;
        const profile = { name, standupPreference: this.standupSlider.valueAsNumber };
        saveProfile(profile);
        form.removeEventListener("submit", onSubmit);
        this.joinDialog.close();
        resolve(profile);
      };
      form.addEventListener("submit", onSubmit);
    });
  }

  /**
   * Shows the screen while this page is outside a round and the room is too, in the lobby or its
   * results, and shrinks it to the rules beside the round while the room plays one without this page.
   */
  setVisible(isOutsideRound: boolean): void {
    const isRoundOn = this.room?.phase === "loading" || this.room?.phase === "playing";
    const isRules = isOutsideRound && isRoundOn;
    // Called every frame, so it touches the page only on a change.
    if (this.panel.hidden !== !isOutsideRound) this.panel.hidden = !isOutsideRound;
    if (this.panel.classList.contains("is-rules") !== isRules) this.panel.classList.toggle("is-rules", isRules);
  }

  /** Follows the room: its players and host, and what this page may do in it. */
  showRoom(room: RoomMessage, myId: number): void {
    this.room = room;
    this.myId = myId;
    if (room.phase === "lobby") this.hasResults = false;
    if (room.phase === "lobby" || room.phase === "results") this.isSatOut = false;
    if (room.host === myId) this.claim.close();
    this.players.replaceChildren(...room.members.map((member) => this.buildPlayer(member, room.host)));
    this.startButton.disabled = false;
    this.returnButton.disabled = false;
    this.syncControls();
  }

  /** Follows the next round's cake, which the host's button names and the picker marks. */
  showPick(pick: PickMessage): void {
    this.pick = pick;
    const selected = pick.is_random ? RANDOM_CAKE : pick.cake;
    for (const tile of this.tiles.querySelectorAll<HTMLButtonElement>(".cake-tile")) tile.ariaPressed = String(tile.dataset.cake === selected);
    this.pickButton.textContent = `Cake: ${pick.is_random ? "Random" : CAKES.find((cake) => cake.id === pick.cake)!.name}`;
  }

  showClaimRefused(): void {
    this.claimError.hidden = false;
    this.password.select();
  }

  /** The round's leaderboard and standup order, filling the screen until the host returns everyone to the lobby. */
  showResults(results: ResultsMessage): void {
    this.leaderboard.replaceChildren(...results.standings.map(buildStanding));
    this.standupOrder.replaceChildren(...results.standup.map((member) => this.buildName(member)));
    this.hasResults = true;
    this.syncControls();
  }

  /** Tells the player they watch the round on, since their cake took too long to load. */
  showSatOut(): void {
    this.isSatOut = true;
    this.syncControls();
  }

  showDisconnected(): void {
    this.isDisconnected = true;
    this.room = null;
    this.picker.close();
    this.claim.close();
    this.syncControls();
  }

  private isHost(): boolean {
    return this.room !== null && this.room.host === this.myId;
  }

  /** Shows the buttons this page may press in the room's phase, and the status line. */
  private syncControls(): void {
    const phase = this.isDisconnected ? null : this.room?.phase;
    const isResults = phase === "results" && this.hasResults;
    this.lobbyScreen.hidden = isResults;
    this.tally.hidden = !isResults;
    this.pickButton.hidden = !(phase === "lobby" && this.isHost());
    this.startButton.hidden = this.pickButton.hidden;
    this.returnButton.hidden = !(isResults && this.isHost());
    this.claimButton.hidden = !(phase === "lobby" || isResults) || this.isHost();
    this.creditsLink.hidden = isResults;
    if (this.pickButton.hidden) this.picker.close();
    this.status.textContent = this.buildStatus(isResults);
  }

  private buildStatus(isResults: boolean): string {
    if (this.isDisconnected) return "Lost the connection to the server. Reload the page to rejoin.";
    if (!this.room) return "Joining...";
    if (this.room.phase !== "lobby" && !isResults) {
      return this.isSatOut ? "The cake took over 10 seconds to load, so you watch this round. You play the next one." : "A round is on. You play the next one.";
    }
    if (this.isHost()) return isResults ? "" : "Pick the cake, then press Start.";
    const host = this.room.members.find((member) => member.id === this.room!.host);
    if (!host) return isResults ? "No one is host. Claim host to return everyone to the lobby." : "No one is host yet. Claim host to pick the cake.";
    return isResults ? `Waiting for ${host.name} to return everyone to the lobby.` : `${host.name} is picking the cake.`;
  }

  private buildPlayer(member: Member, hostId: number | null): HTMLLIElement {
    const item = this.buildName(member);
    if (member.id === hostId) item.append(buildSpan("badge", "host"));
    return item;
  }

  private buildName(member: Member): HTMLLIElement {
    const item = document.createElement("li");
    item.append(buildSpan("member-name", member.name));
    if (member.id === this.myId) item.append(buildSpan("member-you", "(you)"));
    return item;
  }

  /** Opens the picker, building its tiles the first time, so a page that never hosts never downloads a preview. */
  private openPicker(): void {
    if (this.tiles.childElementCount === 0) {
      this.tiles.append(buildTile(RANDOM_CAKE, "Random", buildSpan("cake-tile-mark", "?")), ...PICKER_CAKES.map((cake) => buildTile(cake.id, cake.name, buildPreview(cake))));
      if (this.pick) this.showPick(this.pick);
    }
    this.picker.showModal();
  }

  private openClaim(): void {
    this.password.value = "";
    this.claimError.hidden = true;
    this.claim.showModal();
  }
}

/** Keeps typing in `dialog` from reaching the camera's arrow-key and WASD swing, which listens on the window. */
function stopKeys(dialog: HTMLDialogElement): void {
  for (const type of ["keydown", "keyup"]) dialog.addEventListener(type, (event) => event.stopPropagation());
}

function buildTile(id: string, name: string, art: HTMLElement): HTMLButtonElement {
  const tile = document.createElement("button");
  tile.className = "cake-tile";
  tile.dataset.cake = id;
  tile.append(art, buildSpan("cake-tile-name", name));
  return tile;
}

function buildPreview(cake: CakeEntry): HTMLImageElement {
  const image = document.createElement("img");
  image.src = `${import.meta.env.BASE_URL}previews/${cake.id}.webp`;
  image.alt = "";
  image.decoding = "async";
  return image;
}

function buildStanding(standing: Standing): HTMLLIElement {
  const item = document.createElement("li");
  item.append(buildSpan("standing-rank", `${standing.rank}.`), buildSpan("member-name", standing.name), buildSpan("standing-score", `${standing.score.toFixed(1)}%`));
  return item;
}

function buildSpan(className: string, text: string): HTMLSpanElement {
  return Object.assign(document.createElement("span"), { className, textContent: text });
}
