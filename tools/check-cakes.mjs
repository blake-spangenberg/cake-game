// Fails when the client's cake list (src/client/src/cakes.ts) and the packed cakes in assets/cakes/ disagree.
import { readdirSync } from "node:fs";
import { CAKES } from "../src/client/src/cakes.ts";

const PACKED_EXT = ".cake.br";

const listedIds = new Set(CAKES.map((cake) => cake.id));
const packedNames = readdirSync(new URL("../assets/cakes/", import.meta.url)).filter((name) => name.endsWith(PACKED_EXT));
const packedIds = new Set(packedNames.map((name) => name.slice(0, -PACKED_EXT.length)));
const missingIds = [...listedIds].filter((id) => !packedIds.has(id));
const unlistedIds = [...packedIds].filter((id) => !listedIds.has(id));
for (const id of missingIds) console.error(`src/client/src/cakes.ts: lists ${id}, which has no assets/cakes/${id}${PACKED_EXT}`);
for (const id of unlistedIds) console.error(`assets/cakes/${id}${PACKED_EXT}: not listed in src/client/src/cakes.ts`);
process.exit(missingIds.length + unlistedIds.length > 0 ? 1 : 0);
