// Compresses baked .cake files for git and unpacks them for the client, with Node's built-in brotli.
// Usage: node tools/cake-pack.mjs pack <in.cake> <out.cake.br>
//        node tools/cake-pack.mjs sync <packed dir> <unpacked dir>
// sync unpacks each <id>.cake.br newer than its <id>.cake, and deletes each <id>.cake with no source.
import { brotliCompressSync, brotliDecompressSync, constants } from "node:zlib";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";

const PACKED_EXT = ".cake.br";
const UNPACKED_EXT = ".cake";

function pack(source, destination) {
  const bytes = readFileSync(source);
  const params = { [constants.BROTLI_PARAM_QUALITY]: 11, [constants.BROTLI_PARAM_LGWIN]: 24, [constants.BROTLI_PARAM_SIZE_HINT]: bytes.length };
  mkdirSync(path.dirname(destination), { recursive: true });
  writeFileSync(destination, brotliCompressSync(bytes, { params }));
}

function listIds(dir, ext) {
  return readdirSync(dir).filter((name) => name.endsWith(ext)).map((name) => name.slice(0, -ext.length));
}

function sync(packedDir, unpackedDir) {
  mkdirSync(unpackedDir, { recursive: true });
  const packedIds = listIds(packedDir, PACKED_EXT);
  for (const id of packedIds) {
    const source = path.join(packedDir, id + PACKED_EXT);
    const destination = path.join(unpackedDir, id + UNPACKED_EXT);
    if (existsSync(destination) && statSync(destination).mtimeMs >= statSync(source).mtimeMs) continue;
    writeFileSync(destination, brotliDecompressSync(readFileSync(source)));
    console.log(`unpacked ${destination}`);
  }
  for (const id of listIds(unpackedDir, UNPACKED_EXT)) {
    if (packedIds.includes(id)) continue;
    const stale = path.join(unpackedDir, id + UNPACKED_EXT);
    rmSync(stale);
    console.log(`deleted ${stale}`);
  }
}

const modes = { pack, sync };
const [mode, source, destination] = process.argv.slice(2);
if (!Object.hasOwn(modes, mode) || !source || !destination) {
  console.error("usage: node tools/cake-pack.mjs pack <in.cake> <out.cake.br> | sync <packed dir> <unpacked dir>");
  process.exit(2);
}
modes[mode](source, destination);
