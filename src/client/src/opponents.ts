import * as THREE from "three";
import type { Game, Layout } from "@wasm/cake_wasm.js";
import { calcCutMotion } from "./cutMotion";
import type { Member, PlayerPose } from "./net";
import { addLights, buildCakeGeometry, buildKnife, buildStand, decodeCakeMaterial, placeKnife } from "./props";

/** Each tile's backdrop, a neutral shade a step above the page's, so the tiles read as separate views. */
const TILE_COLOR = 0x2e2a26;
const TILE_GAP_PX = 6;
/** The one angle every tile shares: the player's starting camera's yaw and pitch. */
const CAMERA_YAW_RAD = 0.5;
const CAMERA_PITCH_RAD = 0.5;
const CAMERA_FOV_DEG = 30;
/** Every tile frames a sphere this far around a point this far above the stand's plate: the stand, a cake on it, and the knife over it. */
const FRAME_RADIUS_M = 0.3;
const FRAME_ABOVE_STAND_M = 0.1;
const SHADOW_EXTENT_M = 0.4;
const SHADOW_MAP_PX = 1024;
/**
 * How far behind an opponent's newest pose their tile plays, in ticks: poses arrive every 2 ticks,
 * so this rides out one late frame without stopping.
 */
const BUFFER_TICKS = 4;
/** A tile further than this from where its buffer says it should be jumps there rather than easing. */
const SNAP_TICKS = 30;
/** Share of the gap to where the buffer says it should be that a tile's playback closes each frame. */
const CORRECTION_SHARE = 0.05;
/**
 * Frames to let pass before building the cake, since it is asked for right after the player's
 * own round swaps in, whose next frames already run long drawing the new cake for the first time.
 */
const SETTLE_FRAMES = 4;
/** Records kept behind a tile's playback, enough to interpolate from. */
const HISTORY_BEHIND_LEN = 2;
/** The pose values `Net` reads: cake translation (3) and rotation quaternion (4), knife tilt, lowering, and rest, stand angle. */
const POSE_LEN = 11;
const POSE_QUATERNION = 3;
const POSE_TILT = 7;
const POSE_LOWER = 8;
const POSE_STAND = 10;

/** An opponent's world at one tick, in `PlayerPose.values`' layout. */
interface PoseRecord {
  tick: number;
  values: Float32Array;
}

/** An opponent's cut as the server announced it: its tick, and the knife's lowering at the press and at the cut's bottom. */
interface Cut {
  tick: number;
  fromLowerM: number;
  toLowerM: number;
  /** When this page began playing the cut's motion, or none until playback reaches the cut's tick. */
  startMs: number | null;
}

interface Opponent {
  id: number;
  tag: HTMLDivElement;
  name: HTMLSpanElement;
  check: HTMLSpanElement;
  /** The tile's top left corner in the panel, from the grid's last layout. */
  tilePx: THREE.Vector2;
  /** Poses by ascending tick. */
  history: PoseRecord[];
  /** The tick the tile shows, between poses, or none before the first pose arrives. */
  playTick: number | null;
  /** When this page first saw the newest pose in `history`. */
  newestSeenMs: number;
  cut: Cut | null;
}

/**
 * The right side of the screen during an online round: every opponent's stand, cake, and knife in
 * a tile of a grid, from one shared angle, with their name and a check mark once they cut. Their
 * poses arrive from the server about 30 times a second, so each tile plays a few ticks behind the
 * newest and interpolates between them. A cut opponent's knife plays the cut's motion down into
 * the cake, which stays whole until the judging reveal parts it.
 */
export class Opponents {
  private readonly panel = document.querySelector<HTMLDivElement>("#opponents")!;
  private readonly renderer: THREE.WebGLRenderer;
  private readonly scene = new THREE.Scene();
  private readonly camera = new THREE.PerspectiveCamera(CAMERA_FOV_DEG, 1, 0.01, 10);
  private readonly stand: THREE.Group;
  private readonly knife: THREE.Group;
  private readonly tickMs: number;
  private cake: THREE.Mesh<THREE.BufferGeometry, THREE.MeshStandardMaterial> | null = null;
  /** The round's starting pose, shown for an opponent before any of theirs arrives. */
  private startPose: Float32Array | null = null;
  private opponents: Opponent[] = [];
  private readonly names = new Map<number, string>();
  /** Counts closes, which every open starts with, so a cake that finished loading after its round closed is dropped. */
  private roundCloses = 0;
  private isOpen = false;
  /** The tiles' size, for the grid's last layout, which changes with the panel's size and the opponents' count. */
  private readonly tileSizePx = new THREE.Vector2();
  private layoutKey = "";
  private lastRenderMs: number | null = null;
  private readonly shown = new Float32Array(POSE_LEN);

  constructor(
    private readonly layout: Layout,
    ticksPerS: number,
  ) {
    this.tickMs = 1000 / ticksPerS;
    const canvas = this.panel.querySelector<HTMLCanvasElement>("canvas")!;
    this.renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true, powerPreference: "high-performance" });
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFShadowMap;
    addLights(this.scene, SHADOW_EXTENT_M, SHADOW_MAP_PX);
    this.scene.background = new THREE.Color(TILE_COLOR);
    this.stand = buildStand(layout);
    this.knife = buildKnife(layout);
    this.scene.add(this.stand, this.knife);
    // While the page loads, rather than in the first frame of the first round, which ran 90 ms long compiling these.
    this.drawWarmUp();
  }

  /** Shows a tile for each of `ids`, the round's players but this page's own, with no cake until `loadCake`. */
  open(ids: readonly number[]): void {
    this.close();
    this.isOpen = true;
    this.opponents = ids.map((id) => this.addOpponent(id));
    document.body.classList.add("is-split");
    this.panel.hidden = false;
  }

  /** Hides the tiles until the next round opens. */
  close(): void {
    this.roundCloses += 1;
    this.isOpen = false;
    for (const opponent of this.opponents) opponent.tag.remove();
    this.opponents = [];
    this.layoutKey = "";
    this.lastRenderMs = null;
    this.startPose = null;
    this.removeCake();
    document.body.classList.remove("is-split");
    this.panel.hidden = true;
  }

  /**
   * Gives the tiles the open round's cake from `game`, which must stay alive until this resolves.
   * The work spreads over several frames, so none runs long, and finishes with a first draw, so
   * the tiles' next frame uploads and compiles nothing.
   */
  async loadCake(game: Game): Promise<void> {
    const roundCloses = this.roundCloses;
    await waitFrames(SETTLE_FRAMES);
    if (roundCloses !== this.roundCloses) return;
    const startPose = readPose(game);
    const geometry = buildCakeGeometry(game);
    const material = await decodeCakeMaterial(game, this.renderer.capabilities.getMaxAnisotropy());
    const cake = new THREE.Mesh(geometry, material);
    cake.castShadow = true;
    cake.receiveShadow = true;
    await waitFrames(1);
    if (material.map) this.renderer.initTexture(material.map);
    await this.renderer.compileAsync(cake, this.camera, this.scene);
    if (roundCloses !== this.roundCloses) {
      disposeCake(cake);
      return;
    }
    this.removeCake();
    this.cake = cake;
    this.startPose = startPose;
    this.scene.add(cake);
    this.drawWarmUp();
  }

  /** Draws the scene into a single pixel, so it uploads the cake's buffers and compiles its shadow pass now. */
  private drawWarmUp(): void {
    this.renderer.setScissorTest(true);
    this.renderer.setViewport(0, 0, 1, 1);
    this.renderer.setScissor(0, 0, 1, 1);
    this.renderer.render(this.scene, this.camera);
  }

  /** Marks `id` cut at `tick`, with `pose` as the cut froze it and the knife to lower `cutLowerM` below the pivot. */
  addCut(id: number, tick: number, pose: { cake: number[]; knife: number[]; stand_rad: number }, cutLowerM: number): void {
    const opponent = this.opponents.find((candidate) => candidate.id === id);
    if (!opponent || opponent.cut) return;
    const values = Float32Array.from([...pose.cake, ...pose.knife, pose.stand_rad]);
    opponent.history = [...opponent.history.filter((record) => record.tick < tick), { tick, values }];
    // A cut from before this page saw the opponent move, as a late joiner gets, shows as finished.
    const startMs = opponent.playTick === null ? -Infinity : null;
    opponent.cut = { tick, fromLowerM: pose.knife[1], toLowerM: cutLowerM, startMs };
    opponent.check.hidden = false;
  }

  /** Follows the room's names, and drops the tile of anyone who left. */
  syncMembers(members: readonly Member[]): void {
    this.names.clear();
    for (const member of members) this.names.set(member.id, member.name);
    for (const opponent of this.opponents) {
      if (this.names.has(opponent.id)) {
        opponent.name.textContent = this.names.get(opponent.id)!;
        continue;
      }
      opponent.tag.remove();
    }
    this.opponents = this.opponents.filter((opponent) => this.names.has(opponent.id));
  }

  /** Draws every tile at `nowMs`, from the newest poses the server sent. */
  render(nowMs: number, poses: ReadonlyMap<number, PlayerPose>): void {
    if (!this.isOpen) return;
    const dtMs = this.lastRenderMs === null ? 0 : nowMs - this.lastRenderMs;
    this.lastRenderMs = nowMs;
    this.ensureLayout();
    this.renderer.setScissorTest(false);
    this.renderer.setClearColor(0x000000, 0);
    this.renderer.clear();
    this.renderer.setScissorTest(true);
    const canvasHeightPx = this.renderer.domElement.height;
    for (const opponent of this.opponents) {
      this.followPoses(opponent, poses.get(opponent.id), nowMs, dtMs);
      const yFromBottomPx = canvasHeightPx - opponent.tilePx.y - this.tileSizePx.y;
      this.renderer.setViewport(opponent.tilePx.x, yFromBottomPx, this.tileSizePx.x, this.tileSizePx.y);
      this.renderer.setScissor(opponent.tilePx.x, yFromBottomPx, this.tileSizePx.x, this.tileSizePx.y);
      if (this.poseOpponent(opponent, nowMs)) {
        this.renderer.render(this.scene, this.camera);
        continue;
      }
      this.renderer.setClearColor(TILE_COLOR, 1);
      this.renderer.clear();
    }
  }

  private addOpponent(id: number): Opponent {
    const tag = document.createElement("div");
    tag.className = "opponent-tag";
    const name = Object.assign(document.createElement("span"), { className: "opponent-name", textContent: this.names.get(id) ?? "" });
    const check = Object.assign(document.createElement("span"), { className: "opponent-check", hidden: true });
    check.ariaLabel = "cut";
    tag.append(name, check);
    this.panel.append(tag);
    return { id, tag, name, check, tilePx: new THREE.Vector2(), history: [], playTick: null, newestSeenMs: 0, cut: null };
  }

  /** Records the server's newest pose for `opponent`, and advances the tick its tile plays toward a few ticks behind it. */
  private followPoses(opponent: Opponent, pose: PlayerPose | undefined, nowMs: number, dtMs: number): void {
    const newest = opponent.history.at(-1);
    if (pose && !opponent.cut && (!newest || pose.tick > newest.tick)) {
      opponent.history.push({ tick: pose.tick, values: pose.values });
      opponent.newestSeenMs = nowMs;
    }
    const last = opponent.history.at(-1);
    if (!last || (opponent.playTick === null && !pose)) return;
    const targetTick = last.tick + (nowMs - opponent.newestSeenMs) / this.tickMs - BUFFER_TICKS;
    const isFar = opponent.playTick === null || Math.abs(targetTick - opponent.playTick) > SNAP_TICKS;
    const eased = isFar ? targetTick : opponent.playTick! + dtMs / this.tickMs + (targetTick - opponent.playTick!) * CORRECTION_SHARE;
    opponent.playTick = Math.min(Math.max(eased, opponent.history[0].tick), last.tick);
    const behindLen = opponent.history.filter((record) => record.tick <= opponent.playTick!).length;
    if (behindLen > HISTORY_BEHIND_LEN) opponent.history.splice(0, behindLen - HISTORY_BEHIND_LEN);
  }

  /** Poses the shared stand, cake, and knife as `opponent`'s tile shows them, returning whether there is anything to show. */
  private poseOpponent(opponent: Opponent, nowMs: number): boolean {
    const shown = this.shown;
    if (opponent.playTick === null) {
      if (!this.startPose) return false;
      shown.set(this.startPose);
    } else {
      interpolate(opponent.history, opponent.playTick, shown);
    }
    if (this.cake) {
      this.cake.position.fromArray(shown, 0);
      this.cake.quaternion.fromArray(shown, POSE_QUATERNION);
    }
    this.stand.rotation.y = shown[POSE_STAND];
    const cut = opponent.cut;
    if (cut && cut.startMs === null && opponent.playTick !== null && opponent.playTick >= cut.tick) cut.startMs = nowMs;
    if (!cut || cut.startMs === null) {
      placeKnife(this.knife, this.layout, shown[POSE_TILT], shown[POSE_LOWER], 0);
      return true;
    }
    const motion = calcCutMotion((nowMs - cut.startMs) / 1000, cut.fromLowerM, cut.toLowerM);
    placeKnife(this.knife, this.layout, shown[POSE_TILT], motion.lowerM, motion.rockRad);
    return true;
  }

  /** Fits the canvas to its panel and lays the tiles out in the grid that gives each the largest square. */
  private ensureLayout(): void {
    const canvas = this.renderer.domElement;
    const [widthPx, heightPx] = [canvas.clientWidth, canvas.clientHeight];
    const key = `${widthPx}x${heightPx}x${this.opponents.length}`;
    if (key === this.layoutKey) return;
    this.layoutKey = key;
    this.renderer.setSize(widthPx, heightPx, false);
    const len = Math.max(this.opponents.length, 1);
    let best = { columns: 1, rows: len, sidePx: 0 };
    for (let columns = 1; columns <= len; columns += 1) {
      const rows = Math.ceil(len / columns);
      const sidePx = Math.min((widthPx - TILE_GAP_PX * (columns + 1)) / columns, (heightPx - TILE_GAP_PX * (rows + 1)) / rows);
      if (sidePx > best.sidePx) best = { columns, rows, sidePx };
    }
    const tileWidthPx = Math.floor((widthPx - TILE_GAP_PX * (best.columns + 1)) / best.columns);
    const tileHeightPx = Math.floor((heightPx - TILE_GAP_PX * (best.rows + 1)) / best.rows);
    this.tileSizePx.set(tileWidthPx, tileHeightPx);
    this.opponents.forEach((opponent, idx) => {
      const [column, row] = [idx % best.columns, Math.floor(idx / best.columns)];
      opponent.tilePx.set(TILE_GAP_PX + column * (tileWidthPx + TILE_GAP_PX), TILE_GAP_PX + row * (tileHeightPx + TILE_GAP_PX));
      Object.assign(opponent.tag.style, { left: `${opponent.tilePx.x}px`, top: `${opponent.tilePx.y}px`, width: `${tileWidthPx}px`, height: `${tileHeightPx}px` });
    });
    this.aimCamera(tileWidthPx / tileHeightPx);
  }

  /** Aims the shared camera from its fixed angle, as near as keeps the framed sphere inside a tile of `aspect`. */
  private aimCamera(aspect: number): void {
    const center = new THREE.Vector3(0, this.layout.stand_top_y_m + FRAME_ABOVE_STAND_M, 0);
    const halfFovRad = THREE.MathUtils.degToRad(CAMERA_FOV_DEG / 2);
    const narrowHalfFovRad = Math.min(halfFovRad, Math.atan(Math.tan(halfFovRad) * aspect));
    const distanceM = FRAME_RADIUS_M / Math.sin(narrowHalfFovRad);
    const toCamera = new THREE.Vector3(
      Math.sin(CAMERA_YAW_RAD) * Math.cos(CAMERA_PITCH_RAD),
      Math.sin(CAMERA_PITCH_RAD),
      Math.cos(CAMERA_YAW_RAD) * Math.cos(CAMERA_PITCH_RAD),
    );
    this.camera.position.copy(center).addScaledVector(toCamera, distanceM);
    this.camera.lookAt(center);
    this.camera.aspect = aspect;
    this.camera.updateProjectionMatrix();
  }

  private removeCake(): void {
    if (!this.cake) return;
    this.scene.remove(this.cake);
    disposeCake(this.cake);
    this.cake = null;
  }
}

/** The world's pose in `PlayerPose.values`' layout. */
function readPose(game: Game): Float32Array {
  return Float32Array.from([...game.get_cake_pose(), ...game.get_knife_pose(), game.get_stand_angle_rad()]);
}

/** Writes into `out` the pose at the fractional `tick`, between the records either side of it, or the nearest end's. */
function interpolate(history: readonly PoseRecord[], tick: number, out: Float32Array): void {
  const afterIdx = history.findIndex((record) => record.tick > tick);
  if (afterIdx <= 0) {
    out.set((afterIdx === 0 ? history[0] : history.at(-1)!).values);
    return;
  }
  const [before, after] = [history[afterIdx - 1], history[afterIdx]];
  const t = (tick - before.tick) / (after.tick - before.tick);
  for (let i = 0; i < POSE_LEN; i += 1) out[i] = before.values[i] + (after.values[i] - before.values[i]) * t;
  const rotation = new THREE.Quaternion().fromArray(before.values, POSE_QUATERNION).slerp(new THREE.Quaternion().fromArray(after.values, POSE_QUATERNION), t);
  out.set([rotation.x, rotation.y, rotation.z, rotation.w], POSE_QUATERNION);
}

function disposeCake(cake: THREE.Mesh<THREE.BufferGeometry, THREE.MeshStandardMaterial>): void {
  cake.geometry.dispose();
  cake.material.map?.dispose();
  cake.material.dispose();
}

/** Resolves just after the `count`th next frame is drawn, so work awaiting it lands between frames rather than stacking onto one. */
async function waitFrames(count: number): Promise<void> {
  for (let i = 0; i < count; i += 1) await new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0)));
}
