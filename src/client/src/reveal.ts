/**
 * The timing of a cut's reveal, shared by the online judging and the offline practice's own cut:
 * the halves together, then parting slowly in place while a gram counter over each counts up
 * from 0, then the score once the counting ends.
 */

/** When each part of a reveal happens, in seconds from its start. */
export interface RevealTiming {
  /** How long the halves stay together before they part and the counters start. */
  holdS: number;
  /** How long the halves take to slide apart. */
  partS: number;
  /** How long the count takes from 0 to half the cake's weight. */
  toHalfS: number;
  /** How long it takes on from half the weight to the whole, which only a miss reaches. */
  pastHalfS: number;
}

/** A judged player's scene, which `src/server`'s `judging_scene_ms` holds the score up after for at least 0.8 s. */
export const JUDGED_TIMING: RevealTiming = { holdS: 0.5, partS: 2.8, toHalfS: 2.8, pastHalfS: 0.9 };
/** The offline practice's own cut, revealed once the knife has settled in it. */
export const OWN_TIMING: RevealTiming = { holdS: 0, partS: 1.8, toHalfS: 1.8, pastHalfS: 0.6 };
/**
 * How sharply the count slows into half the weight and speeds away from it: the time to the
 * last grams before half grows as their share's root of this power, so the last few tick slowly.
 */
const COUNT_EASE_POWER = 4;

/** A reveal at one moment: how far the halves have parted, from 0 to 1, each counter's grams, and whether the counting has ended. */
export interface RevealFrame {
  partShare: number;
  shownG: number[];
  isCounted: boolean;
}

/**
 * The reveal `elapsedS` in for halves of `halvesG` grams, or one whole cake's for a cut that
 * missed. Every counter shows the same count until it reaches its own half's weight, so a
 * perfect cut stops both at once and a bad one counts the larger half on alone.
 */
export function calcRevealFrame(elapsedS: number, timing: RevealTiming, halvesG: readonly number[]): RevealFrame {
  const halfG = halvesG.reduce((sum, g) => sum + g, 0) / 2;
  const countG = calcCountG(elapsedS - timing.holdS, timing, halfG);
  return {
    partShare: smoothstep((elapsedS - timing.holdS) / timing.partS),
    shownG: halvesG.map((g) => Math.round(Math.min(countG, g))),
    isCounted: countG >= Math.max(...halvesG),
  };
}

/** When the counting of halves of `halvesG` ends and the score shows, in seconds from the reveal's start. */
export function calcCountedS(timing: RevealTiming, halvesG: readonly number[]): number {
  const halfG = halvesG.reduce((sum, g) => sum + g, 0) / 2;
  const pastShare = Math.min(Math.max((Math.max(...halvesG) - halfG) / halfG, 0), 1);
  return timing.holdS + timing.toHalfS + timing.pastHalfS * pastShare ** (1 / COUNT_EASE_POWER);
}

/** The shared count `countS` into counting, fast at first, slowest at `halfG`, and quickening again past it to the whole. */
function calcCountG(countS: number, timing: RevealTiming, halfG: number): number {
  if (countS <= 0) return 0;
  if (countS < timing.toHalfS) return halfG * (1 - (1 - countS / timing.toHalfS) ** COUNT_EASE_POWER);
  return halfG * (1 + Math.min((countS - timing.toHalfS) / timing.pastHalfS, 1) ** COUNT_EASE_POWER);
}

function smoothstep(share: number): number {
  const t = Math.min(Math.max(share, 0), 1);
  return t * t * (3 - 2 * t);
}
