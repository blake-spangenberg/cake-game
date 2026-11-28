/** The sharpest drawing resolution, in drawn pixels per CSS pixel, however dense the display. */
const MAX_PIXEL_RATIO = 2;
/** The coarsest, one drawn pixel per CSS pixel, as the game drew before it adapted. */
const MIN_PIXEL_RATIO = 1;
const PIXEL_RATIO_STEP = 0.25;
/**
 * Frames are judged together by their mean interval, this many or as many as a second brings,
 * about half a second at the cap. A mean, since a display faster than the cap draws at alternating
 * intervals around it, at 144 Hz 14 and 21 ms.
 */
const WINDOW_FRAMES = 30;
const WINDOW_MS = 1000;
/** The fewest frames a second-long window is judged on. */
const MIN_WINDOW_FRAMES = 3;
/**
 * Each interval counts for at most this many times the frame cap's, so a hitch such as a cut adds
 * about a millisecond to the mean, under `SLOW_SHARE`'s slack, while frames slow throughout still count.
 */
const MAX_COUNTED_SHARE = 3;
/**
 * The hysteresis band: a mean interval over this many times the frame cap's runs slow, at most
 * `KEEPING_UP_SHARE` times it keeps up, and between the two neither, which breaks either run.
 */
const SLOW_SHARE = 1.2;
const KEEPING_UP_SHARE = 1.05;
/** An interval this long is a hidden tab, not the frame rate, and starts the window over. */
const GAP_MS = 1000;
/** How long frames must run slow before the resolution steps down, so a brief slow patch such as a round swap costs no sharpness. */
const LOWER_AFTER_MS = 1000;
/** How long frames must keep up before the resolution steps up. */
const RAISE_AFTER_MS = 5000;
/** How long a resolution that ran slow is not tried again, doubling each time it runs slow again. */
const RETRY_SLOW_AFTER_MS = 30_000;

/** Whether the windows since `sinceMs` all ran slow or all kept up, with nothing else between. */
type Run = { kind: "slow" | "keeping-up"; sinceMs: number };

/**
 * Picks the play view's drawing resolution from how fast frames come: the display's density up to
 * `MAX_PIXEL_RATIO`, a step coarser once frames have run slower than the cap for `LOWER_AFTER_MS`,
 * and a step sharper once they have kept up for `RAISE_AFTER_MS`, never retrying a resolution soon
 * after it ran slow. The opponents' tiles keep one pixel per CSS pixel, since their snapshots and
 * viewports count CSS pixels.
 */
export class AdaptiveResolution {
  private readonly maxRatio = Math.max(MIN_PIXEL_RATIO, Math.min(MAX_PIXEL_RATIO, devicePixelRatio));
  private ratio = this.maxRatio;
  private readonly intervalsMs: number[] = [];
  private windowStartMs = 0;
  private run: Run | null = null;
  /** The coarsest resolution that ran slow lately, until when it is not tried again, and for how long it was held off last. */
  private slow: { ratio: number; untilMs: number; heldOffMs: number } | null = null;

  constructor(private readonly frameMs: number) {}

  getPixelRatio(): number {
    return this.ratio;
  }

  /** Counts a frame drawn at `nowMs`, `intervalMs` after the one before it, returning the new resolution where it changes. */
  record(nowMs: number, intervalMs: number): number | null {
    if (intervalMs > GAP_MS) {
      this.intervalsMs.length = 0;
      this.run = null;
      return null;
    }
    if (this.intervalsMs.length === 0) this.windowStartMs = nowMs - intervalMs;
    this.intervalsMs.push(Math.min(intervalMs, this.frameMs * MAX_COUNTED_SHARE));
    const isWindowFull = this.intervalsMs.length >= WINDOW_FRAMES || (nowMs - this.windowStartMs >= WINDOW_MS && this.intervalsMs.length >= MIN_WINDOW_FRAMES);
    if (!isWindowFull) return null;
    const meanMs = this.intervalsMs.reduce((sum, ms) => sum + ms, 0) / this.intervalsMs.length;
    this.intervalsMs.length = 0;
    const kind = meanMs > this.frameMs * SLOW_SHARE ? "slow" : meanMs <= this.frameMs * KEEPING_UP_SHARE ? "keeping-up" : null;
    if (kind === null) {
      this.run = null;
      return null;
    }
    if (this.run?.kind !== kind) this.run = { kind, sinceMs: this.windowStartMs };
    const runMs = nowMs - this.run.sinceMs;
    if (kind === "slow") return runMs >= LOWER_AFTER_MS ? this.lower(nowMs) : null;
    return runMs >= RAISE_AFTER_MS ? this.tryRaise(nowMs) : null;
  }

  private lower(nowMs: number): number | null {
    if (this.ratio <= MIN_PIXEL_RATIO) return null;
    const heldOffMs = this.slow?.ratio === this.ratio ? 2 * this.slow.heldOffMs : RETRY_SLOW_AFTER_MS;
    this.slow = { ratio: this.ratio, untilMs: nowMs + heldOffMs, heldOffMs };
    return this.step(this.ratio - PIXEL_RATIO_STEP);
  }

  private tryRaise(nowMs: number): number | null {
    if (this.ratio >= this.maxRatio) return null;
    const next = Math.min(this.maxRatio, this.ratio + PIXEL_RATIO_STEP);
    if (this.slow && next >= this.slow.ratio && nowMs < this.slow.untilMs) return null;
    return this.step(next);
  }

  /** Changes to `ratio`, after which the next step needs a whole run of its own. */
  private step(ratio: number): number {
    this.run = null;
    this.ratio = Math.max(MIN_PIXEL_RATIO, ratio);
    return this.ratio;
  }
}
