/**
 * The cake, stand, knife, and lights as the round draws them, for views other than the player's
 * own table: the opponents' tiles. `scene.ts` still builds its own copies, as `previews.ts` does
 * the cake's geometry; both can move onto these.
 */
import * as THREE from "three";
import { mergeGeometries } from "three/addons/utils/BufferGeometryUtils.js";
import { type Game, type Layout, get_blade_outline, get_handle_centers, get_stand_parts } from "@wasm/cake_wasm.js";

/** Neighboring faces meeting at a sharper angle than this keep a hard edge; gentler ones shade smoothly. */
const CREASE_ANGLE_RAD = Math.PI / 3;
/** The color of a cake with no texture of its own. */
const FROSTING = 0xf3e3ec;
const RIM_BEAD_COUNT = 16;
/** The knife's edge always runs along the world's z axis: the cake turns on its stand instead (`Knife::calc_frame`). */
const KNIFE_EDGE = new THREE.Vector3(0, 0, 1);

/**
 * The cake's mesh, each triangle with its own corners so texture seams stay sharp, shaded smoothly
 * but for creases.
 */
export function buildCakeGeometry(game: Game): THREE.BufferGeometry {
  const indexed = new THREE.BufferGeometry();
  indexed.setAttribute("position", new THREE.BufferAttribute(game.get_cake_mesh_positions(), 3));
  indexed.setIndex(new THREE.BufferAttribute(game.get_cake_mesh_triangles(), 1));
  const corners = indexed.toNonIndexed();
  const uvs = game.get_cake_corner_uvs();
  if (uvs.length > 0) corners.setAttribute("uv", new THREE.BufferAttribute(uvs, 2));
  corners.setAttribute("normal", new THREE.BufferAttribute(game.calc_cake_corner_normals(CREASE_ANGLE_RAD), 3));
  return corners;
}

/**
 * The cake's skin: its texture at full size, mipmapped, or plain frosting for a cake without one.
 * Decoding runs off the main thread. Asking `createImageBitmap` to also scale it down blocked the
 * main thread for 120-150 ms on a 2048 texture, longer than uploading it at full size takes.
 */
export async function decodeCakeMaterial(game: Game, anisotropy: number): Promise<THREE.MeshStandardMaterial> {
  const file = game.get_cake_texture();
  if (file.length === 0) return new THREE.MeshStandardMaterial({ color: FROSTING });
  const blob = new Blob([new Uint8Array(file)], { type: "image/webp" });
  const bitmap = await createImageBitmap(blob, { colorSpaceConversion: "none", premultiplyAlpha: "none" });
  const texture = new THREE.Texture(bitmap);
  texture.flipY = false;
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.anisotropy = anisotropy;
  texture.needsUpdate = true;
  return new THREE.MeshStandardMaterial({ map: texture });
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

/** A mezzaluna: a crescent blade from the core's outline, with an upright handle at each end. Its matrix is set by `placeKnife`. */
export function buildKnife(layout: Layout): THREE.Group {
  const knife = new THREE.Group();
  knife.matrixAutoUpdate = false;
  const outline = get_blade_outline();
  const shape = new THREE.Shape();
  shape.moveTo(outline[0], outline[1]);
  for (let i = 2; i < outline.length; i += 2) shape.lineTo(outline[i], outline[i + 1]);
  shape.closePath();
  const blade = new THREE.Mesh(
    new THREE.ExtrudeGeometry(shape, { depth: layout.blade_thickness_m, bevelEnabled: false }).translate(0, 0, -layout.blade_thickness_m / 2),
    new THREE.MeshStandardMaterial({ color: 0xcfd6dc, metalness: 0.8, roughness: 0.3, side: THREE.DoubleSide }),
  );
  blade.castShadow = true;
  knife.add(blade);
  const centers = get_handle_centers();
  const wood = new THREE.MeshStandardMaterial({ color: 0x5b3a29, roughness: 0.7 });
  const handle = new THREE.CylinderGeometry(layout.handle_radius_m, layout.handle_radius_m, layout.handle_len_m, 24);
  for (let i = 0; i < centers.length; i += 3) {
    const mesh = new THREE.Mesh(handle, wood);
    mesh.position.set(centers[i], centers[i + 1], centers[i + 2]);
    mesh.castShadow = true;
    knife.add(mesh);
  }
  return knife;
}

/**
 * Poses `knife` tilted by `tiltRad`, its edge lowered `lowerM` below the pivot within its plane,
 * and rocked by `rockRad` as its curved edge rolls along the line it rests on without slipping.
 */
export function placeKnife(knife: THREE.Group, layout: Layout, tiltRad: number, lowerM: number, rockRad: number): void {
  const down = new THREE.Vector3(-Math.sin(tiltRad), -Math.cos(tiltRad), 0);
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

/**
 * A warm key light low enough to the side that it is plainly not overhead, casting shadows over a
 * square `shadowExtentM` either side of the stand, and a cool unshadowed fill from the far side.
 */
export function addLights(scene: THREE.Scene, shadowExtentM: number, shadowMapPx: number): void {
  scene.add(new THREE.HemisphereLight(0xfff4e0, 0x2a2420, 0.7));
  const fill = new THREE.DirectionalLight(0x9fb8ff, 0.9);
  fill.position.set(-1.2, 0.9, -0.9);
  scene.add(fill);
  const sun = new THREE.DirectionalLight(0xfff1dc, 2.4);
  sun.position.set(1.7, 1.2, 0.6);
  sun.castShadow = true;
  sun.shadow.mapSize.set(shadowMapPx, shadowMapPx);
  sun.shadow.bias = -0.0005;
  Object.assign(sun.shadow.camera, { left: -shadowExtentM, right: shadowExtentM, top: shadowExtentM, bottom: -shadowExtentM, near: 0.5, far: 5 });
  scene.add(sun);
}
