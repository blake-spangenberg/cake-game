import * as THREE from "three";
import { Spring } from "./spring";

/** Negative, with the cake's spawn and the knife's start tilt, so the cake starts on the screen's left, away from the opponents' view. */
const START_YAW_RAD = -0.5;
const START_PITCH_RAD = 0.5;
/** Distances suit the scene camera's field of view: the start frames the table, the closest the cake with the knife resting over it. */
const START_DISTANCE_M = 1.15;
const MIN_PITCH_RAD = 0.08;
const MAX_PITCH_RAD = 1.5;
const MIN_DISTANCE_M = 0.38;
const MAX_DISTANCE_M = 2.2;
/** A 180 px drag swings the camera about 62 degrees, as `doc/design.md` Controls asks. */
const DRAG_RAD_PER_PX = 0.006;
const KEY_RAD_PER_S = 1.6;
const ZOOM_PER_WHEEL_DELTA = 0.0012;
const EASE_HALF_LIFE_S = 0.06;
/** Caps how much a low view stretches mouse travel away from the camera. */
const MIN_FORESHORTENING = 0.35;
const KEY_YAW: Record<string, number> = { ArrowLeft: -1, KeyA: -1, ArrowRight: 1, KeyD: 1 };
const KEY_PITCH: Record<string, number> = { ArrowUp: 1, KeyW: 1, ArrowDown: -1, KeyS: -1 };
/** How far up a cake the play and judging cameras aim, as a share of its height. */
export const AIM_HEIGHT_SHARE = 2 / 3;

/**
 * Swings the camera around a point over the stand's axis, by drag, wheel, or arrow keys and WASD,
 * from where every round starts.
 */
export class Orbit {
  private readonly yaw = new Spring(START_YAW_RAD, EASE_HALF_LIFE_S);
  private readonly pitch = new Spring(START_PITCH_RAD, EASE_HALF_LIFE_S);
  private readonly distance = new Spring(START_DISTANCE_M, EASE_HALF_LIFE_S);
  private readonly heldKeys = new Set<string>();
  private readonly center = new THREE.Vector3();

  /** A camera aimed over the stand's plate at `standTopYM`, until `resetForCake` aims it at a cake. */
  constructor(
    private readonly camera: THREE.PerspectiveCamera,
    private readonly standTopYM: number,
  ) {
    this.center.setY(standTopYM);
    window.addEventListener("keydown", (event) => {
      if (!(event.code in KEY_YAW || event.code in KEY_PITCH)) return;
      event.preventDefault();
      this.heldKeys.add(event.code);
    });
    window.addEventListener("keyup", (event) => this.heldKeys.delete(event.code));
    window.addEventListener("blur", () => this.heldKeys.clear());
    this.update(0);
  }

  /**
   * Table directions that follow the mouse on screen at `point`: rightward, then away from the
   * camera, lengthened to undo the foreshortening of the line of sight to `point`, which a wide
   * view tilts well away from the camera's own pitch near the screen's edges.
   */
  getGroundAxes(point: THREE.Vector3): [THREE.Vector3, THREE.Vector3] {
    const yaw = this.yaw.value;
    const sightSinElevation = (this.camera.position.y - point.y) / this.camera.position.distanceTo(point);
    const stretch = 1 / Math.max(sightSinElevation, MIN_FORESHORTENING);
    return [new THREE.Vector3(Math.cos(yaw), 0, -Math.sin(yaw)), new THREE.Vector3(-Math.sin(yaw), 0, -Math.cos(yaw)).multiplyScalar(stretch)];
  }

  /**
   * Puts the camera back where every round starts, aimed over the stand at two thirds of the
   * height of a round's cake `cakeHeightM` tall as served, as if it stood on the plate.
   */
  resetForCake(cakeHeightM: number): void {
    this.center.setY(this.standTopYM + AIM_HEIGHT_SHARE * cakeHeightM);
    this.yaw.snap(START_YAW_RAD);
    this.pitch.snap(START_PITCH_RAD);
    this.distance.snap(START_DISTANCE_M);
    this.update(0);
  }

  dragBy(dxPx: number, dyPx: number): void {
    this.yaw.target -= dxPx * DRAG_RAD_PER_PX;
    this.setPitchTarget(this.pitch.target + dyPx * DRAG_RAD_PER_PX);
  }

  zoomBy(wheelDelta: number): void {
    const distance = this.distance.target * Math.exp(wheelDelta * ZOOM_PER_WHEEL_DELTA);
    this.distance.target = THREE.MathUtils.clamp(distance, MIN_DISTANCE_M, MAX_DISTANCE_M);
  }

  update(dtS: number): void {
    for (const code of this.heldKeys) {
      this.yaw.target += (KEY_YAW[code] ?? 0) * KEY_RAD_PER_S * dtS;
      this.setPitchTarget(this.pitch.target + (KEY_PITCH[code] ?? 0) * KEY_RAD_PER_S * dtS);
    }
    const [yaw, pitch, distance] = [this.yaw.step(dtS), this.pitch.step(dtS), this.distance.step(dtS)];
    const offset = new THREE.Vector3(Math.sin(yaw) * Math.cos(pitch), Math.sin(pitch), Math.cos(yaw) * Math.cos(pitch));
    this.camera.position.copy(this.center).addScaledVector(offset, distance);
    this.camera.lookAt(this.center);
  }

  private setPitchTarget(pitch: number): void {
    this.pitch.target = THREE.MathUtils.clamp(pitch, MIN_PITCH_RAD, MAX_PITCH_RAD);
  }
}
