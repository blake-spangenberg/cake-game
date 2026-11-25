// Fails when the client's cake list (src/client/src/cakes.ts), the packed cakes in assets/cakes/, and the lobby's
// previews in assets/previews/ disagree: a cake unlisted or unpacked, or a preview missing or of an older bake.
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { CAKES } from "../src/client/src/cakes.ts";

const PACKED_EXT = ".cake.br";
const packedUrl = new URL("../assets/cakes/", import.meta.url);
const previewsUrl = new URL("../assets/previews/", import.meta.url);
const sourcesUrl = new URL("sources.json", previewsUrl);

const listedIds = new Set(CAKES.map((cake) => cake.id));
const packedNames = readdirSync(packedUrl).filter((name) => name.endsWith(PACKED_EXT));
const packedIds = new Set(packedNames.map((name) => name.slice(0, -PACKED_EXT.length)));
const missingIds = [...listedIds].filter((id) => !packedIds.has(id));
const unlistedIds = [...packedIds].filter((id) => !listedIds.has(id));
for (const id of missingIds) console.error(`src/client/src/cakes.ts: lists ${id}, which has no assets/cakes/${id}${PACKED_EXT}`);
for (const id of unlistedIds) console.error(`assets/cakes/${id}${PACKED_EXT}: not listed in src/client/src/cakes.ts`);

const sources = existsSync(sourcesUrl) ? JSON.parse(readFileSync(sourcesUrl, "utf8")) : {};
const stalePreviews = [...listedIds].filter((id) => packedIds.has(id)).flatMap((id) => findStalePreview(id));
for (const message of stalePreviews) console.error(message);
process.exit(missingIds.length + unlistedIds.length + stalePreviews.length > 0 ? 1 : 0);

/** Why cake `id`'s preview does not show its current bake, or nothing if it does. */
function findStalePreview(id) {
  if (!existsSync(new URL(`${id}.webp`, previewsUrl))) return [`assets/previews/${id}.webp: missing (make previews renders it)`];
  const hash = createHash("sha256").update(readFileSync(new URL(id + PACKED_EXT, packedUrl))).digest("hex");
  if (sources[id] !== hash) return [`assets/previews/${id}.webp: rendered from another bake than assets/cakes/${id}${PACKED_EXT} (make previews rerenders it)`];
  return [];
}
