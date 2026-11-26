/**
 * Wood grain painted once on the CPU, covering a whole surface so it never tiles. `woodWorker.ts`
 * paints it off the page's thread, so this module imports nothing: three alone would add 90 kB to
 * the worker's bundle.
 */

/** How a wood texture looks: its grain runs along its width, across `boards` boards glued edge to edge. */
export interface WoodLook {
  widthPx: number;
  heightPx: number;
  /** Real size the texture covers, along and across the grain, in meters. */
  lengthM: number;
  breadthM: number;
  boards: number;
  /** Growth rings across one meter of breadth, on average: each ring's width varies about it. */
  ringsPerM: number;
  /** How far the rings bend away from straight along a board, in rings. */
  ringWander: number;
  /** As 0xRRGGBB in sRGB. */
  earlywood: number;
  latewood: number;
  /** How far each board's tone may stray from the rest, as a share of its brightness. */
  boardToneSpread: number;
  seed: number;
  /** Knots to paint, none by default. No two may share a row of the texture. */
  knots?: readonly WoodKnot[];
}

/** A knot's place, as shares of the texture's length and breadth, and its size in meters. */
export interface WoodKnot {
  u: number;
  v: number;
  /** The dark oval core's length along the grain. */
  coreLengthM: number;
  /** How far from the knot's center the grain bends around it. */
  reachM: number;
}

/** Where and how a knot is painted over the wood already in `texels`. */
interface KnotPainter {
  texels: Uint8Array;
  widthPx: number;
  ringProfile: Uint8Array;
  random: () => number;
}

/** The board a knot lies in: how its rings run. */
interface KnotBoard {
  rowStart: number;
  ringRows: Float32Array;
  wander: Float32Array;
  fan: Float32Array;
  boardPx: number;
}

/** A knot in texels, with the rows and columns its reach covers. */
interface KnotTexels extends WoodKnot {
  centerX: number;
  centerY: number;
  reachX: number;
  reachY: number;
  /** Texels per meter along and across the grain. */
  texelsPerMX: number;
  texelsPerMY: number;
}

const RING_PROFILE_LEN = 256;
/** Darkening of the glue line between boards, as a share of brightness. */
const SEAM_SHADE = 0.3;
/** Peak brightness swing of the fine fiber streaks that run along the grain. */
const FIBER_SHADE = 0.07;
/** Peak brightness swing of each texel's own speckle. */
const SPECKLE_SHADE = 0.03;
/** The narrowest and widest a ring grows, as shares of the average. */
const RING_WIDTH_RANGE = [0.5, 1.7] as const;
/** A knot's core is this much narrower across the grain than along it. */
const KNOT_CORE_ASPECT = 0.6;
/** How far a knot pulls the grain it samples toward itself at its center, as a share of the distance, so the grain lies pushed outward around it. */
const KNOT_PUSH = 0.9;
/** The core's brightness at its darkest and lightest concentric ring, as shares of the wood's. */
const KNOT_CORE_SHADE = [0.5, 0.62] as const;
/** Concentric rings across a knot's core. */
const KNOT_CORE_RINGS = 1.5;
/** How much darker the wood is at a knot's core edge than far away, fading out over `KNOT_HALO_CORES` core radii. */
const KNOT_HALO_SHADE = 0.12;
const KNOT_HALO_CORES = 2.6;
/** Where the core's edge fades into the halo, from its inner to its outer end, in core radii. */
const KNOT_CORE_EDGE_RADII = [0.85, 1.1] as const;
const KNOT_SEED_SALT = 0x4b4e4f54;

/** `look`'s opaque sRGB RGBA texels, row by row, its grain along the rows. The same `look` always paints the same texels. */
export function paintWood(look: WoodLook): Uint8Array {
  const { widthPx, heightPx } = look;
  const random = createRandom(look.seed);
  const ringProfile = buildRingProfile(look);
  const fiberRows = buildFiberRows(heightPx, random);
  const boardPx = heightPx / look.boards;
  const ringsPerPx = (look.ringsPerM * look.breadthM) / heightPx;
  const knots = (look.knots ?? []).map((knot) => toKnotTexels(knot, look));
  validateKnotRows(knots);
  const texels = new Uint8Array(widthPx * heightPx * 4);
  // Knots draw from their own stream, so the rest of the wood is the same with or without them.
  const knotPainter = { texels, widthPx, ringProfile, random: createRandom(look.seed ^ KNOT_SEED_SALT) };
  for (let board = 0; board < look.boards; board += 1) {
    const tone = 1 + (random() * 2 - 1) * look.boardToneSpread;
    const rowStart = Math.round(board * boardPx);
    const rowEnd = Math.round((board + 1) * boardPx);
    const ringRows = buildRingRows(rowEnd - rowStart, ringsPerPx, random);
    // The rings bend together, and fan apart toward the board's far edge.
    const wander = buildWanderColumns(widthPx, look.ringWander, random);
    const fan = buildWanderColumns(widthPx, look.ringWander, random);
    const knotBoard = { rowStart, ringRows, wander, fan, boardPx };
    for (let y = rowStart; y < rowEnd; y += 1) {
      const isSeam = y === rowStart && board > 0;
      const rowShade = tone * (1 + fiberRows[y] * FIBER_SHADE) * (isSeam ? 1 - SEAM_SHADE : 1);
      const rowRing = ringRows[y - rowStart];
      const fanShare = (y - rowStart) / boardPx;
      for (let x = 0; x < widthPx; x += 1) {
        const ring = rowRing + wander[x] + fan[x] * fanShare;
        const profile = (((ring - Math.floor(ring)) * RING_PROFILE_LEN) | 0) * 3;
        const shade = rowShade * (1 + (random() * 2 - 1) * SPECKLE_SHADE);
        const texel = (y * widthPx + x) * 4;
        texels[texel] = Math.min(255, ringProfile[profile] * shade);
        texels[texel + 1] = Math.min(255, ringProfile[profile + 1] * shade);
        texels[texel + 2] = Math.min(255, ringProfile[profile + 2] * shade);
        texels[texel + 3] = 255;
      }
      const knot = knots.find((candidate) => Math.abs(y - candidate.centerY) < candidate.reachY);
      if (knot) repaintKnotRow(knotPainter, knot, knotBoard, y, rowShade);
    }
  }
  return texels;
}

/**
 * Color across one growth ring, as 8-bit sRGB triples: pale earlywood darkening into a band of
 * latewood that ends sharply where the next year's growth begins, mixed in linear light.
 */
function buildRingProfile(look: WoodLook): Uint8Array {
  const early = toLinearRgb(look.earlywood);
  const late = toLinearRgb(look.latewood);
  const profile = new Uint8Array(RING_PROFILE_LEN * 3);
  for (let i = 0; i < RING_PROFILE_LEN; i += 1) {
    const t = i / RING_PROFILE_LEN;
    const lateShare = smoothstep(t, 0.55, 0.9) * (1 - smoothstep(t, 0.97, 1));
    for (let channel = 0; channel < 3; channel += 1) {
      const linear = early[channel] + (late[channel] - early[channel]) * lateShare;
      profile[3 * i + channel] = toSrgb(linear) * 255;
    }
  }
  return profile;
}

/** `hex`'s channels in linear light, as three's `Color` holds them. */
function toLinearRgb(hex: number): [number, number, number] {
  return [(hex >> 16) & 255, (hex >> 8) & 255, hex & 255].map((channel) => toLinear(channel / 255)) as [number, number, number];
}

/** The sRGB transfer function both ways, with three's constants, so the wood is the same as when three's `Color` mixed it. */
function toLinear(srgb: number): number {
  return srgb < 0.04045 ? srgb * 0.0773993808 : Math.pow(srgb * 0.9478672986 + 0.0521327014, 2.4);
}

function toSrgb(linear: number): number {
  return linear < 0.0031308 ? linear * 12.92 : 1.055 * Math.pow(linear, 0.41666) - 0.055;
}

/** How many rings lie below each row of a board, each ring a random width within `RING_WIDTH_RANGE` of the average. */
function buildRingRows(rows: number, ringsPerPx: number, random: () => number): Float32Array {
  const [narrowest, widest] = RING_WIDTH_RANGE;
  const ringRows = new Float32Array(rows);
  let ring = random() * 100;
  let rowsPerRing = (narrowest + (widest - narrowest) * random()) / ringsPerPx;
  for (let y = 0; y < rows; y += 1) {
    ringRows[y] = ring;
    const next = ring + 1 / rowsPerRing;
    if (Math.floor(next) !== Math.floor(ring)) rowsPerRing = (narrowest + (widest - narrowest) * random()) / ringsPerPx;
    ring = next;
  }
  return ringRows;
}

function toKnotTexels(knot: WoodKnot, look: WoodLook): KnotTexels {
  const texelsPerMX = look.widthPx / look.lengthM;
  const texelsPerMY = look.heightPx / look.breadthM;
  return {
    ...knot,
    centerX: knot.u * look.widthPx,
    centerY: knot.v * look.heightPx,
    reachX: knot.reachM * texelsPerMX,
    reachY: knot.reachM * texelsPerMY,
    texelsPerMX,
    texelsPerMY,
  };
}

/** The painting finds one knot per row, so knots whose reaches share a row are rejected. */
function validateKnotRows(knots: readonly KnotTexels[]): void {
  for (const [idx, knot] of knots.entries()) {
    const overlapping = knots.slice(idx + 1).find((other) => Math.abs(other.centerY - knot.centerY) < knot.reachY + other.reachY);
    if (overlapping) throw new Error(`wood knots at v=${knot.v} and v=${overlapping.v} share rows of the texture`);
  }
}

/**
 * Paints over the texels of row `y` that `knot` reaches, with the wood of `board` as it looks with
 * the knot there: its grain sampled from nearer the knot, so it lies pushed away from it, and darker.
 */
function repaintKnotRow(painter: KnotPainter, knot: KnotTexels, board: KnotBoard, y: number, rowShade: number): void {
  const { texels, widthPx, ringProfile, random } = painter;
  const offsetYM = (y - knot.centerY) / knot.texelsPerMY;
  const xFrom = Math.max(0, Math.ceil(knot.centerX - knot.reachX));
  const xTo = Math.min(widthPx, Math.floor(knot.centerX + knot.reachX) + 1);
  for (let x = xFrom; x < xTo; x += 1) {
    const offsetXM = (x - knot.centerX) / knot.texelsPerMX;
    const distanceM = Math.sqrt(offsetXM * offsetXM + offsetYM * offsetYM);
    if (distanceM >= knot.reachM) continue;
    const closeness = 1 - distanceM / knot.reachM;
    const pulledRow = knot.centerY + (y - knot.centerY) * (1 - KNOT_PUSH * closeness * closeness) - board.rowStart;
    const ring = sampleRing(board.ringRows, pulledRow) + board.wander[x] + (board.fan[x] * pulledRow) / board.boardPx;
    const profile = (((ring - Math.floor(ring)) * RING_PROFILE_LEN) | 0) * 3;
    const shade = rowShade * calcKnotShade(knot, offsetXM, offsetYM) * (1 + (random() * 2 - 1) * SPECKLE_SHADE);
    const texel = (y * widthPx + x) * 4;
    texels[texel] = Math.min(255, ringProfile[profile] * shade);
    texels[texel + 1] = Math.min(255, ringProfile[profile + 1] * shade);
    texels[texel + 2] = Math.min(255, ringProfile[profile + 2] * shade);
  }
}

/** A brightness multiplier at a texel `offsetXM` along and `offsetYM` across the grain from the knot: a dark oval core of concentric rings inside a fading dark halo. */
function calcKnotShade(knot: KnotTexels, offsetXM: number, offsetYM: number): number {
  const coreRadiusM = knot.coreLengthM / 2;
  const coreX = offsetXM / coreRadiusM;
  const coreY = offsetYM / (coreRadiusM * KNOT_CORE_ASPECT);
  const coreRadii = Math.sqrt(coreX * coreX + coreY * coreY);
  if (coreRadii >= KNOT_HALO_CORES) return 1;
  const halo = 1 - KNOT_HALO_SHADE * (1 - smoothstep(coreRadii, 1, KNOT_HALO_CORES));
  if (coreRadii >= KNOT_CORE_EDGE_RADII[1]) return halo;
  const [darkest, lightest] = KNOT_CORE_SHADE;
  const ringBand = 0.5 + 0.5 * Math.cos(2 * Math.PI * KNOT_CORE_RINGS * coreRadii);
  const core = darkest + (lightest - darkest) * ringBand;
  return lerp(halo, core, 1 - smoothstep(coreRadii, KNOT_CORE_EDGE_RADII[0], KNOT_CORE_EDGE_RADII[1]));
}

/** The ring count at fractional row `row` of a board, between its two nearest rows. */
function sampleRing(ringRows: Float32Array, row: number): number {
  const clamped = Math.max(0, Math.min(ringRows.length - 1, row));
  const below = Math.floor(clamped);
  const above = Math.min(below + 1, ringRows.length - 1);
  return lerp(ringRows[below], ringRows[above], clamped - below);
}

/** How far the rings sit from straight at each column, in rings: a few gentle waves along the board. */
function buildWanderColumns(widthPx: number, ringWander: number, random: () => number): Float32Array {
  const waves = [0.6, 0.28, 0.12].map((amplitude, idx) => ({
    amplitude: amplitude * ringWander,
    cycles: (0.5 + random()) * (idx + 1) * 0.8,
    phase: random() * 2 * Math.PI,
  }));
  const wander = new Float32Array(widthPx);
  for (let x = 0; x < widthPx; x += 1) {
    const t = x / widthPx;
    wander[x] = waves.reduce((sum, wave) => sum + wave.amplitude * Math.sin(2 * Math.PI * wave.cycles * t + wave.phase), 0);
  }
  return wander;
}

/** A brightness swing per row in -1..1, smoothed over a few rows, for fibers running along the grain. */
function buildFiberRows(heightPx: number, random: () => number): Float32Array {
  const raw = Float32Array.from({ length: heightPx }, () => random() * 2 - 1);
  const rows = new Float32Array(heightPx);
  for (let y = 0; y < heightPx; y += 1) {
    const prev = raw[Math.max(0, y - 1)];
    const next = raw[Math.min(heightPx - 1, y + 1)];
    rows[y] = (prev + 2 * raw[y] + next) / 4;
  }
  return rows;
}

/** 0 at or below `min`, 1 at or above `max`, and a smooth S-curve between, as three's `MathUtils.smoothstep`. */
function smoothstep(x: number, min: number, max: number): number {
  if (x <= min) return 0;
  if (x >= max) return 1;
  const t = (x - min) / (max - min);
  return t * t * (3 - 2 * t);
}

function lerp(from: number, to: number, t: number): number {
  return (1 - t) * from + t * to;
}

/** A seeded uniform random number generator in 0..1 (mulberry32), so the wood looks the same every load. */
function createRandom(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
