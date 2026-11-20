import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));

export default defineConfig({
  resolve: { tsconfigPaths: true },
  server: { fs: { allow: [".", `${repoRoot}build/wasm`] } },
  build: { outDir: `${repoRoot}build/client`, emptyOutDir: true },
});
