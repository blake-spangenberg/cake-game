/**
 * The inside of a cut cake: level sponge layers between thin bands of buttercream, laid out in the
 * cake model's own frame so they stay as baked whatever way the cake was set down or cut.
 */
import * as THREE from "three";

/**
 * Golden vanilla sponge, deep enough that white buttercream stands out against it even facing the
 * key light, which washes a paler sponge out to the buttercream's white.
 */
const SPONGE = 0xc08540;
const BUTTERCREAM = 0xfff8ec;
/** One sponge layer and the buttercream on it, as a layer cake is built. */
const LAYER_PITCH_M = 0.034;
const BUTTERCREAM_M = 0.0075;
const SPONGE_ROUGHNESS = 0.92;
const BUTTERCREAM_ROUGHNESS = 0.5;

/**
 * A cut face's material for a plane masked by the stencil to where the cake's inside shows. Before
 * it draws, `setCake` gives it the half's mesh, whose model frame the layers lie level in.
 */
export class CutFaceMaterial extends THREE.MeshStandardMaterial {
  private readonly cakeFromWorld = { value: new THREE.Matrix4() };
  private readonly cakeBottomY = { value: 0 };

  constructor(parameters: THREE.MeshStandardMaterialParameters) {
    super({ ...parameters, color: 0xffffff });
    const uniforms = {
      cakeFromWorld: this.cakeFromWorld,
      cakeBottomY: this.cakeBottomY,
      sponge: { value: new THREE.Color(SPONGE) },
      buttercream: { value: new THREE.Color(BUTTERCREAM) },
    };
    // The same source for every instance, so they share one shader program, with uniforms each their own.
    this.onBeforeCompile = (shader) => {
      Object.assign(shader.uniforms, uniforms);
      shader.vertexShader = shader.vertexShader
        .replace("#include <common>", `#include <common>\n${VERTEX_DECLARATIONS}`)
        .replace("#include <project_vertex>", `#include <project_vertex>\n${VERTEX_LAYERS}`);
      shader.fragmentShader = shader.fragmentShader
        .replace("#include <common>", `#include <common>\n${FRAGMENT_DECLARATIONS}`)
        .replace("#include <color_fragment>", `#include <color_fragment>\n${FRAGMENT_LAYERS}`)
        .replace("#include <roughnessmap_fragment>", `#include <roughnessmap_fragment>\n${FRAGMENT_ROUGHNESS}`);
    };
  }

  /** Lays the layers level in `cakeMesh`'s model frame, from its lowest point at `bottomY` up, as of its current world matrix. */
  setCake(cakeMesh: THREE.Object3D, bottomY: number): void {
    this.cakeFromWorld.value.copy(cakeMesh.matrixWorld).invert();
    this.cakeBottomY.value = bottomY;
  }
}

const VERTEX_DECLARATIONS = /* glsl */ `
uniform mat4 cakeFromWorld;
varying vec3 vCakePosition;
`;

const VERTEX_LAYERS = /* glsl */ `
vCakePosition = (cakeFromWorld * modelMatrix * vec4(transformed, 1.0)).xyz;
`;

const FRAGMENT_DECLARATIONS = /* glsl */ `
uniform float cakeBottomY;
uniform vec3 sponge;
uniform vec3 buttercream;
varying vec3 vCakePosition;

float hashCutFace(vec3 p) {
  p = fract(p * 0.3183099 + 0.1);
  p *= 17.0;
  return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
}

float noiseCutFace(vec3 p) {
  vec3 i = floor(p);
  vec3 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(
    mix(mix(hashCutFace(i), hashCutFace(i + vec3(1, 0, 0)), f.x), mix(hashCutFace(i + vec3(0, 1, 0)), hashCutFace(i + vec3(1, 1, 0)), f.x), f.y),
    mix(mix(hashCutFace(i + vec3(0, 0, 1)), hashCutFace(i + vec3(1, 0, 1)), f.x), mix(hashCutFace(i + vec3(0, 1, 1)), hashCutFace(i + vec3(1, 1, 1)), f.x), f.y),
    f.z);
}
`;

// Bands sit at whole multiples of the pitch after shifting the bottom by one sponge layer, each
// band's thickness and height wandering a little so the layers look spread by hand. Detail finer
// than about two pixels fades to its average, so a distant or grazing face does not shimmer.
const FRAGMENT_LAYERS = /* glsl */ `
float cutFaceButtercream;
{
  const float pitchM = ${LAYER_PITCH_M.toFixed(4)};
  const float bandM = ${BUTTERCREAM_M.toFixed(4)};
  vec3 p = vCakePosition;
  float wanderM = (noiseCutFace(p * 40.0) - 0.5) * 0.004 + (noiseCutFace(p * 140.0) - 0.5) * 0.0008;
  float heightM = p.y - cakeBottomY - (pitchM - bandM / 2.0) + wanderM;
  float band = floor(heightM / pitchM + 0.5);
  float fromBandM = abs(heightM - band * pitchM);
  float halfBandM = bandM * (0.4 + 0.25 * hashCutFace(vec3(band, 3.7, 1.3)));
  float pixelM = fwidth(heightM);
  float edge = smoothstep(halfBandM - pixelM, halfBandM + pixelM, fromBandM);
  float bandShare = 2.0 * halfBandM / pitchM;
  cutFaceButtercream = mix(1.0 - edge, bandShare, smoothstep(0.3, 0.6, pixelM / halfBandM));
  float crumbFade = 1.0 - smoothstep(0.0004, 0.0012, pixelM);
  float crumb = noiseCutFace(p * 900.0);
  float pores = smoothstep(0.6, 0.85, crumb) * crumbFade;
  float bake = noiseCutFace(p * 25.0);
  vec3 spongeColor = sponge * (0.92 + 0.12 * bake) * (1.0 - 0.28 * pores) * (1.0 + 0.06 * (crumb - 0.5) * crumbFade);
  vec3 creamColor = buttercream * (0.97 + 0.03 * noiseCutFace(p * 300.0));
  diffuseColor.rgb *= mix(spongeColor, creamColor, cutFaceButtercream);
}
`;

const FRAGMENT_ROUGHNESS = /* glsl */ `
roughnessFactor = mix(${SPONGE_ROUGHNESS.toFixed(2)}, ${BUTTERCREAM_ROUGHNESS.toFixed(2)}, cutFaceButtercream);
`;
