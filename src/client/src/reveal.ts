/**
 * The timing of a cut's reveal, shared by the online judging and the offline practice's own cut:
 * the halves together, then each sliding apart in place while a gram counter over it counts up
 * from 0, each half stopping as its counter does, then the score the instant the larger half's
 * counter stops. A cut that missed the cake skips the count.
 */

/**
 * One cut's reveal, in seconds from its start: the counters wait at 0 for `holdS` while the halves
 * start sliding apart, then count for `countS`, ending with the score. Online, the server sets both
 * for every page, as `src/server`'s `calc_count_ms` does; a miss counts for none.
 */
export interface RevealScene {
  holdS: number;
  countS: number;
}

/** The offline practice's own cut, revealed once the knife has settled in it: a shorter count than the judged one's 3 s at most. */
const OWN_HOLD_S = 0.3;
const OWN_MAX_COUNT_S = 2.4;
/** The count's ease, in grams (see `Wheel`): its last gram takes about `countS / (1 + EASE_G)`, 0.4 s of a perfect cut's 3 s count. */
const EASE_G = 6.5;
/** The time constant of a slide's spring-like start: it is at 63% of its top speed this long after the reveal starts. */
const SLIDE_RAMP_S = 0.12;
/** How much of its speed a slide sheds by its stop, steadily, from a peak near its start. */
const SLIDE_BRAKE = 0.75;
/** Absorbs the float error of a count landing exactly on a whole gram, far under a gram. */
const WHOLE_GRAM_SLACK_G = 1e-9;

/**
 * A reveal at one moment: how far each half has slid apart, from 0 to 1, each counter's grams,
 * both in the order of the halves' grams given (none for a miss), and whether the counting has ended.
 */
export interface RevealFrame {
  partShares: number[];
  shownG: number[];
  isCounted: boolean;
}

/**
 * The count as one wheel slowing smoothly to a stop at `endG` after `countS`, every gram taking
 * longer than the one before. With `gLeft` grams to go, it stops in `limitS * gLeft / (gLeft + EASE_G)`
 * seconds.
 */
interface Wheel {
  /** The larger half's grams as its counter shows them, where the count stops. */
  endG: number;
  countS: number;
  limitS: number;
}

/**
 * The offline practice's reveal of halves of `halvesG` grams, or of a miss, `[]`, as the server
 * paces a judged one: its count is longest for a perfect cut and shorter by the square of the
 * score's share of a perfect one.
 */
export function buildOwnScene(halvesG: readonly number[]): RevealScene {
  if (halvesG.length === 0) return { holdS: OWN_HOLD_S, countS: 0 };
  const totalG = halvesG.reduce((sum, g) => sum + g, 0);
  const scoreShare = (halvesG.length * Math.min(...halvesG)) / totalG;
  return { holdS: OWN_HOLD_S, countS: OWN_MAX_COUNT_S * scoreShare * scoreShare };
}

/**
 * `scene` `elapsedS` in for halves of `halvesG` grams, or for a cut that missed, `[]`. Every
 * counter shows the same count until it reaches its own half's weight, so a perfect cut stops both
 * at once and a bad one counts the larger half on alone. Each half slides from the reveal's start
 * until its own counter stops, braking into the stop.
 */
export function calcRevealFrame(elapsedS: number, scene: RevealScene, halvesG: readonly number[]): RevealFrame {
  if (halvesG.length === 0) return { partShares: [], shownG: [], isCounted: elapsedS >= calcCountedS(scene) };
  const wheel = buildWheel(scene, halvesG);
  const countG = Math.floor(calcCountG(wheel, elapsedS - scene.holdS) + WHOLE_GRAM_SLACK_G);
  return {
    partShares: halvesG.map((g) => calcSlideShare(elapsedS, calcReachedS(scene, wheel, Math.round(g)))),
    shownG: halvesG.map((g) => Math.min(countG, Math.round(g))),
    isCounted: elapsedS >= calcCountedS(scene),
  };
}

/** When `scene`'s counting ends with its last gram and the score shows, in seconds from its start. */
export function calcCountedS(scene: RevealScene): number {
  return scene.holdS + scene.countS;
}

/**
 * When the counters of halves of `halvesG` tick in `scene`, in seconds from its start, none for a
 * miss (`[]`): on each gram from the last, which ends the count with the score, back to where
 * grams come faster than one every `minGapS`, and before that every `minGapS` back to the first
 * gram, so the fast start rattles and the ticks never come closer together as the count slows.
 */
export function calcCountTicksS(scene: RevealScene, halvesG: readonly number[], minGapS: number): number[] {
  if (halvesG.length === 0) return [];
  const wheel = buildWheel(scene, halvesG);
  const ticksS: number[] = [];
  let g = wheel.endG;
  for (; g >= 1; g -= 1) {
    const laterS = ticksS.at(-1);
    if (laterS !== undefined && laterS - calcReachedS(scene, wheel, g) < minGapS) break;
    ticksS.push(calcReachedS(scene, wheel, g));
  }
  if (g >= 1) for (let s = ticksS.at(-1)! - minGapS; s >= calcReachedS(scene, wheel, 1); s -= minGapS) ticksS.push(s);
  return ticksS.reverse();
}

/** When `wheel`'s count reaches `g` grams in `scene`, in seconds from its start, where a half of `g` grams stops sliding. */
function calcReachedS(scene: RevealScene, wheel: Wheel, g: number): number {
  return scene.holdS + wheel.countS - calcTimeToGoS(wheel, wheel.endG - g);
}

/** The wheel of halves of `halvesG` grams that stops `scene.countS` after it starts. */
function buildWheel(scene: RevealScene, halvesG: readonly number[]): Wheel {
  const endG = Math.round(Math.max(...halvesG));
  return { endG, countS: scene.countS, limitS: endG > 0 ? (scene.countS * (endG + EASE_G)) / endG : 0 };
}

/** How long `wheel` takes to stop from `gLeft` grams short of its end. */
function calcTimeToGoS(wheel: Wheel, gLeft: number): number {
  return (wheel.limitS * gLeft) / (gLeft + EASE_G);
}

/** The wheel's count `countS` into counting, the inverse of `calcTimeToGoS`. */
function calcCountG(wheel: Wheel, countS: number): number {
  if (countS <= 0) return 0;
  const toGoS = wheel.countS - countS;
  if (toGoS <= 0) return wheel.endG;
  return wheel.endG - (EASE_G * toGoS) / (wheel.limitS - toGoS);
}

/**
 * How far a half has slid `elapsedS` into the reveal, its slide ending at `stopS`, as its counter
 * stops. Its speed `(1 - e^(-s / SLIDE_RAMP_S)) * (1 - SLIDE_BRAKE * s / stopS)` rises like a
 * released spring, then brakes steadily into a stop still at a quarter of its peak speed, so the
 * stop shows. A half's counter can stop within a frame or two of the count's start (the smaller
 * half of an uneven cut of a heavy cake), so the slides start with the reveal, `holdS` before the
 * counters, and never last less than that.
 */
function calcSlideShare(elapsedS: number, stopS: number): number {
  if (elapsedS <= 0) return 0;
  if (elapsedS >= stopS) return 1;
  return calcSlideDistance(elapsedS / stopS, SLIDE_RAMP_S / stopS) / calcSlideDistance(1, SLIDE_RAMP_S / stopS);
}

/** The integral of the slide's speed over `x` of its duration, the ramp's time constant being `rampShare` of the duration. */
function calcSlideDistance(x: number, rampShare: number): number {
  const decay = Math.exp(-x / rampShare);
  return x - (SLIDE_BRAKE * x * x) / 2 - rampShare * (1 - decay) + SLIDE_BRAKE * (rampShare * rampShare * (1 - decay) - rampShare * x * decay);
}
