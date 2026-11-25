// Fails when the credits page's library list (src/client/src/libraries.ts) and the libraries the game ships disagree:
// the cake-wasm crates built for the browser and the client's package.json dependencies.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const WORKSPACE_PREFIX = "cake-";
const LIBRARIES_PATH = "src/client/src/libraries.ts";
const rootUrl = new URL("../", import.meta.url);

const treeText = execFileSync(
  "cargo",
  ["tree", "-p", "cake-wasm", "--target", "wasm32-unknown-unknown", "-e", "normal,no-proc-macro", "--prefix", "none", "--format", "{p}"],
  { cwd: rootUrl, encoding: "utf8", maxBuffer: 1 << 24 },
);
const crateNames = treeText
  .split(/\r?\n/)
  .map((line) => line.match(/^(\S+) v\S+/)?.[1])
  .filter((name) => name !== undefined && !name.startsWith(WORKSPACE_PREFIX));
const packageJson = JSON.parse(readFileSync(new URL("src/client/package.json", rootUrl), "utf8"));
const shippedNames = new Set([...crateNames, ...Object.keys(packageJson.dependencies)]);

const listedText = readFileSync(new URL(LIBRARIES_PATH, rootUrl), "utf8");
const listedNames = new Set([...listedText.matchAll(/\{ name: "([^"]+)"/g)].map((match) => match[1]));
const missingNames = [...shippedNames].filter((name) => !listedNames.has(name));
const staleNames = [...listedNames].filter((name) => !shippedNames.has(name));
for (const name of missingNames) console.error(`${LIBRARIES_PATH}: does not list ${name}, which ships to players`);
for (const name of staleNames) console.error(`${LIBRARIES_PATH}: lists ${name}, which no longer ships to players`);
process.exit(missingNames.length + staleNames.length > 0 ? 1 : 0);
