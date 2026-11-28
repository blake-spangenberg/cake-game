// Renders the lobby picker's preview of each cake in src/client/src/cakes.ts from its packed bake, in headless Edge
// or Chrome on SwiftShader so every machine draws the same pixels. A preview records the hash of the bake it shows in
// assets/previews/sources.json, which tools/check-cakes.mjs compares against assets/cakes/.
// Usage: node tools/render-previews.mjs [--all]
// Renders only cakes whose preview is missing or shows an older bake, unless --all, and deletes previews of unlisted cakes.
// Needs the WASM build that BUILD_DIR names (make wasm), as the client's dev server does.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { brotliDecompressSync } from "node:zlib";
import { CAKES } from "../src/client/src/cakes.ts";
import { resolveBrowserPath } from "./browser-path.mjs";

const PREVIEW_SIZE_PX = 256;
const PREVIEW_EXT = ".webp";
const rootUrl = new URL("../", import.meta.url);
const clientDir = fileURLToPath(new URL("src/client/", rootUrl));
const previewsUrl = new URL("assets/previews/", rootUrl);
const sourcesUrl = new URL("sources.json", previewsUrl);

const args = process.argv.slice(2);
if (args.some((arg) => arg !== "--all")) {
  console.error(`usage: node tools/render-previews.mjs [--all], got ${args.join(" ")}`);
  process.exit(2);
}
const isAll = args.includes("--all");
const browserPath = resolveBrowserPath();

mkdirSync(previewsUrl, { recursive: true });
const sources = existsSync(sourcesUrl) ? JSON.parse(readFileSync(sourcesUrl, "utf8")) : {};
const listedIds = new Set(CAKES.map((cake) => cake.id));
for (const name of readdirSync(previewsUrl).filter((name) => name.endsWith(PREVIEW_EXT))) {
  const id = name.slice(0, -PREVIEW_EXT.length);
  if (listedIds.has(id)) continue;
  rmSync(new URL(name, previewsUrl));
  delete sources[id];
  console.log(`deleted assets/previews/${name}`);
}
const bakes = CAKES.map((cake) => ({ cake, packed: readFileSync(new URL(`assets/cakes/${cake.id}.cake.br`, rootUrl)) }))
  .map((bake) => ({ ...bake, hash: createHash("sha256").update(bake.packed).digest("hex") }))
  .filter(({ cake, hash }) => isAll || sources[cake.id] !== hash || !existsSync(new URL(cake.id + PREVIEW_EXT, previewsUrl)));

if (bakes.length > 0) await renderAll(bakes);
const sortedSources = Object.fromEntries(Object.entries(sources).sort(([a], [b]) => a.localeCompare(b)));
writeFileSync(sourcesUrl, `${JSON.stringify(sortedSources, null, 2)}\n`);

async function renderAll(bakes) {
  const requireClient = createRequire(new URL("package.json", pathToFileURL(clientDir)));
  const vite = await import(pathToFileURL(requireClient.resolve("vite")));
  const puppeteer = requireClient("puppeteer-core");
  const server = await vite.createServer({ root: clientDir, logLevel: "warn", server: { port: 0, strictPort: false } });
  await server.listen();
  const browser = await puppeteer.launch({ executablePath: browserPath, headless: true, args: ["--enable-unsafe-swiftshader", "--use-angle=swiftshader"] });
  try {
    const page = await browser.newPage();
    page.on("pageerror", (error) => console.error(`previews page: ${error.message}`));
    await page.goto(new URL("previews.html", server.resolvedUrls.local[0]).href);
    await page.waitForFunction(() => window.isPreviewReady === true);
    for (const { cake, packed, hash } of bakes) {
      const cakeBase64 = brotliDecompressSync(packed).toString("base64");
      const dataUrl = await page.evaluate((base64, sizePx) => window.renderPreview(base64, sizePx), cakeBase64, PREVIEW_SIZE_PX);
      if (!dataUrl.startsWith("data:image/webp;base64,")) throw new Error(`${cake.id}: the browser encoded ${dataUrl.slice(0, 30)}, not WebP`);
      const webp = Buffer.from(dataUrl.slice(dataUrl.indexOf(",") + 1), "base64");
      writeFileSync(new URL(cake.id + PREVIEW_EXT, previewsUrl), webp);
      sources[cake.id] = hash;
      console.log(`rendered assets/previews/${cake.id}${PREVIEW_EXT} (${cake.name}, ${webp.length} bytes)`);
    }
  } finally {
    await browser.close();
    await server.close();
  }
}
