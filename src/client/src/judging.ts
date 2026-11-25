import * as THREE from "three";
import type { Layout } from "@wasm/cake_wasm.js";
import type { SlicedJob } from "./halfRest";
import type { CutMessage, JudgingMessage, Standing } from "./net";
import { JUDGED_TIMING, calcCountedS, calcRevealFrame } from "./reveal";
import { type PreparedCut, type Scene, type StagedCut, finishJob } from "./scene";
import { playCountTick } from "./sound";

/** How long a frame may spend preparing judging scenes, as cuts come in during play and through the reveal. */
const PREPARE_SLICE_MS = 2;
/** Where each scene's orbit starts, from the player's side of the table, looking along the knife's edge so the halves part sideways. */
const ORBIT_START_YAW_RAD = 0.8;
/** Clockwise seen from above. */
const ORBIT_RAD_PER_S = -0.11;
/** High enough to look over the knife's near knob into the cut. */
const ORBIT_PITCH_RAD = 0.55;
/** How far the camera stands from the cake's middle, per meter of its bounds' half diagonal, and at least. */
const FRAME_DISTANCE_PER_RADIUS = 3.4;
const MIN_FRAME_DISTANCE_M = 0.5;
/** How far above the cake's middle the camera looks, as a share of its bounds' half diagonal, which lowers the cake on screen, under the name and the labels over it. */
const LOOK_ABOVE_SHARE = 0.35;
/** The least time between two of the counters' ticks, so their fast start rattles rather than buzzes. */
const TICK_GAP_MS = 70;

/** A cut the judging will show, and what `Scene.prepareCut` found for it once done. */
interface Prep {
  cut: StagedCut;
  job: SlicedJob<PreparedCut> | null;
  prepared: PreparedCut | null;
}

/** The reveal under way: when it began on `performance.now()`'s clock, its scenes, and how far this page has shown them. */
interface Reveal {
  startMs: number;
  order: readonly number[];
  sceneMs: number;
  standings: ReadonlyMap<number, Standing>;
  /** The scene now on screen, or -1 before the first is staged. */
  stagedIdx: number;
  /** The players whose score shows on the leaderboard. */
  revealed: Set<number>;
  shownG: number;
  lastTickMs: number;
}

/**
 * The judging reveal every page shows in step once play ends: one player at a time in the
 * server's random order, each on their own stand with their cake's halves still together, the
 * knife plunged, and their name at the top, under a camera orbiting slowly clockwise. The halves
 * part as a gram counter over each counts up, then the score shows and the player joins a
 * leaderboard beside the view that starts empty. Each scene is built from the player's `cut`
 * message, prepared a slice per frame as the cuts arrive, so no scene starts with a long frame.
 */
export class Judging {
  private readonly overlay = document.querySelector<HTMLDivElement>("#judging")!;
  private readonly nameLabel = document.querySelector<HTMLHeadingElement>("#judged-name")!;
  private readonly scoreLabel = document.querySelector<HTMLDivElement>("#judged-score")!;
  private readonly board = document.querySelector<HTMLElement>("#live-board")!;
  private readonly boardList = document.querySelector<HTMLOListElement>("#live-leaderboard")!;
  private readonly preps = new Map<number, Prep>();
  /** Whether the scene holds the round's cake, which every prepared cut is found on. */
  private isCakeReady = false;
  private reveal: Reveal | null = null;

  constructor(
    private readonly scene: Scene,
    private readonly layout: Layout,
  ) {}

  /** Forgets the last round, for one whose cake the scene does not hold yet. */
  open(): void {
    this.stop();
    this.preps.clear();
    this.isCakeReady = false;
  }

  /** Notes player `id`'s cut, to prepare its scene once the round's cake is in the scene. */
  addCut(cut: CutMessage): void {
    const [tiltRad] = cut.pose.knife;
    this.preps.set(cut.id, {
      cut: { cakePose: cut.pose.cake, standRad: cut.pose.stand_rad, tiltRad, lowerM: cut.cut_lower_m, isCutThrough: cut.is_cut_through },
      job: null,
      prepared: null,
    });
  }

  /** Notes that the scene now holds the round's cake, as swapping it in left it whole, so the scene on screen is staged again. */
  setCakeReady(): void {
    this.isCakeReady = true;
    if (this.reveal) this.reveal.stagedIdx = -1;
  }

  /** Starts showing the reveal `message` describes, begun at `startMs` on `performance.now()`'s clock, with an empty leaderboard. */
  start(message: JudgingMessage, startMs: number): void {
    const standings = new Map(message.standings.map((standing) => [standing.id, standing]));
    this.reveal = { startMs, order: message.order, sceneMs: message.scene_ms, standings, stagedIdx: -1, revealed: new Set(), shownG: 0, lastTickMs: -Infinity };
    this.boardList.replaceChildren();
    this.nameLabel.textContent = "";
    this.scoreLabel.hidden = true;
    this.overlay.hidden = false;
  }

  /** Ends the reveal, if one shows, giving the scene back with its cake whole. */
  stop(): void {
    if (this.reveal) this.scene.showWholeCake();
    this.reveal = null;
    this.overlay.hidden = true;
  }

  isShowing(): boolean {
    return this.reveal !== null;
  }

  /** How far left of the canvas's middle the view should center the cake, to center it in what the leaderboard leaves. */
  getViewShiftPx(): number {
    if (!this.reveal) return 0;
    return Math.round((document.documentElement.clientWidth - this.board.getBoundingClientRect().left) / 2);
  }

  /** Prepares scenes for a slice of the frame, then poses the scene, camera, and labels for the reveal at `nowMs`, if one shows. */
  update(nowMs: number): void {
    this.stepPreps();
    const reveal = this.reveal;
    if (!reveal || reveal.order.length === 0) return;
    const elapsedMs = Math.max(nowMs - reveal.startMs, 0);
    const sceneIdx = Math.min(Math.floor(elapsedMs / reveal.sceneMs), reveal.order.length - 1);
    const sceneS = (elapsedMs - sceneIdx * reveal.sceneMs) / 1000;
    const id = reveal.order[sceneIdx];
    const standing = reveal.standings.get(id)!;
    const prep = this.preps.get(id);
    this.revealScores(reveal, sceneIdx, sceneS);
    const prepared = prep && this.ensurePrepared(prep);
    if (!prep || !prepared) {
      this.labelUnstaged(standing, reveal.revealed.has(id));
      return;
    }
    if (reveal.stagedIdx !== sceneIdx) this.stageScene(reveal, sceneIdx, prep.cut, prepared, standing);
    const halvesG = this.calcHalvesG(prep.cut, standing);
    const frame = calcRevealFrame(sceneS, JUDGED_TIMING, halvesG);
    this.scene.setHalfPartShare(frame.partShare);
    this.scene.showHalfGrams(frame.shownG);
    this.tickCount(reveal, frame.shownG, frame.isCounted, nowMs);
    this.scoreLabel.hidden = !frame.isCounted;
    this.aimCamera(prepared.bounds, sceneS);
  }

  /** Runs the preparations for up to `PREPARE_SLICE_MS`, the reveal's next scenes first. */
  private stepPreps(): void {
    if (!this.isCakeReady) return;
    const endMs = performance.now() + PREPARE_SLICE_MS;
    for (const prep of this.listPrepsByNeed()) {
      if (prep.prepared) continue;
      prep.job ??= this.scene.prepareCut(prep.cut);
      let step = prep.job.next();
      while (!step.done && performance.now() < endMs) step = prep.job.next();
      if (!step.done) return;
      prep.prepared = step.value;
      prep.job = null;
      if (performance.now() >= endMs) return;
    }
  }

  /** Every prep, those of the reveal's scenes from the one on screen on first, in order. */
  private listPrepsByNeed(): Prep[] {
    const reveal = this.reveal;
    if (!reveal) return [...this.preps.values()];
    const startIdx = Math.max(reveal.stagedIdx, 0);
    const ids = [...reveal.order.slice(startIdx), ...reveal.order.slice(0, startIdx)];
    return ids.map((id) => this.preps.get(id)).filter((prep) => prep !== undefined);
  }

  /** `prep`'s preparation, finished now if the slices have not got to it, or none before the round's cake is in the scene. */
  private ensurePrepared(prep: Prep): PreparedCut | null {
    if (!this.isCakeReady) return null;
    if (!prep.prepared) {
      prep.prepared = finishJob(prep.job ?? this.scene.prepareCut(prep.cut));
      prep.job = null;
    }
    return prep.prepared;
  }

  /** Cuts to scene `sceneIdx`: the player's stand, cake, and knife as they cut, and their name. */
  private stageScene(reveal: Reveal, sceneIdx: number, cut: StagedCut, prepared: PreparedCut, standing: Standing): void {
    reveal.stagedIdx = sceneIdx;
    reveal.shownG = 0;
    this.scene.stageCut(cut, prepared);
    this.nameLabel.textContent = standing.name;
    this.scoreLabel.textContent = formatScore(standing);
    this.scoreLabel.hidden = true;
  }

  /** Names the judged player, and shows their score once revealed, while the round's cake their scene needs still loads. */
  private labelUnstaged(standing: Standing, isRevealed: boolean): void {
    const score = formatScore(standing);
    // Called every frame, so it touches the page only on a change.
    if (this.nameLabel.textContent !== standing.name) this.nameLabel.textContent = standing.name;
    if (this.scoreLabel.textContent !== score) this.scoreLabel.textContent = score;
    if (this.scoreLabel.hidden === isRevealed) this.scoreLabel.hidden = !isRevealed;
  }

  /** The grams of the halves of `standing`'s cut, the side above the knife's plane first, or the whole cake's alone for a cut that missed. */
  private calcHalvesG(cut: StagedCut, standing: Standing): number[] {
    const halvesM3 = cut.isCutThrough ? [standing.above_m3, standing.below_m3] : [standing.above_m3 + standing.below_m3];
    return halvesM3.map((m3) => m3 * this.layout.density_g_per_m3);
  }

  /** Puts every player judged before scene `sceneIdx` on the leaderboard, and that scene's own once its counting ends `sceneS` in. */
  private revealScores(reveal: Reveal, sceneIdx: number, sceneS: number): void {
    let newestId: number | null = null;
    for (const [idx, id] of reveal.order.entries()) {
      if (idx > sceneIdx || reveal.revealed.has(id)) continue;
      const prep = this.preps.get(id);
      const isCounted = idx < sceneIdx || (prep !== undefined && sceneS >= calcCountedS(JUDGED_TIMING, this.calcHalvesG(prep.cut, reveal.standings.get(id)!)));
      if (!isCounted) continue;
      reveal.revealed.add(id);
      newestId = id;
    }
    if (newestId !== null) this.showBoard(reveal, newestId);
  }

  /**
   * Lists the revealed players best first, each at their rank among those revealed so far, from
   * the final ranks, so the first judged holds 1st until someone outscores them. `newestId`'s row
   * enters with a flourish.
   */
  private showBoard(reveal: Reveal, newestId: number | null): void {
    const shown = [...reveal.revealed].map((id) => reveal.standings.get(id)!).sort((a, b) => a.rank - b.rank);
    this.boardList.replaceChildren(
      ...shown.map((standing) => {
        const rank = 1 + shown.filter((other) => other.rank < standing.rank).length;
        const item = document.createElement("li");
        item.classList.toggle("is-new", standing.id === newestId);
        item.append(buildSpan("standing-rank", `${rank}.`), buildSpan("member-name", standing.name), buildSpan("standing-score", formatScore(standing)));
        return item;
      }),
    );
  }

  /** Ticks as the counters climb, one tick at most every `TICK_GAP_MS`, so the slow last grams tick one by one. */
  private tickCount(reveal: Reveal, shownG: readonly number[], isCounted: boolean, nowMs: number): void {
    const totalG = shownG.reduce((sum, g) => sum + g, 0);
    if (totalG === reveal.shownG) return;
    reveal.shownG = totalG;
    if (isCounted || nowMs - reveal.lastTickMs < TICK_GAP_MS) return;
    reveal.lastTickMs = nowMs;
    playCountTick();
  }

  /** Orbits the camera `sceneS` into the scene, clockwise about the middle of the cake `bounds`, from far enough to frame it. */
  private aimCamera(bounds: THREE.Box3, sceneS: number): void {
    const center = bounds.getCenter(new THREE.Vector3());
    const radiusM = bounds.getSize(new THREE.Vector3()).length() / 2;
    const yaw = ORBIT_START_YAW_RAD + ORBIT_RAD_PER_S * sceneS;
    const toCamera = new THREE.Vector3(Math.sin(yaw) * Math.cos(ORBIT_PITCH_RAD), Math.sin(ORBIT_PITCH_RAD), Math.cos(yaw) * Math.cos(ORBIT_PITCH_RAD));
    const target = center.clone().setY(center.y + radiusM * LOOK_ABOVE_SHARE);
    const camera = this.scene.camera;
    camera.position.copy(target).addScaledVector(toCamera, Math.max(radiusM * FRAME_DISTANCE_PER_RADIUS, MIN_FRAME_DISTANCE_M));
    camera.lookAt(target);
  }
}

/** A score as players see it, to the nearest 0.1%. */
function formatScore(standing: Standing): string {
  return `${standing.score.toFixed(1)}%`;
}

function buildSpan(className: string, text: string): HTMLSpanElement {
  return Object.assign(document.createElement("span"), { className, textContent: text });
}
