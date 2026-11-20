import * as THREE from "three";
import type { Layout } from "@wasm/cake_wasm.js";
import type { Scene } from "./scene";

type Drag =
  | { kind: "cake"; grabAt: [number, number, number]; planeY: number; target: THREE.Vector2 }
  | { kind: "tilt" }
  | { kind: "heading"; lastAngle: number }
  | { kind: "lower" };

/** Lift the drag plane above the grab point, since the held cake rises off the table. */
const CARRY_LIFT_M = 0.05;
const FLAG_HOLDING = 1 << 0;
const FLAG_LOWERING = 1 << 1;

/**
 * Turns mouse drags into the core's per-tick input: drag the cake to carry it, the knife's
 * handle to tilt it, the ring to turn it, and hold the blade to lower it.
 */
export class Controls {
  private tiltRad: number;
  private headingRad: number;
  private drag: Drag | null = null;
  private readonly raycaster = new THREE.Raycaster();
  private readonly pointer = new THREE.Vector2();
  private readonly pivot: THREE.Vector3;

  constructor(
    private readonly scene: Scene,
    private readonly layout: Layout,
    knifePose: Float64Array,
  ) {
    [this.tiltRad, this.headingRad] = knifePose;
    this.pivot = new THREE.Vector3(0, layout.pivot_y_m, 0);
    const canvas = scene.renderer.domElement;
    canvas.addEventListener("pointerdown", (event) => this.onPointerDown(event));
    canvas.addEventListener("pointermove", (event) => this.onPointerMove(event));
    canvas.addEventListener("pointerup", () => (this.drag = null));
    canvas.addEventListener("pointercancel", () => (this.drag = null));
    canvas.addEventListener("contextmenu", (event) => event.preventDefault());
  }

  reset(knifePose: Float64Array): void {
    [this.tiltRad, this.headingRad] = knifePose;
    this.drag = null;
  }

  /** The current input in the core's wire format, `Input::encode` in `core/src/input.rs`. */
  encodeInput(): Int32Array {
    const drag = this.drag;
    let flags = drag?.kind === "lower" ? FLAG_LOWERING : 0;
    const encoded = new Int32Array(8);
    if (drag?.kind === "cake") {
      flags |= FLAG_HOLDING;
      encoded.set(drag.grabAt, 1);
      encoded.set([this.quantizeM(drag.target.x), this.quantizeM(drag.target.y)], 4);
    }
    encoded[0] = flags;
    encoded[6] = Math.round(this.tiltRad / this.layout.angle_unit_rad);
    encoded[7] = Math.round(this.headingRad / this.layout.angle_unit_rad);
    return encoded;
  }

  private onPointerDown(event: PointerEvent): void {
    if (event.button !== 0) return;
    this.aim(event);
    const { blade, handle, ring, cake } = this.scene;
    const hit = this.raycaster.intersectObjects([handle, blade, ring, cake], true)[0];
    if (!hit) return;
    this.scene.renderer.domElement.setPointerCapture(event.pointerId);
    if (hit.object === handle) {
      this.drag = { kind: "tilt" };
    } else if (hit.object === blade) {
      this.drag = { kind: "lower" };
    } else if (hit.object === ring) {
      this.drag = { kind: "heading", lastAngle: calcTableAngle(hit.point) };
    } else {
      const grabAt: [number, number, number] = [hit.point.x, hit.point.y, hit.point.z].map((m) => this.quantizeM(m)) as [
        number,
        number,
        number,
      ];
      const target = new THREE.Vector2(hit.point.x, hit.point.z);
      this.drag = { kind: "cake", grabAt, planeY: hit.point.y + CARRY_LIFT_M, target };
    }
  }

  private onPointerMove(event: PointerEvent): void {
    this.aim(event);
    const drag = this.drag;
    if (!drag) {
      this.updateCursor();
      return;
    }
    if (drag.kind === "cake") {
      const point = this.raycaster.ray.intersectPlane(new THREE.Plane(new THREE.Vector3(0, -1, 0), drag.planeY), new THREE.Vector3());
      if (point) drag.target.set(point.x, point.z);
    } else if (drag.kind === "tilt") {
      this.tiltRad = this.calcDragTilt();
    } else if (drag.kind === "heading") {
      const point = this.raycaster.ray.intersectPlane(new THREE.Plane(new THREE.Vector3(0, 1, 0), 0), new THREE.Vector3());
      if (!point) return;
      const angle = calcTableAngle(point);
      this.headingRad += wrapAngle(angle - drag.lastAngle);
      drag.lastAngle = angle;
    }
  }

  /** Tilt that puts the handle under the cursor on its dome, swinging about the edge. */
  private calcDragTilt(): number {
    const levelNormal = new THREE.Vector3(Math.sin(this.headingRad), 0, Math.cos(this.headingRad));
    const handleDirection = new THREE.Vector3(0, Math.cos(this.tiltRad), 0).addScaledVector(levelNormal, -Math.sin(this.tiltRad));
    const handle = this.pivot.clone().addScaledVector(handleDirection, this.scene.handleRadiusM);
    const onDome = this.findDomePoint(handle);
    const fromPivot = onDome.sub(this.pivot);
    const tilt = Math.atan2(-fromPivot.dot(levelNormal), fromPivot.y);
    return THREE.MathUtils.clamp(tilt, -this.layout.max_tilt_rad, this.layout.max_tilt_rad);
  }

  /** Where the cursor's ray meets the handle's dome, on the side nearer the handle now. */
  private findDomePoint(handle: THREE.Vector3): THREE.Vector3 {
    const ray = this.raycaster.ray;
    const toPivot = this.pivot.clone().sub(ray.origin);
    const along = toPivot.dot(ray.direction);
    const missSq = toPivot.lengthSq() - along * along;
    const radiusSq = this.scene.handleRadiusM ** 2;
    if (missSq > radiusSq) return ray.at(along, new THREE.Vector3());
    const halfChord = Math.sqrt(radiusSq - missSq);
    const near = ray.at(along - halfChord, new THREE.Vector3());
    const far = ray.at(along + halfChord, new THREE.Vector3());
    return near.distanceToSquared(handle) <= far.distanceToSquared(handle) ? near : far;
  }

  private updateCursor(): void {
    const { blade, handle, ring, cake } = this.scene;
    const hit = this.raycaster.intersectObjects([handle, blade, ring, cake], true)[0];
    const cursor = !hit ? "default" : hit.object === blade ? "pointer" : "grab";
    this.scene.renderer.domElement.style.cursor = cursor;
  }

  private aim(event: PointerEvent): void {
    const rect = this.scene.renderer.domElement.getBoundingClientRect();
    this.pointer.set(((event.clientX - rect.left) / rect.width) * 2 - 1, -((event.clientY - rect.top) / rect.height) * 2 + 1);
    this.raycaster.setFromCamera(this.pointer, this.scene.camera);
  }

  private quantizeM(meters: number): number {
    return Math.round(meters / this.layout.position_unit_m);
  }
}

/** Angle about the vertical axis, measured the way the core measures heading. */
function calcTableAngle(point: THREE.Vector3): number {
  return Math.atan2(-point.z, point.x);
}

function wrapAngle(angle: number): number {
  return Math.atan2(Math.sin(angle), Math.cos(angle));
}
