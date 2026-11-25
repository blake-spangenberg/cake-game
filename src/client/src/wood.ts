/**
 * Wood grain painted once on the CPU, as a texture covering a whole surface so it never tiles.
 */
import * as THREE from "three";

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
  earlywood: THREE.ColorRepresentation;
  latewood: THREE.ColorRepresentation;
  /** How far each board's tone may stray from the rest, as a share of its brightness. */
  boardToneSpread: number;
  seed: number;
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

/** Paints `look` into a mipmapped sRGB texture, as sharp at grazing angles as `anisotropy` allows. */
export function buildWoodTexture(look: WoodLook, anisotropy: number): THREE.DataTexture {
  const { widthPx, heightPx } = look;
  const random = createRandom(look.seed);
  const ringProfile = buildRingProfile(look);
  const fiberRows = buildFiberRows(heightPx, random);
  const boardPx = heightPx / look.boards;
  const ringsPerPx = (look.ringsPerM * look.breadthM) / heightPx;
  const texels = new Uint8Array(widthPx * heightPx * 4);
  for (let board = 0; board < look.boards; board += 1) {
    const tone = 1 + (random() * 2 - 1) * look.boardToneSpread;
    const rowStart = Math.round(board * boardPx);
    const rowEnd = Math.round((board + 1) * boardPx);
    const ringRows = buildRingRows(rowEnd - rowStart, ringsPerPx, random);
    // The rings bend together, and fan apart toward the board's far edge.
    const wander = buildWanderColumns(widthPx, look.ringWander, random);
    const fan = buildWanderColumns(widthPx, look.ringWander, random);
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
    }
  }
  const texture = new THREE.DataTexture(texels, widthPx, heightPx);
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.wrapS = THREE.RepeatWrapping;
  texture.wrapT = THREE.RepeatWrapping;
  texture.magFilter = THREE.LinearFilter;
  texture.minFilter = THREE.LinearMipmapLinearFilter;
  texture.generateMipmaps = true;
  texture.anisotropy = anisotropy;
  texture.needsUpdate = true;
  return texture;
}

/**
 * Gives a box-like `geometry` texture coordinates that lay the wood's grain along `grainAxis` at
 * its real size, projecting each face along its normal, so a long thin part shows long grain.
 */
export function setWoodUvs(geometry: THREE.BufferGeometry, look: WoodLook, grainAxis: 0 | 1 | 2): void {
  const positions = geometry.getAttribute("position");
  const normals = geometry.getAttribute("normal");
  const uvs = new Float32Array(positions.count * 2);
  const point = new THREE.Vector3();
  const normal = new THREE.Vector3();
  for (let i = 0; i < positions.count; i += 1) {
    point.fromBufferAttribute(positions, i);
    normal.fromBufferAttribute(normals, i);
    const [alongAxis, acrossAxis] = calcFaceAxes(findDominantAxis(normal), grainAxis);
    uvs[2 * i] = point.getComponent(alongAxis) / look.lengthM + 0.5;
    uvs[2 * i + 1] = point.getComponent(acrossAxis) / look.breadthM + 0.5;
  }
  geometry.setAttribute("uv", new THREE.BufferAttribute(uvs, 2));
}

function findDominantAxis(normal: THREE.Vector3): 0 | 1 | 2 {
  const [ax, ay, az] = [Math.abs(normal.x), Math.abs(normal.y), Math.abs(normal.z)];
  if (ax >= ay && ax >= az) return 0;
  return ay >= az ? 1 : 2;
}

/** The face's two in-plane axes, the grain's first where the face shows it, else the end grain's. */
function calcFaceAxes(faceAxis: 0 | 1 | 2, grainAxis: 0 | 1 | 2): [number, number] {
  const inPlane = [0, 1, 2].filter((axis) => axis !== faceAxis);
  if (!inPlane.includes(grainAxis)) return [inPlane[0], inPlane[1]];
  return [grainAxis, inPlane.find((axis) => axis !== grainAxis)!];
}

/**
 * Color across one growth ring, as 8-bit sRGB triples: pale earlywood darkening into a band of
 * latewood that ends sharply where the next year's growth begins.
 */
function buildRingProfile(look: WoodLook): Uint8Array {
  const early = new THREE.Color(look.earlywood);
  const late = new THREE.Color(look.latewood);
  const profile = new Uint8Array(RING_PROFILE_LEN * 3);
  const color = new THREE.Color();
  for (let i = 0; i < RING_PROFILE_LEN; i += 1) {
    const t = i / RING_PROFILE_LEN;
    const lateShare = THREE.MathUtils.smoothstep(t, 0.55, 0.9) * (1 - THREE.MathUtils.smoothstep(t, 0.97, 1));
    color.copy(early).lerp(late, lateShare).convertLinearToSRGB();
    profile[3 * i] = color.r * 255;
    profile[3 * i + 1] = color.g * 255;
    profile[3 * i + 2] = color.b * 255;
  }
  return profile;
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
