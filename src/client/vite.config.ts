import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { mkdir, readFile, readdir, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { brotliCompress, brotliDecompressSync, constants, gzip } from "node:zlib";
import { type Plugin, defineConfig } from "vite";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
// The Makefile's BUILD_DIR, relative to the repo root, where this agent's WASM build lives.
const buildDir = path.resolve(repoRoot, process.env.BUILD_DIR ?? "build");
const wasmDir = path.join(buildDir, "wasm");
const outDir = path.join(buildDir, "client");
/** Compressed variants by the content they compress, so a rebuild compresses only what changed: brotli at its best takes about a minute for the WASM. */
const precompressedCacheDir = path.join(buildDir, "precompressed");
/** The tracked brotli packs of the cakes, already compressed at brotli's best, which a cake whose bytes match reuses. */
const packedCakesDir = path.join(repoRoot, "assets", "cakes");
/**
 * What compresses: text, the WASM, the cakes, whose meshes shrink by about half, and the knife's
 * studio reflections (`.bin`), a tenth. Images and fonts come compressed.
 */
const COMPRESSED_EXTENSIONS = new Set([".html", ".js", ".css", ".wasm", ".cake", ".bin", ".json", ".svg"]);
/** A variant must save this share of its file's size to be worth a decode. */
const MIN_SAVED_SHARE = 0.05;
const BROTLI_WINDOW_BITS = 24;
const GZIP_LEVEL = 9;
/**
 * The game's chunk, in kB, which is about 90% three.js: it loads while the join dialog shows
 * (`src/startup.ts`), and its scene needs all of it at once, so splitting it further would shorten
 * no wait. The warning returns if the chunk grows well past its 640 kB.
 */
const CHUNK_SIZE_WARNING_LIMIT_KB = 700;

const compressBrotli = promisify(brotliCompress);
const compressGzip = promisify(gzip);

/**
 * Writes a brotli (`.br`) and a gzip (`.gz`) variant beside every compressible file of the build,
 * where it saves enough, for the server to send to a browser that accepts it.
 */
function precompress(): Plugin {
  return {
    name: "precompress",
    apply: "build",
    async closeBundle() {
      await mkdir(precompressedCacheDir, { recursive: true });
      const files = (await listFiles(outDir)).filter((file) => COMPRESSED_EXTENSIONS.has(path.extname(file)));
      const sizes = await Promise.all(files.map(precompressFile));
      const [original, brotli] = sizes.reduce(([a, b], [c, d]) => [a + c, b + d], [0, 0]);
      console.log(`precompressed ${files.length} files: ${(original / 1e6).toFixed(2)} MB, ${(brotli / 1e6).toFixed(2)} MB as brotli`);
    },
  };
}

async function listFiles(dir: string): Promise<string[]> {
  const entries = await readdir(dir, { withFileTypes: true, recursive: true });
  return entries.filter((entry) => entry.isFile()).map((entry) => path.join(entry.parentPath, entry.name));
}

/** Writes `file`'s variants, returning its size and the size it goes out in as brotli. */
async function precompressFile(file: string): Promise<[number, number]> {
  const bytes = await readFile(file);
  const hash = createHash("sha256").update(bytes).digest("hex");
  const [brotli, gzipped] = await Promise.all([
    fetchCached(`${hash}.br`, () => findPackedCake(file, bytes) ?? compressBrotliBest(bytes)),
    fetchCached(`${hash}.gz`, () => compressGzip(bytes, { level: GZIP_LEVEL })),
  ]);
  const maxSize = bytes.length * (1 - MIN_SAVED_SHARE);
  for (const [variant, ext] of [[brotli, ".br"], [gzipped, ".gz"]] as const) {
    if (variant.length <= maxSize) await writeFile(file + ext, variant);
  }
  return [bytes.length, Math.min(brotli.length, bytes.length)];
}

/** The cached variant named `name`, made by `compress` and cached first if there is none. */
async function fetchCached(name: string, compress: () => Buffer | Promise<Buffer>): Promise<Buffer> {
  const cached = path.join(precompressedCacheDir, name);
  try {
    return await readFile(cached);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const variant = await compress();
  // Renamed into place whole, since a build killed mid-write would leave a cut-short variant that later builds serve.
  const partial = `${cached}.${process.pid}.partial`;
  await writeFile(partial, variant);
  await rename(partial, cached);
  return variant;
}

/** The tracked brotli pack of a cake file, if it unpacks to exactly `bytes`, as it does unless the cake was rebaked since `make cakes`. */
function findPackedCake(file: string, bytes: Buffer): Buffer | null {
  if (path.extname(file) !== ".cake") return null;
  let packed: Buffer;
  try {
    packed = readFileSync(path.join(packedCakesDir, `${path.basename(file)}.br`));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw error;
  }
  return brotliDecompressSync(packed).equals(bytes) ? packed : null;
}

function compressBrotliBest(bytes: Buffer): Promise<Buffer> {
  const params = { [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY, [constants.BROTLI_PARAM_LGWIN]: BROTLI_WINDOW_BITS, [constants.BROTLI_PARAM_SIZE_HINT]: bytes.length };
  return compressBrotli(bytes, { params });
}

export default defineConfig({
  // Relative asset paths, so the build works behind a reverse proxy at a subpath as at the root.
  base: "./",
  publicDir: `${repoRoot}build/public`,
  resolve: { alias: { "@wasm": wasmDir } },
  server: { fs: { allow: [".", wasmDir] } },
  build: { outDir, emptyOutDir: true, chunkSizeWarningLimit: CHUNK_SIZE_WARNING_LIMIT_KB },
  plugins: [precompress()],
});
