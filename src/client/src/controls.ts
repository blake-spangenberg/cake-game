import * as THREE from "three";
import type { Game, Layout } from "@wasm/cake_wasm.js";
import type { Orbit } from "./orbit";
import type { Scene } from "./scene";
import { Spring } from "./spring";

type Drag =
  | { kind: "camera" }
  | { kind: "cake"; grabAt: [number, number, number]; metersPerPx: number }
  | { kind: "knife"; tiltSign: number }
  | { kind: "stand"; axisPx: THREE.Vector2; lastBearingRad: number; turnSign: number };

const FLAG_HOLDING = 1 << 0;
const ENCODED_LEN = 9;
const TILT_RAD_PER_PX = 0.004;
const LOWER_M_PER_PX = 0.0018;
/** Mouse travel this near the stand's axis on screen gives no clear bearing, so it is ignored. */
const STAND_DEAD_ZONE_PX = 12;
/** How far the knife may be pressed past where it stopped, so drawing back raises it at once. */
const LOWER_SLACK_M = 0.005;
const HAND_HALF_LIFE_S = 0.08;
const KNIFE_HALF_LIFE_S = 0.05;
const KNIFE_RETURN_HALF_LIFE_S = 0.3;
const STAND_HALF_LIFE_S = 0.08;
/** Nudge used to find which way on screen a control moves an object. */
const PROBE_STEP = 0.01;

/**
 * Turns mouse drags into the core's per-tick input. Every drag moves its object by how far the
 * mouse travels from where it pressed, never toward the cursor: drag the cake to carry it, the
 * knife to tilt (sideways) and lower it (up and down), the stand to turn it by circling its axis,
 * and anywhere else to swing the camera. Every input eases through a
 * spring, and a drag stops where the core stops its object.
 */
export class Controls {
  private drag: Drag | null = null;
  private readonly lastPx = new THREE.Vector2();
  private readonly raycaster = new THREE.Raycaster();
  private readonly tilt: Spring;
  private readonly lower = new Spring(0, KNIFE_RETURN_HALF_LIFE_S);
  /** The tilt and lowering last sent, to tell a knife stopped by a solid from one still easing. */
  private sentTiltRad = 0;
  private sentLowerM = 0;
  private readonly stand = new Spring(0, STAND_HALF_LIFE_S);
  private readonly handX = new Spring(0, HAND_HALF_LIFE_S);
  private readonly handZ = new Spring(0, HAND_HALF_LIFE_S);

  constructor(
    private readonly scene: Scene,
    private readonly orbit: Orbit,
    private readonly layout: Layout,
    knifeTiltRad: number,
  ) {
    this.tilt = new Spring(knifeTiltRad, KNIFE_HALF_LIFE_S);
    this.sentTiltRad = knifeTiltRad;
    const canvas = scene.renderer.domElement;
    canvas.addEventListener("pointerdown", (event) => this.onPointerDown(event));
    canvas.addEventListener("pointermove", (event) => this.onPointerMove(event));
    canvas.addEventListener("pointerup", () => this.onPointerUp());
    canvas.addEventListener("pointercancel", () => this.onPointerUp());
    canvas.addEventListener("contextmenu", (event) => event.preventDefault());
    canvas.addEventListener(
      "wheel",
      (event) => {
        event.preventDefault();
        this.orbit.zoomBy(event.deltaY);
      },
      { passive: false },
    );
  }

  reset(knifeTiltRad: number): void {
    this.drag = null;
    this.tilt.snap(knifeTiltRad);
    this.sentTiltRad = knifeTiltRad;
    this.lower.snap(0);
    this.sentLowerM = 0;
    this.stand.snap(0);
  }

  /**
   * Eases every input one tick and returns it in the core's wire format, `Input::encode` in
   * `src/core/src/input.rs`. A control the core stopped short last tick stops there.
   */
  stepInput(game: Game): Int32Array {
    const dtS = 1 / this.layout.ticks_per_s;
    const [knifeTiltRad, knifeLowerM] = game.knife_pose();
    if (Math.abs(knifeTiltRad - this.sentTiltRad) > this.layout.angle_unit_rad / 2) this.tilt.snap(knifeTiltRad);
    if (knifeLowerM < this.sentLowerM - this.layout.position_unit_m / 2) this.lower.capAt(knifeLowerM + LOWER_SLACK_M);
    if (game.is_stand_blocked()) this.stand.snap(game.stand_angle_rad());
    const encoded = new Int32Array(ENCODED_LEN);
    const [handX, handZ] = [this.handX.step(dtS), this.handZ.step(dtS)];
    if (this.drag?.kind === "cake") {
      encoded[0] = FLAG_HOLDING;
      encoded.set(this.drag.grabAt, 1);
      encoded.set([this.quantizeM(handX), this.quantizeM(handZ)], 4);
    }
    encoded[6] = this.quantizeRad(this.tilt.step(dtS));
    this.sentTiltRad = encoded[6] * this.layout.angle_unit_rad;
    encoded[7] = this.quantizeM(this.lower.step(dtS));
    this.sentLowerM = encoded[7] * this.layout.position_unit_m;
    encoded[8] = this.quantizeRad(this.stand.step(dtS));
    return encoded;
  }

  private onPointerDown(event: PointerEvent): void {
    this.scene.renderer.domElement.setPointerCapture(event.pointerId);
    this.lastPx.set(event.clientX, event.clientY);
    const hit = event.button === 0 ? this.scene.pick(this.aim(event)) : null;
    if (hit?.kind === "cake") {
      const grabAt = hit.point.toArray().map((m) => this.quantizeM(m)) as [number, number, number];
      this.handX.snap(hit.point.x);
      this.handZ.snap(hit.point.z);
      this.drag = { kind: "cake", grabAt, metersPerPx: this.calcMetersPerPx(hit.point) };
    } else if (hit?.kind === "knife") {
      this.lower.halfLifeS = KNIFE_HALF_LIFE_S;
      this.drag = { kind: "knife", tiltSign: this.calcTiltSign() };
    } else if (hit?.kind === "stand") {
      const axisPx = this.toClientPx(new THREE.Vector3(0, hit.point.y, 0));
      const lastBearingRad = calcBearingRad(axisPx, this.lastPx);
      this.drag = { kind: "stand", axisPx, lastBearingRad, turnSign: this.calcTurnSign(hit.point, axisPx) };
    } else {
      this.drag = { kind: "camera" };
    }
    this.updateCursor(hit?.kind ?? null);
  }

  private onPointerMove(event: PointerEvent): void {
    const drag = this.drag;
    if (!drag) {
      this.updateCursor(this.scene.pick(this.aim(event))?.kind ?? null);
      return;
    }
    const dxPx = event.clientX - this.lastPx.x;
    const dyPx = event.clientY - this.lastPx.y;
    this.lastPx.set(event.clientX, event.clientY);
    if (drag.kind === "camera") {
      this.orbit.dragBy(dxPx, dyPx);
    } else if (drag.kind === "cake") {
      const [right, away] = this.orbit.getGroundAxes();
      const delta = right.multiplyScalar(dxPx).addScaledVector(away, -dyPx).multiplyScalar(drag.metersPerPx);
      const { table_half_x_m: halfX, table_half_z_m: halfZ } = this.layout;
      this.handX.target = THREE.MathUtils.clamp(this.handX.target + delta.x, -halfX, halfX);
      this.handZ.target = THREE.MathUtils.clamp(this.handZ.target + delta.z, -halfZ, halfZ);
    } else if (drag.kind === "knife") {
      const maxTilt = this.layout.max_tilt_rad;
      this.tilt.target = THREE.MathUtils.clamp(this.tilt.target + drag.tiltSign * dxPx * TILT_RAD_PER_PX, -maxTilt, maxTilt);
      this.lower.target = THREE.MathUtils.clamp(this.lower.target + dyPx * LOWER_M_PER_PX, 0, this.layout.pivot_y_m);
    } else {
      this.turnStand(drag);
    }
  }

  private onPointerUp(): void {
    if (this.drag?.kind === "knife") {
      this.lower.halfLifeS = KNIFE_RETURN_HALF_LIFE_S;
      this.lower.target = 0;
    }
    this.drag = null;
    this.updateCursor(null);
  }

  /** Meters on the table per pixel of mouse travel at the grabbed point's depth. */
  private calcMetersPerPx(point: THREE.Vector3): number {
    const camera = this.scene.camera;
    const viewHeightM = 2 * camera.position.distanceTo(point) * Math.tan(THREE.MathUtils.degToRad(camera.fov / 2));
    return viewHeightM / this.scene.renderer.domElement.clientHeight;
  }

  /** Which way the knife tilts for the handles to follow the mouse rightward. */
  private calcTiltSign(): number {
    const handleHeightM = this.layout.blade_back_m + this.layout.handle_len_m;
    const pivot = new THREE.Vector3(0, this.layout.pivot_y_m, 0);
    const handleAt = (tilt: number) => new THREE.Vector3(Math.sin(tilt), Math.cos(tilt), 0).multiplyScalar(handleHeightM).add(pivot);
    return this.calcScreenDirection(handleAt(this.tilt.value), handleAt(this.tilt.value + PROBE_STEP));
  }

  /**
   * Turns the stand by how far the mouse swept around the stand's axis on screen, so circling the
   * axis keeps turning it one way and the grabbed point roughly follows the mouse.
   */
  private turnStand(drag: Extract<Drag, { kind: "stand" }>): void {
    if (this.lastPx.distanceTo(drag.axisPx) < STAND_DEAD_ZONE_PX) return;
    const bearingRad = calcBearingRad(drag.axisPx, this.lastPx);
    const sweptRad = Math.atan2(Math.sin(bearingRad - drag.lastBearingRad), Math.cos(bearingRad - drag.lastBearingRad));
    drag.lastBearingRad = bearingRad;
    this.stand.target += drag.turnSign * sweptRad;
  }

  /**
   * +1 where turning the stand sweeps the grabbed point the same way around the axis on screen
   * as a growing bearing, else -1. A grab on the axis takes the rim nearest the camera.
   */
  private calcTurnSign(point: THREE.Vector3, axisPx: THREE.Vector2): number {
    const camera = this.scene.camera.position;
    const bearing = point.x * point.x + point.z * point.z > 1e-8 ? point : camera;
    const rim = new THREE.Vector3(bearing.x, 0, bearing.z).setLength(this.layout.stand_radius_m).setY(point.y);
    const turned = rim.clone().applyAxisAngle(new THREE.Vector3(0, 1, 0), PROBE_STEP);
    const [from, to] = [rim, turned].map((p) => calcBearingRad(axisPx, this.toClientPx(p)));
    return Math.sin(to - from) >= 0 ? 1 : -1;
  }

  private toClientPx(point: THREE.Vector3): THREE.Vector2 {
    const ndc = point.clone().project(this.scene.camera);
    const rect = this.scene.renderer.domElement.getBoundingClientRect();
    return new THREE.Vector2(rect.left + ((ndc.x + 1) / 2) * rect.width, rect.top + ((1 - ndc.y) / 2) * rect.height);
  }

  /** +1 where moving from `from` to `to` goes rightward on screen, else -1. */
  private calcScreenDirection(from: THREE.Vector3, to: THREE.Vector3): number {
    const camera = this.scene.camera;
    return to.clone().project(camera).x >= from.clone().project(camera).x ? 1 : -1;
  }

  private updateCursor(kind: string | null): void {
    const isDragging = this.drag !== null && this.drag.kind !== "camera";
    this.scene.renderer.domElement.style.cursor = isDragging ? "grabbing" : kind ? "grab" : "default";
  }

  private aim(event: PointerEvent): THREE.Raycaster {
    const rect = this.scene.renderer.domElement.getBoundingClientRect();
    const pointer = new THREE.Vector2(((event.clientX - rect.left) / rect.width) * 2 - 1, -((event.clientY - rect.top) / rect.height) * 2 + 1);
    this.raycaster.setFromCamera(pointer, this.scene.camera);
    return this.raycaster;
  }

  private quantizeM(meters: number): number {
    return Math.round(meters / this.layout.position_unit_m);
  }

  private quantizeRad(radians: number): number {
    return Math.round(radians / this.layout.angle_unit_rad);
  }
}

/** Angle of `px` around `axisPx` in screen pixels, y down. */
function calcBearingRad(axisPx: THREE.Vector2, px: THREE.Vector2): number {
  return Math.atan2(px.y - axisPx.y, px.x - axisPx.x);
}
