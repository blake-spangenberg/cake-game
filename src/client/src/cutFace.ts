/**
 * The inside of a cut cake: level sponge layers between thin bands of buttercream, every other one
 * with a thin stripe of jam under it, laid out in the cake model's own frame so they stay as baked
 * whatever way the cake was set down or cut.
 */
import * as THREE from "three";

/**
 * Yellow vanilla sponge, deep enough that white buttercream stands out against it even facing the
 * key light, which washes a paler sponge out to the buttercream's white, and warm enough that the
 * blue fill light does not turn it olive.
 */
const SPONGE = 0xe6b440;
const BUTTERCREAM = 0xfff8ec;
/** Raspberry jam, deep enough to read as red rather than pink between golden sponge and white cream. */
const JAM = 0x9a1424;
/** One sponge layer and the buttercream on it, as a layer cake is built. */
const SPONGE_M = 0.0133;
const BUTTERCREAM_M = 0.0033;
const LAYER_PITCH_M = SPONGE_M + BUTTERCREAM_M;
/** The jam spread on every other sponge layer before its buttercream. */
const JAM_M = 0.0013;
/**
 * The crumb's air cells in three sizes, each seeded one to a lattice cell of this size with a share
 * of them present: a few holes of 1-4 mm, many under 2 mm, and a fine grain under a millimeter.
 */
const BIG_PORE_CELL_M = 0.004;
const BIG_PORE_SHARE = 0.45;
const PORE_CELL_M = 0.0015;
const PORE_SHARE = 0.9;
const FINE_PORE_CELL_M = 0.0006;
const FINE_PORE_SHARE = 1.0;
/** A pore size's average shade over a face where all its pores are present, which it draws as once too fine for its pixel. */
const MEAN_PORE_SHADE = 0.17;
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
        .replace("#include <roughnessmap_fragment>", `#include <roughnessmap_fragment>\n${FRAGMENT_ROUGHNESS}`)
        .replace("#include <normal_fragment_maps>", `#include <normal_fragment_maps>\n${FRAGMENT_BUMP}`);
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

vec4 hash4CutFace(vec3 p) {
  vec4 p4 = fract(p.xyzx * vec4(0.1031, 0.1030, 0.0973, 0.1099));
  p4 += dot(p4, p4.wzxy + 33.33);
  return fract((p4.xxyz + p4.yzzw) * p4.zywx);
}

// How deep the crumb's air cells reach at p, 0 on the crumb wall to 1 at a cell's center, for cells
// seeded one to a lattice cell of cellM, a share of them present, each a radius up to half the cell.
// Since no cell reaches past half a lattice cell, the two nearest lattice cells per axis hold every
// cell that can cover p, so cells may overlap and merge as real crumb's do. Cells stand taller than
// wide, as the batter rose. The edge blurs over the
// pixel's footprint so it does not crawl.
float poresCutFace(vec3 p, float cellM, float presentShare, float footM) {
  vec3 q = p / (cellM * vec3(1.0, 1.35, 1.0));
  vec3 base = floor(q);
  vec3 nearSide = step(0.5, q - base) - 1.0;
  float edge = max(footM / cellM, 0.02);
  float depth = 0.0;
  for (int i = 0; i < 8; i++) {
    vec3 cell = base + nearSide + vec3(float(i & 1), float((i >> 1) & 1), float((i >> 2) & 1));
    vec4 h = hash4CutFace(cell);
    float radius = mix(0.3, 0.5, h.w);
    vec3 center = cell + h.xyz;
    float present = step(fract(h.w * 7.31 + h.x), presentShare);
    float fromCenter = length(q - center) / radius;
    depth = max(depth, present * (1.0 - smoothstep(1.0 - edge / radius, 1.0, fromCenter)) * max(1.0 - fromCenter * fromCenter, 0.0));
  }
  return depth;
}

// The normal of a surface raised heightM along it, from the height's change across the pixel.
vec3 bumpCutFace(vec3 viewPosition, vec3 surfaceNormal, float heightM, float faceDirection) {
  vec3 sigmaX = dFdx(viewPosition);
  vec3 sigmaY = dFdy(viewPosition);
  vec3 r1 = cross(sigmaY, surfaceNormal);
  vec3 r2 = cross(surfaceNormal, sigmaX);
  float det = dot(sigmaX, r1) * faceDirection;
  vec3 gradient = sign(det) * (dFdx(heightM) * r1 + dFdy(heightM) * r2);
  return normalize(abs(det) * surfaceNormal - gradient);
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
float cutFaceHeightM;
{
  const float bigPoreCellM = ${BIG_PORE_CELL_M.toFixed(4)};
  const float bigPoreShare = ${BIG_PORE_SHARE.toFixed(2)};
  const float poreCellM = ${PORE_CELL_M.toFixed(4)};
  const float poreShare = ${PORE_SHARE.toFixed(2)};
  const float finePoreCellM = ${FINE_PORE_CELL_M.toFixed(4)};
  const float finePoreShare = ${FINE_PORE_SHARE.toFixed(2)};
  const float meanPoreShade = ${MEAN_PORE_SHADE.toFixed(2)};
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
  float spongeShare = 1.0 - cutFaceButtercream - cutFaceJam;
  // A pore size draws its shade until its lattice cell spans about two pixels, then its average.
  // Its relief fades sooner: a slope only a few pixels wide turns the bump's normal to noise.
  float bigShown = 1.0 - smoothstep(0.35 * bigPoreCellM, 0.9 * bigPoreCellM, faceFootM);
  float poreShown = 1.0 - smoothstep(0.35 * poreCellM, 0.9 * poreCellM, faceFootM);
  float finePoreShown = 1.0 - smoothstep(0.35 * finePoreCellM, 0.9 * finePoreCellM, faceFootM);
  float bigRelief = 1.0 - smoothstep(0.1 * bigPoreCellM, 0.3 * bigPoreCellM, faceFootM);
  float poreRelief = 1.0 - smoothstep(0.1 * poreCellM, 0.3 * poreCellM, faceFootM);
  float finePoreRelief = 1.0 - smoothstep(0.1 * finePoreCellM, 0.3 * finePoreCellM, faceFootM);
  bool isSponge = spongeShare > 0.0;
  float bigPores = isSponge && bigShown > 0.0 ? poresCutFace(p, bigPoreCellM, bigPoreShare, faceFootM) : 0.0;
  float pores = isSponge && poreShown > 0.0 ? poresCutFace(p + 0.37, poreCellM, poreShare, faceFootM) : 0.0;
  float finePores = isSponge && finePoreShown > 0.0 ? poresCutFace(p + 0.71, finePoreCellM, finePoreShare, faceFootM) : 0.0;
  float poreShade = max(max(
    mix(meanPoreShade * bigPoreShare, sqrt(bigPores), bigShown),
    mix(meanPoreShade * poreShare, sqrt(pores), poreShown)),
    mix(meanPoreShade * finePoreShare, sqrt(finePores), finePoreShown));
  float tornM = (noiseCutFace(p * 1500.0) - 0.5) * 0.00012;
  cutFaceHeightM = spongeShare * (tornM * finePoreRelief - bigPores * bigRelief * 0.0008 - pores * poreRelief * 0.00035 - finePores * finePoreRelief * 0.00015);
  float bake = noiseCutFace(p * 25.0);
  float speckle = (noiseCutFace(p * 2200.0) - 0.5) * finePoreShown;
  // Crumb grain a few millimeters across, which still shows where single pores no longer do.
  float grain = (noiseCutFace(p * 350.0) - 0.5) * (1.0 - smoothstep(0.0015, 0.004, faceFootM));
  vec3 crumbWall = sponge * (0.94 + 0.12 * bake) * (1.0 + 0.1 * speckle + 0.16 * grain);
  vec3 spongeColor = mix(crumbWall, sponge * vec3(0.62, 0.55, 0.46), poreShade);
  vec3 creamColor = buttercream * (0.97 + 0.03 * noiseCutFace(p * 300.0));
  // Darker at its edges, where it soaks into the sponge and lies thinner, and brighter in its depth.
  float jamDepth = 1.0 - abs(fromBandM + halfCreamM + jamThickM * 0.5) / max(jamThickM * 0.5, 1e-6);
  vec3 jamColor = jam * (0.75 + 0.4 * mix(0.5, clamp(jamDepth, 0.0, 1.0), fineFade));
  diffuseColor.rgb *= spongeColor * spongeShare + creamColor * cutFaceButtercream + jamColor * cutFaceJam;
}
`;

const FRAGMENT_BUMP = /* glsl */ `
normal = bumpCutFace(-vViewPosition, normal, cutFaceHeightM, faceDirection);
`;

const FRAGMENT_ROUGHNESS = /* glsl */ `
roughnessFactor = ${SPONGE_ROUGHNESS.toFixed(2)} * (1.0 - cutFaceButtercream - cutFaceJam) + ${BUTTERCREAM_ROUGHNESS.toFixed(2)} * cutFaceButtercream + ${JAM_ROUGHNESS.toFixed(2)} * cutFaceJam;
`;
