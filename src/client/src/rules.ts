import "./rules.css";

/** The stills' size in pixels, about twice the width of a panel in a 640px column, so they stay sharp on a 2x screen. */
const PANEL_PX = { width: 640, height: 480 };
/** Set in the script `Cut!` is on its button wherever a caption names it. */
const CUT_LABEL = "Cut!";

/** One step of a round: its still in `rules/`, which `tools/render-rules.mjs` renders, the caption under it, and the still's alt text. */
interface RulePanel {
  name: string;
  caption: string;
  alt: string;
}

const RULE_PANELS: readonly RulePanel[] = [
  {
    name: "1-carry",
    caption: "Click and drag the cake onto the cake stand",
    alt: "A cursor drags a yellow sneaker cake across the table toward the cake stand",
  },
  {
    name: "2-aim",
    caption: "Adjust the rotation of the cake stand and the angle of the knife to line up the perfect cut",
    alt: "The sneaker cake on the cake stand, with arrows for turning the stand and tilting the knife",
  },
  {
    name: "3-cut",
    caption: "Press Cut! to lock in, cutting your cake in half",
    alt: "A cursor presses the Cut! button as the knife cuts down through the sneaker cake",
  },
  {
    name: "4-weigh",
    caption: "The winner is the player whose halves are most similar by weight",
    alt: "The two halves of the sneaker cake parted on the stand, each labeled with its weight in grams",
  },
];

/** A camera control as the rules show it: what it does, then pictures of its inputs with as few words as they need, as markup. */
interface CameraControl {
  action: string;
  inputsHtml: string;
}

/** Outlined in the text's color: a mouse, its left button pressed, under an arrow swinging both ways. */
const DRAG_ICON = `<svg viewBox="0 0 32 32" aria-hidden="true">
  <path d="M5 10Q16 1 27 10" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
  <path d="M4 5v6h6M28 5v6h-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
  <rect x="10" y="12" width="12" height="18" rx="6" fill="none" stroke="currentColor" stroke-width="2"/>
  <path d="M16 12a6 6 0 0 0-6 6v1h6z" fill="currentColor"/>
  <path d="M16 12v7" stroke="currentColor" stroke-width="2"/>
</svg>`;
/** A mouse with its wheel filled in, between arrows up and down. */
const WHEEL_ICON = `<svg viewBox="0 0 32 32" aria-hidden="true">
  <rect x="7" y="7" width="14" height="22" rx="7" fill="none" stroke="currentColor" stroke-width="2"/>
  <rect x="12.5" y="10" width="3" height="6" rx="1.5" fill="currentColor"/>
  <path d="M25 9l3-4 3 4M25 23l3 4 3-4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
</svg>`;
/** The centers of four keycaps in an inverted T, as on a keyboard: the top key, then the bottom row left to right. */
const KEY_CENTERS = [
  [17, 6],
  [6, 17],
  [17, 17],
  [28, 17],
] as const;
/** Triangles for the arrow keys, in `KEY_CENTERS` order: up, left, down, right. */
const ARROW_POINTS = ["0,-2.6 2.6,1.6 -2.6,1.6", "-2.6,0 1.6,2.6 1.6,-2.6", "0,2.6 2.6,-1.6 -2.6,-1.6", "2.6,0 -1.6,2.6 -1.6,-2.6"] as const;
const WASD = "WASD";
const ARROW_KEYS_ICON = buildKeysIcon((idx) => `<polygon points="${ARROW_POINTS[idx]}" fill="currentColor"/>`);
const WASD_KEYS_ICON = buildKeysIcon((idx) => `<text dy="2.4" text-anchor="middle" font-size="7" font-weight="800" fill="currentColor">${WASD[idx]}</text>`);

const CAMERA_CONTROLS: readonly CameraControl[] = [
  { action: "Look around", inputsHtml: `${DRAG_ICON}<span>drag empty space, or</span>${ARROW_KEYS_ICON}${WASD_KEYS_ICON}` },
  { action: "Zoom", inputsHtml: WHEEL_ICON },
];

/**
 * The rules as a list of four numbered and captioned stills, one per step of a round, laid out
 * two by two, with the camera's controls under them.
 */
export function buildRules(): HTMLElement {
  const list = document.createElement("ol");
  list.className = "rules";
  list.append(...RULE_PANELS.map(buildPanel));
  const camera = document.createElement("p");
  camera.className = "camera-controls";
  camera.append(...CAMERA_CONTROLS.map(buildCameraControl));
  const rules = document.createElement("div");
  rules.className = "rules-with-camera";
  rules.append(list, camera);
  return rules;
}

function buildPanel(panel: RulePanel, idx: number): HTMLLIElement {
  const image = document.createElement("img");
  image.src = `${import.meta.env.BASE_URL}rules/${panel.name}.webp`;
  image.alt = panel.alt;
  Object.assign(image, PANEL_PX);
  const numeral = document.createElement("span");
  numeral.className = "rules-numeral";
  numeral.textContent = String(idx + 1);
  numeral.ariaHidden = "true";
  const text = document.createElement("span");
  text.append(...buildCaption(panel.caption));
  const caption = document.createElement("figcaption");
  caption.append(numeral, text);
  const figure = document.createElement("figure");
  figure.append(image, caption);
  const item = document.createElement("li");
  item.append(figure);
  return item;
}

function buildCameraControl(control: CameraControl): HTMLSpanElement {
  const item = document.createElement("span");
  item.className = "camera-control";
  item.innerHTML = control.inputsHtml;
  const action = document.createElement("b");
  action.textContent = control.action;
  item.prepend(action);
  return item;
}

/** Four outlined keycaps at `KEY_CENTERS`, each marked by `buildMark` with its index, drawn about the key's center. */
function buildKeysIcon(buildMark: (idx: number) => string): string {
  const caps = KEY_CENTERS.map(
    ([cx, cy], idx) => `<rect x="${cx - 5}" y="${cy - 5}" width="10" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1.4"/><g transform="translate(${cx} ${cy})">${buildMark(idx)}</g>`,
  );
  return `<svg viewBox="0 0 34 23" aria-hidden="true">${caps.join("")}</svg>`;
}

/** The caption's text, with each `Cut!` in it set apart in the button's style. */
function buildCaption(text: string): (string | HTMLElement)[] {
  return text.split(CUT_LABEL).flatMap((part, idx) => (idx === 0 ? [part] : [buildCutLabel(), part]));
}

function buildCutLabel(): HTMLElement {
  const label = document.createElement("span");
  label.className = "rules-cut";
  label.textContent = CUT_LABEL;
  return label;
}
