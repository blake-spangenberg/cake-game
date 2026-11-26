import * as THREE from "three";
import { type Game, type Layout, get_blade_outline, get_stand_parts } from "@wasm/cake_wasm.js";
import { CutFaceMaterial } from "./cutFace";
import { waitFrames } from "./frames";
import { HalfLabels } from "./halfLabels";
import { type Highlight, addHighlight } from "./highlight";
import { type SlicedJob, type Supports, calcHalfRests } from "./halfRest";
import { FROSTING, type PropAssets, type ShadowArea, addLights, buildCakeGeometry, buildKnife, buildStand, buildTable, calcKnifeNormal, decodeCakeTexture, placeKnife } from "./props";

/** What a click grabbed: the cake, any part of the knife (blade or handles), or the cake stand. */
export type PickKind = "cake" | "knife" | "stand";

/** The color a textured cake's material multiplies its texture by, which leaves it as it is. */
const UNTINTED = 0xffffff;
/** The whole table, which the camera can see from any side. */
const SHADOW: ShadowArea = { extentM: 1.05, mapPx: 2048 };
/** Clearance either side of the blade's faces in the cut, so neither half's cut face fights the blade's. */
const BLADE_CLEARANCE_M = 0.0005;
/** Vertices `boxHalves` visits per slice, about a millisecond's work. */
const BOX_SLICE_VERTICES = 8192;
/** How long a frame spends finding where the halves come to rest, which takes several before the code warms up. */
const REST_SLICE_MS = 2;
/** Side of the plane drawn over a cut face, centered on the cake, larger than any cake's section. */
const CAP_SIZE_M = 1;
/** Thickness of the invisible copy of the blade that takes its clicks, since the blade seen edge-on is a hairline. */
const KNIFE_GRIP_THICKNESS_M = 0.04;
/** Which way along the knife's normal each half keeps the cake. */
const HALF_SIDES = [1, -1];
/** A height above everything, so a clipping plane keeping only what lies above it keeps nothing. */
const CLIP_ALL_Y_M = 100;
/** The frame after the cake's first swap draws it, and the one after that warms its halves. */
const WARM_DELAY_FRAMES = 2;
/** Vertical field of view, wide so close objects keep their depth rather than flattening as in a telephoto view. `Orbit`'s distances frame the table for it. */
const CAMERA_FOV_DEG = 60;
/**
 * How far up the canvas the view centers the table, as a share of its height, so the cake's spawn
 * near the table's front clears the band of the `Cut!` button in the bottom right corner, using
 * the empty space above the knife.
 */
const VIEW_RAISE_SHARE = 0.12;

/**
 * One side of a cut cake, drawn clipped at the knife's plane, whose `normal` points away from the
 * other half, moved from where the cut left it by `fromM` in the cut and by `toM` once parted.
 */
type Half = { group: THREE.Group; cap: THREE.Mesh<THREE.PlaneGeometry, CutFaceMaterial>; frosting: THREE.MeshStandardMaterial; clip: THREE.Plane; side: number; normal: THREE.Vector3; fromM: THREE.Vector3; toM: THREE.Vector3 };

/**
 * A round's cake as `Scene.loadCake` builds it ahead of the round swap: its mesh's local vertex
 * positions, flattened, its geometry, and its texture, already on the GPU, or none for a plain cake.
 */
export interface LoadedCake {
  positions: Float32Array;
  geometry: THREE.BufferGeometry;
  texture: THREE.Texture | null;
}

/**
 * A cut as a player made it, for the judging reveal to show on the round's cake: the cake's
 * translation then rotation quaternion, the stand's angle, the knife's tilt and the lowering it
 * plunged to, and whether the cut divided the cake, which a miss leaves whole.
 */
export interface StagedCut {
  cakePose: readonly number[];
  standRad: number;
  tiltRad: number;
  lowerM: number;
  isCutThrough: boolean;
}

/** What `Scene.prepareCut` finds for `Scene.stageCut`: each half's rest, where its label points (the whole cake's first, for a miss), and the whole cake's world bounds. */
export interface PreparedCut {
  rests: THREE.Vector3[];
  labelTops: (THREE.Vector3 | null)[];
  bounds: THREE.Box3;
}

/** Every object the player sees, posed from the game, with each round's cake swapped in. */
export class Scene {
  readonly renderer: THREE.WebGLRenderer;
  readonly camera = new THREE.PerspectiveCamera(CAMERA_FOV_DEG, 1, 0.01, 20);
  private readonly scene = new THREE.Scene();
  private readonly cake = new THREE.Group();
  private readonly knife = new THREE.Group();
  private readonly stand: THREE.Group;
  /** The point every cut plane passes through. */
  private readonly pivot: THREE.Vector3;
  /** How far each half has parted, from 0 in the cut to 1 at rest, as `setHalfPartShares` sets them. */
  private partShares: readonly number[] = [];
  /** The search for where the halves come to rest, run a slice per frame from `showHalves` until it sets each half's `toM`. */
  private halfRestsJob: SlicedJob<THREE.Vector3[]> | null = null;
  private readonly halves: Half[];
  /** The whole cake's mesh, then each half's frosting and stencil counters, all drawing the round's one geometry. */
  private readonly cakeMeshes: THREE.Mesh[];
  /** The materials skinned with the round's texture: the whole cake's, then each half's frosting. */
  private readonly frostings: THREE.MeshStandardMaterial[];
  private readonly halfLabels: HalfLabels;
  /**
   * What a pick's ray tests: what a click can grab, and the table, which hides a cake fallen below
   * it. Leaving out the hidden halves, which each test every triangle of the cake around the stand
   * three times over, keeps a pick over the stand from taking 20-70 ms.
   */
  private readonly pickables: THREE.Object3D[];
  /** The highlight of every material each kind of object a click can grab draws with. */
  private readonly highlights: Record<PickKind, Highlight[]>;
  /** Where each label in `halfLabels` points before the halves slide, once `showHalfGrams` has found them. */
  private labelTops: (THREE.Vector3 | null)[] | null = null;
  private cakeLoaded: LoadedCake = { positions: new Float32Array(), geometry: new THREE.BufferGeometry(), texture: null };
  /**
   * In how many frames to draw the hidden halves once, as `warmHalves` describes, or 0 for none.
   * Nothing renders before the first `resetCake`, so the count starts with the first cake.
   */
  private framesUntilWarm = WARM_DELAY_FRAMES;
  private viewShiftPx = 0;
  private isViewRaised = true;

  constructor(
    canvas: HTMLCanvasElement,
    private readonly layout: Layout,
    assets: PropAssets,
  ) {
    this.pivot = new THREE.Vector3(0, layout.pivot_y_m, 0);
    this.renderer = new THREE.WebGLRenderer({ canvas, antialias: true, stencil: true, powerPreference: "high-performance" });
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFShadowMap;
    this.renderer.localClippingEnabled = true;
    this.scene.background = new THREE.Color(0x1d1b19);
    this.halfLabels = new HalfLabels(canvas, HALF_SIDES.length);
    addLights(this.scene, SHADOW);
    const table = buildTable(layout, assets.tableWood, this.renderer.capabilities.getMaxAnisotropy());
    this.scene.add(table);
    this.stand = buildStand(layout);
    this.stand.userData.pick = "stand";
    this.knife.add(buildKnife(layout, assets.studio), buildKnifeGrip());
    this.knife.userData.pick = "knife";
    const whole = new THREE.Mesh(this.cakeLoaded.geometry, new THREE.MeshStandardMaterial({ color: FROSTING }));
    whole.castShadow = true;
    whole.receiveShadow = true;
    this.cake.add(whole);
    this.cake.userData.pick = "cake";
    this.halves = HALF_SIDES.map((side, idx) => this.addHalf(side, idx));
    this.cakeMeshes = [whole, ...this.halves.flatMap((half) => half.group.children as THREE.Mesh[])];
    this.frostings = [whole.material, ...this.halves.map((half) => half.frosting)];
    this.highlights = { cake: [addHighlight(whole.material)], knife: addGroupHighlights(this.knife), stand: addGroupHighlights(this.stand) };
    this.knife.matrixAutoUpdate = false;
    this.scene.add(this.cake, this.knife, this.stand);
    this.pickables = [table, this.cake, this.knife, this.stand];
  }

  /** The kind of object the ray first meets, and where, or none for the room around them. */
  pick(raycaster: THREE.Raycaster): { kind: PickKind; point: THREE.Vector3 } | null {
    const hit = raycaster.intersectObjects(this.pickables, true).find((candidate) => candidate.object.visible);
    for (let object: THREE.Object3D | null = hit?.object ?? null; object; object = object.parent) {
      if (object.userData.pick) return { kind: object.userData.pick, point: hit!.point };
    }
    return null;
  }

  /** Faintly highlights the whole cake, the knife, or the stand, or none of them. */
  setHighlight(kind: PickKind | null): void {
    for (const [highlightKind, highlights] of Object.entries(this.highlights)) {
      for (const highlight of highlights) highlight.value = highlightKind === kind ? 1 : 0;
    }
  }

  /**
   * Builds `game`'s cake for `resetCake` ahead of the round swap and uploads its texture, a step
   * per frame so no frame runs long and the swap itself only trades references. `game` must stay
   * alive until this resolves. A cake never given to `resetCake` goes to `disposeLoadedCake`.
   */
  async loadCake(game: Game): Promise<LoadedCake> {
    const decoding = decodeCakeTexture(game, this.renderer.capabilities.getMaxAnisotropy());
    await waitFrames(1);
    const positions = game.get_cake_mesh_positions();
    const geometry = buildCakeGeometry(game, positions);
    // For the cut faces' layers, which start at the cake's bottom.
    geometry.computeBoundingBox();
    const texture = await decoding;
    await waitFrames(1);
    if (texture) this.renderer.initTexture(texture);
    await waitFrames(1);
    return { positions, geometry, texture };
  }

  /**
   * Replaces the cake with `cake` from `loadCake`, whole, and takes ownership of it. Every round's
   * cake draws through the same meshes and materials, so their shaders compile once per page:
   * three deletes a shader as soon as no material uses it, which recompiled them all every round.
   */
  resetCake(cake: LoadedCake): void {
    disposeLoadedCake(this.cakeLoaded);
    this.cakeLoaded = cake;
    for (const mesh of this.cakeMeshes) mesh.geometry = cake.geometry;
    this.setSkin(cake.texture);
    this.showWholeCake();
  }

  /** How tall the round's cake stands as served, the pose its model is baked in. */
  getCakeHeightM(): number {
    const box = this.cakeLoaded.geometry.boundingBox!;
    return box.max.y - box.min.y;
  }

  /** Shows the cake whole again, without labels, for `sync` to pose. */
  showWholeCake(): void {
    this.cake.visible = true;
    this.hideHalves();
    this.labelTops = null;
    this.halfRestsJob = null;
  }

  /** Hides the cake, whole or in halves, until the next `resetCake`, leaving the stand empty. */
  hideCake(): void {
    this.cake.visible = false;
    this.hideHalves();
  }

  private hideHalves(): void {
    for (const half of this.halves) {
      half.group.position.set(0, 0, 0);
      for (const mesh of [...half.group.children, half.cap]) mesh.visible = false;
    }
    this.halfLabels.hide();
  }

  /**
   * Skins every frosting with `texture`, or plain frosting for none. Switching between the two
   * needs other shaders, so the halves warm up again.
   */
  private setSkin(texture: THREE.Texture | null): void {
    const isSwitched = (this.frostings[0].map !== null) !== (texture !== null);
    for (const material of this.frostings) {
      material.map = texture;
      material.color.set(texture ? UNTINTED : FROSTING);
      if (isSwitched) material.needsUpdate = true;
    }
    if (isSwitched) this.framesUntilWarm = WARM_DELAY_FRAMES;
  }

  /**
   * One half, hidden until `showHalves`. Its cut face is a sponge plane masked to the half's
   * inside through the stencil buffer, which works for any closed mesh without building cap
   * geometry.
   */
  private addHalf(side: number, idx: number): Half {
    const geometry = this.cakeLoaded.geometry;
    const clip = new THREE.Plane();
    const group = new THREE.Group();
    // Both sides cast shadows: the half is open where its cut face is drawn, which the shadow pass
    // does not see, so its back faces alone let light through where the cut faces away from it.
    const frosting = new THREE.MeshStandardMaterial({ color: FROSTING, clippingPlanes: [clip], clipShadows: true, shadowSide: THREE.DoubleSide });
    const frostingMesh = new THREE.Mesh(geometry, frosting);
    frostingMesh.castShadow = true;
    group.add(frostingMesh);
    for (const [faces, stencilOp] of [
      [THREE.BackSide, THREE.IncrementWrapStencilOp],
      [THREE.FrontSide, THREE.DecrementWrapStencilOp],
    ] as const) {
      const counter = new THREE.Mesh(geometry, buildStencilCounter(faces, stencilOp, clip));
      counter.renderOrder = idx + 1;
      group.add(counter);
    }
    const cap = new THREE.Mesh(new THREE.PlaneGeometry(CAP_SIZE_M, CAP_SIZE_M), buildCapMaterial());
    cap.receiveShadow = true;
    cap.renderOrder = idx + 1.5;
    cap.onAfterRender = (renderer) => renderer.clearStencil();
    // Hide the meshes, not the group: a raycast skips only meshes hidden themselves.
    for (const mesh of [...group.children, cap]) mesh.visible = false;
    this.scene.add(group, cap);
    return { group, cap, frosting, clip, side, normal: new THREE.Vector3(), fromM: new THREE.Vector3(), toM: new THREE.Vector3() };
  }

  /**
   * Draws the hidden halves once, clipped away entirely, so every shader they need compiles now
   * rather than when the blade reaches the cake, which would stall the cut's animation. It waits
   * two frames after the first cake swaps in, so it lands on neither the swap nor the cake's first
   * draw. `renderer.compile` would miss the shadow pass's clipped depth shader.
   */
  private warmHalves(): void {
    this.framesUntilWarm = 0;
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
    const [tiltRad, lowerM] = game.get_knife_pose();
    placeKnife(this.knife, this.layout, tiltRad, lowerM, 0);
  }

  /** Draws `game`'s knife lowered by `lowerM` and rocked by `rockRad`, as `props.ts`'s `placeKnife` does. */
  placeKnife(game: Game, lowerM: number, rockRad: number): void {
    placeKnife(this.knife, this.layout, game.get_knife_pose()[0], lowerM, rockRad);
  }

  /**
   * Swaps the whole cake for its two halves along the knife's plane, apart by just the blade's
   * thickness until `setHalfPartShares` parts them, so the blade shows in the cut as it plunges.
   */
  showHalves(game: Game): void {
    const normal = calcKnifeNormal(game.get_knife_pose()[0]);
    this.splitHalves(normal);
    const pose = new THREE.Matrix4().compose(this.cake.position, this.cake.quaternion, new THREE.Vector3(1, 1, 1));
    this.halfRestsJob = calcHalfRests(this.cakeLoaded.positions, pose, this.pivot, normal, this.calcBladeClearSlideM(normal), this.calcSupports());
  }

  /**
   * Finds what `stageCut` needs to show `cut` on the round's cake without a long frame: where its
   * halves come to rest, where their labels point, and the whole cake's bounds, a slice per `next()`.
   */
  *prepareCut(cut: StagedCut): SlicedJob<PreparedCut> {
    const pose = calcCakePose(cut.cakePose);
    const normal = cut.isCutThrough ? calcKnifeNormal(cut.tiltRad) : null;
    const positions = this.cakeLoaded.positions;
    const boxes = yield* boxHalves(positions, pose, this.pivot, normal);
    const rests = normal ? yield* calcHalfRests(positions, pose, this.pivot, normal, this.calcBladeClearSlideM(normal), this.calcSupports()) : HALF_SIDES.map(() => new THREE.Vector3());
    return { rests, labelTops: boxes.map(findBoxTop), bounds: boxes[0].clone().union(boxes[1]) };
  }

  /**
   * Shows `cut` as it was made, for the judging reveal to part with `setHalfPartShares`: the
   * stand, the cake in halves still together, or whole for a cut that missed, and the knife
   * plunged in it, with `prepared` from `prepareCut`. The labels show once `showHalfGrams` asks.
   */
  stageCut(cut: StagedCut, prepared: PreparedCut): void {
    this.cake.position.fromArray(cut.cakePose, 0);
    this.cake.quaternion.fromArray(cut.cakePose, 3);
    this.stand.rotation.y = cut.standRad;
    placeKnife(this.knife, this.layout, cut.tiltRad, cut.lowerM, 0);
    this.showWholeCake();
    this.labelTops = prepared.labelTops;
    if (!cut.isCutThrough) return;
    this.splitHalves(calcKnifeNormal(cut.tiltRad));
    this.halves.forEach((half, idx) => half.toM.copy(prepared.rests[idx]));
  }

  /** Swaps the whole cake, posed as it is, for its two halves along the plane through the pivot of unit `normal`, together. */
  private splitHalves(normal: THREE.Vector3): void {
    // Their own drawing compiles whatever a pending warm-up would have.
    this.framesUntilWarm = 0;
    const level = normal.clone().setY(0).normalize();
    this.partShares = [];
    for (const half of this.halves) {
      half.normal.copy(normal).multiplyScalar(half.side);
      half.fromM.copy(level).multiplyScalar(half.side * this.calcBladeClearSlideM(normal));
      half.toM.copy(half.fromM);
      for (const mesh of half.group.children) {
        mesh.position.copy(this.cake.position);
        mesh.quaternion.copy(this.cake.quaternion);
        mesh.visible = true;
      }
      half.cap.visible = true;
    }
    this.cake.visible = false;
  }

  /** The level slide that moves a half off the knife's plane, of unit `normal`, just clear of the blade's face. */
  private calcBladeClearSlideM(normal: THREE.Vector3): number {
    return (this.layout.blade_thickness_m / 2 + BLADE_CLEARANCE_M) / Math.hypot(normal.x, normal.z);
  }

  /**
   * Parts each half from `showHalves` or `stageCut` by `shares[i]`, in the order `showHalfGrams`
   * labels them, from 0 together to 1 where it comes to rest, as `calcHalfRests` chose, which
   * `hasHalfRests` tells has been found.
   */
  setHalfPartShares(shares: readonly number[]): void {
    this.partShares = [...shares];
  }

  /** How far each shown half has slid from where it was cut, in the order `showHalfGrams` labels them, none while the cake shows whole. */
  getHalfSlidesM(): number[] {
    return this.halves.filter((half) => half.cap.visible).map((half) => half.group.position.distanceTo(half.fromM));
  }

  hasHalfRests(): boolean {
    return this.halfRestsJob === null;
  }

  /** Runs `halfRestsJob` for up to `REST_SLICE_MS`, setting each half's rest once it is done. */
  private stepHalfRests(): void {
    const job = this.halfRestsJob;
    if (!job) return;
    const endMs = performance.now() + REST_SLICE_MS;
    let step = job.next();
    while (!step.done && performance.now() < endMs) step = job.next();
    if (!step.done) return;
    this.halves.forEach((half, idx) => half.toM.copy(step.value[idx]));
    this.halfRestsJob = null;
  }

  private calcSupports(): Supports {
    const layout = this.layout;
    return { tableHalfXM: layout.table_half_x_m, tableHalfZM: layout.table_half_z_m, plateRadiusM: layout.stand_radius_m, plateTopYM: layout.stand_top_y_m, standParts: get_stand_parts() };
  }

  /**
   * Shows `grams[i]` over the half on the side `HALF_SIDES[i]` of the knife's normal, tied to it
   * as it slides, until the next round. Before `showHalves` the cake is whole, and only the first
   * shows, over the whole cake. Cheap to call every frame with new counts.
   */
  showHalfGrams(grams: readonly number[]): void {
    this.labelTops ??= this.findLabelTops();
    const tops = this.labelTops;
    this.halfLabels.show(grams.map((g, idx) => (tops[idx] ? `${g} g` : null)));
  }

  /**
   * Compiles the shaders of every mesh, the hidden halves' included, without holding the page
   * where the browser compiles in parallel, so a draw after this resolves compiles only the shadow
   * pass's, which this skips. A later skin with or without a texture needs other shaders.
   */
  async compile(): Promise<void> {
    await this.renderer.compileAsync(this.scene, this.camera);
  }

  render(): void {
    if (this.framesUntilWarm > 0 && --this.framesUntilWarm === 0) this.warmHalves();
    this.stepHalfRests();
    for (const [idx, half] of this.halves.entries()) {
      if (!half.cap.visible) continue;
      half.group.position.lerpVectors(half.fromM, half.toM, this.partShares[idx] ?? 0);
      const cutPoint = this.pivot.clone().add(half.group.position);
      half.clip.setFromNormalAndCoplanarPoint(half.normal, cutPoint);
      half.clip.projectPoint(this.cake.position.clone().add(half.group.position), half.cap.position);
      half.cap.quaternion.setFromUnitVectors(new THREE.Vector3(0, 0, 1), half.normal.clone().negate());
      half.group.updateMatrixWorld();
      half.cap.material.setCake(half.group.children[0], this.cakeLoaded.geometry.boundingBox!.min.y);
    }
    this.fitCanvas();
    this.renderer.render(this.scene, this.camera);
    if (this.labelTops) this.halfLabels.place(this.camera, this.labelTops.map((top, idx) => top && top.clone().add(this.halves[idx].group.position)));
  }

  /**
   * The top middle of each half's bounding box, from the vertices on its side of the cut, or of
   * the whole cake for the first label while it is whole. A half is never empty, since the halves
   * show only for a cut through the cake.
   */
  private findLabelTops(): (THREE.Vector3 | null)[] {
    const isSplit = this.halves.some((half) => half.cap.visible);
    const pose = new THREE.Matrix4().compose(this.cake.position, this.cake.quaternion, new THREE.Vector3(1, 1, 1));
    return finishJob(boxHalves(this.cakeLoaded.positions, pose, this.pivot, isSplit ? this.halves[0].normal : null)).map(findBoxTop);
  }

  /**
   * Centers the camera's view `shiftPx` left of the canvas's middle, keeping its vertical field of
   * view, so the table centers in the part of the screen the opponents' tiles leave uncovered.
   */
  setViewShiftPx(shiftPx: number): void {
    if (shiftPx === this.viewShiftPx) return;
    this.viewShiftPx = shiftPx;
    this.fitCamera();
  }

  /**
   * Raises the view's center by `VIEW_RAISE_SHARE` for play, or centers it for the judging, whose
   * camera aims at the judged cake rather than the table and has no `Cut!` button to clear.
   */
  setViewRaised(isRaised: boolean): void {
    if (isRaised === this.isViewRaised) return;
    this.isViewRaised = isRaised;
    this.fitCamera();
  }

  private fitCanvas(): void {
    const canvas = this.renderer.domElement;
    if (canvas.width === canvas.clientWidth && canvas.height === canvas.clientHeight) return;
    this.renderer.setSize(canvas.clientWidth, canvas.clientHeight, false);
    this.fitCamera();
  }

  /**
   * Fits the camera to the canvas as a window onto a view wider by twice the shift and taller by
   * twice the raise, at the larger view's bottom right corner, so the table sits left and up of
   * the canvas's middle, or at its vertical middle while the view is not raised.
   */
  private fitCamera(): void {
    const { width, height } = this.renderer.domElement;
    const shiftPx = this.viewShiftPx;
    const raisePx = Math.round(height * VIEW_RAISE_SHARE);
    const fullHeightPx = height + 2 * raisePx;
    // The larger view spreads the field of view over its full height, so zooming out by as much
    // keeps the window's field of view, and with it how far a drag moves, as without the raise.
    this.camera.zoom = height / fullHeightPx;
    this.camera.setViewOffset(width + 2 * shiftPx, fullHeightPx, 2 * shiftPx, this.isViewRaised ? 2 * raisePx : raisePx, width, height);
  }
}

/** An invisible thick copy of the core's blade outline, to take the knife's clicks. */
function buildKnifeGrip(): THREE.Mesh {
  const outline = get_blade_outline();
  const shape = new THREE.Shape();
  shape.moveTo(outline[0], outline[1]);
  for (let i = 2; i < outline.length; i += 2) shape.lineTo(outline[i], outline[i + 1]);
  shape.closePath();
  const geometry = new THREE.ExtrudeGeometry(shape, { depth: KNIFE_GRIP_THICKNESS_M, bevelEnabled: false }).translate(0, 0, -KNIFE_GRIP_THICKNESS_M / 2);
  return new THREE.Mesh(geometry, new THREE.MeshBasicMaterial({ visible: false }));
}

/** Highlights for every lit material `group` draws with, each once. */
function addGroupHighlights(group: THREE.Object3D): Highlight[] {
  const materials = new Set<THREE.MeshStandardMaterial>();
  group.traverse((object) => {
    if (object instanceof THREE.Mesh && object.material instanceof THREE.MeshStandardMaterial) materials.add(object.material);
  });
  return [...materials].map(addHighlight);
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

/** Sponge and buttercream drawn only where the stencil count marks the cut face, resetting the count as it goes. */
function buildCapMaterial(): CutFaceMaterial {
  return new CutFaceMaterial({
    side: THREE.DoubleSide,
    stencilWrite: true,
    stencilRef: 0,
    stencilFunc: THREE.NotEqualStencilFunc,
    stencilFail: THREE.ReplaceStencilOp,
    stencilZFail: THREE.ReplaceStencilOp,
    stencilZPass: THREE.ReplaceStencilOp,
  });
}

/** Frees a cake `Scene.loadCake` built that never went to `Scene.resetCake`. */
export function disposeLoadedCake(cake: LoadedCake): void {
  cake.geometry.dispose();
  cake.texture?.dispose();
}

/** Runs `job` to its end at once, returning its result. */
export function finishJob<T>(job: SlicedJob<T>): T {
  let step = job.next();
  while (!step.done) step = job.next();
  return step.value;
}

/** The world matrix of a cake at `cakePose`, its translation then rotation quaternion. */
function calcCakePose(cakePose: readonly number[]): THREE.Matrix4 {
  const position = new THREE.Vector3().fromArray(cakePose, 0);
  const rotation = new THREE.Quaternion().fromArray(cakePose, 3);
  return new THREE.Matrix4().compose(position, rotation, new THREE.Vector3(1, 1, 1));
}

/**
 * The world bounds of the cake's vertices `positions` placed by `pose`, on the `HALF_SIDES[0]`
 * side of the plane through `pivot` of unit `normal` and then the other's, or all in the first
 * and none in the second with no plane.
 */
function* boxHalves(positions: Float32Array, pose: THREE.Matrix4, pivot: THREE.Vector3, normal: THREE.Vector3 | null): SlicedJob<THREE.Box3[]> {
  const boxes = HALF_SIDES.map(() => new THREE.Box3());
  const point = new THREE.Vector3();
  const fromPivot = new THREE.Vector3();
  for (let i = 0; i < positions.length; i += 3) {
    if (i % (3 * BOX_SLICE_VERTICES) === 0) yield;
    point.fromArray(positions, i).applyMatrix4(pose);
    const idx = !normal || fromPivot.subVectors(point, pivot).dot(normal) >= 0 ? 0 : 1;
    boxes[idx].expandByPoint(point);
  }
  return boxes;
}

/** The top middle of `box`, where a label over what it bounds points, or none for an empty box. */
function findBoxTop(box: THREE.Box3): THREE.Vector3 | null {
  return box.isEmpty() ? null : box.getCenter(new THREE.Vector3()).setY(box.max.y);
}
