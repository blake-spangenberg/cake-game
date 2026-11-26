/**
 * Prefilters the small photo studio the knife's steel reflects, for `tools/bake-studio.mjs`, which
 * drives this dev-only page and stores the result for `fetchStudioEnvironment` in `props.ts`.
 * Prefiltering in the page held a player's first visit 0.2-0.3 s compiling three's PMREM shaders.
 */
import * as THREE from "three";
import { STUDIO_CUBE_PX, STUDIO_LEVELS, encodeStudioLevels } from "./studioFormat";

declare global {
  interface Window {
    /** The prefiltered studio as `encodeStudioLevels` packs it, base64. */
    bakeStudio: () => string;
    isStudioReady: boolean;
  }
}

const STUDIO_WIDTH_PX = 512;
const STUDIO_BLUR_PX = 6;
/**
 * Top to bottom: a dim ceiling, gray walls brightening toward the horizon, then a gray floor
 * darkening below, only faintly warm so the steel stays steel-colored over the brown table.
 */
const STUDIO_GRADIENT: readonly (readonly [number, string])[] = [
  [0, "#666564"],
  [0.35, "#8a8783"],
  [0.49, "#b4b0aa"],
  [0.51, "#7c7671"],
  [0.7, "#55504c"],
  [1, "#2a2826"],
];
/** Softboxes in the directions of `addLights`' key and fill lights in `props.ts`. */
const STUDIO_LIGHTS = [
  { direction: new THREE.Vector3(1.7, 1.2, 0.6), widthShare: 0.14, heightShare: 0.16, color: "#fff6ea" },
  { direction: new THREE.Vector3(-1.2, 0.9, -0.9), widthShare: 0.1, heightShare: 0.12, color: "#c9d6ff" },
];
const STUDIO_STRIP_COLOR = "#d8d6d2";
const STUDIO_STRIP_TOP_SHARE = 0.16;
const STUDIO_STRIP_HEIGHT_SHARE = 0.05;
const CUBE_FACES = 6;
const RGBA = 4;

const renderer = new THREE.WebGLRenderer();
window.bakeStudio = bakeStudio;
window.isStudioReady = true;

function bakeStudio(): string {
  const equirect = buildStudioEquirect();
  const generator = new THREE.PMREMGenerator(renderer);
  const target = generator.fromEquirectangular(equirect);
  if (target.width !== STUDIO_CUBE_PX || target.texture.mipmaps.length !== STUDIO_LEVELS) {
    throw new Error(`three prefiltered a ${target.width} px cube of ${target.texture.mipmaps.length} levels, the format holds ${STUDIO_CUBE_PX} px of ${STUDIO_LEVELS}`);
  }
  const levels = readCubeLevels(target);
  generator.dispose();
  target.dispose();
  equirect.dispose();
  let binary = "";
  for (const byte of encodeStudioLevels(levels)) binary += String.fromCharCode(byte);
  return btoa(binary);
}

/**
 * A small photo studio as an equirectangular image: a softbox where the key light shines from, a
 * cooler one at the fill, a bright strip overhead, gray walls, and a darker floor below the horizon.
 */
function buildStudioEquirect(): THREE.Texture {
  const canvas = document.createElement("canvas");
  canvas.width = STUDIO_WIDTH_PX;
  canvas.height = STUDIO_WIDTH_PX / 2;
  const ctx = canvas.getContext("2d")!;
  const { width, height } = canvas;
  const sky = ctx.createLinearGradient(0, 0, 0, height);
  for (const [stop, color] of STUDIO_GRADIENT) sky.addColorStop(stop, color);
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, width, height);
  ctx.filter = `blur(${STUDIO_BLUR_PX}px)`;
  for (const light of STUDIO_LIGHTS) {
    const [x, y] = calcEquirectPx(light.direction, width, height);
    ctx.fillStyle = light.color;
    ctx.fillRect(x - (light.widthShare * width) / 2, y - (light.heightShare * height) / 2, light.widthShare * width, light.heightShare * height);
  }
  ctx.fillStyle = STUDIO_STRIP_COLOR;
  ctx.fillRect(0, height * STUDIO_STRIP_TOP_SHARE, width, height * STUDIO_STRIP_HEIGHT_SHARE);
  const texture = new THREE.CanvasTexture(canvas);
  texture.mapping = THREE.EquirectangularReflectionMapping;
  texture.colorSpace = THREE.SRGBColorSpace;
  return texture;
}

/** Where `direction` lands in an equirectangular image, as three samples one. */
function calcEquirectPx(direction: THREE.Vector3, width: number, height: number): [number, number] {
  const unit = direction.clone().normalize();
  const u = Math.atan2(unit.z, unit.x) / (2 * Math.PI) + 0.5;
  const v = Math.asin(unit.y) / Math.PI + 0.5;
  return [u * width, (1 - v) * height];
}

/**
 * Every mip level of the prefiltered cube `target`, largest first, each as its six faces of 8-bit
 * sRGB texels, laid out as three uploads a cube texture from images: three mirrors such a texture
 * across x when sampling it, unlike its own render target, so each face is stored mirrored, with +x
 * and -x swapped, which samples the same.
 */
function readCubeLevels(target: THREE.WebGLCubeRenderTarget): Uint8Array[][] {
  const gl = renderer.getContext() as WebGL2RenderingContext;
  const texture = (renderer.properties.get(target.texture) as { __webglTexture: WebGLTexture }).__webglTexture;
  const framebuffer = gl.createFramebuffer();
  gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
  const levels: Uint8Array[][] = [];
  for (let level = 0; level < target.texture.mipmaps.length; level += 1) {
    const sizePx = target.width >> level;
    const linear = new Float32Array(sizePx * sizePx * RGBA);
    const faces: Uint8Array[] = [];
    for (let face = 0; face < CUBE_FACES; face += 1) {
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_CUBE_MAP_POSITIVE_X + face, texture, level);
      gl.readPixels(0, 0, sizePx, sizePx, gl.RGBA, gl.FLOAT, linear);
      faces.push(toMirroredSrgb(linear, sizePx));
    }
    [faces[0], faces[1]] = [faces[1], faces[0]];
    levels.push(faces);
  }
  gl.bindFramebuffer(gl.FRAMEBUFFER, null);
  gl.deleteFramebuffer(framebuffer);
  return levels;
}

function toMirroredSrgb(linear: Float32Array, sizePx: number): Uint8Array {
  const srgb = new Uint8Array(linear.length);
  for (let y = 0; y < sizePx; y += 1) {
    for (let x = 0; x < sizePx; x += 1) {
      const from = (y * sizePx + x) * RGBA;
      const to = (y * sizePx + sizePx - 1 - x) * RGBA;
      for (let channel = 0; channel < 3; channel += 1) srgb[to + channel] = Math.round(THREE.MathUtils.clamp(toSrgb(linear[from + channel]), 0, 1) * 255);
      srgb[to + 3] = 255;
    }
  }
  return srgb;
}

function toSrgb(linear: number): number {
  return linear <= 0.0031308 ? linear * 12.92 : 1.055 * Math.pow(linear, 1 / 2.4) - 0.055;
}

