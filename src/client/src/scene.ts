import * as THREE from "three";
import { toCreasedNormals } from "three/addons/utils/BufferGeometryUtils.js";
import { type Game, type Layout, get_blade_outline, get_handle_centers, get_stand_parts } from "@wasm/cake_wasm.js";
import { Spring } from "./spring";

/** What a click grabbed. The knife's blade and handles differ only in which way its tilt follows the mouse. */
export type PickKind = "cake" | "blade" | "handle" | "stand";

const SPONGE = 0xe8c07a;
/** The color of a cake with no texture of its own. */
const FROSTING = 0xf3e3ec;
/** Neighboring faces meeting at a sharper angle than this keep a hard edge; gentler ones shade smoothly. */
const CREASE_ANGLE_RAD = Math.PI / 3;
const TABLE_THICKNESS_M = 0.04;
const LEG_SIDE_M = 0.06;
const LEG_INSET_M = 0.08;
const LEG_LEN_M = 0.72;
const RIM_BEAD_COUNT = 16;
const HALF_GAP_M = 0.03;
/** Clearance either side of the blade's faces in the cut, so neither half's cut face fights the blade's. */
const BLADE_CLEARANCE_M = 0.0005;
const HALF_GAP_HALF_LIFE_S = 0.12;
/** Side of the plane drawn over a cut face, centered on the cake, larger than any cake's section. */
const CAP_SIZE_M = 1;
/** Thickness of the invisible copy of the blade that takes its clicks, since the blade seen edge-on is a hairline. */
const KNIFE_GRIP_THICKNESS_M = 0.04;
/** Which way along the knife's normal each half keeps the cake. */
const HALF_SIDES = [1, -1];
/** A height above everything, so a clipping plane keeping only what lies above it keeps nothing. */
const CLIP_ALL_Y_M = 100;

/** One side of a cut cake, drawn clipped at the knife's plane and parted from the other along `offset`. */
type Half = { group: THREE.Group; cap: THREE.Mesh; clip: THREE.Plane; side: number; offset: THREE.Vector3 };

/** Every object the player sees, rebuilt per round from the game's mesh and poses. */
export class Scene {
  readonly renderer: THREE.WebGLRenderer;
  readonly camera = new THREE.PerspectiveCamera(40, 1, 0.01, 20);
  private readonly scene = new THREE.Scene();
  private readonly cake = new THREE.Group();
  private readonly knife = new THREE.Group();
  private readonly stand = new THREE.Group();
  /** The point every cut plane passes through. */
  private readonly pivot: THREE.Vector3;
  private readonly halfGap = new Spring(0, HALF_GAP_HALF_LIFE_S);
  private halves: Half[] = [];
  private cakeTexture: THREE.Texture | null = null;

  constructor(
    canvas: HTMLCanvasElement,
    private readonly layout: Layout,
  ) {
    this.pivot = new THREE.Vector3(0, layout.pivot_y_m, 0);
    this.renderer = new THREE.WebGLRenderer({ canvas, antialias: true, stencil: true, powerPreference: "high-performance" });
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFShadowMap;
    this.renderer.localClippingEnabled = true;
    this.scene.background = new THREE.Color(0x1d1b19);
    this.addLights();
    this.addTable();
    this.buildStand();
    this.buildKnife();
    this.cake.userData.pick = "cake";
    this.knife.matrixAutoUpdate = false;
    this.scene.add(this.cake, this.knife, this.stand);
  }

  /** The kind of object the ray first meets, and where, or none for the room around them. */
  pick(raycaster: THREE.Raycaster): { kind: PickKind; point: THREE.Vector3 } | null {
    const hit = raycaster.intersectObjects(this.scene.children, true).find((candidate) => candidate.object.visible);
    for (let object: THREE.Object3D | null = hit?.object ?? null; object; object = object.parent) {
      if (object.userData.pick) return { kind: object.userData.pick, point: hit!.point };
    }
    return null;
  }

  /**
   * Replaces the cake with the game's mesh, whole, skinned with `texture` from `loadCakeTexture`.
   * Builds the hidden halves and compiles their shaders now, since doing either when the blade
   * reaches the cake stalls the cut's animation.
   */
  resetCake(game: Game, texture: THREE.Texture | null): void {
    this.removeCake();
    this.cakeTexture = texture;
    const geometry = buildCakeGeometry(game);
    const mesh = new THREE.Mesh(geometry, this.buildCakeMaterial());
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    this.cake.add(mesh);
    this.cake.visible = true;
    this.halves = HALF_SIDES.map((side, idx) => this.addHalf(geometry, side, idx));
    this.warmHalves();
  }

  private removeCake(): void {
    const roots = [this.cake, ...this.halves.flatMap((half) => [half.group, half.cap])];
    for (const root of roots) {
      root.traverse((object) => {
        if (!(object instanceof THREE.Mesh)) return;
        object.geometry.dispose();
        object.material.dispose();
      });
    }
    this.scene.remove(...this.halves.flatMap((half) => [half.group, half.cap]));
    this.halves = [];
    this.cake.clear();
    this.cakeTexture?.dispose();
  }

  /**
   * One half, hidden until `showHalves`. Its cut face is a sponge plane masked to the half's
   * inside through the stencil buffer, which works for any closed mesh without building cap
   * geometry.
   */
  private addHalf(geometry: THREE.BufferGeometry, side: number, idx: number): Half {
    const clip = new THREE.Plane();
    const group = new THREE.Group();
    const frosting = new THREE.Mesh(geometry, this.buildCakeMaterial({ clippingPlanes: [clip], clipShadows: true }));
    frosting.castShadow = true;
    group.add(frosting);
    for (const [faces, stencilOp] of [
      [THREE.BackSide, THREE.IncrementWrapStencilOp],
      [THREE.FrontSide, THREE.DecrementWrapStencilOp],
    ] as const) {
      const counter = new THREE.Mesh(geometry, buildStencilCounter(faces, stencilOp, clip));
      counter.renderOrder = idx + 1;
      group.add(counter);
    }
    const cap = new THREE.Mesh(new THREE.PlaneGeometry(CAP_SIZE_M, CAP_SIZE_M), buildCapMaterial());
    cap.renderOrder = idx + 1.5;
    cap.onAfterRender = (renderer) => renderer.clearStencil();
    // Hide the meshes, not the group: a raycast skips only meshes hidden themselves.
    for (const mesh of [...group.children, cap]) mesh.visible = false;
    this.scene.add(group, cap);
    return { group, cap, clip, side, offset: new THREE.Vector3() };
  }

  /**
   * Draws the hidden halves once, clipped away entirely, so every shader they need compiles now.
   * `renderer.compile` would miss the shadow pass's clipped depth shader.
   */
  private warmHalves(): void {
    const meshes = this.halves.flatMap((half) => [...half.group.children, half.cap]);
    for (const half of this.halves) half.clip.set(new THREE.Vector3(0, 1, 0), -CLIP_ALL_Y_M);
    for (const mesh of meshes) mesh.visible = true;
    this.renderer.render(this.scene, this.camera);
    for (const mesh of meshes) mesh.visible = false;
  }

  sync(game: Game): void {
    const pose = game.get_cake_pose();
    this.cake.position.set(pose[0], pose[1], pose[2]);
    this.cake.quaternion.set(pose[3], pose[4], pose[5], pose[6]);
    this.stand.rotation.y = game.get_stand_angle_rad();
    this.placeKnife(game, game.get_knife_pose()[1], 0);
  }

  /**
   * Draws the knife lowered by `lowerM` within its plane, rocked by `rockRad` as its curved edge
   * rolls along the line it rests on without slipping, as a mezzaluna rocks through a cut.
   */
  placeKnife(game: Game, lowerM: number, rockRad: number): void {
    const f = game.get_knife_frame();
    const edge = new THREE.Vector3(f[0], f[1], f[2]);
    const down = new THREE.Vector3(f[3], f[4], f[5]);
    const up = down.clone().negate();
    const edgeMiddle = this.pivot.clone().addScaledVector(down, lowerM);
    const arcCenter = new THREE.Vector3(0, this.layout.blade_arc_radius_m, 0);
    const rollM = -this.layout.blade_arc_radius_m * rockRad;
    const rock = new THREE.Matrix4()
      .makeTranslation(arcCenter.clone().setX(rollM))
      .multiply(new THREE.Matrix4().makeRotationZ(rockRad))
      .multiply(new THREE.Matrix4().makeTranslation(arcCenter.clone().negate()));
    this.knife.matrix.makeBasis(edge, up, edge.clone().cross(up)).setPosition(edgeMiddle).multiply(rock);
    this.knife.matrixWorldNeedsUpdate = true;
  }

  /**
   * Swaps the whole cake for its two halves along the knife's plane, apart by just the blade's
   * thickness until `partHalves`, so the blade shows in the cut as it plunges.
   */
  showHalves(game: Game): void {
    const f = game.get_knife_frame();
    const normal = new THREE.Vector3(f[6], f[7], f[8]);
    this.halfGap.snap(this.layout.blade_thickness_m / 2 + BLADE_CLEARANCE_M);
    for (const half of this.halves) {
      half.offset.copy(normal).multiplyScalar(half.side);
      for (const mesh of half.group.children) {
        mesh.position.copy(this.cake.position);
        mesh.quaternion.copy(this.cake.quaternion);
        mesh.visible = true;
      }
      half.cap.visible = true;
    }
    this.cake.visible = false;
  }

  /** The cake's texture, mipmapped and as sharp at grazing angles as the GPU allows, or none for a plain cake. */
  async loadCakeTexture(game: Game): Promise<THREE.Texture | null> {
    const file = game.get_cake_texture();
    if (file.length === 0) return null;
    const bitmap = await createImageBitmap(new Blob([new Uint8Array(file)], { type: "image/webp" }), {
      colorSpaceConversion: "none",
      premultiplyAlpha: "none",
    });
    const texture = new THREE.Texture(bitmap);
    texture.flipY = false;
    texture.colorSpace = THREE.SRGBColorSpace;
    texture.anisotropy = this.renderer.capabilities.getMaxAnisotropy();
    texture.needsUpdate = true;
    return texture;
  }

  private buildCakeMaterial(parameters: THREE.MeshStandardMaterialParameters = {}): THREE.Material {
    const skin = this.cakeTexture ? { map: this.cakeTexture } : { color: FROSTING };
    return new THREE.MeshStandardMaterial({ ...skin, ...parameters });
  }

  partHalves(): void {
    this.halfGap.target = HALF_GAP_M;
  }

  render(dtS: number): void {
    const gap = this.halfGap.step(dtS);
    for (const half of this.halves) {
      if (!half.cap.visible) continue;
      const cutPoint = this.pivot.clone().addScaledVector(half.offset, gap);
      half.group.position.copy(half.offset).multiplyScalar(gap);
      half.clip.setFromNormalAndCoplanarPoint(half.offset, cutPoint);
      half.clip.projectPoint(this.cake.position.clone().add(half.group.position), half.cap.position);
      half.cap.quaternion.setFromUnitVectors(new THREE.Vector3(0, 0, 1), half.offset.clone().negate());
    }
    this.fitCanvas();
    this.renderer.render(this.scene, this.camera);
  }

  private fitCanvas(): void {
    const canvas = this.renderer.domElement;
    if (canvas.width === canvas.clientWidth && canvas.height === canvas.clientHeight) return;
    this.renderer.setSize(canvas.clientWidth, canvas.clientHeight, false);
    this.camera.aspect = canvas.clientWidth / canvas.clientHeight;
    this.camera.updateProjectionMatrix();
  }

  /** A warm key light low enough to the side that it is plainly not overhead, and a cool unshadowed fill from the far side. */
  private addLights(): void {
    this.scene.add(new THREE.HemisphereLight(0xfff4e0, 0x2a2420, 0.7));
    const fill = new THREE.DirectionalLight(0x9fb8ff, 0.9);
    fill.position.set(-1.2, 0.9, -0.9);
    this.scene.add(fill);
    const sun = new THREE.DirectionalLight(0xfff1dc, 2.4);
    sun.position.set(1.7, 1.2, 0.6);
    sun.castShadow = true;
    sun.shadow.mapSize.set(2048, 2048);
    sun.shadow.bias = -0.0005;
    const extent = 1.3;
    Object.assign(sun.shadow.camera, { left: -extent, right: extent, top: extent, bottom: -extent, near: 0.5, far: 5 });
    this.scene.add(sun);
  }

  private addTable(): void {
    const { table_half_x_m: halfX, table_half_z_m: halfZ } = this.layout;
    const wood = new THREE.MeshStandardMaterial({ color: 0x7a5236, roughness: 0.8 });
    const top = new THREE.Mesh(new THREE.BoxGeometry(2 * halfX, TABLE_THICKNESS_M, 2 * halfZ).translate(0, -TABLE_THICKNESS_M / 2, 0), wood);
    top.castShadow = true;
    top.receiveShadow = true;
    this.scene.add(top);
    for (const [sx, sz] of [
      [1, 1],
      [1, -1],
      [-1, 1],
      [-1, -1],
    ]) {
      const leg = new THREE.Mesh(new THREE.BoxGeometry(LEG_SIDE_M, LEG_LEN_M, LEG_SIDE_M), wood);
      leg.position.set(sx * (halfX - LEG_INSET_M), -TABLE_THICKNESS_M - LEG_LEN_M / 2, sz * (halfZ - LEG_INSET_M));
      leg.castShadow = true;
      this.scene.add(leg);
    }
  }

  /** Plate, column, and base, with beads around the rim so its turning shows. */
  private buildStand(): void {
    const layout = this.layout;
    const ceramic = new THREE.MeshStandardMaterial({ color: 0xf2efe8, roughness: 0.35 });
    const parts = get_stand_parts();
    for (let i = 0; i < parts.length; i += 3) {
      const [radiusM, y0M, y1M] = parts.slice(i, i + 3);
      const part = new THREE.Mesh(new THREE.CylinderGeometry(radiusM, radiusM, y1M - y0M, 64).translate(0, (y0M + y1M) / 2, 0), ceramic);
      part.castShadow = true;
      part.receiveShadow = true;
      this.stand.add(part);
    }
    const beadRadiusM = layout.stand_plate_thickness_m * 0.45;
    const bead = new THREE.SphereGeometry(beadRadiusM, 12, 8);
    const beadMaterial = new THREE.MeshStandardMaterial({ color: 0xc9a86a, roughness: 0.4, metalness: 0.5 });
    for (let i = 0; i < RIM_BEAD_COUNT; i += 1) {
      const angle = (2 * Math.PI * i) / RIM_BEAD_COUNT;
      const mesh = new THREE.Mesh(bead, beadMaterial);
      mesh.position.set(Math.cos(angle) * layout.stand_radius_m, layout.stand_top_y_m - layout.stand_plate_thickness_m / 2, Math.sin(angle) * layout.stand_radius_m);
      this.stand.add(mesh);
    }
    this.stand.userData.pick = "stand";
  }

  /** A mezzaluna: a crescent blade from the core's outline, with an upright handle at each end. */
  private buildKnife(): void {
    const layout = this.layout;
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
    const grip = new THREE.Mesh(
      new THREE.ExtrudeGeometry(shape, { depth: KNIFE_GRIP_THICKNESS_M, bevelEnabled: false }).translate(0, 0, -KNIFE_GRIP_THICKNESS_M / 2),
      new THREE.MeshBasicMaterial({ visible: false }),
    );
    blade.userData.pick = "blade";
    grip.userData.pick = "blade";
    this.knife.add(blade, grip);
    const centers = get_handle_centers();
    const wood = new THREE.MeshStandardMaterial({ color: 0x5b3a29, roughness: 0.7 });
    const handle = new THREE.CylinderGeometry(layout.handle_radius_m, layout.handle_radius_m, layout.handle_len_m, 24);
    for (let i = 0; i < centers.length; i += 3) {
      const mesh = new THREE.Mesh(handle, wood);
      mesh.position.set(centers[i], centers[i + 1], centers[i + 2]);
      mesh.castShadow = true;
      mesh.userData.pick = "handle";
      this.knife.add(mesh);
    }
  }
}

/** Counts, per pixel, the half's surfaces behind the cut: nonzero exactly where the cut face shows. */
function buildStencilCounter(faces: THREE.Side, stencilOp: THREE.StencilOp, clip: THREE.Plane): THREE.Material {
  return new THREE.MeshBasicMaterial({
    side: faces,
    clippingPlanes: [clip],
    colorWrite: false,
    depthWrite: false,
    depthTest: false,
    stencilWrite: true,
    stencilFunc: THREE.AlwaysStencilFunc,
    stencilFail: stencilOp,
    stencilZFail: stencilOp,
    stencilZPass: stencilOp,
  });
}

/** Sponge drawn only where the stencil count marks the cut face, resetting the count as it goes. */
function buildCapMaterial(): THREE.Material {
  return new THREE.MeshStandardMaterial({
    color: SPONGE,
    side: THREE.DoubleSide,
    stencilWrite: true,
    stencilRef: 0,
    stencilFunc: THREE.NotEqualStencilFunc,
    stencilFail: THREE.ReplaceStencilOp,
    stencilZFail: THREE.ReplaceStencilOp,
    stencilZPass: THREE.ReplaceStencilOp,
  });
}

/** The cake's mesh, each triangle with its own corners so texture seams stay sharp, shaded smoothly but for creases. */
function buildCakeGeometry(game: Game): THREE.BufferGeometry {
  const indexed = new THREE.BufferGeometry();
  indexed.setAttribute("position", new THREE.BufferAttribute(game.get_cake_mesh_positions(), 3));
  indexed.setIndex(new THREE.BufferAttribute(game.get_cake_mesh_triangles(), 1));
  const corners = indexed.toNonIndexed();
  const uvs = game.get_cake_corner_uvs();
  if (uvs.length > 0) corners.setAttribute("uv", new THREE.BufferAttribute(uvs, 2));
  return toCreasedNormals(corners, CREASE_ANGLE_RAD);
}
