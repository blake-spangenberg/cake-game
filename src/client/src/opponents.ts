import * as THREE from "three";
import type { Game, Layout } from "@wasm/cake_wasm.js";
import { calcCutMotion } from "./cutMotion";
import { waitFrames } from "./frames";
import type { Member, PlayerPose } from "./net";
import { type ShadowArea, addLights, buildCakeGeometry, buildCakeMaterial, buildKnife, buildStand, decodeCakeTexture, placeKnife } from "./props";
import { playChop } from "./sound";

/** Each tile's backdrop, a neutral shade a step above the page's, so the tiles read as separate views. */
const TILE_COLOR = 0x2e2a26;
const TILE_GAP_PX = 6;
/** Every tile's width over its height, since a low view of a cake on its stand is wider than tall. */
const TILE_ASPECT = 4 / 3;
/** The one low angle every tile shares, from the player's starting side of the table. */
const CAMERA_YAW_RAD = 0.5;
const CAMERA_PITCH_RAD = 0.2;
const CAMERA_FOV_DEG = 30;
/** Room each tile leaves around the farthest the cake reaches from the stand's axis. */
const FRAME_SIDE_MARGIN_M = 0.015;
/**
 * How far above the cake's modeled top each tile reaches, to show the knife's edge, which floats
 * 5 cm over the cake, and more when the cake stands taller than modeled, as a cake hung by its
 * grab often lands.
 */
const FRAME_ABOVE_CAKE_M = 0.12;
/** How far below the plate's top each tile reaches, to show its rim. */
const FRAME_BELOW_PLATE_M = 0.01;
/** Points around each end of the framed cylinder that the camera's fit keeps in view. */
const FRAME_CIRCLE_POINT_LEN = 16;
/** How a finished opponent's frozen view is drawn: gray and a little dim, so the check mark over it stands out. */
const FROZEN_FILTER = "grayscale(1) brightness(0.8)";
/** Follows the name of a player whose page closed. */
const LEFT_MARK = "(left)";
/** The stand and what stands on it, the only things a tile shows. */
const SHADOW: ShadowArea = { extentM: 0.4, mapPx: 1024 };
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
/** Frames the warm-up waits, so its prefiltering lands in a task of its own rather than on whatever asked for it. */
const WARM_UP_DELAY_FRAMES = 3;
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
  /** The name this page last knew for the player, from the round's start on, which a player who left keeps. */
  playerName: string;
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
  /** Once the cut's motion has played, the tile's last drawing, grayed, which shows in place of drawing it again. */
  snapshot: HTMLCanvasElement | null;
}

/** What a tile has to show: nothing before the round's cake loads, the opponent playing, or their finished cut. */
type TileState = "empty" | "playing" | "finished";

/** The cylinder about the stand's axis that every tile keeps in view: the cake on the plate and the knife's edge over it. */
interface Frame {
  radiusM: number;
  bottomYM: number;
  topYM: number;
}

/** The tiles' grid: its columns, each tile's size, and the size of the panel's top right corner it covers, gaps included. */
interface Grid {
  columns: number;
  tileWidthPx: number;
  tileHeightPx: number;
  widthPx: number;
  heightPx: number;
}

/**
 * The opponents' tiles over the right of the screen during an online round, beneath which the
 * player's own game still draws: every opponent's stand, cake, and knife in a tile of a grid, from
 * one shared low angle, with their name. Their poses arrive from the server about 30 times a
 * second, so each tile plays a few ticks behind the newest and interpolates between them. A cut
 * opponent's knife plays the cut's motion down into the cake, which stays whole until the judging
 * reveal parts it, and then their tile grays, freezes, and shows a check mark.
 */
export class Opponents {
  private readonly panel = document.querySelector<HTMLDivElement>("#opponents")!;
  private readonly renderer: THREE.WebGLRenderer;
  private readonly scene = new THREE.Scene();
  private readonly camera = new THREE.PerspectiveCamera(CAMERA_FOV_DEG, TILE_ASPECT, 0.01, 10);
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
  private frame: Frame;
  private viewShiftPx = 0;
  /** The stand's and knife's shaders compiling once `ensureWarm` started it, which a cake's first draw must not overtake, or it would wait on them. */
  private warmingUp: Promise<void> | null = null;

  constructor(
    private readonly layout: Layout,
    ticksPerS: number,
  ) {
    this.tickMs = 1000 / ticksPerS;
    const canvas = this.panel.querySelector<HTMLCanvasElement>("canvas")!;
    this.renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true, powerPreference: "high-performance" });
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFShadowMap;
    addLights(this.scene, SHADOW);
    this.scene.background = new THREE.Color(TILE_COLOR);
    this.stand = buildStand(layout);
    this.knife = buildKnife(layout);
    this.scene.add(this.stand, this.knife);
    this.frame = calcFrame(layout, layout.stand_radius_m, 0);
  }

  /**
   * Compiles the stand's and knife's shaders and prefilters the steel's reflections, once, so the
   * first round's first frame, which ran 90 ms long compiling them, compiles nothing. It holds the
   * page while three prefilters the reflections, 0.2-0.25 s on a browser's first visit, so the
   * page calls it once the player has joined rather than while they type their name.
   */
  ensureWarm(): Promise<void> {
    this.warmingUp ??= this.warmUp();
    return this.warmingUp;
  }

  /**
   * Compiles the tiles' shaders without holding the page where the browser compiles in parallel,
   * which one draw compiling them all did for 0.5-1.2 s, then draws once for the shadow pass's
   * shaders, which `compileAsync` skips.
   */
  private async warmUp(): Promise<void> {
    await waitFrames(WARM_UP_DELAY_FRAMES);
    await this.renderer.compileAsync(this.scene, this.camera);
    this.drawWarmUp();
  }

  /** Shows a tile for each of `players`, the round's players but this page's own, named as the round started, with no cake until `loadCake`. */
  open(players: readonly Member[]): void {
    this.close();
    this.isOpen = true;
    this.opponents = players.map((player) => this.addOpponent(player));
    this.panel.hidden = false;
  }

  /** Drops the tiles of `ids`, players who left the round before it started to spectate it instead. */
  removeOpponents(ids: readonly number[]): void {
    for (const opponent of this.opponents) if (ids.includes(opponent.id)) opponent.tag.remove();
    this.opponents = this.opponents.filter((opponent) => !ids.includes(opponent.id));
    this.layoutKey = "";
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
    this.setViewShiftPx(0);
    this.panel.hidden = true;
  }

  /**
   * How far left of the screen's middle the player's own view should center the table, in pixels,
   * to center it in what the tiles leave uncovered: 0 while no tiles show.
   */
  getViewShiftPx(): number {
    return this.viewShiftPx;
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
    const material = buildCakeMaterial(await decodeCakeTexture(game, this.renderer.capabilities.getMaxAnisotropy()));
    const cake = new THREE.Mesh(geometry, material);
    cake.castShadow = true;
    cake.receiveShadow = true;
    await waitFrames(1);
    if (material.map) this.renderer.initTexture(material.map);
    await this.ensureWarm();
    await this.renderer.compileAsync(cake, this.camera, this.scene);
    if (roundCloses !== this.roundCloses) {
      disposeCake(cake);
      return;
    }
    this.removeCake();
    this.cake = cake;
    this.startPose = startPose;
    this.scene.add(cake);
    const reach = calcCakeReach(geometry);
    this.frame = calcFrame(this.layout, reach.radiusM, reach.heightM);
    this.layoutKey = "";
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
  }

  /**
   * Follows the room's names. A player whose page closed stays in the round, so their tile stays
   * until the round closes, its label marked left and keeping the last name this page knew.
   */
  syncMembers(members: readonly Member[]): void {
    this.names.clear();
    for (const member of members) this.names.set(member.id, member.name);
    for (const opponent of this.opponents) this.labelOpponent(opponent);
  }

  /** Writes `opponent`'s name label from the room's names, marked left when the room no longer lists them. */
  private labelOpponent(opponent: Opponent): void {
    const name = this.names.get(opponent.id);
    if (name !== undefined) opponent.playerName = name;
    const hasLeft = this.names.size > 0 && name === undefined;
    opponent.name.textContent = hasLeft ? `${opponent.playerName} ${LEFT_MARK}`.trim() : opponent.playerName;
    opponent.name.classList.toggle("is-left", hasLeft);
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
      if (opponent.snapshot) continue;
      this.followPoses(opponent, poses.get(opponent.id), nowMs, dtMs);
      const yFromBottomPx = canvasHeightPx - opponent.tilePx.y - this.tileSizePx.y;
      this.renderer.setViewport(opponent.tilePx.x, yFromBottomPx, this.tileSizePx.x, this.tileSizePx.y);
      this.renderer.setScissor(opponent.tilePx.x, yFromBottomPx, this.tileSizePx.x, this.tileSizePx.y);
      const state = this.poseOpponent(opponent, nowMs);
      if (state === "empty") {
        this.renderer.setClearColor(TILE_COLOR, 1);
        this.renderer.clear();
        continue;
      }
      this.renderer.render(this.scene, this.camera);
      if (state === "finished") this.freeze(opponent);
    }
  }

  private addOpponent({ id, name: playerName }: Member): Opponent {
    const tag = document.createElement("div");
    tag.className = "opponent-tag";
    const name = Object.assign(document.createElement("span"), { className: "opponent-name" });
    const check = Object.assign(document.createElement("span"), { className: "opponent-check", hidden: true });
    check.ariaLabel = "cut";
    tag.append(name, check);
    this.panel.append(tag);
    const opponent = { id, tag, name, playerName, check, tilePx: new THREE.Vector2(), history: [], playTick: null, newestSeenMs: 0, cut: null, snapshot: null };
    this.labelOpponent(opponent);
    return opponent;
  }

  /**
   * Copies `opponent`'s tile, just drawn, grayed into a canvas of its own that stands in for it,
   * and shows their check mark. The copy must run before the frame ends, while the drawing lasts.
   */
  private freeze(opponent: Opponent): void {
    const [widthPx, heightPx] = [this.tileSizePx.x, this.tileSizePx.y];
    const snapshot = Object.assign(document.createElement("canvas"), { className: "opponent-snapshot", width: widthPx, height: heightPx });
    const context = snapshot.getContext("2d");
    if (!context) throw new Error("a new canvas gave no 2D context to freeze an opponent's tile into");
    context.filter = FROZEN_FILTER;
    context.drawImage(this.renderer.domElement, opponent.tilePx.x, opponent.tilePx.y, widthPx, heightPx, 0, 0, widthPx, heightPx);
    opponent.tag.prepend(snapshot);
    opponent.snapshot = snapshot;
    opponent.check.hidden = false;
  }

  /** Drops `opponent`'s frozen copy, which no longer fits its tile, so the next frame draws and freezes it again. */
  private thaw(opponent: Opponent): void {
    opponent.snapshot?.remove();
    opponent.snapshot = null;
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

  /** Poses the shared stand, cake, and knife as `opponent`'s tile shows them, returning what that is. */
  private poseOpponent(opponent: Opponent, nowMs: number): TileState {
    const shown = this.shown;
    if (opponent.playTick === null) {
      if (!this.startPose) return "empty";
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
    if (cut && cut.startMs === null && opponent.playTick !== null && opponent.playTick >= cut.tick) {
      cut.startMs = nowMs;
      playChop("opponent");
    }
    if (!cut || cut.startMs === null) {
      placeKnife(this.knife, this.layout, shown[POSE_TILT], shown[POSE_LOWER], 0);
      return "playing";
    }
    const motion = calcCutMotion((nowMs - cut.startMs) / 1000, cut.fromLowerM, cut.toLowerM);
    placeKnife(this.knife, this.layout, shown[POSE_TILT], motion.lowerM, motion.rockRad);
    return motion.isDone && this.cake ? "finished" : "playing";
  }

  /**
   * Fits the canvas to its panel and lays the tiles out in its top right corner, in the grid that
   * gives each the largest tile, each row's tiles to its right, so the uncovered play view stays
   * in one piece.
   */
  private ensureLayout(): void {
    const canvas = this.renderer.domElement;
    const [widthPx, heightPx] = [canvas.clientWidth, canvas.clientHeight];
    const [screenWidthPx, screenHeightPx] = [document.documentElement.clientWidth, document.documentElement.clientHeight];
    const key = `${widthPx}x${heightPx}x${screenWidthPx}x${screenHeightPx}x${this.opponents.length}`;
    if (key === this.layoutKey) return;
    this.layoutKey = key;
    this.renderer.setSize(widthPx, heightPx, false);
    const len = this.opponents.length;
    const grid = calcGrid(len, widthPx, heightPx);
    this.tileSizePx.set(grid.tileWidthPx, grid.tileHeightPx);
    this.opponents.forEach((opponent, idx) => {
      this.thaw(opponent);
      const row = Math.floor(idx / grid.columns);
      const rowLen = Math.min(grid.columns, len - row * grid.columns);
      const column = (idx % grid.columns) + grid.columns - rowLen;
      const leftPx = widthPx - grid.widthPx + TILE_GAP_PX + column * (grid.tileWidthPx + TILE_GAP_PX);
      opponent.tilePx.set(leftPx, TILE_GAP_PX + row * (grid.tileHeightPx + TILE_GAP_PX));
      Object.assign(opponent.tag.style, { left: `${opponent.tilePx.x}px`, top: `${opponent.tilePx.y}px`, width: `${grid.tileWidthPx}px`, height: `${grid.tileHeightPx}px` });
    });
    this.setViewShiftPx(calcViewShiftPx(grid.widthPx, grid.heightPx, screenWidthPx, screenHeightPx));
    if (grid.tileHeightPx > 0) this.aimCamera(grid.tileWidthPx / grid.tileHeightPx);
  }

  /** Records how far the play view should shift, and shows it to the overlays centered on the table (`--view-shift`). */
  private setViewShiftPx(shiftPx: number): void {
    this.viewShiftPx = shiftPx;
    document.body.style.setProperty("--view-shift", `${shiftPx}px`);
  }

  /** Aims the shared camera from its fixed angle at the frame's middle, as near as keeps the whole frame inside a tile of `aspect`. */
  private aimCamera(aspect: number): void {
    const { radiusM, bottomYM, topYM } = this.frame;
    const center = new THREE.Vector3(0, (bottomYM + topYM) / 2, 0);
    const toCamera = new THREE.Vector3(
      Math.sin(CAMERA_YAW_RAD) * Math.cos(CAMERA_PITCH_RAD),
      Math.sin(CAMERA_PITCH_RAD),
      Math.cos(CAMERA_YAW_RAD) * Math.cos(CAMERA_PITCH_RAD),
    );
    this.camera.position.copy(center).add(toCamera);
    this.camera.lookAt(center);
    this.camera.updateMatrixWorld();
    const right = new THREE.Vector3().setFromMatrixColumn(this.camera.matrixWorld, 0);
    const up = new THREE.Vector3().setFromMatrixColumn(this.camera.matrixWorld, 1);
    const tanHalfFovY = Math.tan(THREE.MathUtils.degToRad(CAMERA_FOV_DEG / 2));
    const tanHalfFovX = tanHalfFovY * aspect;
    const offset = new THREE.Vector3();
    let distanceM = 0;
    for (const yM of [bottomYM, topYM]) {
      for (let i = 0; i < FRAME_CIRCLE_POINT_LEN; i += 1) {
        const angle = (2 * Math.PI * i) / FRAME_CIRCLE_POINT_LEN;
        offset.set(radiusM * Math.cos(angle), yM, radiusM * Math.sin(angle)).sub(center);
        const towardM = offset.dot(toCamera);
        distanceM = Math.max(distanceM, towardM + Math.abs(offset.dot(right)) / tanHalfFovX, towardM + Math.abs(offset.dot(up)) / tanHalfFovY);
      }
    }
    this.camera.position.copy(center).addScaledVector(toCamera, distanceM);
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

/**
 * The grid of `len` tiles of `TILE_ASPECT` that fits the largest tiles in a panel of the given
 * size, or an empty one for no tiles.
 */
function calcGrid(len: number, panelWidthPx: number, panelHeightPx: number): Grid {
  let best = { columns: 1, tileWidthPx: 0 };
  for (let columns = 1; columns <= len; columns += 1) {
    const rows = Math.ceil(len / columns);
    const byWidthPx = (panelWidthPx - TILE_GAP_PX * (columns + 1)) / columns;
    const byHeightPx = ((panelHeightPx - TILE_GAP_PX * (rows + 1)) / rows) * TILE_ASPECT;
    const tileWidthPx = Math.floor(Math.min(byWidthPx, byHeightPx));
    if (tileWidthPx > best.tileWidthPx) best = { columns, tileWidthPx };
  }
  if (best.tileWidthPx <= 0) return { columns: 1, tileWidthPx: 0, tileHeightPx: 0, widthPx: 0, heightPx: 0 };
  const { columns, tileWidthPx } = best;
  const tileHeightPx = Math.floor(tileWidthPx / TILE_ASPECT);
  const rows = Math.ceil(len / columns);
  return { columns, tileWidthPx, tileHeightPx, widthPx: columns * (tileWidthPx + TILE_GAP_PX) + TILE_GAP_PX, heightPx: rows * (tileHeightPx + TILE_GAP_PX) + TILE_GAP_PX };
}

/**
 * How far left of the screen's middle lies the middle of what a cover of the given size in its top
 * right corner leaves uncovered (its centroid's x), in whole pixels.
 */
function calcViewShiftPx(coverWidthPx: number, coverHeightPx: number, screenWidthPx: number, screenHeightPx: number): number {
  const coverArea = coverWidthPx * coverHeightPx;
  const uncoveredArea = screenWidthPx * screenHeightPx - coverArea;
  if (uncoveredArea <= 0) return 0;
  return Math.round((coverArea * (screenWidthPx - coverWidthPx)) / (2 * uncoveredArea));
}

/** How far the cake reaches from its own vertical axis, and its height, resting the way it was modeled, serving side down. */
function calcCakeReach(geometry: THREE.BufferGeometry): { radiusM: number; heightM: number } {
  const positions = geometry.getAttribute("position").array;
  let radiusSquaredM2 = 0;
  let heightM = 0;
  for (let i = 0; i < positions.length; i += 3) {
    radiusSquaredM2 = Math.max(radiusSquaredM2, positions[i] * positions[i] + positions[i + 2] * positions[i + 2]);
    heightM = Math.max(heightM, positions[i + 1]);
  }
  return { radiusM: Math.sqrt(radiusSquaredM2), heightM };
}

/** The frame that holds a cake reaching `cakeRadiusM` from the stand's axis and standing `cakeHeightM` tall on the plate, with the knife's edge over it. */
function calcFrame(layout: Layout, cakeRadiusM: number, cakeHeightM: number): Frame {
  return {
    radiusM: cakeRadiusM + FRAME_SIDE_MARGIN_M,
    bottomYM: layout.stand_top_y_m - FRAME_BELOW_PLATE_M,
    topYM: layout.stand_top_y_m + cakeHeightM + FRAME_ABOVE_CAKE_M,
  };
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
