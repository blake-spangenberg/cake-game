import { buildLink } from "./cakes";

/** The open-source libraries that ship to players, by name; `tools/check-libraries.mjs` keeps it in sync. */
export const LIBRARIES: readonly { name: string; url: string; licenses: readonly string[] }[] = [
  { name: "approx", url: "https://github.com/brendanzab/approx", licenses: ["Apache-2.0"] },
  { name: "arrayvec", url: "https://github.com/bluss/arrayvec", licenses: ["MIT", "Apache-2.0"] },
  { name: "bitflags", url: "https://github.com/bitflags/bitflags", licenses: ["MIT", "Apache-2.0"] },
  { name: "bytemuck", url: "https://github.com/Lokathor/bytemuck", licenses: ["Zlib", "Apache-2.0", "MIT"] },
  { name: "byteorder", url: "https://github.com/BurntSushi/byteorder", licenses: ["Unlicense", "MIT"] },
  { name: "cfg-if", url: "https://github.com/rust-lang/cfg-if", licenses: ["MIT", "Apache-2.0"] },
  { name: "downcast-rs", url: "https://github.com/marcianx/downcast-rs", licenses: ["MIT", "Apache-2.0"] },
  { name: "either", url: "https://github.com/rayon-rs/either", licenses: ["MIT", "Apache-2.0"] },
  { name: "ena", url: "https://github.com/rust-lang/ena", licenses: ["MIT", "Apache-2.0"] },
  { name: "equivalent", url: "https://github.com/indexmap-rs/equivalent", licenses: ["Apache-2.0", "MIT"] },
  { name: "foldhash", url: "https://github.com/orlp/foldhash", licenses: ["Zlib"] },
  { name: "glam", url: "https://github.com/bitshifter/glam-rs", licenses: ["MIT", "Apache-2.0"] },
  { name: "glamx", url: "https://github.com/dimforge/glamx", licenses: ["MIT", "Apache-2.0"] },
  { name: "hash32", url: "https://github.com/japaric/hash32", licenses: ["MIT", "Apache-2.0"] },
  { name: "hashbrown", url: "https://github.com/rust-lang/hashbrown", licenses: ["MIT", "Apache-2.0"] },
  { name: "heapless", url: "https://github.com/rust-embedded/heapless", licenses: ["MIT", "Apache-2.0"] },
  { name: "indexmap", url: "https://github.com/indexmap-rs/indexmap", licenses: ["Apache-2.0", "MIT"] },
  { name: "libm", url: "https://github.com/rust-lang/compiler-builtins", licenses: ["MIT"] },
  { name: "log", url: "https://github.com/rust-lang/log", licenses: ["MIT", "Apache-2.0"] },
  { name: "matrixmultiply", url: "https://github.com/bluss/matrixmultiply", licenses: ["MIT", "Apache-2.0"] },
  { name: "nalgebra", url: "https://github.com/dimforge/nalgebra", licenses: ["Apache-2.0"] },
  { name: "num-complex", url: "https://github.com/rust-num/num-complex", licenses: ["MIT", "Apache-2.0"] },
  { name: "num-integer", url: "https://github.com/rust-num/num-integer", licenses: ["MIT", "Apache-2.0"] },
  { name: "num-rational", url: "https://github.com/rust-num/num-rational", licenses: ["MIT", "Apache-2.0"] },
  { name: "num-traits", url: "https://github.com/rust-num/num-traits", licenses: ["MIT", "Apache-2.0"] },
  { name: "ordered-float", url: "https://github.com/reem/rust-ordered-float", licenses: ["MIT"] },
  { name: "parry3d", url: "https://github.com/dimforge/parry", licenses: ["Apache-2.0"] },
  { name: "profiling", url: "https://github.com/aclysma/profiling", licenses: ["MIT", "Apache-2.0"] },
  { name: "rapier3d", url: "https://github.com/dimforge/rapier", licenses: ["Apache-2.0"] },
  { name: "rawpointer", url: "https://github.com/bluss/rawpointer", licenses: ["MIT", "Apache-2.0"] },
  { name: "rstar", url: "https://github.com/georust/rstar", licenses: ["MIT", "Apache-2.0"] },
  { name: "serde_core", url: "https://github.com/serde-rs/serde", licenses: ["MIT", "Apache-2.0"] },
  { name: "simba", url: "https://github.com/dimforge/simba", licenses: ["Apache-2.0"] },
  { name: "slab", url: "https://github.com/tokio-rs/slab", licenses: ["MIT"] },
  { name: "smallvec", url: "https://github.com/servo/rust-smallvec", licenses: ["MIT", "Apache-2.0"] },
  { name: "stable_deref_trait", url: "https://github.com/storyyeller/stable_deref_trait", licenses: ["MIT", "Apache-2.0"] },
  { name: "static_assertions", url: "https://github.com/nvzqz/static-assertions-rs", licenses: ["MIT", "Apache-2.0"] },
  { name: "thiserror", url: "https://github.com/dtolnay/thiserror", licenses: ["MIT", "Apache-2.0"] },
  { name: "three", url: "https://github.com/mrdoob/three.js", licenses: ["MIT"] },
  { name: "typenum", url: "https://github.com/paholg/typenum", licenses: ["MIT", "Apache-2.0"] },
  { name: "unicode-ident", url: "https://github.com/dtolnay/unicode-ident", licenses: ["MIT", "Apache-2.0", "Unicode-3.0"] },
  { name: "wasm-bindgen", url: "https://github.com/wasm-bindgen/wasm-bindgen", licenses: ["MIT", "Apache-2.0"] },
  { name: "wasm-bindgen-shared", url: "https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/shared", licenses: ["MIT", "Apache-2.0"] },
  { name: "wide", url: "https://github.com/Lokathor/wide", licenses: ["Zlib", "Apache-2.0", "MIT"] },
];

export function buildLibrariesList(): HTMLUListElement {
  const list = document.createElement("ul");
  for (const library of LIBRARIES) {
    const item = document.createElement("li");
    item.append(buildLink(library.name, library.url), " (");
    library.licenses.forEach((id, idx) => {
      if (idx > 0) item.append(" or ");
      item.append(buildLink(id, `https://spdx.org/licenses/${id}.html`));
    });
    item.append(")");
    list.append(item);
  }
  return list;
}
