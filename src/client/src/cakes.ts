/** The groups the cake picker lists cakes under, in the order it shows them. */
export const CAKE_KINDS = ["Food", "Shoes", "Gloves", "Instruments", "Gadgets", "Furniture", "Figures", "Animals", "Nature"] as const;
export type CakeKind = (typeof CAKE_KINDS)[number];

/**
 * A sourced cake model: its baked file in `build/public/cakes/` (unpacked by `make cakes` from `assets/cakes/`),
 * the name players see, the credit its license requires, and how `tools/bake-cakes.sh` bakes it.
 */
export interface CakeEntry {
  /** The model's Sketchfab id, which names its baked file and its page. */
  id: string;
  /** What the game calls the cake. */
  name: string;
  /** The group the cake picker lists it under. */
  kind: CakeKind;
  /** The Sketchfab user to credit. */
  author: string;
  license: LicenseName;
  /** Options for `tools/repair-model.py` beyond those every cake gets, such as a recolor, and `tools/bake-cake.sh`'s `--pose-s` for an animated model. */
  repair?: readonly string[];
  /** Options for the `cake-bake` collider build, such as `--trim-overhangs` for a cake whose hulls float it on some side or bridge a wide gap. */
  bake?: readonly string[];
}

/** Order: the owner's favorites, then those that passed with reservations, then new candidates not yet reviewed. */
export const CAKES: readonly CakeEntry[] = [
  { id: "ccc3f77d27f94ae5acc77992b90b146b", name: "Leather Sofa", kind: "Furniture", author: "gla_bot", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "ecd6448e6dae4b93b60008913bdf8554", name: "Heritage Potato", kind: "Food", author: "Tony.Richards", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "3c889b5ee6b64a2aafb9b8977f8a8219", name: "Turkey Leg", kind: "Food", author: "BillieBones", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "79b95e7cf3dc422e814598b0eea6c83f", name: "Knitted Mitten", kind: "Gloves", author: "nebulousflynn", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "89b7408463b144d0bd157cc788dd496d", name: "Yellow Sneaker", kind: "Shoes", author: "mr.pinoux", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "8e0288f099bb418199e8896c19bed05e", name: "Toy Dinosaur", kind: "Figures", author: "TheCactusFactory", license: "CC BY 4.0", repair: ["--recolor", "103", "0.63", "0.63", "--first-use-vertex-order"] },
  { id: "0aeba0a56d414a6bb1865cf3375aef6b", name: "Lucky Cat", kind: "Figures", author: "squishynoize", license: "CC BY 4.0", repair: ["--first-use-vertex-order"], bake: ["--trim-overhangs"] },
  { id: "511db88f24e04eb1be8b0c0c080e1e5e", name: "Chicken Nugget", kind: "Food", author: "Laserdesign", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "348c77822bff4de3b6df59839b2fd201", name: "Camo Sneaker", kind: "Shoes", author: "matousekfoto", license: "CC BY 4.0", repair: ["--hollow", "0.004", "0.022", "--inside-color", "138", "161", "32"] },
  { id: "d9506d9a318e4c56ac3d8ab273bd2540", name: "Work Boot", kind: "Shoes", author: "inciprocal.com", license: "CC BY 4.0", repair: ["--first-use-vertex-order"], bake: ["--trim-overhangs"] },
  { id: "a5923998c60547c7a7887f11f2f2088d", name: "Ginger Root", kind: "Food", author: "insomnia333", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "53fdf984962948128db5f4ccfcae0843", name: "Old Pear", kind: "Food", author: "inciprocal.com", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "143e23a0436449f686ea0dd947e6f634", name: "Croissant", kind: "Food", author: "rozza3d", license: "CC BY 4.0", repair: ["--fill-unlit-underside", "--first-use-vertex-order"] },
  { id: "a00d2eaa0c1d459f893ae4cbb08b866e", name: "Boxing Glove", kind: "Gloves", author: "urheilumuseo", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "b400281a14bf4f3a87bc6a053a19581a", name: "Garden Scarecrow", kind: "Figures", author: "edemaistre", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "e4b7d4137f444265ad98fb02780ccc5e", name: "Garden Gnome", kind: "Figures", author: "tiozombi", license: "CC BY 4.0", repair: ["--first-use-vertex-order"] },
  { id: "ec9c0ac738fd4495af334ea2092e8d89", name: "Common Raven", kind: "Animals", author: "WirtualneMuzeaMalopolski", license: "CC0 1.0", repair: ["--plinth", "0.07", "0.014"] },
  { id: "c5fad940863f47f784d792ca95e16b42", name: "Tuna Fish", kind: "Animals", author: "dhjwdwd", license: "CC BY 4.0", repair: ["--turn", "z", "90"] },
  { id: "daeadd913644438aa3096bb357a02016", name: "Electric Guitar", kind: "Instruments", author: "abazibiz", license: "CC BY 4.0", repair: ["--hull-open-pieces", "--size-m", "0.335"] },
  { id: "4ce9e8dbeb74425eb215e4168807dcb5", name: "Grand Piano", kind: "Instruments", author: "richterbelmont26", license: "CC BY 4.0", repair: ["--join-within", "0.003"], bake: ["--trim-overhangs"] },
  { id: "c65a00ca4a174653beb4c59cb42b9143", name: "Mossy Log", kind: "Nature", author: "julianmalik", license: "CC BY 4.0" },
  { id: "fc6e731a0131471ba8e45511c7ea9996", name: "Marble Bust", kind: "Figures", author: "WirtualneMuzeaMalopolski", license: "CC0 1.0" },
  { id: "2e9082cfd8c444d9b3b1b20821a0e101", name: "Duck", kind: "Animals", author: "darwinmuseum.ru", license: "CC BY-NC 4.0", repair: ["--trim-below", "0.29"] },
  { id: "1ca90c233cfd4fa0bd52077b37efde97", name: "Conch Shell", kind: "Nature", author: "jblock", license: "CC BY 4.0", repair: ["--hollow", "0.006", "0.006", "--inside-color", "176", "122", "104", "--max-triangles", "80000"] },
  { id: "c8c3f307e93f4c829d579d1b369e6b02", name: "Camera", kind: "Gadgets", author: "Santy", license: "CC BY 4.0", repair: ["--recolor", "200", "0.5", "0.45", "--size-m", "0.235"] },
];

/** The license deeds a credit links to, by the name it shows. */
const LICENSE_URLS = {
  "CC BY 4.0": "https://creativecommons.org/licenses/by/4.0/",
  "CC BY-NC 4.0": "https://creativecommons.org/licenses/by-nc/4.0/",
  "CC0 1.0": "https://creativecommons.org/publicdomain/zero/1.0/",
} as const;
type LicenseName = keyof typeof LICENSE_URLS;

/**
 * The credit line, after `lead`: the author, linked to the model's page so the license's link to
 * the material is kept, and the license, linked to its terms.
 */
function buildCreditHtml(cake: CakeEntry, lead: string): HTMLElement {
  const credit = document.createElement("span");
  credit.append(lead, buildLink(cake.author, `https://sketchfab.com/3d-models/${cake.id}`), " (", buildLink(cake.license, LICENSE_URLS[cake.license]), ")");
  return credit;
}

/** The credit line a round shows for its cake. */
export function buildRoundCreditHtml(cake: CakeEntry): HTMLElement {
  return buildCreditHtml(cake, "Model by ");
}

/** Every cake's credit line under its in-game name, alphabetically, for the credits page. */
export function buildCreditsList(): HTMLUListElement {
  const list = document.createElement("ul");
  for (const cake of [...CAKES].sort((a, b) => a.name.localeCompare(b.name))) {
    const item = document.createElement("li");
    item.append(buildCreditHtml(cake, `${cake.name} by `));
    list.append(item);
  }
  return list;
}

export function buildLink(text: string, href: string): HTMLAnchorElement {
  const link = document.createElement("a");
  link.href = href;
  link.target = "_blank";
  link.rel = "noreferrer";
  link.textContent = text;
  return link;
}

export async function fetchCakeFile(cake: CakeEntry): Promise<Uint8Array> {
  const response = await fetch(`${import.meta.env.BASE_URL}cakes/${cake.id}.cake`);
  if (!response.ok) throw new Error(`cake ${cake.id}: ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}
