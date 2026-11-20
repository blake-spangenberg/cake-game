import * as THREE from "three";
import type { Game, Layout } from "@wasm/cake_wasm.js";

const FROSTING = 0xf3e3ec;
const SPONGE = 0xe8c07a;
const BLADE_HEIGHT_M = 0.06;
const HANDLE_RADIUS_M = 0.012;
const HANDLE_LEN_M = 0.3;
const RING_TUBE_M = 0.012;
const HALF_GAP_M = 0.03;

/** Every object the player sees, rebuilt per round from the game's mesh and poses. */
export class Scene {
  readonly renderer: THREE.WebGLRenderer;
  readonly camera = new THREE.PerspectiveCamera(40, 1, 0.01, 20);
  readonly cake = new THREE.Group();
  readonly blade: THREE.Mesh;
  readonly handle: THREE.Mesh;
  readonly ring: THREE.Mesh;
  /** Distance from the pivot to the handle's axis, the radius of the dome the handle rides. */
  readonly handleRadiusM = BLADE_HEIGHT_M + HANDLE_RADIUS_M;
  private readonly scene = new THREE.Scene();
  private readonly knife = new THREE.Group();
  private readonly ringGroup = new THREE.Group();
  private halves: THREE.Group[] = [];

  constructor(canvas: HTMLCanvasElement, private readonly layout: Layout) {
    this.renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFShadowMap;
    this.renderer.localClippingEnabled = true;
    this.scene.background = new THREE.Color(0x1d1b19);
    this.addLights();
    this.addTable();
    this.blade = new THREE.Mesh(
      new THREE.BoxGeometry(layout.blade_len_m, BLADE_HEIGHT_M, 0.002).translate(0, BLADE_HEIGHT_M / 2, 0),
      new THREE.MeshStandardMaterial({ color: 0xcfd6dc, metalness: 0.8, roughness: 0.3 }),
    );
    this.handle = new THREE.Mesh(
      new THREE.CylinderGeometry(HANDLE_RADIUS_M, HANDLE_RADIUS_M, HANDLE_LEN_M, 24)
        .rotateZ(Math.PI / 2)
        .translate(0, this.handleRadiusM, 0),
      new THREE.MeshStandardMaterial({ color: 0x5b3a29, roughness: 0.7 }),
    );
    for (const part of [this.blade, this.handle]) {
      part.castShadow = true;
      this.knife.add(part);
    }
    this.knife.matrixAutoUpdate = false;
    this.ring = new THREE.Mesh(
      new THREE.TorusGeometry(layout.cutting_region_radius_m + 0.04, RING_TUBE_M, 12, 96).rotateX(Math.PI / 2),
      new THREE.MeshStandardMaterial({ color: 0x8a9a5b, roughness: 0.6 }),
    );
    this.ring.position.y = RING_TUBE_M;
    this.ring.receiveShadow = true;
    this.ringGroup.add(this.ring, ...this.buildRingNotches());
    this.scene.add(this.cake, this.knife, this.ringGroup);
  }

  /** Replaces the cake with the game's mesh, whole. */
  resetCake(game: Game): void {
    for (const half of this.halves) this.scene.remove(half);
    this.halves = [];
    this.cake.clear();
    const mesh = new THREE.Mesh(buildCakeGeometry(game), new THREE.MeshStandardMaterial({ color: FROSTING }));
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    this.cake.add(mesh);
    this.cake.visible = true;
  }

  sync(game: Game): void {
    const pose = game.cake_pose();
    this.cake.position.set(pose[0], pose[1], pose[2]);
    this.cake.quaternion.set(pose[3], pose[4], pose[5], pose[6]);
    const f = game.knife_frame();
    const edge = new THREE.Vector3(f[0], f[1], f[2]);
    const up = new THREE.Vector3(-f[3], -f[4], -f[5]);
    const edgeCrossUp = new THREE.Vector3(-f[6], -f[7], -f[8]);
    this.knife.matrix.makeBasis(edge, up, edgeCrossUp).setPosition(f[9], f[10], f[11]);
    this.knife.matrixWorldNeedsUpdate = true;
    const heading = game.knife_pose()[1];
    this.ringGroup.rotation.y = heading;
  }

  /** Swaps the whole cake for its two halves, parted along the knife's plane, with sponge showing on the cut. */
  showHalves(game: Game): void {
    const f = game.knife_frame();
    const normal = new THREE.Vector3(f[6], f[7], f[8]);
    const pivot = new THREE.Vector3(0, this.layout.pivot_y_m, 0);
    const geometry = buildCakeGeometry(game);
    for (const side of [1, -1]) {
      const offset = normal.clone().multiplyScalar(side * HALF_GAP_M);
      const keepNormal = normal.clone().multiplyScalar(side);
      const clip = new THREE.Plane().setFromNormalAndCoplanarPoint(keepNormal, pivot.clone().add(offset));
      const half = new THREE.Group();
      for (const [color, faces] of [[FROSTING, THREE.FrontSide], [SPONGE, THREE.BackSide]] as const) {
        const material = new THREE.MeshStandardMaterial({ color, side: faces, clippingPlanes: [clip] });
        const mesh = new THREE.Mesh(geometry, material);
        mesh.castShadow = true;
        mesh.position.copy(this.cake.position).add(offset);
        mesh.quaternion.copy(this.cake.quaternion);
        half.add(mesh);
      }
      this.halves.push(half);
      this.scene.add(half);
    }
    this.cake.visible = false;
  }

  render(): void {
    const canvas = this.renderer.domElement;
    if (canvas.width !== canvas.clientWidth || canvas.height !== canvas.clientHeight) {
      this.renderer.setSize(canvas.clientWidth, canvas.clientHeight, false);
      this.camera.aspect = canvas.clientWidth / canvas.clientHeight;
      this.camera.updateProjectionMatrix();
    }
    this.renderer.render(this.scene, this.camera);
  }

  private addLights(): void {
    this.scene.add(new THREE.HemisphereLight(0xfff4e0, 0x2a2420, 1.2));
    const sun = new THREE.DirectionalLight(0xffffff, 2.2);
    sun.position.set(0.35, 1.6, 0.55);
    sun.castShadow = true;
    sun.shadow.mapSize.set(2048, 2048);
    sun.shadow.bias = -0.0005;
    const extent = 0.8;
    Object.assign(sun.shadow.camera, { left: -extent, right: extent, top: extent, bottom: -extent, near: 0.5, far: 3 });
    this.scene.add(sun);
  }

  private addTable(): void {
    const { table_half_x_m: halfX, table_half_z_m: halfZ } = this.layout;
    const thickness = 0.04;
    const table = new THREE.Mesh(
      new THREE.BoxGeometry(2 * halfX, thickness, 2 * halfZ).translate(0, -thickness / 2, 0),
      new THREE.MeshStandardMaterial({ color: 0x7a5236, roughness: 0.8 }),
    );
    table.receiveShadow = true;
    const board = new THREE.Mesh(
      new THREE.CircleGeometry(this.layout.cutting_region_radius_m, 96).rotateX(-Math.PI / 2).translate(0, 0.0005, 0),
      new THREE.MeshStandardMaterial({ color: 0xe9dcc3, roughness: 0.9 }),
    );
    board.receiveShadow = true;
    this.scene.add(table, board);
  }

  /** Marks where the edge points, so the ring reads as a dial turning with the knife. */
  private buildRingNotches(): THREE.Mesh[] {
    const radius = this.layout.cutting_region_radius_m + 0.04;
    return [1, -1].map((side) => {
      const notch = new THREE.Mesh(
        new THREE.SphereGeometry(RING_TUBE_M * 2, 16, 12),
        new THREE.MeshStandardMaterial({ color: 0xc0392b }),
      );
      notch.position.set(side * radius, RING_TUBE_M, 0);
      notch.castShadow = true;
      return notch;
    });
  }
}

function buildCakeGeometry(game: Game): THREE.BufferGeometry {
  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute("position", new THREE.BufferAttribute(game.cake_positions(), 3));
  geometry.setIndex(new THREE.BufferAttribute(game.cake_triangles(), 1));
  const faceted = geometry.toNonIndexed();
  faceted.computeVertexNormals();
  return faceted;
}
