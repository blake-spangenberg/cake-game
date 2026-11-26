/**
 * The inside of a cut cake: level sponge layers between thin bands of buttercream, every other one
 * with a thin stripe of jam under it, laid out in the cake model's own frame so they stay as baked
 * whatever way the cake was set down or cut.
 */
import * as THREE from "three";

/**
 * Golden vanilla sponge, deep enough that white buttercream stands out against it even facing the
 * key light, which washes a paler sponge out to the buttercream's white.
 */
const SPONGE = 0xc08540;
const BUTTERCREAM = 0xfff8ec;
/** Raspberry jam, deep enough to read as red rather than pink between golden sponge and white cream. */
const JAM = 0x9a1424;
/** One sponge layer and the buttercream on it, as a layer cake is built. */
const SPONGE_M = 0.0133;
const BUTTERCREAM_M = 0.005;
const LAYER_PITCH_M = SPONGE_M + BUTTERCREAM_M;
/** The jam spread on every other sponge layer before its buttercream. */
const JAM_M = 0.002;
const SPONGE_ROUGHNESS = 0.92;
const BUTTERCREAM_ROUGHNESS = 0.5;
/** Glossier than the cream, as jam's wet surface is. */
const JAM_ROUGHNESS = 0.3;

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
      jam: { value: new THREE.Color(JAM) },
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
uniform vec3 jam;
varying vec3 vCakePosition;

// The share of a pixel's footprint, footM wide around offsetM, that lies between loM and hiM.
float coverCutFace(float offsetM, float footM, float loM, float hiM) {
  return clamp((min(offsetM + footM * 0.5, hiM) - max(offsetM - footM * 0.5, loM)) / footM, 0.0, 1.0);
}

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
// band's thickness and height wandering a little so the layers look spread by hand, and even bands
// carry jam just under their cream. Each pixel shows the share of its footprint the nearest band's
// cream and jam cover, a box filter that keeps a band a few pixels wide crisp and blends one under a
// pixel into a faint line rather than letting it shimmer. Only a footprint reaching the next band
// fades to the layers' average. Texture finer than a pixel fades by the footprint on the face.
const FRAGMENT_LAYERS = /* glsl */ `
float cutFaceButtercream;
float cutFaceJam;
{
  const float pitchM = ${LAYER_PITCH_M.toFixed(4)};
  const float bandM = ${BUTTERCREAM_M.toFixed(4)};
  const float jamM = ${JAM_M.toFixed(4)};
  vec3 p = vCakePosition;
  float faceFootM = length(fwidth(p));
  float fineFade = 1.0 - smoothstep(0.0004, 0.0012, faceFootM);
  float wanderM = (noiseCutFace(p * 40.0) - 0.5) * 0.002 + (noiseCutFace(p * 140.0) - 0.5) * 0.0004 * (1.0 - smoothstep(0.001, 0.003, faceFootM));
  float heightM = p.y - cakeBottomY - (pitchM - bandM / 2.0) + wanderM;
  float band = floor(heightM / pitchM + 0.5);
  float fromBandM = heightM - band * pitchM;
  float halfCreamM = bandM * (0.4 + 0.25 * hashCutFace(vec3(band, 3.7, 1.3)));
  float jamThickM = jamM * (0.7 + 0.6 * hashCutFace(vec3(band, 5.1, 2.9))) * step(mod(band, 2.0), 0.5);
  float footM = max(fwidth(heightM), 1e-6);
  float farShare = smoothstep(0.4 * pitchM, 0.8 * pitchM, footM);
  cutFaceButtercream = mix(coverCutFace(fromBandM, footM, -halfCreamM, halfCreamM), 2.0 * halfCreamM / pitchM, farShare);
  cutFaceJam = mix(coverCutFace(fromBandM, footM, -halfCreamM - jamThickM, -halfCreamM), 0.5 * jamM / pitchM, farShare);
  float crumb = noiseCutFace(p * 900.0);
  float pores = smoothstep(0.6, 0.85, crumb) * fineFade;
  float bake = noiseCutFace(p * 25.0);
  vec3 spongeColor = sponge * (0.92 + 0.12 * bake) * (1.0 - 0.28 * pores) * (1.0 + 0.06 * (crumb - 0.5) * fineFade);
  vec3 creamColor = buttercream * (0.97 + 0.03 * noiseCutFace(p * 300.0));
  // Darker at its edges, where it soaks into the sponge and lies thinner, and brighter in its depth.
  float jamDepth = 1.0 - abs(fromBandM + halfCreamM + jamThickM * 0.5) / max(jamThickM * 0.5, 1e-6);
  vec3 jamColor = jam * (0.75 + 0.4 * mix(0.5, clamp(jamDepth, 0.0, 1.0), fineFade));
  float spongeShare = 1.0 - cutFaceButtercream - cutFaceJam;
  diffuseColor.rgb *= spongeColor * spongeShare + creamColor * cutFaceButtercream + jamColor * cutFaceJam;
}
`;

const FRAGMENT_ROUGHNESS = /* glsl */ `
roughnessFactor = ${SPONGE_ROUGHNESS.toFixed(2)} * (1.0 - cutFaceButtercream - cutFaceJam) + ${BUTTERCREAM_ROUGHNESS.toFixed(2)} * cutFaceButtercream + ${JAM_ROUGHNESS.toFixed(2)} * cutFaceJam;
`;
