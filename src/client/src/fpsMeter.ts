/** The span the readout counts frames over and finds the longest in. */
const WINDOW_MS = 3000;
/** How often the readout's text changes, slow enough to read. */
const REFRESH_MS = 250;

/**
 * A corner readout of the frames drawn per second and the longest interval between two drawn
 * frames over the past `WINDOW_MS`, both rounded to whole numbers. Made for `?debug` pages only.
 */
export class FpsMeter {
  private readonly readout = document.createElement("div");
  /** Each drawn frame's start and the interval since the frame before it, oldest first, within the window. */
  private readonly frames: { startMs: number; intervalMs: number }[] = [];
  private nextRefreshMs = 0;

  constructor() {
    this.readout.id = "fps";
    document.body.append(this.readout);
  }

  /** Counts a frame drawn at `startMs`, `intervalMs` after the one before it. */
  record(startMs: number, intervalMs: number): void {
    this.frames.push({ startMs, intervalMs });
    while (this.frames[0].startMs <= startMs - WINDOW_MS) this.frames.shift();
    if (startMs < this.nextRefreshMs) return;
    this.nextRefreshMs = startMs + REFRESH_MS;
    this.readout.textContent = this.formatReadout();
  }

  private formatReadout(): string {
    const intervalsMs = this.frames.map((frame) => frame.intervalMs);
    const spanMs = intervalsMs.reduce((sum, intervalMs) => sum + intervalMs, 0);
    const fps = Math.round((intervalsMs.length * 1000) / spanMs);
    return `${fps} fps  peak ${Math.round(Math.max(...intervalsMs))} ms`;
  }
}
