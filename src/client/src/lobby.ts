import { CAKES, CAKE_KINDS, type CakeEntry } from "./cakes";
import { MAX_NAME_LEN, type Member, type PickMessage, type Profile, RANDOM_CAKE, type ResultsMessage, type RoomMessage } from "./net";
import { DEFAULT_STANDUP_PREFERENCE, isActive, loadProfile, saveProfile } from "./profile";
import { buildRules } from "./rules";
import { buildMedal, buildSpan, buildStandingRow, findShownPlace } from "./standings";

/** Unicode control characters, which the server refuses in a name. */
const CONTROL_CHARS = /\p{Cc}/gu;
/** Every cake the picker offers, Random first, then grouped by kind, each kind's by name, with no group titles. */
const PICKER_CAKES: readonly CakeEntry[] = CAKE_KINDS.flatMap((kind) => CAKES.filter((cake) => cake.kind === kind).sort((a, b) => a.name.localeCompare(b.name)));
/** What each fatal error says, full screen, until the page reloads or asks for the name again. */
const FATAL_TEXTS = {
  lost: "Lost the connection to the server. Reload the page to rejoin.",
  refused: "The server refused to seat you. Reload the page to try again.",
  failed: "The game failed to load. Reload the page to try again.",
} as const;
export type FatalError = keyof typeof FATAL_TEXTS;

/**
 * What fills the page: "Loading..." alone while it has nothing else to show, the lobby or its
 * results, the rules beside a round this page spectates, or the round.
 */
export type Screen = "loading" | "lobby" | "rules" | "round";

const loadingScreen = document.querySelector<HTMLElement>("#loading")!;
const fatalScreen = document.querySelector<HTMLElement>("#fatal")!;

/** Shows "Loading..." over the whole page, or takes it away. */
export function setLoadingShown(isShown: boolean): void {
  // Called every frame, so it touches the page only on a change.
  if (loadingScreen.hidden !== !isShown) loadingScreen.hidden = !isShown;
}

/** Shows `error` over the whole page, over "Loading..." and everything else, until `hideFatal`. */
export function showFatal(error: FatalError): void {
  fatalScreen.textContent = FATAL_TEXTS[error];
  fatalScreen.hidden = false;
}

function hideFatal(): void {
  fatalScreen.hidden = true;
}

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
 * Until it has the room to show, the page shows "Loading...", and a lost connection shows full screen.
 */
export class Lobby {
  private readonly panel = document.querySelector<HTMLElement>("#lobby")!;
  private readonly lobbyScreen = document.querySelector<HTMLElement>("#lobby-screen")!;
  private readonly tally = document.querySelector<HTMLElement>("#tally")!;
  private readonly players = document.querySelector<HTMLUListElement>("#players")!;
  private readonly leaderboard = document.querySelector<HTMLOListElement>("#leaderboard")!;
  private readonly standupOrder = document.querySelector<HTMLOListElement>("#standup-order")!;
  private readonly pickButton = document.querySelector<HTMLButtonElement>("#pick-cake")!;
  private readonly startButton = document.querySelector<HTMLButtonElement>("#start")!;
  private readonly returnButton = document.querySelector<HTMLButtonElement>("#return-to-lobby")!;
  private readonly claimButton = document.querySelector<HTMLButtonElement>("#claim-host")!;
  private readonly picker = document.querySelector<HTMLDialogElement>("#picker")!;
  private readonly tiles = document.querySelector<HTMLDivElement>("#cake-tiles")!;
  private readonly claim = document.querySelector<HTMLDialogElement>("#claim")!;
  private readonly password = document.querySelector<HTMLInputElement>("#password")!;
  private readonly claimError = document.querySelector<HTMLParagraphElement>("#claim-error")!;
  private readonly joinDialog = document.querySelector<HTMLDialogElement>("#join")!;
  private readonly nameInput = document.querySelector<HTMLInputElement>("#name")!;
  private readonly standupSlider = document.querySelector<HTMLInputElement>("#standup")!;
  private readonly scrumUpdateBox = document.querySelector<HTMLInputElement>("#has-scrum-update")!;
  private readonly standupPreference = document.querySelector<HTMLFieldSetElement>("#standup-preference")!;
  private readonly nameTaken = document.querySelector<HTMLParagraphElement>("#name-taken")!;
  private room: RoomMessage | null = null;
  private pick: PickMessage | null = null;
  private hasResults = false;
  private myId = 0;

  constructor() {
    document.querySelector("#how-to-play")!.append(buildRules());
    this.claimButton.addEventListener("click", () => this.openClaim());
    document.querySelector("#claim-cancel")!.addEventListener("click", () => this.claim.close());
    this.joinDialog.addEventListener("cancel", (event) => event.preventDefault());
    for (const dialog of [this.claim, this.joinDialog]) stopKeys(dialog);
    this.nameInput.maxLength = MAX_NAME_LEN;
    this.nameInput.addEventListener("input", () => (this.nameTaken.hidden = true));
    this.scrumUpdateBox.addEventListener("change", () => this.syncStandupPreference());
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
   * The profile to join with: the one remembered on this browser if the player has been active on
   * the site within the hour, or else whatever they enter, filled in as last set.
   */
  waitProfile(): Promise<Profile> {
    const remembered = loadProfile();
    if (remembered && isActive()) return Promise.resolve(remembered);
    return this.askProfile();
  }

  /**
   * Asks for the name, scrum update, and standup preference, filled in as last set, until the
   * player gives a name, telling them beside the name box if `isNameTaken` by someone in the room.
   */
  askProfile(isNameTaken = false): Promise<Profile> {
    const remembered = loadProfile();
    this.nameInput.value = remembered?.name ?? "";
    this.standupSlider.valueAsNumber = remembered?.standupPreference ?? DEFAULT_STANDUP_PREFERENCE;
    this.scrumUpdateBox.checked = remembered?.hasScrumUpdate ?? true;
    this.syncStandupPreference();
    this.nameTaken.hidden = !isNameTaken;
    // The page joins anew once the player answers, even after a lost connection.
    hideFatal();
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
        const profile = { name, standupPreference: this.standupSlider.valueAsNumber, hasScrumUpdate: this.scrumUpdateBox.checked };
        saveProfile(profile);
        form.removeEventListener("submit", onSubmit);
        this.joinDialog.close();
        resolve(profile);
      };
      form.addEventListener("submit", onSubmit);
    });
  }

  /** Shows `screen`: the whole lobby, shrunk to the rules beside the round, or neither. */
  setScreen(screen: Screen): void {
    setLoadingShown(screen === "loading");
    const isHidden = screen !== "lobby" && screen !== "rules";
    const isRules = screen === "rules";
    // Called every frame, so it touches the page only on a change.
    if (this.panel.hidden !== isHidden) this.panel.hidden = isHidden;
    if (this.panel.classList.contains("is-rules") !== isRules) this.panel.classList.toggle("is-rules", isRules);
  }

  /** Whether the lobby has the room's lobby or its round's results to show, rather than waiting on the room or a round. */
  hasRoomToShow(): boolean {
    return this.room?.phase === "lobby" || this.isShowingResults();
  }

  /** Follows the room: its players and host, and what this page may do in it. */
  showRoom(room: RoomMessage, myId: number): void {
    this.room = room;
    this.myId = myId;
    if (room.phase === "lobby") this.hasResults = false;
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
    this.leaderboard.replaceChildren(...results.standings.map((standing) => buildStandingRow(standing, standing.rank)));
    // A standup slot carries its player's leaderboard medal, and a spectator's none.
    const places = new Map(results.standings.map((standing) => [standing.id, findShownPlace(standing, standing.rank)]));
    this.standupOrder.replaceChildren(
      ...results.standup.map((member) => {
        const item = this.buildName(member);
        item.prepend(buildMedal(places.get(member.id) ?? null));
        return item;
      }),
    );
    this.hasResults = true;
    this.syncControls();
  }

  /** Shows `error`, the reason the connection ended, until the page asks for the name again. */
  showDisconnected(error: FatalError): void {
    this.room = null;
    this.picker.close();
    this.claim.close();
    this.syncControls();
    showFatal(error);
  }

  /** Grays out and disables the standup preference for a player without a scrum update. */
  private syncStandupPreference(): void {
    this.standupPreference.disabled = !this.scrumUpdateBox.checked;
  }

  private isHost(): boolean {
    return this.room !== null && this.room.host === this.myId;
  }

  private isShowingResults(): boolean {
    return this.room?.phase === "results" && this.hasResults;
  }

  /** Shows the buttons this page may press in the room's phase. */
  private syncControls(): void {
    const isLobby = this.room?.phase === "lobby";
    const isResults = this.isShowingResults();
    this.lobbyScreen.hidden = isResults;
    this.tally.hidden = !isResults;
    this.pickButton.hidden = !(isLobby && this.isHost());
    this.startButton.hidden = this.pickButton.hidden;
    this.returnButton.hidden = !(isResults && this.isHost());
    this.claimButton.hidden = !(isLobby || isResults) || this.isHost();
    if (this.pickButton.hidden) this.picker.close();
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
