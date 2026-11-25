import * as THREE from "three";

const SVG_NS = "http://www.w3.org/2000/svg";
/** The result's text color. */
const INK = "#f4ead8";
const FONT_PX = 28;
/** How far above its anchor a label's baseline sits, the leader line spanning the rise. */
const RISE_PX = 56;
/** Room between the leader line's top and the label's baseline. */
const LINE_TOP_GAP_PX = 6;
/** A digit's width as a share of the font size, tabular figures being even, so overlap needs no layout read. */
const CHAR_WIDTH_EM = 0.6;
/** Least space between two labels side by side, and between a label and the view's edge. */
const LABEL_MARGIN_PX = 12;
const DOT_RADIUS_PX = 3.5;

type Tag = { group: SVGGElement; text: SVGTextElement; line: SVGLineElement; dot: SVGCircleElement };
type Placed = { anchorPx: THREE.Vector2; xPx: number; yPx: number; widthPx: number };

/**
 * Text floating over points in the scene, drawn over the canvas, each tied to its point by a
 * leader line ending in a dot there. Labels whose spots would overlap on screen are pushed apart
 * sideways, their lines slanting back to their points. Made for the weight over each half of a cut
 * cake, which the judging reveal counts up as the halves part.
 */
export class HalfLabels {
  private readonly svg: SVGSVGElement;
  private readonly tags: Tag[];
  private texts: (string | null)[] = [];

  /** Labels for up to `len` points, hidden until `show`. */
  constructor(
    private readonly canvas: HTMLCanvasElement,
    len: number,
  ) {
    this.svg = document.createElementNS(SVG_NS, "svg");
    Object.assign(this.svg.style, {
      position: "absolute",
      pointerEvents: "none",
      overflow: "visible",
      filter: "drop-shadow(0 0 2px rgb(0 0 0 / 85%))",
      fontSize: `${FONT_PX}px`,
      fontVariantNumeric: "tabular-nums",
    });
    this.tags = Array.from({ length: len }, () => this.addTag());
    canvas.after(this.svg);
    this.hide();
  }

  /** Shows `texts[i]` over the `i`th point given to `place`, or no label there for a null. */
  show(texts: readonly (string | null)[]): void {
    this.texts = [...texts];
    this.tags.forEach((tag, idx) => (tag.text.textContent = this.texts[idx] ?? ""));
    this.svg.style.display = "";
  }

  hide(): void {
    this.texts = [];
    this.svg.style.display = "none";
  }

  /** Moves each shown label over its point in world space as `camera`, already rendered this frame, sees it. */
  place(camera: THREE.Camera, points: readonly (THREE.Vector3 | null)[]): void {
    if (this.texts.length === 0) return;
    const [widthPx, heightPx] = [this.canvas.clientWidth, this.canvas.clientHeight];
    Object.assign(this.svg.style, { left: `${this.canvas.offsetLeft}px`, top: `${this.canvas.offsetTop}px` });
    this.svg.setAttribute("width", `${widthPx}`);
    this.svg.setAttribute("height", `${heightPx}`);
    const viewPx = new THREE.Vector2(widthPx, heightPx);
    const placed = this.tags.map((_, idx) => findSpot(camera, points[idx] ?? null, this.texts[idx] ?? null, viewPx));
    spreadOverlapping(placed);
    placed.forEach((spot, idx) => drawTag(this.tags[idx], spot, widthPx));
  }

  private addTag(): Tag {
    const group = document.createElementNS(SVG_NS, "g");
    const line = document.createElementNS(SVG_NS, "line");
    line.setAttribute("stroke", INK);
    line.setAttribute("stroke-width", "2");
    line.setAttribute("stroke-linecap", "round");
    const dot = document.createElementNS(SVG_NS, "circle");
    dot.setAttribute("r", `${DOT_RADIUS_PX}`);
    dot.setAttribute("fill", INK);
    const text = document.createElementNS(SVG_NS, "text");
    text.setAttribute("fill", INK);
    text.setAttribute("text-anchor", "middle");
    group.append(line, dot, text);
    this.svg.append(group);
    return { group, text, line, dot };
  }
}

/** Where a label goes before any overlap is resolved, or none with no text or a point behind the camera. */
function findSpot(camera: THREE.Camera, point: THREE.Vector3 | null, text: string | null, viewPx: THREE.Vector2): Placed | null {
  if (!point || !text) return null;
  const ndc = point.clone().project(camera);
  if (ndc.z > 1) return null;
  const anchorPx = new THREE.Vector2(((ndc.x + 1) / 2) * viewPx.x, ((1 - ndc.y) / 2) * viewPx.y);
  const yPx = Math.max(anchorPx.y - RISE_PX, FONT_PX + LABEL_MARGIN_PX);
  return { anchorPx, xPx: anchorPx.x, yPx, widthPx: text.length * FONT_PX * CHAR_WIDTH_EM };
}

/** Pushes two labels that would overlap apart sideways about their middle, keeping their left-to-right order. */
function spreadOverlapping(placed: (Placed | null)[]): void {
  const [a, b] = placed;
  if (!a || !b) return;
  const needPx = (a.widthPx + b.widthPx) / 2 + LABEL_MARGIN_PX;
  const isOverlapping = Math.abs(a.xPx - b.xPx) < needPx && Math.abs(a.yPx - b.yPx) < FONT_PX;
  if (!isOverlapping) return;
  const middlePx = (a.xPx + b.xPx) / 2;
  const [left, right] = a.anchorPx.x <= b.anchorPx.x ? [a, b] : [b, a];
  left.xPx = middlePx - needPx / 2;
  right.xPx = middlePx + needPx / 2;
}

function drawTag(tag: Tag, spot: Placed | null, viewWidthPx: number): void {
  tag.group.style.display = spot ? "" : "none";
  if (!spot) return;
  const halfWidthPx = spot.widthPx / 2 + LABEL_MARGIN_PX;
  const xPx = THREE.MathUtils.clamp(spot.xPx, halfWidthPx, Math.max(viewWidthPx - halfWidthPx, halfWidthPx));
  tag.text.setAttribute("x", `${xPx}`);
  tag.text.setAttribute("y", `${spot.yPx}`);
  tag.line.setAttribute("x1", `${xPx}`);
  tag.line.setAttribute("y1", `${spot.yPx + LINE_TOP_GAP_PX}`);
  tag.line.setAttribute("x2", `${spot.anchorPx.x}`);
  tag.line.setAttribute("y2", `${spot.anchorPx.y}`);
  tag.dot.setAttribute("cx", `${spot.anchorPx.x}`);
  tag.dot.setAttribute("cy", `${spot.anchorPx.y}`);
}
