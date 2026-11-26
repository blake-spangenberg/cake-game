import * as THREE from "three";
import type { Layout } from "@wasm/cake_wasm.js";
import type { SlicedJob } from "./halfRest";
import type { CutMessage, JudgingMessage, Standing } from "./net";
import { AIM_HEIGHT_SHARE } from "./orbit";
import { type RevealScene, calcCountTicksS, calcCountedS, calcRevealFrame } from "./reveal";
import { type PreparedCut, type Scene, type StagedCut, finishJob } from "./scene";
import { RevealSounds } from "./sound";
import { buildStandingRow, findShownPlace, isMedalPlace, isMiss, setScoreText } from "./standings";

/** How long a frame may spend preparing judging scenes, as cuts come in during play and through the reveal. */
const PREPARE_SLICE_MS = 2;
/** Where the orbit starts, from the player's side of the table, to carry on through every scene. */
const ORBIT_START_YAW_RAD = 0.8;
/** Clockwise seen from above, a full turn in about 50 s. */
const ORBIT_RAD_PER_S = -0.1265;
/** High enough to look over the knife's near knob into the cut. */
const ORBIT_PITCH_RAD = 0.55;
/** How far the camera stands from the cake's middle, per meter of its bounds' half diagonal, and at least. */
const FRAME_DISTANCE_PER_RADIUS = 3.4;
const MIN_FRAME_DISTANCE_M = 0.5;
/** The least time between two of the counters' ticks, so their fast start rattles rather than buzzes. */
const TICK_GAP_MS = 70;

/** A cut the judging will show, and what `Scene.prepareCut` found for it once done. */
interface Prep {
  cut: StagedCut;
  job: SlicedJob<PreparedCut> | null;
  prepared: PreparedCut | null;
}

/** A judged player's scene: their standing, when it starts in ms from the reveal's start, and its timing from there. */
interface JudgedScene {
  standing: Standing;
  startMs: number;
  timing: RevealScene;
}

/** The reveal under way: when it began on `performance.now()`'s clock, its scenes in the order judged, and how far this page has shown them. */
interface Reveal {
  startMs: number;
  scenes: readonly JudgedScene[];
  /** The scene now on screen, or -1 before the first is staged. */
  stagedIdx: number;
  /** How many scenes, from the first, have their score on the leaderboard. */
  revealedLen: number;
}

/**
 * The judging reveal every page shows in step once play ends: one player at a time in the
 * server's order, every miss first and the rest at random, each on their own stand with their
 * cake's halves still together, the knife plunged, and their name at the top, under a camera
 * orbiting slowly clockwise. The halves part as a gram counter over each counts up, then the score
 * shows and the player joins a leaderboard beside the view that starts empty. Each scene is built
 * from the player's `cut` message, prepared a slice per frame as the cuts arrive, so no scene
 * starts with a long frame.
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
  private readonly sounds = new RevealSounds();

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
    this.reveal = { startMs, scenes: buildJudgedScenes(message), stagedIdx: -1, revealedLen: 0 };
    this.startSounds(this.reveal);
    this.boardList.replaceChildren();
    this.nameLabel.textContent = "";
    this.scoreLabel.hidden = true;
    this.overlay.hidden = false;
    this.scene.setViewRaised(false);
  }

  /** Ends the reveal, if one shows, giving the scene back with its cake whole. */
  stop(): void {
    if (this.reveal) this.scene.showWholeCake();
    this.reveal = null;
    this.sounds.close();
    this.overlay.hidden = true;
    this.scene.setViewRaised(true);
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
    if (!reveal || reveal.scenes.length === 0) return;
    const elapsedMs = Math.max(nowMs - reveal.startMs, 0);
    const sceneIdx = reveal.scenes.filter((scene) => scene.startMs <= elapsedMs).length - 1;
    const { standing, startMs, timing } = reveal.scenes[sceneIdx];
    const sceneS = (elapsedMs - startMs) / 1000;
    this.revealScores(reveal, sceneIdx, sceneS);
    const prep = this.preps.get(standing.id);
    const prepared = prep && this.ensurePrepared(prep);
    if (!prep || !prepared) {
      this.labelUnstaged(standing, sceneIdx < reveal.revealedLen);
      return;
    }
    if (reveal.stagedIdx !== sceneIdx) this.stageScene(reveal, sceneIdx, prep.cut, prepared);
    const frame = calcRevealFrame(sceneS, timing, this.calcHalvesG(standing));
    this.scene.setHalfPartShares(frame.partShares);
    if (frame.shownG.length > 0) this.scene.showHalfGrams(frame.shownG);
    this.scoreLabel.hidden = !frame.isCounted;
    this.aimCamera(prepared.bounds, elapsedMs / 1000);
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
    const scenes = [...reveal.scenes.slice(startIdx), ...reveal.scenes.slice(0, startIdx)];
    return scenes.map((scene) => this.preps.get(scene.standing.id)).filter((prep) => prep !== undefined);
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
  private stageScene(reveal: Reveal, sceneIdx: number, cut: StagedCut, prepared: PreparedCut): void {
    const { standing } = reveal.scenes[sceneIdx];
    reveal.stagedIdx = sceneIdx;
    this.scene.stageCut(cut, prepared);
    this.nameLabel.textContent = standing.name;
    setScoreText(this.scoreLabel, standing);
    this.scoreLabel.hidden = true;
  }

  /** Names the judged player, and shows their score once revealed, while the round's cake their scene needs still loads. */
  private labelUnstaged(standing: Standing, isRevealed: boolean): void {
    // Called every frame, so it touches the page only on a change.
    if (this.nameLabel.textContent !== standing.name) this.nameLabel.textContent = standing.name;
    setScoreText(this.scoreLabel, standing);
    if (this.scoreLabel.hidden === isRevealed) this.scoreLabel.hidden = !isRevealed;
  }

  /** The grams of the halves of `standing`'s cut, the side above the knife's plane first, or none for a cut that missed, which skips the count. */
  private calcHalvesG(standing: Standing): number[] {
    if (isMiss(standing)) return [];
    return [standing.above_m3, standing.below_m3].map((m3) => m3 * this.layout.density_g_per_m3);
  }

  /** Puts every player judged before scene `sceneIdx` on the leaderboard, and that scene's own once its counting ends `sceneS` in. */
  private revealScores(reveal: Reveal, sceneIdx: number, sceneS: number): void {
    const isCounted = sceneS >= calcCountedS(reveal.scenes[sceneIdx].timing);
    const revealedLen = sceneIdx + (isCounted ? 1 : 0);
    if (revealedLen <= reveal.revealedLen) return;
    reveal.revealedLen = revealedLen;
    this.showBoard(reveal.scenes.slice(0, revealedLen).map((scene) => scene.standing));
  }

  /**
   * Lists `revealed` best first, each at their place among them from the final ranks, so the
   * first judged holds 1st until someone outscores them. The last judged's row enters with a
   * flourish.
   */
  private showBoard(revealed: readonly Standing[]): void {
    const newest = revealed.at(-1);
    const best = [...revealed].sort((a, b) => a.rank - b.rank);
    this.boardList.replaceChildren(
      ...best.map((standing) => {
        const item = buildStandingRow(standing, calcBoardPlace(standing, revealed));
        item.classList.toggle("is-new", standing === newest);
        return item;
      }),
    );
  }

  /**
   * Schedules every scene's counter ticks, and the ding of a player joining the leaderboard with a
   * medal when `revealScores` shows their score, on the audio clock as the reveal starts, rather
   * than from frames, which a background tab does not draw, so it hears the reveal too.
   */
  private startSounds(reveal: Reveal): void {
    const tickAtMs: number[] = [];
    const dingAtMs: number[] = [];
    const judged: Standing[] = [];
    for (const { standing, startMs, timing } of reveal.scenes) {
      const sceneStartMs = reveal.startMs + startMs;
      for (const s of calcCountTicksS(timing, this.calcHalvesG(standing), TICK_GAP_MS / 1000)) tickAtMs.push(sceneStartMs + s * 1000);
      judged.push(standing);
      if (isMedalPlace(findShownPlace(standing, calcBoardPlace(standing, judged)))) dingAtMs.push(sceneStartMs + calcCountedS(timing) * 1000);
    }
    this.sounds.start(tickAtMs, dingAtMs);
  }

  /**
   * Orbits the camera `revealS` into the reveal, clockwise about the cake `bounds` and aimed at
   * the same share of its height as play's camera, from far enough to frame it.
   */
  private aimCamera(bounds: THREE.Box3, revealS: number): void {
    const target = bounds.getCenter(new THREE.Vector3()).setY(THREE.MathUtils.lerp(bounds.min.y, bounds.max.y, AIM_HEIGHT_SHARE));
    const radiusM = bounds.getSize(new THREE.Vector3()).length() / 2;
    const yaw = ORBIT_START_YAW_RAD + ORBIT_RAD_PER_S * revealS;
    const toCamera = new THREE.Vector3(Math.sin(yaw) * Math.cos(ORBIT_PITCH_RAD), Math.sin(ORBIT_PITCH_RAD), Math.cos(yaw) * Math.cos(ORBIT_PITCH_RAD));
    const camera = this.scene.camera;
    camera.position.copy(target).addScaledVector(toCamera, Math.max(radiusM * FRAME_DISTANCE_PER_RADIUS, MIN_FRAME_DISTANCE_M));
    camera.lookAt(target);
  }
}

/**
 * Each judged player's scene in `message`, one after another in its order: the halves together
 * for `hold_ms`, the count for the player's `count_ms`, which the server derives from their score,
 * then `after_ms` with the score up before the next.
 */
function buildJudgedScenes(message: JudgingMessage): JudgedScene[] {
  const standings = new Map(message.standings.map((standing) => [standing.id, standing]));
  let startMs = 0;
  return message.order.map((id, idx) => {
    const countMs = message.count_ms[idx];
    const scene = { standing: standings.get(id)!, startMs, timing: { holdS: message.hold_ms / 1000, countS: countMs / 1000 } };
    startMs += message.hold_ms + countMs + message.after_ms;
    return scene;
  });
}

/** The place `standing` holds on a leaderboard of `shown`, itself included, from the final ranks: 1 plus how many there rank better. */
function calcBoardPlace(standing: Standing, shown: readonly Standing[]): number {
  return 1 + shown.filter((other) => other.rank < standing.rank).length;
}
