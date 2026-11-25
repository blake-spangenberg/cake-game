/**
 * Renders one cake alone on a transparent background for the lobby's cake picker, from about the
 * direction the round's starting camera sees it. `tools/render-previews.mjs` drives this page.
 */
import * as THREE from "three";
import init, { Game } from "@wasm/cake_wasm.js";
import { addLights, buildCakeGeometry, buildCakeMaterial, decodeCakeTexture } from "./props";

const CAMERA_YAW_RAD = 0.5;
const CAMERA_PITCH_RAD = 0.45;
const CAMERA_FOV_DEG = 30;
/** How much of the frame's half width the cake's farthest point from center reaches. */
const FILL_SHARE = 0.92;
const WEBP_QUALITY = 0.95;
/** Brighter than the round's sky, with no table or shadows. */
const SKY_INTENSITY = 0.9;

declare global {
  interface Window {
    /** A `.cake` file's preview as a square WebP data URL `sizePx` on a side. */
    renderPreview: (cakeBase64: string, sizePx: number) => Promise<string>;
    isPreviewReady: boolean;
  }
}

await init();
const renderer = new THREE.WebGLRenderer({ canvas: document.querySelector<HTMLCanvasElement>("#preview")!, alpha: true, antialias: true, preserveDrawingBuffer: true });
renderer.setClearColor(0x000000, 0);
window.renderPreview = renderPreview;
window.isPreviewReady = true;

async function renderPreview(cakeBase64: string, sizePx: number): Promise<string> {
  const game = new Game(undefined, Uint8Array.from(atob(cakeBase64), (char) => char.charCodeAt(0)));
  try {
    const texture = await decodeCakeTexture(game, renderer.capabilities.getMaxAnisotropy());
    const mesh = new THREE.Mesh(buildCakeGeometry(game), buildCakeMaterial(texture));
    const pose = game.get_cake_pose();
    mesh.quaternion.set(pose[3], pose[4], pose[5], pose[6]);
    const scene = new THREE.Scene();
    addLights(scene, null, SKY_INTENSITY);
    scene.add(mesh);
    const camera = buildCamera(mesh);
    renderer.setSize(sizePx, sizePx, false);
    renderer.render(scene, camera);
    mesh.geometry.dispose();
    mesh.material.dispose();
    texture?.dispose();
    return renderer.domElement.toDataURL("image/webp", WEBP_QUALITY);
  } finally {
    game.free();
  }
}

/** A camera aimed at the cake's bounding box, as near as keeps every vertex within `FILL_SHARE` of the frame. */
function buildCamera(cake: THREE.Mesh): THREE.PerspectiveCamera {
  cake.updateMatrixWorld();
  const center = new THREE.Box3().setFromObject(cake).getCenter(new THREE.Vector3());
  const camera = new THREE.PerspectiveCamera(CAMERA_FOV_DEG, 1, 0.01, 10);
  const toCamera = new THREE.Vector3(Math.sin(CAMERA_YAW_RAD) * Math.cos(CAMERA_PITCH_RAD), Math.sin(CAMERA_PITCH_RAD), Math.cos(CAMERA_YAW_RAD) * Math.cos(CAMERA_PITCH_RAD));
  camera.position.copy(center).add(toCamera);
  camera.lookAt(center);
  camera.updateMatrixWorld();
  const right = new THREE.Vector3().setFromMatrixColumn(camera.matrixWorld, 0);
  const up = new THREE.Vector3().setFromMatrixColumn(camera.matrixWorld, 1);
  const tanHalfFov = Math.tan(THREE.MathUtils.degToRad(CAMERA_FOV_DEG / 2)) * FILL_SHARE;
  const positions = cake.geometry.getAttribute("position");
  const vertex = new THREE.Vector3();
  let distanceM = 0;
  for (let i = 0; i < positions.count; i += 1) {
    vertex.fromBufferAttribute(positions, i).applyMatrix4(cake.matrixWorld).sub(center);
    const offsetM = Math.max(Math.abs(vertex.dot(right)), Math.abs(vertex.dot(up)));
    distanceM = Math.max(distanceM, vertex.dot(toCamera) + offsetM / tanHalfFov);
  }
  camera.position.copy(center).addScaledVector(toCamera, distanceM);
  return camera;
}
