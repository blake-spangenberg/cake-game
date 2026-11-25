/**
 * The knife's client-only motion through a cut: it sweeps down through the cake, then rocks on
 * its curved edge and settles. The player's own cut and every opponent's play the same motion.
 */

const DESCEND_S = 0.4;
const ROCK_S = 1.2;
/** How far the knife rocks either way through the cut, a gentle mezzaluna rock. */
const ROCK_RAD = 0.36;
/** Extra rock time, as a share of `ROCK_S`, for the knife to swing a little past center and settle. */
const SETTLE_SHARE = 0.5;

export interface CutMotion {
  /** How far below the pivot the knife's edge is, in meters. */
  lowerM: number;
  rockRad: number;
  isDone: boolean;
}

/** The knife's motion `elapsedS` into a cut that lowers it from `fromLowerM` to `toLowerM`. */
export function calcCutMotion(elapsedS: number, fromLowerM: number, toLowerM: number): CutMotion {
  const descend = Math.min(elapsedS / DESCEND_S, 1);
  const rockEnd = 1 + SETTLE_SHARE;
  const rock = Math.min(Math.max((elapsedS - DESCEND_S) / ROCK_S, 0), rockEnd);
  return { lowerM: fromLowerM + (toLowerM - fromLowerM) * descend * descend, rockRad: calcRockRad(rock), isDone: rock >= rockEnd };
}

/**
 * The knife's tilt in radians, given how far through its rock it is: 0 to 1 for one full
 * back-and-forth, then up to `1 + SETTLE_SHARE` for the swing past center and back. The settle
 * continues the same sine under a fade from 1 to 0 with no slope at either end, so it neither
 * jumps nor stops abruptly, and swings out to about an eighth of the rock's extent.
 */
function calcRockRad(rock: number): number {
  const settle = Math.max(rock - 1, 0) / SETTLE_SHARE;
  const fade = settle >= 1 ? 0 : ((1 + Math.cos(Math.PI * settle)) / 2) ** 3;
  return ROCK_RAD * Math.sin(2 * Math.PI * rock) * fade;
}
