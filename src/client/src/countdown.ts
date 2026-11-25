import { playTick } from "./sound";

/** The red numeric timer shows only once this many seconds or fewer are left. */
const URGENT_S = 10;

/**
 * The round's countdown: a pie that loses area as the round's ticks run out, its edge sweeping
 * clockwise from 12 o'clock, and a red count of whole seconds over it in the last `URGENT_S`,
 * which ticks audibly as it counts down.
 */
export class Countdown {
  private readonly pie: HTMLElement;
  private readonly seconds: HTMLElement;
  private lastSeconds: number | null = null;

  constructor(
    root: HTMLElement,
    private readonly ticksPerS: number,
  ) {
    this.pie = root.querySelector<HTMLElement>(".clock-pie")!;
    this.seconds = root.querySelector<HTMLElement>(".clock-seconds")!;
  }

  /** Shows `ticksLeft` of a round `roundTicks` long. Cheap to call every frame. */
  show(ticksLeft: number, roundTicks: number): void {
    const shareLeft = Math.min(Math.max(ticksLeft / roundTicks, 0), 1);
    this.pie.style.setProperty("--share-left", shareLeft.toFixed(4));
    const seconds = Math.max(Math.ceil(ticksLeft / this.ticksPerS), 0);
    if (seconds === this.lastSeconds) return;
    const isUrgent = seconds <= URGENT_S;
    // 0 s is the whistle's, which `RoundWhistle` blows.
    if (isUrgent && seconds > 0 && this.lastSeconds !== null && seconds < this.lastSeconds) playTick();
    this.lastSeconds = seconds;
    this.seconds.hidden = !isUrgent;
    this.seconds.textContent = `${seconds}`;
  }
}
