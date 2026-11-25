/**
 * The table, cake, stand, knife, and lights, as the player's own scene, the opponents' tiles, and
 * the lobby's cake previews draw them.
 */
import * as THREE from "three";
import { mergeGeometries } from "three/addons/utils/BufferGeometryUtils.js";
import { type Game, type Layout, get_blade_outline, get_handle_centers, get_stand_parts } from "@wasm/cake_wasm.js";
import { type WoodLook, buildWoodTexture, setWoodUvs } from "./wood";

/** The color of a cake with no texture of its own. */
export const FROSTING = 0xf3e3ec;
const RIM_BEAD_COUNT = 16;
const SKY_INTENSITY = 0.7;
/** The knife's edge always runs along the world's z axis: the cake turns on its stand instead (`Knife::calc_frame`). */
const KNIFE_EDGE = new THREE.Vector3(0, 0, 1);

/** Polished stainless steel: a near-white base color, since a metal's color is its reflectance. */
const STEEL = 0xe4e7ea;
const STEEL_ROUGHNESS = 0.14;
const STEEL_ENV_INTENSITY = 1.4;
/** How far the blade's back dips at its middle below its ends, where the handles stand, as in the reference's gentle crescent. */
const BLADE_BACK_SAG_M = 0.02;
/** Height of the bevel ground along the edge, and the edge's own half thickness, a hair so it catches light. */
const BLADE_BEVEL_M = 0.007;
const EDGE_HALF_THICKNESS_M = 0.00015;
/** The ferrule's height and how far it sinks into the blade's back. Its radius is the core's handle radius. */
const FERRULE_LEN_M = 0.016;
const FERRULE_SINK_M = 0.001;
/**
 * The knob's outline from its foot up, as (share of its height, radius in meters): a collar over
 * the ferrule, a waist, and a bulb, after the reference. The bulb is wider than the core's
 * handle, which only a held cake collides with, so a held cake could brush into its widest part.
 */
const KNOB_PROFILE: readonly (readonly [number, number])[] = [
  [0, 0.012],
  [0.04, 0.0155],
  [0.09, 0.0175],
  [0.15, 0.0158],
  [0.25, 0.0128],
  [0.34, 0.0124],
  [0.46, 0.0152],
  [0.6, 0.0192],
  [0.72, 0.0212],
  [0.82, 0.0208],
  [0.9, 0.0184],
  [0.96, 0.0128],
  [0.99, 0.006],
  [1, 0],
];
const KNOB_PROFILE_SEGMENTS = 40;
const KNOB_TURN_SEGMENTS = 32;
const KNOB_ROUGHNESS = 0.55;
/** Pale beech. */
const KNOB_WOOD: WoodLook = {
  widthPx: 256,
  heightPx: 256,
  lengthM: 0.08,
  breadthM: 0.14,
  boards: 1,
  ringsPerM: 130,
  ringWander: 0.6,
  earlywood: 0xe9cda8,
  latewood: 0xdab98f,
  boardToneSpread: 0,
  seed: 7,
};
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
/** Softboxes in the directions of `addLights`' key and fill lights. */
const STUDIO_LIGHTS = [
  { direction: new THREE.Vector3(1.7, 1.2, 0.6), widthShare: 0.14, heightShare: 0.16, color: "#fff6ea" },
  { direction: new THREE.Vector3(-1.2, 0.9, -0.9), widthShare: 0.1, heightShare: 0.12, color: "#c9d6ff" },
];
const STUDIO_STRIP_COLOR = "#d8d6d2";
const STUDIO_STRIP_TOP_SHARE = 0.16;
const STUDIO_STRIP_HEIGHT_SHARE = 0.05;

const TABLE_THICKNESS_M = 0.04;
const LEG_SIDE_M = 0.06;
const LEG_INSET_M = 0.08;
const LEG_LEN_M = 0.72;
const LEG_CORNERS = [
  [1, 1],
  [1, -1],
  [-1, 1],
  [-1, -1],
] as const;
const TABLE_ROUGHNESS = 0.6;
/** Walnut-toned boards. Its size is the table top's, set by `buildTable`. */
const TABLE_WOOD: Omit<WoodLook, "lengthM" | "breadthM"> = {
  widthPx: 2048,
  heightPx: 1280,
  boards: 5,
  ringsPerM: 85,
  ringWander: 3,
  earlywood: 0x8c5e3f,
  latewood: 0x633e26,
  boardToneSpread: 0.1,
  seed: 11,
};

/**
 * The cake's mesh from its vertex `positions`, each triangle with its own corners so texture seams
 * stay sharp, shaded smoothly but for creases by the normals baked into the cake file.
 */
export function buildCakeGeometry(game: Game, positions = game.get_cake_mesh_positions()): THREE.BufferGeometry {
  const indexed = new THREE.BufferGeometry();
  indexed.setAttribute("position", new THREE.BufferAttribute(positions, 3));
  indexed.setIndex(new THREE.BufferAttribute(game.get_cake_mesh_triangles(), 1));
  const corners = indexed.toNonIndexed();
  const uvs = game.get_cake_corner_uvs();
  if (uvs.length > 0) corners.setAttribute("uv", new THREE.BufferAttribute(uvs, 2));
  corners.setAttribute("normal", new THREE.BufferAttribute(game.get_cake_corner_normals(), 3));
  return corners;
}

/**
 * The cake's texture at full size, mipmapped and as sharp at grazing angles as `anisotropy` allows,
 * decoded off the main thread, or none for a plain cake. Asking `createImageBitmap` to also scale
 * it down blocked the main thread for 120-150 ms on a 2048 texture, longer than its upload takes.
 */
export async function decodeCakeTexture(game: Game, anisotropy: number): Promise<THREE.Texture | null> {
  const file = game.get_cake_texture();
  if (file.length === 0) return null;
  const blob = new Blob([new Uint8Array(file)], { type: "image/webp" });
  const bitmap = await createImageBitmap(blob, { colorSpaceConversion: "none", premultiplyAlpha: "none" });
  const texture = new THREE.Texture(bitmap);
  texture.flipY = false;
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.anisotropy = anisotropy;
  texture.needsUpdate = true;
  return texture;
}

/** The cake's skin: `texture` from `decodeCakeTexture`, or plain frosting for none. */
export function buildCakeMaterial(texture: THREE.Texture | null): THREE.MeshStandardMaterial {
  return new THREE.MeshStandardMaterial(texture ? { map: texture } : { color: FROSTING });
}

/** Plate, column, and base, with beads around the rim so its turning shows. */
export function buildStand(layout: Layout): THREE.Group {
  const stand = new THREE.Group();
  const ceramic = new THREE.MeshStandardMaterial({ color: 0xf2efe8, roughness: 0.35 });
  const parts = get_stand_parts();
  for (let i = 0; i < parts.length; i += 3) {
    const [radiusM, y0M, y1M] = parts.slice(i, i + 3);
    const part = new THREE.Mesh(new THREE.CylinderGeometry(radiusM, radiusM, y1M - y0M, 64).translate(0, (y0M + y1M) / 2, 0), ceramic);
    part.castShadow = true;
    part.receiveShadow = true;
    stand.add(part);
  }
  // One mesh for every bead, since each opponent's tile draws the stand again and draw calls add up.
  const beads = Array.from({ length: RIM_BEAD_COUNT }, (_, idx) => {
    const angle = (2 * Math.PI * idx) / RIM_BEAD_COUNT;
    const bead = new THREE.SphereGeometry(layout.stand_plate_thickness_m * 0.45, 12, 8);
    return bead.translate(Math.cos(angle) * layout.stand_radius_m, layout.stand_top_y_m - layout.stand_plate_thickness_m / 2, Math.sin(angle) * layout.stand_radius_m);
  });
  stand.add(new THREE.Mesh(mergeGeometries(beads), new THREE.MeshStandardMaterial({ color: 0xc9a86a, roughness: 0.4, metalness: 0.5 })));
  return stand;
}

/**
 * A mezzaluna after `.agent/doc/knife-reference.jpg`: a broad polished steel blade whose edge is
 * the core's arc exactly, ground to a bevel along it, under a gently curved back, and at each end
 * a steel ferrule topped by a pear-shaped beech knob. Its matrix is set by `placeKnife`.
 */
export function buildKnife(layout: Layout): THREE.Group {
  const knife = new THREE.Group();
  knife.matrixAutoUpdate = false;
  const steel = new THREE.MeshStandardMaterial({
    color: STEEL,
    metalness: 1,
    roughness: STEEL_ROUGHNESS,
    envMap: buildStudioEnvironment(),
    envMapIntensity: STEEL_ENV_INTENSITY,
  });
  const knobWood = new THREE.MeshStandardMaterial({ map: buildWoodTexture(KNOB_WOOD, 1), roughness: KNOB_ROUGHNESS });
  const steelParts = [buildBladeGeometry(layout)];
  const knobs: THREE.BufferGeometry[] = [];
  const centers = get_handle_centers();
  const handleTopM = layout.blade_back_m + layout.handle_len_m;
  for (let i = 0; i < centers.length; i += 3) {
    const [xM, zM] = [centers[i], centers[i + 2]];
    const ferruleBottomM = calcBladeBackM(layout, xM) - FERRULE_SINK_M;
    const ferruleTopM = ferruleBottomM + FERRULE_LEN_M;
    const ferrule = new THREE.CylinderGeometry(layout.handle_radius_m, layout.handle_radius_m, FERRULE_LEN_M, 32);
    ferrule.deleteAttribute("uv");
    steelParts.push(ferrule.translate(xM, (ferruleBottomM + ferruleTopM) / 2, zM));
    knobs.push(buildKnobGeometry(handleTopM - ferruleTopM).translate(xM, ferruleTopM, zM));
  }
  // One mesh per material, since each opponent's tile draws the knife again and draw calls add up.
  for (const mesh of [new THREE.Mesh(mergeGeometries(steelParts), steel), new THREE.Mesh(mergeGeometries(knobs), knobWood)]) {
    mesh.castShadow = true;
    knife.add(mesh);
  }
  return knife;
}

/** Height of the blade's curved back above the edge's lowest point at `xM` along it. */
function calcBladeBackM(layout: Layout, xM: number): number {
  const halfSpanM = layout.blade_half_span_m;
  const backRadiusM = (BLADE_BACK_SAG_M * BLADE_BACK_SAG_M + halfSpanM * halfSpanM) / (2 * BLADE_BACK_SAG_M);
  return layout.blade_back_m - BLADE_BACK_SAG_M + backRadiusM - Math.sqrt(backRadiusM * backRadiusM - xM * xM);
}

/**
 * The blade as a closed solid, its cross-section a loop from the edge up one face, over the back,
 * and down the other face. Its edge runs through the core's outline exactly, and its back never
 * rises above the core's straight one, so the blade drawn never reaches past the blade the core
 * cuts and collides with.
 */
function buildBladeGeometry(layout: Layout): THREE.BufferGeometry {
  const outline = get_blade_outline();
  // The outline runs along the edge, then back along the straight back's two corners.
  const edgeLen = outline.length / 2 - 2;
  const faceM = layout.blade_thickness_m / 2;
  const sections = [
    (x: number, edgeM: number) => new THREE.Vector3(x, edgeM, EDGE_HALF_THICKNESS_M),
    (x: number, edgeM: number) => new THREE.Vector3(x, edgeM + BLADE_BEVEL_M, faceM),
    (x: number) => new THREE.Vector3(x, calcBladeBackM(layout, x), faceM),
    (x: number) => new THREE.Vector3(x, calcBladeBackM(layout, x), -faceM),
    (x: number, edgeM: number) => new THREE.Vector3(x, edgeM + BLADE_BEVEL_M, -faceM),
    (x: number, edgeM: number) => new THREE.Vector3(x, edgeM, -EDGE_HALF_THICKNESS_M),
  ];
  const rows = sections.map((section) => Array.from({ length: edgeLen }, (_, i) => section(outline[2 * i], outline[2 * i + 1])));
  const strips = rows.map((row, idx) => buildStrip(row, rows[(idx + 1) % rows.length]));
  const ends = [buildFan(rows.map((row) => row[0])), buildFan(rows.map((row) => row[edgeLen - 1]).reverse())];
  return mergeGeometries([...strips, ...ends]);
}

/**
 * The surface between two rows of points running the same way along the blade, shaded smoothly
 * along it. Facing outward needs `below` before `above` going round the cross-section's loop.
 */
function buildStrip(below: THREE.Vector3[], above: THREE.Vector3[]): THREE.BufferGeometry {
  const geometry = new THREE.BufferGeometry().setFromPoints([...below, ...above]);
  const indices: number[] = [];
  const len = below.length;
  for (let i = 0; i + 1 < len; i += 1) indices.push(i, i + 1, len + i, i + 1, len + i + 1, len + i);
  geometry.setIndex(indices);
  geometry.computeVertexNormals();
  return geometry;
}

/** A flat convex polygon as a fan of triangles, facing the way its points wind counterclockwise. */
function buildFan(points: THREE.Vector3[]): THREE.BufferGeometry {
  const geometry = new THREE.BufferGeometry().setFromPoints(points);
  const indices: number[] = [];
  for (let i = 1; i + 1 < points.length; i += 1) indices.push(0, i, i + 1);
  geometry.setIndex(indices);
  geometry.computeVertexNormals();
  return geometry;
}

/**
 * A turned pear-shaped knob `lenM` tall standing on the origin: a small collar, a waist, then the
 * bulb. Its texture's first coordinate runs up the knob, the way the wood's grain runs.
 */
function buildKnobGeometry(lenM: number): THREE.LatheGeometry {
  const profile = new THREE.SplineCurve(KNOB_PROFILE.map(([heightShare, radiusM]) => new THREE.Vector2(radiusM, heightShare * lenM)));
  const knob = new THREE.LatheGeometry(profile.getPoints(KNOB_PROFILE_SEGMENTS), KNOB_TURN_SEGMENTS);
  const uvs = knob.getAttribute("uv");
  for (let i = 0; i < uvs.count; i += 1) uvs.setXY(i, uvs.getY(i), uvs.getX(i));
  return knob;
}

/**
 * A small photo studio as an equirectangular image for the steel to reflect: a softbox where the
 * key light shines from, a cooler one at the fill, a bright strip overhead, gray walls, and the
 * table's brown below the horizon. Each renderer turns it into its own prefiltered map on first use.
 */
function buildStudioEnvironment(): THREE.Texture {
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

/** The table top and legs in glued-up boards of wood, its grain whole across the top with no repeat. */
export function buildTable(layout: Layout, anisotropy: number): THREE.Group {
  const { table_half_x_m: halfX, table_half_z_m: halfZ } = layout;
  const look = { ...TABLE_WOOD, lengthM: 2 * halfX, breadthM: 2 * halfZ };
  const wood = new THREE.MeshStandardMaterial({ map: buildWoodTexture(look, anisotropy), roughness: TABLE_ROUGHNESS });
  const table = new THREE.Group();
  const topGeometry = new THREE.BoxGeometry(2 * halfX, TABLE_THICKNESS_M, 2 * halfZ).translate(0, -TABLE_THICKNESS_M / 2, 0);
  setWoodUvs(topGeometry, look, 0);
  const top = new THREE.Mesh(topGeometry, wood);
  top.castShadow = true;
  top.receiveShadow = true;
  table.add(top);
  const legGeometry = new THREE.BoxGeometry(LEG_SIDE_M, LEG_LEN_M, LEG_SIDE_M);
  setWoodUvs(legGeometry, look, 1);
  for (const [sx, sz] of LEG_CORNERS) {
    const leg = new THREE.Mesh(legGeometry, wood);
    leg.position.set(sx * (halfX - LEG_INSET_M), -TABLE_THICKNESS_M - LEG_LEN_M / 2, sz * (halfZ - LEG_INSET_M));
    leg.castShadow = true;
    table.add(leg);
  }
  return table;
}

/**
 * Poses `knife` tilted by `tiltRad`, its edge lowered `lowerM` below the pivot within its plane,
 * and rocked by `rockRad` as its curved edge rolls along the line it rests on without slipping.
 */
export function placeKnife(knife: THREE.Group, layout: Layout, tiltRad: number, lowerM: number, rockRad: number): void {
  const down = calcKnifeDown(tiltRad);
  const up = down.clone().negate();
  const edgeMiddle = new THREE.Vector3(0, layout.pivot_y_m, 0).addScaledVector(down, lowerM);
  const arcCenter = new THREE.Vector3(0, layout.blade_arc_radius_m, 0);
  const rock = new THREE.Matrix4()
    .makeTranslation(arcCenter.clone().setX(-layout.blade_arc_radius_m * rockRad))
    .multiply(new THREE.Matrix4().makeRotationZ(rockRad))
    .multiply(new THREE.Matrix4().makeTranslation(arcCenter.clone().negate()));
  knife.matrix.makeBasis(KNIFE_EDGE, up, KNIFE_EDGE.clone().cross(up)).setPosition(edgeMiddle).multiply(rock);
  knife.matrixWorldNeedsUpdate = true;
}

/** The unit direction the knife tilted by `tiltRad` lowers along within its plane, as `Knife::calc_frame` has it. */
function calcKnifeDown(tiltRad: number): THREE.Vector3 {
  return new THREE.Vector3(-Math.sin(tiltRad), -Math.cos(tiltRad), 0);
}

/** The unit normal of the knife's plane at `tiltRad`, pointing to the side the core weighs as above, as `Knife::calc_frame` has it. */
export function calcKnifeNormal(tiltRad: number): THREE.Vector3 {
  return KNIFE_EDGE.clone().cross(calcKnifeDown(tiltRad));
}

/** The square `extentM` either side of the stand the key light casts shadows over, at `mapPx` a side. */
export interface ShadowArea {
  extentM: number;
  mapPx: number;
}

/**
 * A warm sky, a warm key light low enough to the side that it is plainly not overhead, casting
 * shadows over `shadow` unless none, and a cool unshadowed fill from the far side.
 */
export function addLights(scene: THREE.Scene, shadow: ShadowArea | null, skyIntensity = SKY_INTENSITY): void {
  const fill = new THREE.DirectionalLight(0x9fb8ff, 0.9);
  fill.position.set(-1.2, 0.9, -0.9);
  const sun = new THREE.DirectionalLight(0xfff1dc, 2.4);
  sun.position.set(1.7, 1.2, 0.6);
  scene.add(new THREE.HemisphereLight(0xfff4e0, 0x2a2420, skyIntensity), fill, sun);
  if (!shadow) return;
  const { extentM, mapPx } = shadow;
  sun.castShadow = true;
  sun.shadow.mapSize.set(mapPx, mapPx);
  sun.shadow.bias = -0.0005;
  Object.assign(sun.shadow.camera, { left: -extentM, right: extentM, top: extentM, bottom: -extentM, near: 0.5, far: 5 });
}
