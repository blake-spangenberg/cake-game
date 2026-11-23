/** A sourced cake model: its baked file in `build/public/cakes/` (unpacked by `make cakes` from `assets/cakes/`) and the credit its license requires. */
export interface CakeEntry {
  /** The model's Sketchfab id, which names its baked file and its page. */
  id: string;
  title: string;
  author: string;
}

/** Every model is CC BY 4.0. Order: the stylized picks first, then the scan backups. */
export const CAKES: readonly CakeEntry[] = [
  { id: "ccc3f77d27f94ae5acc77992b90b146b", title: "Verona Leather Corner Sofa", author: "gla_bot" },
  { id: "9cef8fd19dd84684b91830f35cc54d4f", title: "Corner Sofa", author: "Heliona" },
  { id: "138f793adde045a5a5247edf48f61eb1", title: "Paloma couch", author: "skaf13" },
  { id: "8e0288f099bb418199e8896c19bed05e", title: "Dinosaur Toy", author: "TheCactusFactory" },
  { id: "89b7408463b144d0bd157cc788dd496d", title: "Sneakers Adidas Yellow & Black", author: "mr.pinoux" },
  { id: "0aeba0a56d414a6bb1865cf3375aef6b", title: "cat lucky", author: "squishynoize" },
  { id: "118f19a1e9a1479285a3141c0d1f9b3f", title: "Maneki-Neko", author: "OctaDC" },
  { id: "bfce6e6485744250b75615b3d97c88dc", title: "Sneaker Scan Adidas", author: "ruda5" },
  { id: "9cfc981e3335404993adc4b457854c89", title: "Worn Hugo Boss Sneaker", author: "inciprocal.com" },
  { id: "ecd6448e6dae4b93b60008913bdf8554", title: "Heritage Potato Imaging", author: "Tony.Richards" },
  { id: "3c889b5ee6b64a2aafb9b8977f8a8219", title: "Turkey leg", author: "BillieBones" },
  { id: "511db88f24e04eb1be8b0c0c080e1e5e", title: "Chicken Nugget", author: "Laserdesign" },
  { id: "79b95e7cf3dc422e814598b0eea6c83f", title: "Knitted mitten", author: "nebulousflynn" },
  { id: "9c1cd4d051bc419b8ce121f9cd2730ff", title: "Maneki-neko ~ Asian lucky cat", author: "Arestame" },
];

export function buildCreditHtml(cake: CakeEntry): HTMLElement {
  const credit = document.createElement("span");
  const link = document.createElement("a");
  link.href = `https://sketchfab.com/3d-models/${cake.id}`;
  link.target = "_blank";
  link.rel = "noreferrer";
  link.textContent = cake.title;
  credit.append(link, ` by ${cake.author} (CC BY 4.0)`);
  return credit;
}

export async function fetchCakeFile(cake: CakeEntry): Promise<Uint8Array> {
  const response = await fetch(`${import.meta.env.BASE_URL}cakes/${cake.id}.cake`);
  if (!response.ok) throw new Error(`cake ${cake.id}: ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}
