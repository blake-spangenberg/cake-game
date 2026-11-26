/**
 * Where each half of a cut cake comes to rest once the halves part: presentation only, it never
 * touches the weighed halves. The halves only slide level across the table, never up or down. A
 * half resting on the plate or the table slides away from the cut, no farther than keeps what it
 * rests on under it and keeps it out of the stand. A half resting on neither, held up by the other
 * half's cut face, stays against the standing blade, and the other half parts the whole gap.
 */
import * as THREE from "three";

/** What the halves can rest on, in meters, with the table top at y = 0 and the stand's axis at the origin. */
export interface Supports {
  tableHalfXM: number;
  tableHalfZM: number;
  plateRadiusM: number;
  plateTopYM: number;
  /** The stand's cylinders about its axis, flattened as radius, bottom, top for each. */
  standParts: Float64Array;
}

/** A search that runs a slice of its work per `next()`, so no frame takes all of it, and returns its result once done. */
export type SlicedJob<T> = Generator<void, T, void>;

/**
 * How far apart the halves end, level, where they have room, so they tell apart at a glance. Each
 * half takes half of it, and a half short of room leaves the rest to the other.
 */
const PARTED_GAP_M = 0.18;
/**
 * The least gap worth letting a half's footprint hang over the plate's rim for, by up to
 * `RIM_OVERHANG_M` and with the middle of its footprint still on the plate, when keeping all of
 * it on the plate parts the halves less.
 */
const MIN_PARTED_GAP_M = 0.1;
const RIM_OVERHANG_M = 0.02;
/** How far above the surface under it a point may sit and still count as resting on it, for the mesh's slack inside its collider. */
const REST_TOLERANCE_M = 0.006;
/** How far inside the plate's rim a resting point must stay, clear of the rim's beads. */
const RIM_MARGIN_M = 0.008;
const TABLE_MARGIN_M = 0.01;
/** Points a slice visits, as flattened coordinates, small enough that a slice takes about a millisecond before the code warms up. */
const SLICE_COORDS = 3 * 2048;

/** How a half's points resting on the plate must stay on it: every one inside the rim, or hanging over it a little with their middle inside. */
type RimRule = "inside" | "overhang";

/**
 * The displacement of each half from where the cut left it, for halves on the `+normal` and
 * `-normal` sides of the plane through `pivot`, in that order. `positions` are the cake's vertex
 * positions in its own frame, flattened, and `pose` places them in the world; `minSlideM` is the
 * least level slide that clears the blade.
 */
export function* calcHalfRests(positions: Float32Array, pose: THREE.Matrix4, pivot: THREE.Vector3, normal: THREE.Vector3, minSlideM: number, supports: Supports): SlicedJob<THREE.Vector3[]> {
  const halves = yield* splitPoints(positions, pose, pivot, normal);
  const level = new THREE.Vector3(normal.x, 0, normal.z).normalize();
  const dirs = [level, level.clone().negate()];
  const isResting = [yield* hasRestingPoint(halves[0], supports), yield* hasRestingPoint(halves[1], supports)];
  function* calcCapsM(rule: RimRule): SlicedJob<number[]> {
    const capsM = [0, 0];
    for (const idx of [0, 1]) {
      if (isResting[idx]) capsM[idx] = yield* calcLevelCapM(halves[idx], dirs[idx], supports, rule);
    }
    return capsM;
  }
  let slidesM = shareSlidesM(yield* calcCapsM("inside"), PARTED_GAP_M);
  if (slidesM[0] + slidesM[1] < MIN_PARTED_GAP_M) slidesM = shareSlidesM(yield* calcCapsM("overhang"), MIN_PARTED_GAP_M);
  return [0, 1].map((idx) => dirs[idx].clone().multiplyScalar(Math.max(slidesM[idx], minSlideM)));
}

/** `positions` placed by `pose`, split by which side of the plane through `pivot` with `normal` they lie on, `+normal` first. */
function* splitPoints(positions: Float32Array, pose: THREE.Matrix4, pivot: THREE.Vector3, normal: THREE.Vector3): SlicedJob<Float32Array[]> {
  const points = new Float32Array(positions.length);
  const e = pose.elements;
  const sides = new Uint8Array(positions.length / 3);
  let sideOneLen = 0;
  for (let i = 0; i < positions.length; i += 3) {
    if (i % SLICE_COORDS === 0) yield;
    const lx = positions[i];
    const ly = positions[i + 1];
    const lz = positions[i + 2];
    const x = e[0] * lx + e[4] * ly + e[8] * lz + e[12];
    const y = e[1] * lx + e[5] * ly + e[9] * lz + e[13];
    const z = e[2] * lx + e[6] * ly + e[10] * lz + e[14];
    points[i] = x;
    points[i + 1] = y;
    points[i + 2] = z;
    const side = (x - pivot.x) * normal.x + (y - pivot.y) * normal.y + (z - pivot.z) * normal.z >= 0 ? 0 : 1;
    sides[i / 3] = side;
    sideOneLen += side;
  }
  const halves = [new Float32Array(3 * (sides.length - sideOneLen)), new Float32Array(3 * sideOneLen)];
  const ends = [0, 0];
  for (let i = 0; i < sides.length; i += 1) {
    const half = halves[sides[i]];
    const j = ends[sides[i]];
    half[j] = points[3 * i];
    half[j + 1] = points[3 * i + 1];
    half[j + 2] = points[3 * i + 2];
    ends[sides[i]] = j + 3;
  }
  return halves;
}

/** Each half's level slide, sharing `gapM` evenly where both have room under their `capsM`, else giving one's shortfall to the other. */
function shareSlidesM(capsM: number[], gapM: number): number[] {
  return capsM.map((capM, idx) => Math.min(capM, Math.max(gapM / 2, gapM - capsM[1 - idx])));
}

function isOnPlate(x: number, z: number, supports: Supports): boolean {
  return x * x + z * z <= supports.plateRadiusM * supports.plateRadiusM;
}

/** How far the point is above the plate or table top under it, or infinitely far beyond the table. */
function calcHeightAboveSupport(x: number, y: number, z: number, supports: Supports): number {
  if (isOnPlate(x, z, supports)) return y - supports.plateTopYM;
  if (Math.abs(x) <= supports.tableHalfXM && Math.abs(z) <= supports.tableHalfZM) return y;
  return Infinity;
}

function* hasRestingPoint(half: Float32Array, supports: Supports): SlicedJob<boolean> {
  for (let i = 0; i < half.length; i += 3) {
    if (i % SLICE_COORDS === 0) yield;
    if (calcHeightAboveSupport(half[i], half[i + 1], half[i + 2], supports) <= REST_TOLERANCE_M) return true;
  }
  return false;
}

/**
 * The farthest `half` can slide level along `dir` with what it rests on staying under it, as
 * `rule` sets for the plate, or at least getting no farther off it, and no point entering the stand.
 */
function* calcLevelCapM(half: Float32Array, dir: THREE.Vector3, supports: Supports, rule: RimRule): SlicedJob<number> {
  const rimRadiusM = supports.plateRadiusM - RIM_MARGIN_M;
  let capM = Infinity;
  let plateSumX = 0;
  let plateSumZ = 0;
  let plateCount = 0;
  for (let i = 0; i < half.length; i += 3) {
    if (i % SLICE_COORDS === 0) yield;
    const x = half[i];
    const y = half[i + 1];
    const z = half[i + 2];
    // Nothing a point can rest on or slide into reaches above the plate's top.
    if (y > supports.plateTopYM + REST_TOLERANCE_M) continue;
    capM = Math.min(capM, calcStandEntryM(x, y, z, dir, supports));
    if (calcHeightAboveSupport(x, y, z, supports) > REST_TOLERANCE_M) continue;
    if (!isOnPlate(x, z, supports)) {
      capM = Math.min(capM, calcTableStayM(x, z, dir, supports));
      continue;
    }
    capM = Math.min(capM, calcCircleStayM(x, z, dir, rule === "inside" ? rimRadiusM : supports.plateRadiusM + RIM_OVERHANG_M));
    plateSumX += x;
    plateSumZ += z;
    plateCount += 1;
  }
  if (rule === "overhang" && plateCount > 0) capM = Math.min(capM, calcCircleStayM(plateSumX / plateCount, plateSumZ / plateCount, dir, rimRadiusM));
  return capM;
}

/** The farthest the point moves along level `dir` before leaving the circle of `radiusM` about the axis, or, from outside it, before getting any farther out. */
function calcCircleStayM(x: number, z: number, dir: THREE.Vector3, radiusM: number): number {
  const along = x * dir.x + z * dir.z;
  const distSq = x * x + z * z;
  const keepSq = Math.max(radiusM * radiusM, distSq);
  return -along + Math.sqrt(Math.max(along * along - distSq + keepSq, 0));
}

/** The farthest the point moves along level `dir` before leaving the table top less its margin, or, from outside that, before getting any farther out. */
function calcTableStayM(x: number, z: number, dir: THREE.Vector3, supports: Supports): number {
  return Math.min(calcAxisStayM(x, dir.x, supports.tableHalfXM - TABLE_MARGIN_M), calcAxisStayM(z, dir.z, supports.tableHalfZM - TABLE_MARGIN_M));
}

function calcAxisStayM(at: number, step: number, halfM: number): number {
  if (step > 0) return (Math.max(halfM, at) - at) / step;
  if (step < 0) return (Math.min(-halfM, at) - at) / step;
  return Infinity;
}

/** How far the point moves along level `dir` before entering a stand cylinder whose height it lies within, or infinitely far if it never does or starts inside. */
function calcStandEntryM(x: number, y: number, z: number, dir: THREE.Vector3, supports: Supports): number {
  const along = x * dir.x + z * dir.z;
  if (along >= 0) return Infinity;
  const distSq = x * x + z * z;
  const parts = supports.standParts;
  let entryM = Infinity;
  for (let i = 0; i < parts.length; i += 3) {
    const radiusM = parts[i];
    if (y <= parts[i + 1] - REST_TOLERANCE_M || y >= parts[i + 2] - REST_TOLERANCE_M) continue;
    const disc = along * along - distSq + radiusM * radiusM;
    if (distSq <= radiusM * radiusM || disc <= 0) continue;
    entryM = Math.min(entryM, -along - Math.sqrt(disc));
  }
  return entryM;
}
