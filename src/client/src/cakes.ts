/** A sourced cake model: its baked file in `build/public/cakes/` (unpacked by `make cakes` from `assets/cakes/`) and the credit its license requires. */
export interface CakeEntry {
  /** The model's Sketchfab id, which names its baked file and its page. */
  id: string;
  title: string;
  author: string;
}

/** Every model is CC BY 4.0. Order: the owner's favorites, then those that passed with reservations, then new candidates not yet reviewed. */
export const CAKES: readonly CakeEntry[] = [
  { id: "ccc3f77d27f94ae5acc77992b90b146b", title: "Verona Leather Corner Sofa", author: "gla_bot" },
  { id: "ecd6448e6dae4b93b60008913bdf8554", title: "Heritage Potato Imaging", author: "Tony.Richards" },
  { id: "3c889b5ee6b64a2aafb9b8977f8a8219", title: "Turkey leg", author: "BillieBones" },
  { id: "79b95e7cf3dc422e814598b0eea6c83f", title: "Knitted mitten", author: "nebulousflynn" },
  { id: "89b7408463b144d0bd157cc788dd496d", title: "Sneakers Adidas Yellow & Black", author: "mr.pinoux" },
  { id: "9c1cd4d051bc419b8ce121f9cd2730ff", title: "Maneki-neko ~ Asian lucky cat", author: "Arestame" },
  { id: "8e0288f099bb418199e8896c19bed05e", title: "Dinosaur Toy", author: "TheCactusFactory" },
  { id: "0aeba0a56d414a6bb1865cf3375aef6b", title: "cat lucky", author: "squishynoize" },
  { id: "bfce6e6485744250b75615b3d97c88dc", title: "Sneaker Scan Adidas", author: "ruda5" },
  { id: "9cfc981e3335404993adc4b457854c89", title: "Worn Hugo Boss Sneaker", author: "inciprocal.com" },
  { id: "511db88f24e04eb1be8b0c0c080e1e5e", title: "Chicken Nugget", author: "Laserdesign" },
  { id: "348c77822bff4de3b6df59839b2fd201", title: "Air Force", author: "matousekfoto" },
  { id: "d9506d9a318e4c56ac3d8ab273bd2540", title: "Harrison Work Boot", author: "inciprocal.com" },
  { id: "42035cf49db7473dbb78f60112838662", title: "Duck slipper 3D Scan", author: "Elcompa_Puente" },
  { id: "a5923998c60547c7a7887f11f2f2088d", title: "[3D-Scan] Ginger Root", author: "insomnia333" },
  { id: "53fdf984962948128db5f4ccfcae0843", title: "Expired Bartlett Pear", author: "inciprocal.com" },
  { id: "143e23a0436449f686ea0dd947e6f634", title: "Croissant Photoscan", author: "rozza3d" },
  { id: "a00d2eaa0c1d459f893ae4cbb08b866e", title: "Boxing Glove from 1952 Olympic Games", author: "urheilumuseo" },
  { id: "b400281a14bf4f3a87bc6a053a19581a", title: "Scarecrow figurine", author: "edemaistre" },
  { id: "e4b7d4137f444265ad98fb02780ccc5e", title: "Garden Gnomes 01", author: "tiozombi" },
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
