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

/** The rules as a list of four captioned stills, one per step of a round, laid out two by two in a narrow column. */
export function buildRules(): HTMLOListElement {
  const list = document.createElement("ol");
  list.className = "rules";
  list.append(...RULE_PANELS.map(buildPanel));
  return list;
}

function buildPanel(panel: RulePanel): HTMLLIElement {
  const image = document.createElement("img");
  image.src = `${import.meta.env.BASE_URL}rules/${panel.name}.webp`;
  image.alt = panel.alt;
  Object.assign(image, PANEL_PX);
  const caption = document.createElement("figcaption");
  caption.append(...buildCaption(panel.caption));
  const figure = document.createElement("figure");
  figure.append(image, caption);
  const item = document.createElement("li");
  item.append(figure);
  return item;
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
