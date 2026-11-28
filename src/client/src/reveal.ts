/**
 * The timing of a cut's reveal, shared by the online judging and the offline practice's own cut:
 * the halves together, then both sliding apart in place, fast at first and slowing to a stop, while a
 * gram counter over each counts up from 0. The slides and both counters start together and end
 * together, as the score shows. A cut that missed the cake skips the count.
 */

/**
 * One cut's reveal, in seconds from its start: the halves stay together and the counters wait at 0
 * for `holdS`, then the halves part as the counters count for `countS`, ending with the score.
 * Online, the server sets both for every page, as `src/server`'s `calc_count_ms` does; a miss
 * counts for none.
 */
export interface RevealScene {
  holdS: number;
  countS: number;
}

/** The offline practice's own cut, revealed once the knife has settled in it, counting as long as a judged one. */
const OWN_HOLD_S = 0.3;
const OWN_MIN_COUNT_S = 1.5;
const OWN_MAX_COUNT_S = 3.3;
/** The share of a count that climbs from 0 up to its last `FINISH_G` grams. */
const CLIMB_SHARE = 1 / 6;
const FINISH_G = 15;
/**
 * How long each of the finish's grams takes against the one after it. The last gram takes about a
 * fifth of the count, 0.66 s of a perfect cut's 3.3 s.
 */
const FINISH_RATIO = 0.7647;
/** Halvings that find a climb's starting speed to a double's precision. */
const CLIMB_SEARCH_STEPS = 60;
/** How long the last gram's tick sounds before the player joins the leaderboard and the ding, long enough to hear the tick apart from it. */
const SCORED_LAG_S = 0.04;
/** Absorbs the float error of a count landing exactly on a whole gram, far under a gram. */
const WHOLE_GRAM_SLACK_G = 1e-6;

/**
 * A reveal at one moment: how far both halves have slid apart, from 0 to 1, each counter's grams in
 * the order of the halves' grams given (none for a miss), and whether the counting has ended.
 */
export interface RevealFrame {
  partShare: number;
  shownG: number[];
  isCounted: boolean;
}

/**
 * One counter as a wheel slowing smoothly to a stop at `endG` after `countS`, every gram taking
 * longer than the one before. With `gLeft` grams to go in its finish, it stops in
 * `finishScaleS * (1 - FINISH_RATIO ** gLeft)` seconds. Its finish is the last `FINISH_G` grams,
 * reached by the climb, or the whole count for a half too light to climb faster than its finish
 * starts.
 */
interface Wheel {
  /** The half's grams as its counter shows them, where the count stops. */
  endG: number;
  countS: number;
  finishScaleS: number;
  climb: Climb | null;
}

/** The count from 0 to the finish: its speed falls exponentially from `startGPerS` into the finish's own first speed. */
interface Climb {
  durationS: number;
  /** The grams it covers. */
  lenG: number;
  startGPerS: number;
  decayPerS: number;
}

/**
 * The offline practice's reveal of halves of `halvesG` grams, or of a miss, `[]`, as the server
 * paces a judged one: its count is longest for a perfect cut and shorter, toward a floor, by the
 * square of the score's share of a perfect one.
 */
export function buildOwnScene(halvesG: readonly number[]): RevealScene {
  if (halvesG.length === 0) return { holdS: OWN_HOLD_S, countS: 0 };
  const totalG = halvesG.reduce((sum, g) => sum + g, 0);
  const scoreShare = (halvesG.length * Math.min(...halvesG)) / totalG;
  return { holdS: OWN_HOLD_S, countS: OWN_MIN_COUNT_S + (OWN_MAX_COUNT_S - OWN_MIN_COUNT_S) * scoreShare * scoreShare };
}

/**
 * `scene` `elapsedS` in for halves of `halvesG` grams, or for a cut that missed, `[]`. Each counter
 * counts to its own half's weight over the whole count, so both stop as the halves do.
 */
export function calcRevealFrame(elapsedS: number, scene: RevealScene, halvesG: readonly number[]): RevealFrame {
  const isCounted = elapsedS >= calcCountedS(scene);
  if (halvesG.length === 0) return { partShare: 0, shownG: [], isCounted };
  return {
    // Checked first, since the division can round a hair short of 1 on the frame the count ends.
    partShare: isCounted ? 1 : calcSlowingShare(Math.max(elapsedS - scene.holdS, 0) / scene.countS),
    shownG: halvesG.map((g) => Math.floor(calcCountG(buildWheel(scene, g), elapsedS - scene.holdS) + WHOLE_GRAM_SLACK_G)),
    isCounted,
  };
}

/** How far a slide that slows evenly to a stop has gone, `timeShare` of the way through it, both from 0 to 1. */
function calcSlowingShare(timeShare: number): number {
  return 1 - (1 - timeShare) ** 2;
}

/** When `scene`'s counting ends with its last gram and the score shows, in seconds from its start. */
export function calcCountedS(scene: RevealScene): number {
  return scene.holdS + scene.countS;
}

/**
 * When the judged player joins the leaderboard, with a ding for a medal place, in seconds from
 * `scene`'s start: `SCORED_LAG_S` after the last gram's tick, which the ding would otherwise drown
 * out or follow by a hair as the audio clock rounds both to the same render quantum.
 */
export function calcScoredS(scene: RevealScene): number {
  return calcCountedS(scene) + SCORED_LAG_S;
}

/**
 * When the count of halves of `halvesG` ticks in `scene`, in seconds from its start, none for a
 * miss (`[]`), following the larger half's counter: on each gram from the last, which ends the
 * count with the score, back to where grams come faster than one every `minGapS`, and before that
 * every `minGapS` back to the first gram, so the fast start rattles and the ticks never come closer
 * together as the count slows.
 */
export function calcCountTicksS(scene: RevealScene, halvesG: readonly number[], minGapS: number): number[] {
  if (halvesG.length === 0) return [];
  const wheel = buildWheel(scene, Math.max(...halvesG));
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

/** When `wheel`'s count reaches `g` grams in `scene`, in seconds from its start. */
function calcReachedS(scene: RevealScene, wheel: Wheel, g: number): number {
  const { climb } = wheel;
  if (climb && g < climb.lenG) return scene.holdS - Math.log(1 - (g * climb.decayPerS) / climb.startGPerS) / climb.decayPerS;
  return scene.holdS + wheel.countS - wheel.finishScaleS * (1 - FINISH_RATIO ** (wheel.endG - g));
}

/** The wheel of a half of `halfG` grams that stops `scene.countS` after it starts. */
function buildWheel(scene: RevealScene, halfG: number): Wheel {
  const endG = Math.round(halfG);
  const { countS } = scene;
  const durationS = countS * CLIMB_SHARE;
  const finishScaleS = (countS - durationS) / (1 - FINISH_RATIO ** FINISH_G);
  const finishStartGPerS = 1 / (finishScaleS * -Math.log(FINISH_RATIO) * FINISH_RATIO ** FINISH_G);
  const lenG = endG - FINISH_G;
  // The climb's mean speed over the finish's first: at 1 or less, no falling speed covers it.
  const meanShare = lenG / (durationS * finishStartGPerS);
  if (meanShare <= 1) return { endG, countS, finishScaleS: countS / (1 - FINISH_RATIO ** endG), climb: null };
  const startShare = calcStartShare(meanShare);
  const climb = { durationS, lenG, startGPerS: startShare * finishStartGPerS, decayPerS: Math.log(startShare) / durationS };
  return { endG, countS, finishScaleS, climb };
}

/**
 * How many times its end speed an exponentially slowing climb starts at, for its mean speed to be
 * `meanShare` times its end speed: the `x > 1` with `(x - 1) / ln x = meanShare`, for `meanShare > 1`.
 */
function calcStartShare(meanShare: number): number {
  let low = 1;
  // `(x - 1) / ln x` exceeds `meanShare` here for any `meanShare > 1`.
  let high = 4 * meanShare * meanShare;
  for (let step = 0; step < CLIMB_SEARCH_STEPS; step += 1) {
    const mid = Math.sqrt(low * high);
    if ((mid - 1) / Math.log(mid) > meanShare) high = mid;
    else low = mid;
  }
  return high;
}

/** The wheel's count `countS` into counting, the inverse of `calcReachedS`. */
function calcCountG(wheel: Wheel, countS: number): number {
  // A half under half a gram has no gram to count, which the curve below does not cover.
  if (countS <= 0 || wheel.endG === 0) return 0;
  const toGoS = wheel.countS - countS;
  if (toGoS <= 0) return wheel.endG;
  const { climb } = wheel;
  if (climb && countS < climb.durationS) return (climb.startGPerS / climb.decayPerS) * (1 - Math.exp(-climb.decayPerS * countS));
  return wheel.endG - Math.log(1 - toGoS / wheel.finishScaleS) / Math.log(FINISH_RATIO);
}
