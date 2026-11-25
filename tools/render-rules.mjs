// Renders the lobby's four rule panels into assets/rules/: still frames of the real client, staged at each step of a
// round with the Yellow Sneaker, then scaled down with arrows and a cursor drawn over them for the motion.
// Usage: [BUILD_DIR=build/agents/<name>] node tools/render-rules.mjs [--raw-dir <dir>]
// Needs the WASM build that BUILD_DIR names (make wasm) and the unpacked cakes in build/public/cakes/ (make cakes).
// --raw-dir also saves each whole frame unmarked as PNG, to re-aim the staging and the marks after the scene changes.
// The page runs on a clock this tool advances one 60 Hz frame at a time, on SwiftShader, so a rerun draws the same
// pixels. The staging's mouse points and the marks' coordinates hold only while the cake, the camera, the controls, and
// the knife stay as they are: after a change to any of them, check the four images and the printed score of the cut.
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const VIEW_PX = { width: 1280, height: 960 };
/** The part of the view each panel shows, at its `cropAt` corner, large enough to hold the cake, the knife, and the Cut! button together. */
const CROP_PX = { width: 960, height: 720 };
/** Twice a panel's width in the lobby's two-column, 640px rules area, so it stays sharp on a 2x screen. */
const PANEL_PX = { width: 640, height: 480 };
const WEBP_QUALITY = 0.82;
const SNEAKER_ID = "89b7408463b144d0bd157cc788dd496d";
const SNEAKER_AUTHOR = "mr.pinoux";
/** Brings the start camera in until the stand and the knife over it fill the view. */
const ZOOM_WHEEL_DELTA = -350;
/** From the sneaker's laces, where a grab lifts it upright, to where it lands near the middle of the stand. */
const CARRY_PX = [[1100, 760], [672, 500]];
/** Grabs the resting blade and tilts it almost upright, where the cut splits the sneaker about 52:48. */
const BLADE_DRAG_PX = [[610, 140], [775, 140]];
const CUT_BUTTON_PX = [1178, 858];
/** Frames a drag holds each mouse position, about the 30 ms between moves of `.agent/scripts/drive-client.mjs`. */
const FRAMES_PER_MOVE = 2;
const LOAD_TIMEOUT_FRAMES = 1200;
const BROWSERS = [
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "/usr/bin/microsoft-edge",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
];
/**
 * Hides the UI no panel shows: the cake picker and credit, the round result, and the Cut! button outside its own step,
 * where it glows as pressed. Enlarges the weight labels to stay legible at panel size.
 */
const STAGE_CSS = `
  #cake-menu, #result { display: none !important; }
  #view { cursor: none !important; }
  body:not(.staged-cut) #cut { display: none !important; }
  body.staged-cut #cut { background: #c0392b !important; opacity: 1 !important; box-shadow: 0 0 0 6px #ffd36b, 0 0 36px 16px rgb(255 211 107 / 70%); }
  body.staged-grams svg { font-size: 84px !important; font-weight: 800; }
`;
/**
 * Each panel's file, named as `src/client/src/rules.ts` loads it, the top left corner of its crop in view pixels, and
 * the marks drawn over it in panel pixels: a curved arrow from its first point to its last, bent toward the middle one,
 * with a head at the end or at both ends; an orbit of two arcs on an ellipse's sides, both turning clockwise on screen;
 * and a cursor with its tip at a point.
 */
const PANELS = [
  {
    name: "1-carry",
    cropAt: [290, 10],
    marks: [
      { kind: "arrow", points: [[313, 307], [255, 215], [185, 300]] },
      { kind: "cursor", at: [350, 385] },
    ],
  },
  {
    name: "2-aim",
    cropAt: [290, 10],
    marks: [
      { kind: "orbit", center: [233, 335], radii: [142, 86] },
      { kind: "arrow", points: [[340, 67], [395, 130], [333, 195]], isDoubleHeaded: true },
    ],
  },
  {
    name: "3-cut",
    cropAt: [320, 230],
    marks: [{ kind: "cursor", at: [575, 422] }],
  },
  {
    name: "4-weigh",
    cropAt: [250, 110],
    marks: [],
  },
];

const rootUrl = new URL("../", import.meta.url);
const clientDir = fileURLToPath(new URL("src/client/", rootUrl));
const rulesUrl = new URL("assets/rules/", rootUrl);

function parseRawDir(args) {
  if (args.length === 0) return null;
  if (args.length === 2 && args[0] === "--raw-dir") return path.resolve(args[1]);
  console.error(`usage: node tools/render-rules.mjs [--raw-dir <dir>], got ${args.join(" ")}`);
  process.exit(2);
}

async function renderAll() {
  const requireClient = createRequire(new URL("package.json", pathToFileURL(clientDir)));
  const vite = await import(pathToFileURL(requireClient.resolve("vite")));
  const puppeteer = requireClient("puppeteer-core");
  // No reload or watch, so another save to the client cannot reload the page mid-stage.
  const server = await vite.createServer({ root: clientDir, logLevel: "warn", server: { port: 0, strictPort: false, hmr: false, watch: null } });
  await server.listen();
  const browser = await puppeteer.launch({ executablePath: browserPath, headless: true, args: ["--enable-unsafe-swiftshader", "--use-angle=swiftshader"] });
  try {
    const page = await browser.newPage();
    await page.setViewport({ ...VIEW_PX, deviceScaleFactor: 1 });
    await page.evaluateOnNewDocument(installFrameClock);
    const stage = new Stage(page);
    await page.goto(server.resolvedUrls.local[0]);
    const frames = await stageRound(stage);
    const painter = await browser.newPage();
    for (const [idx, panel] of PANELS.entries()) await savePanel(painter, panel, frames[idx]);
  } finally {
    await browser.close();
    await server.close();
  }
}

/** Plays one round with the sneaker up to each panel's moment and screenshots it, returning the four PNGs. */
async function stageRound(stage) {
  const { page } = stage;
  // The first frame disables Cut!, the knife resting clear of the cake, once the picker listens.
  await stage.waitFor(() => document.querySelector("#cut")?.disabled === true);
  await page.addStyleTag({ content: STAGE_CSS });
  await page.select("#cake-picker", SNEAKER_ID);
  await stage.waitFor((author) => document.querySelector("#credit").textContent.includes(author), SNEAKER_AUTHOR);
  await stage.advance(60);
  await stage.sendInput(1, (mouse) => mouse.move(VIEW_PX.width / 2, VIEW_PX.height / 2));
  await stage.sendInput(1, (mouse) => mouse.wheel({ deltaY: ZOOM_WHEEL_DELTA }));
  await stage.advance(60);
  const carry = await stage.drag(...CARRY_PX, { moves: 30, shotAtMove: 20 });
  await stage.advance(150);
  const aim = await stage.shot();
  await stage.drag(...BLADE_DRAG_PX, { moves: 25 });
  await stage.advance(60);
  await page.evaluate(() => document.body.classList.add("staged-cut"));
  await stage.sendInput(1, (mouse) => mouse.move(...CUT_BUTTON_PX));
  await stage.sendInput(2, (mouse) => mouse.click(...CUT_BUTTON_PX));
  // The knife has swept down through the cake and starts to rock.
  await stage.advance(27);
  const cut = await stage.shot();
  await page.evaluate(() => document.body.classList.replace("staged-cut", "staged-grams"));
  await stage.advance(210);
  const weigh = await stage.shot();
  // The score shows once the reveal's count-up ends, after the weights.
  await stage.advance(240);
  console.log(`staged cut scores ${await page.evaluate(() => document.querySelector("#score").textContent)}`);
  return [carry, aim, cut, weigh];
}

/** Drives a page whose animation frames run only when `advance` says, on the clock `installFrameClock` put in it. */
class Stage {
  constructor(page) {
    this.page = page;
    this.pageError = null;
    page.on("pageerror", (error) => (this.pageError ??= error));
  }

  async advance(count) {
    if (this.pageError) throw new Error(`render-rules: the page failed while staging: ${this.pageError.message}`);
    await this.page.evaluate((frames) => window.rulesClock.advance(frames), count);
  }

  /** Advances frame by frame until `predicate(arg)` holds in the page. */
  async waitFor(predicate, arg) {
    for (let frame = 0; frame < LOAD_TIMEOUT_FRAMES; frame += 1) {
      if (await this.page.evaluate(predicate, arg)) return;
      await this.advance(1);
    }
    throw new Error(`render-rules: the page did not reach ${predicate} within ${LOAD_TIMEOUT_FRAMES} frames`);
  }

  /**
   * Sends input that raises `eventLen` pointer or wheel events, and waits until the page has seen them all, which may
   * take it several real frames, so no staged frame runs before the input lands.
   */
  async sendInput(eventLen, send) {
    const seenLen = await this.page.evaluate(() => window.rulesClock.getInputCount());
    await send(this.page.mouse);
    await this.page.evaluate((len) => window.rulesClock.waitInput(len), seenLen + eventLen);
  }

  /** A left-button drag in `moves` steps, returning a screenshot taken while held after move `shotAtMove`, if given. */
  async drag(from, to, { moves, shotAtMove = null, holdFrames = 36 }) {
    await this.sendInput(1, (mouse) => mouse.move(...from));
    await this.sendInput(1, (mouse) => mouse.down());
    let held = null;
    for (let move = 1; move <= moves; move += 1) {
      const share = move / moves;
      await this.sendInput(1, (mouse) => mouse.move(from[0] + (to[0] - from[0]) * share, from[1] + (to[1] - from[1]) * share));
      await this.advance(FRAMES_PER_MOVE);
      if (move === shotAtMove) held = await this.shot();
    }
    await this.advance(holdFrames);
    await this.sendInput(1, (mouse) => mouse.up());
    return held;
  }

  shot() {
    return this.page.screenshot({ type: "png" });
  }
}

/**
 * Runs in the page before its scripts: animation frames wait for `rulesClock.advance`, each one
 * 1/60 s on `performance.now`, and `Math.random` is seeded, so the round plays the same every run.
 */
function installFrameClock() {
  // Just over 1000/60 and a multiple of 2^-12, so the clock adds without rounding and every frame lasts the same,
  // stepping the 60 Hz simulation exactly one tick however many frames loading the cake took.
  const frameMs = 68267 / 4096;
  const requestRealFrame = window.requestAnimationFrame.bind(window);
  let nowMs = 0;
  let callbacks = [];
  let seed = 0x9e3779b9;
  performance.now = () => nowMs;
  window.requestAnimationFrame = (callback) => callbacks.push(callback);
  Math.random = () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let mixed = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    mixed = (mixed + Math.imul(mixed ^ (mixed >>> 7), 61 | mixed)) ^ mixed;
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  };
  let inputCount = 0;
  for (const type of ["pointerdown", "pointermove", "pointerup", "wheel"]) {
    window.addEventListener(type, () => (inputCount += 1), { capture: true, passive: true });
  }
  window.rulesClock = {
    getInputCount: () => inputCount,
    /** Resolves once the page has seen `count` input events in all, however many real frames that takes. */
    async waitInput(count) {
      for (let frame = 0; inputCount < count; frame += 1) {
        if (frame === 300) throw new Error(`saw ${inputCount} input events, waited for ${count}`);
        await new Promise((resolve) => requestRealFrame(resolve));
      }
    },
    async advance(count) {
      for (let frame = 0; frame < count; frame += 1) {
        nowMs += frameMs;
        const due = callbacks;
        callbacks = [];
        for (const callback of due) callback(nowMs);
        await new Promise((resolve) => setTimeout(resolve, 0));
      }
    },
  };
}

async function savePanel(painter, panel, png) {
  if (rawDir) writeFileSync(path.join(rawDir, `${panel.name}.png`), png);
  const dataUrl = await painter.evaluate(paintPanel, Buffer.from(png).toString("base64"), panel, CROP_PX, PANEL_PX, WEBP_QUALITY);
  if (!dataUrl.startsWith("data:image/webp;base64,")) throw new Error(`${panel.name}: the browser encoded ${dataUrl.slice(0, 30)}, not WebP`);
  const webp = Buffer.from(dataUrl.slice(dataUrl.indexOf(",") + 1), "base64");
  writeFileSync(new URL(`${panel.name}.webp`, rulesUrl), webp);
  console.log(`rendered assets/rules/${panel.name}.webp (${webp.length} bytes)`);
}

/** Runs in a blank page: scales the panel's crop of the frame down to the panel's size, draws its marks over it, and encodes it as WebP. */
async function paintPanel(pngBase64, { cropAt, marks }, cropPx, sizePx, quality) {
  const ink = "#f4ead8";
  const outline = "rgb(29 27 25 / 80%)";
  const lineWidthPx = 8;
  const outlinePx = 5;
  const headPx = 21;
  const image = new Image();
  image.src = `data:image/png;base64,${pngBase64}`;
  await image.decode();
  const canvas = document.createElement("canvas");
  Object.assign(canvas, sizePx);
  const ctx = canvas.getContext("2d");
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(image, ...cropAt, cropPx.width, cropPx.height, 0, 0, sizePx.width, sizePx.height);
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const mark of marks) {
    if (mark.kind === "arrow") strokeTwice(() => traceArrow(mark.points, mark.isDoubleHeaded ?? false));
    if (mark.kind === "orbit") strokeTwice(() => traceOrbit(mark.center, mark.radii));
    if (mark.kind === "cursor") drawCursor(mark.at);
  }
  return canvas.toDataURL("image/webp", quality);

  function strokeTwice(trace) {
    for (const [style, widthPx] of [[outline, lineWidthPx + 2 * outlinePx], [ink, lineWidthPx]]) {
      ctx.beginPath();
      trace();
      ctx.strokeStyle = style;
      ctx.lineWidth = widthPx;
      ctx.stroke();
    }
  }

  function traceArrow([from, bend, to], isDoubleHeaded) {
    ctx.moveTo(...from);
    ctx.quadraticCurveTo(...bend, ...to);
    traceHead(to, Math.atan2(to[1] - bend[1], to[0] - bend[0]));
    if (isDoubleHeaded) traceHead(from, Math.atan2(from[1] - bend[1], from[0] - bend[0]));
  }

  function traceOrbit([cx, cy], [rx, ry]) {
    for (const [fromRad, toRad] of [[-0.3 * Math.PI, 0.3 * Math.PI], [0.7 * Math.PI, 1.3 * Math.PI]]) {
      const pointAt = (rad) => [cx + rx * Math.cos(rad), cy + ry * Math.sin(rad)];
      ctx.moveTo(...pointAt(fromRad));
      ctx.ellipse(cx, cy, rx, ry, 0, fromRad, toRad);
      const tangent = [-rx * Math.sin(toRad), ry * Math.cos(toRad)];
      traceHead(pointAt(toRad), Math.atan2(tangent[1], tangent[0]));
    }
  }

  function traceHead([x, y], headingRad) {
    for (const sideRad of [-0.6, 0.6]) {
      ctx.moveTo(x, y);
      ctx.lineTo(x - headPx * Math.cos(headingRad + sideRad), y - headPx * Math.sin(headingRad + sideRad));
    }
  }

  /** The usual arrow pointer, its tip at `at`, white over a dark outline. */
  function drawCursor([x, y]) {
    const scale = 2.2;
    const outlinePoints = [[0, 0], [0, 17], [4, 13], [7, 20], [10, 19], [7, 12], [12, 12]];
    ctx.beginPath();
    for (const [px, py] of outlinePoints) ctx.lineTo(x + px * scale, y + py * scale);
    ctx.closePath();
    ctx.fillStyle = "#fff";
    ctx.fill();
    ctx.strokeStyle = outline;
    ctx.lineWidth = 4;
    ctx.stroke();
  }
}

const rawDir = parseRawDir(process.argv.slice(2));
const browserPath = BROWSERS.find((candidate) => existsSync(candidate));
if (!browserPath) {
  console.error(`render-rules: no browser found at ${BROWSERS.join(", ")}`);
  process.exit(1);
}
mkdirSync(rulesUrl, { recursive: true });
if (rawDir) mkdirSync(rawDir, { recursive: true });
await renderAll();
