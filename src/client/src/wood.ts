/** Wood from `woodPaint.ts` as a texture covering a whole surface so it never tiles. */
import * as THREE from "three";
import type { WoodLook } from "./woodPaint";

/** `texels` from `paintWood(look)` as a mipmapped sRGB texture, as sharp at grazing angles as `anisotropy` allows. */
export function buildWoodTexture(texels: Uint8Array, look: WoodLook, anisotropy: number): THREE.DataTexture {
  const texture = new THREE.DataTexture(texels, look.widthPx, look.heightPx);
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.wrapS = THREE.RepeatWrapping;
  texture.wrapT = THREE.RepeatWrapping;
  texture.magFilter = THREE.LinearFilter;
  texture.minFilter = THREE.LinearMipmapLinearFilter;
  texture.generateMipmaps = true;
  texture.anisotropy = anisotropy;
  texture.needsUpdate = true;
  return texture;
}

/**
 * Gives a box-like `geometry` texture coordinates that lay the wood's grain along `grainAxis` at
 * its real size, projecting each face along its normal, so a long thin part shows long grain.
 */
export function setWoodUvs(geometry: THREE.BufferGeometry, look: WoodLook, grainAxis: 0 | 1 | 2): void {
  const positions = geometry.getAttribute("position");
  const normals = geometry.getAttribute("normal");
  const uvs = new Float32Array(positions.count * 2);
  const point = new THREE.Vector3();
  const normal = new THREE.Vector3();
  for (let i = 0; i < positions.count; i += 1) {
    point.fromBufferAttribute(positions, i);
    normal.fromBufferAttribute(normals, i);
    const [alongAxis, acrossAxis] = calcFaceAxes(findDominantAxis(normal), grainAxis);
    uvs[2 * i] = point.getComponent(alongAxis) / look.lengthM + 0.5;
    uvs[2 * i + 1] = point.getComponent(acrossAxis) / look.breadthM + 0.5;
  }
  geometry.setAttribute("uv", new THREE.BufferAttribute(uvs, 2));
}

function findDominantAxis(normal: THREE.Vector3): 0 | 1 | 2 {
  const [ax, ay, az] = [Math.abs(normal.x), Math.abs(normal.y), Math.abs(normal.z)];
  if (ax >= ay && ax >= az) return 0;
  return ay >= az ? 1 : 2;
}

/** The face's two in-plane axes, the grain's first where the face shows it, else the end grain's. */
function calcFaceAxes(faceAxis: 0 | 1 | 2, grainAxis: 0 | 1 | 2): [number, number] {
  const inPlane = [0, 1, 2].filter((axis) => axis !== faceAxis);
  if (!inPlane.includes(grainAxis)) return [inPlane[0], inPlane[1]];
  return [grainAxis, inPlane.find((axis) => axis !== grainAxis)!];
}
