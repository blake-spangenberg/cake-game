/**
 * The file format of the knife's baked studio reflections: every mip level but the first of a
 * prefiltered cube, each level's six faces of 8-bit sRGB texels in turn, their color channels only,
 * each byte stored as its difference from the same channel of the texel before it, which halves
 * the file's compressed size. The first level, which only a roughness under 0.106 samples, is the
 * second doubled, which saves three quarters of the file.
 */

/** The prefiltered cube's side, as three's `PMREMGenerator` makes it. */
export const STUDIO_CUBE_PX = 256;
/** Mip levels down to the 8 px one, as three's `PMREMGenerator` makes them. */
export const STUDIO_LEVELS = 6;
const CUBE_FACES = 6;
const RGBA = 4;
const RGB = 3;

/** `levels[level][face]` as RGBA texels, every level from the first, in the order `decodeStudioLevels` returns them. */
export function encodeStudioLevels(levels: readonly (readonly Uint8Array[])[]): Uint8Array {
  const faces = levels.slice(1).flat();
  const bytes = new Uint8Array(faces.reduce((sum, face) => sum + (face.length / RGBA) * RGB, 0));
  let offset = 0;
  for (const face of faces) {
    for (let texel = 0; texel < face.length; texel += RGBA) {
      bytes.set(face.subarray(texel, texel + RGB), offset);
      offset += RGB;
    }
  }
  for (let i = bytes.length - 1; i >= RGB; i -= 1) bytes[i] -= bytes[i - RGB];
  return bytes;
}

/** Each level's six faces as opaque RGBA texels, from the 256 px level down, from the bytes `encodeStudioLevels` wrote. */
export function decodeStudioLevels(bytes: Uint8Array): Uint8Array[][] {
  const expectedLen = Array.from({ length: STUDIO_LEVELS - 1 }, (_, idx) => calcLevelPx(idx + 1) ** 2 * CUBE_FACES * RGB).reduce((sum, len) => sum + len, 0);
  if (bytes.length !== expectedLen) throw new Error(`studio reflections: ${bytes.length} bytes, expected ${expectedLen}`);
  const rgb = Uint8Array.from(bytes);
  for (let i = RGB; i < rgb.length; i += 1) rgb[i] += rgb[i - RGB];
  const levels: Uint8Array[][] = [];
  let offset = 0;
  for (let level = 1; level < STUDIO_LEVELS; level += 1) {
    const texels = calcLevelPx(level) ** 2;
    levels.push(Array.from({ length: CUBE_FACES }, (_, face) => toRgba(rgb.subarray(offset + face * texels * RGB, offset + (face + 1) * texels * RGB))));
    offset += texels * RGB * CUBE_FACES;
  }
  levels.unshift(levels[0].map((face) => toDoubled(face, calcLevelPx(1))));
  return levels;
}

function calcLevelPx(level: number): number {
  return STUDIO_CUBE_PX >> level;
}

function toRgba(rgb: Uint8Array): Uint8Array {
  const rgba = new Uint8Array((rgb.length / RGB) * RGBA).fill(255);
  for (let texel = 0; texel < rgb.length / RGB; texel += 1) rgba.set(rgb.subarray(texel * RGB, texel * RGB + RGB), texel * RGBA);
  return rgba;
}

/** `face`, `sidePx` texels a side, at twice the side, each texel repeated over two by two. */
function toDoubled(face: Uint8Array, sidePx: number): Uint8Array {
  const doubledPx = 2 * sidePx;
  const doubled = new Uint8Array(doubledPx * doubledPx * RGBA);
  const texels = new Uint32Array(face.buffer, face.byteOffset, face.length / RGBA);
  const doubledTexels = new Uint32Array(doubled.buffer);
  for (let y = 0; y < doubledPx; y += 1) {
    for (let x = 0; x < doubledPx; x += 1) doubledTexels[y * doubledPx + x] = texels[(y >> 1) * sidePx + (x >> 1)];
  }
  return doubled;
}
