/**
 * A sourced cake model: its baked file in `build/public/cakes/` (unpacked by `make cakes` from `assets/cakes/`),
 * the name players see, the credit its license requires, and how `tools/bake-cakes.mjs` bakes it.
 */
export interface CakeEntry {
  /** The model's Sketchfab id, which names its baked file and its page. */
  id: string;
  /** What the game calls the cake. */
  name: string;
  /** The Sketchfab user to credit. */
  author: string;
  license: LicenseName;
  /** Options for `tools/repair-model.py` beyond those every cake gets, such as a recolor. */
  repair?: readonly string[];
}

/** Order: the owner's favorites, then those that passed with reservations, then new candidates not yet reviewed. */
export const CAKES: readonly CakeEntry[] = [
  { id: "ccc3f77d27f94ae5acc77992b90b146b", name: "Leather Sofa", author: "gla_bot", license: "CC BY 4.0" },
  { id: "ecd6448e6dae4b93b60008913bdf8554", name: "Potato", author: "Tony.Richards", license: "CC BY 4.0" },
  { id: "3c889b5ee6b64a2aafb9b8977f8a8219", name: "Turkey Leg", author: "BillieBones", license: "CC BY 4.0" },
  { id: "79b95e7cf3dc422e814598b0eea6c83f", name: "Knitted Mitten", author: "nebulousflynn", license: "CC BY 4.0" },
  { id: "89b7408463b144d0bd157cc788dd496d", name: "Yellow Sneaker", author: "mr.pinoux", license: "CC BY 4.0" },
  { id: "8e0288f099bb418199e8896c19bed05e", name: "Toy Dinosaur", author: "TheCactusFactory", license: "CC BY 4.0", repair: ["--recolor", "103", "0.63", "0.63"] },
  { id: "0aeba0a56d414a6bb1865cf3375aef6b", name: "Lucky Cat", author: "squishynoize", license: "CC BY 4.0" },
  { id: "511db88f24e04eb1be8b0c0c080e1e5e", name: "Chicken Nugget", author: "Laserdesign", license: "CC BY 4.0" },
  { id: "348c77822bff4de3b6df59839b2fd201", name: "Camo Sneaker", author: "matousekfoto", license: "CC BY 4.0", repair: ["--hollow", "0.004", "0.022"] },
  { id: "d9506d9a318e4c56ac3d8ab273bd2540", name: "Work Boot", author: "inciprocal.com", license: "CC BY 4.0" },
  { id: "a5923998c60547c7a7887f11f2f2088d", name: "Ginger Root", author: "insomnia333", license: "CC BY 4.0" },
  { id: "53fdf984962948128db5f4ccfcae0843", name: "Old Pear", author: "inciprocal.com", license: "CC BY 4.0" },
  { id: "143e23a0436449f686ea0dd947e6f634", name: "Croissant", author: "rozza3d", license: "CC BY 4.0", repair: ["--fill-unlit-underside"] },
  { id: "a00d2eaa0c1d459f893ae4cbb08b866e", name: "Boxing Glove", author: "urheilumuseo", license: "CC BY 4.0" },
  { id: "b400281a14bf4f3a87bc6a053a19581a", name: "Scarecrow", author: "edemaistre", license: "CC BY 4.0" },
  { id: "e4b7d4137f444265ad98fb02780ccc5e", name: "Garden Gnome", author: "tiozombi", license: "CC BY 4.0" },
  { id: "ec9c0ac738fd4495af334ea2092e8d89", name: "Raven", author: "WirtualneMuzeaMalopolski", license: "CC0 1.0" },
  { id: "c5fad940863f47f784d792ca95e16b42", name: "Tuna", author: "dhjwdwd", license: "CC BY 4.0", repair: ["--turn", "z", "90"] },
  { id: "daeadd913644438aa3096bb357a02016", name: "Electric Guitar", author: "abazibiz", license: "CC BY 4.0", repair: ["--hull-open-pieces"] },
  { id: "4ce9e8dbeb74425eb215e4168807dcb5", name: "Grand Piano", author: "richterbelmont26", license: "CC BY 4.0" },
  { id: "c65a00ca4a174653beb4c59cb42b9143", name: "Mossy Log", author: "julianmalik", license: "CC BY 4.0" },
  { id: "fc6e731a0131471ba8e45511c7ea9996", name: "Marble Bust", author: "WirtualneMuzeaMalopolski", license: "CC0 1.0" },
  { id: "165d500fb0274a8e88a5bdc12df65492", name: "Classic Piano", author: "kozhokar_kate", license: "CC BY 4.0", repair: ["--hull-open-pieces"] },
];

/** The license deeds a credit links to, by the name it shows. */
const LICENSE_URLS = {
  "CC BY 4.0": "https://creativecommons.org/licenses/by/4.0/",
  "CC0 1.0": "https://creativecommons.org/publicdomain/zero/1.0/",
} as const;
type LicenseName = keyof typeof LICENSE_URLS;

/** The credit line: the author, linked to the model's page so the license's link to the material is kept, and the license, linked to its terms. */
export function buildCreditHtml(cake: CakeEntry): HTMLElement {
  const credit = document.createElement("span");
  credit.append("by ", buildLink(cake.author, `https://sketchfab.com/3d-models/${cake.id}`), " (", buildLink(cake.license, LICENSE_URLS[cake.license]), ")");
  return credit;
}

/** Every cake's credit line under its in-game name, alphabetically, for the credits page. */
export function buildCreditsList(): HTMLUListElement {
  const list = document.createElement("ul");
  for (const cake of [...CAKES].sort((a, b) => a.name.localeCompare(b.name))) {
    const item = document.createElement("li");
    item.append(`${cake.name} `, buildCreditHtml(cake));
    list.append(item);
  }
  return list;
}

function buildLink(text: string, href: string): HTMLAnchorElement {
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
