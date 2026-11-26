/**
 * What a page opened with `?debug` adds, for developing and testing the game: the lobby's debug
 * controls, which for the host alone add and remove the room's bots and set its rounds' length
 * and its bots' latency, and for every page set its own latency, and a timer counting up from
 * Start until the results show. The fps readout is `fpsMeter.ts`.
 */

/** Whether this page was opened with `?debug`. */
export const IS_DEBUG_PAGE = new URLSearchParams(location.search).has("debug");

/** The room's debug settings, as the server sends them to `?debug` pages. */
export interface DebugSettings {
  round_s: number;
  bots: number;
  bot_latency_ms: number;
}

/** What the debug controls ask for. */
export interface DebugActions {
  onAddBot(): void;
  onRemoveBot(): void;
  onSetRoundLength(roundS: number): void;
  onSetBotLatency(latencyMs: number): void;
  onSetOwnLatency(latencyMs: number): void;
}

/** How often the timer's text changes, fine enough to read tenths. */
const TIMER_REFRESH_MS = 100;

/** The lobby's debug controls, in one section beside the players and the rules. */
export class DebugPanel {
  private readonly section = document.createElement("section");
  /** The room-wide controls, which the server accepts only from the host. */
  private readonly hostControls = document.createElement("div");
  private readonly botCount = document.createElement("span");
  private readonly roundS = buildNumberInput(1, 600);
  private readonly botLatencyMs = buildNumberInput(0, 10_000);
  private readonly ownLatencyMs = buildNumberInput(0, 10_000);

  constructor(actions: DebugActions) {
    this.section.id = "debug-panel";
    const addBot = buildButton("Add bot", () => actions.onAddBot());
    const removeBot = buildButton("Remove bot", () => actions.onRemoveBot());
    addBot.id = "add-bot";
    removeBot.id = "remove-bot";
    this.botCount.id = "bot-count";
    this.roundS.id = "round-s";
    this.botLatencyMs.id = "bot-latency-ms";
    this.ownLatencyMs.id = "own-latency-ms";
    this.ownLatencyMs.value = "0";
    onNumberChange(this.roundS, (value) => actions.onSetRoundLength(value));
    onNumberChange(this.botLatencyMs, (value) => actions.onSetBotLatency(value));
    onNumberChange(this.ownLatencyMs, (value) => actions.onSetOwnLatency(value));
    const bots = document.createElement("p");
    bots.append("Bots: ", this.botCount, " ", addBot, " ", removeBot);
    this.hostControls.className = "host-controls";
    this.hostControls.hidden = true;
    this.hostControls.append(bots, buildLabeled("Round length, s", this.roundS), buildLabeled("Bot latency, ms each way", this.botLatencyMs));
    this.section.append(
      Object.assign(document.createElement("h2"), { textContent: "Debug" }),
      this.hostControls,
      buildLabeled("My latency, ms each way", this.ownLatencyMs),
    );
    // Typing a number must not swing the camera, whose arrow keys and WASD listen on the window.
    for (const type of ["keydown", "keyup"]) this.section.addEventListener(type, (event) => event.stopPropagation());
    document.querySelector("#lobby-screen")!.append(this.section);
  }

  /** Shows the room-wide controls only while this page is the room's host. */
  showHost(isHost: boolean): void {
    this.hostControls.hidden = !isHost;
  }

  /** Shows the room's settings, unless the player is typing a new value into that box. */
  showSettings(settings: DebugSettings): void {
    this.botCount.textContent = String(settings.bots);
    for (const [input, value] of [
      [this.roundS, settings.round_s],
      [this.botLatencyMs, settings.bot_latency_ms],
    ] as const) {
      if (document.activeElement !== input) input.value = String(value);
    }
  }
}

/** A readout over the bottom left corner counting up from a round's Start until its results show. */
export class RoundTimer {
  private readonly readout = document.createElement("div");
  private startMs: number | null = null;
  private stopMs: number | null = null;
  private nextRefreshMs = 0;

  constructor() {
    this.readout.id = "round-timer";
    this.readout.hidden = true;
    document.body.append(this.readout);
  }

  /** Starts counting at `nowMs`, unless the round already started the count, as the host's Start click does before its round message. */
  start(nowMs: number): void {
    if (this.startMs !== null && this.stopMs === null) return;
    this.startMs = nowMs;
    this.stopMs = null;
    this.readout.hidden = false;
  }

  /** Stops the count at `nowMs`, leaving the time showing. */
  stop(nowMs: number): void {
    if (this.startMs === null || this.stopMs !== null) return;
    this.stopMs = nowMs;
    this.show(nowMs);
  }

  /** Updates the shown time, cheap to call every frame. */
  update(nowMs: number): void {
    if (this.startMs === null || this.stopMs !== null || nowMs < this.nextRefreshMs) return;
    this.nextRefreshMs = nowMs + TIMER_REFRESH_MS;
    this.show(nowMs);
  }

  private show(nowMs: number): void {
    const endMs = this.stopMs ?? nowMs;
    this.readout.textContent = `round ${((endMs - this.startMs!) / 1000).toFixed(1)} s`;
  }
}

function buildNumberInput(min: number, max: number): HTMLInputElement {
  return Object.assign(document.createElement("input"), { type: "number", min: String(min), max: String(max), step: "1" });
}

function buildButton(text: string, onClick: () => void): HTMLButtonElement {
  const button = Object.assign(document.createElement("button"), { type: "button", textContent: text });
  button.addEventListener("click", onClick);
  return button;
}

function buildLabeled(text: string, input: HTMLInputElement): HTMLLabelElement {
  const label = document.createElement("label");
  label.append(text, " ", input);
  return label;
}

/** Calls `onChange` with `input`'s whole number once the player commits one within its bounds. */
function onNumberChange(input: HTMLInputElement, onChange: (value: number) => void): void {
  input.addEventListener("change", () => {
    const value = input.valueAsNumber;
    if (!Number.isInteger(value) || value < Number(input.min) || value > Number(input.max)) return;
    onChange(value);
  });
}
