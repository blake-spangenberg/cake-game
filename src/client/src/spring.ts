/**
 * A critically damped spring easing `value` toward `target` without overshoot. `halfLifeS` is
 * roughly the time to cover half the remaining distance.
 */
export class Spring {
  target: number;
  private velocity = 0;

  constructor(
    public value: number,
    public halfLifeS: number,
  ) {
    this.target = value;
  }

  step(dtS: number): number {
    const damping = (2 * Math.LN2) / this.halfLifeS;
    const offset = this.value - this.target;
    const drift = this.velocity + offset * damping;
    const decay = Math.exp(-damping * dtS);
    this.value = decay * (offset + drift * dtS) + this.target;
    this.velocity = decay * (this.velocity - drift * damping * dtS);
    return this.value;
  }

  /** Holds both value and target at or below `max`, as at a hard stop. */
  capAt(max: number): void {
    if (this.value > max) {
      this.value = max;
      this.velocity = Math.min(this.velocity, 0);
    }
    this.target = Math.min(this.target, max);
  }

  snap(value: number): void {
    this.value = value;
    this.target = value;
    this.velocity = 0;
  }
}
