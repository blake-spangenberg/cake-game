// Prefilters the studio the knife's steel reflects (src/client/src/studio.ts) in headless Edge or Chrome on
// SwiftShader, so every machine bakes the same bytes, and writes it where the client's props.ts loads it.
// Rerun it after changing the studio's look or upgrading three, whose prefilter it runs.
// Usage: node tools/bake-studio.mjs
import { writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { resolveBrowserPath } from "./browser-path.mjs";

const rootUrl = new URL("../", import.meta.url);
const clientDir = fileURLToPath(new URL("src/client/", rootUrl));
const outUrl = new URL("src/client/src/studio.bin", rootUrl);

if (process.argv.length > 2) {
  console.error(`usage: node tools/bake-studio.mjs, got ${process.argv.slice(2).join(" ")}`);
  process.exit(2);
}
const browserPath = resolveBrowserPath();

const requireClient = createRequire(new URL("package.json", pathToFileURL(clientDir)));
const vite = await import(pathToFileURL(requireClient.resolve("vite")));
const puppeteer = requireClient("puppeteer-core");
const server = await vite.createServer({ root: clientDir, logLevel: "warn", server: { port: 0, strictPort: false } });
await server.listen();
const browser = await puppeteer.launch({ executablePath: browserPath, headless: true, args: ["--enable-unsafe-swiftshader", "--use-angle=swiftshader"] });
try {
  const page = await browser.newPage();
  page.on("pageerror", (error) => console.error(`studio page: ${error.message}`));
  await page.goto(new URL("studio.html", server.resolvedUrls.local[0]).href);
  await page.waitForFunction(() => window.isStudioReady === true);
  const bytes = Buffer.from(await page.evaluate(() => window.bakeStudio()), "base64");
  writeFileSync(outUrl, bytes);
  console.log(`baked ${fileURLToPath(outUrl)} (${bytes.length} bytes)`);
} finally {
  await browser.close();
  await server.close();
}
