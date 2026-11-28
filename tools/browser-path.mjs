// The headless browser the tools and agent scripts launch: `CAKE_BROWSER` where set, else the first installed of
// Edge or Chrome on Windows, a system Chrome or Chromium on Linux, and the newest Chrome headless shell under
// `build/browsers/`, which `src/client/node_modules/.bin/browsers install chrome-headless-shell@<version> --path build/browsers`
// installs without root, at the version puppeteer-core pins. The Linux paths take the newest of the versions there.
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const INSTALLED_BROWSERS = [
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "/usr/bin/microsoft-edge",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
];
const INSTALLED_FIREFOXES = ["C:\\Program Files\\Mozilla Firefox\\firefox.exe", "/usr/bin/firefox"];
const BROWSERS_DIR = fileURLToPath(new URL("../build/browsers/", import.meta.url));

export function resolveBrowserPath() {
  return resolveFrom("CAKE_BROWSER", INSTALLED_BROWSERS, "chrome-headless-shell", join("chrome-headless-shell-linux64", "chrome-headless-shell"));
}

/** Firefox likewise, for the sound checks: `CAKE_FIREFOX`, an installed one, or the newest under `build/browsers/`, which `browsers install firefox@stable --path build/browsers` installs. */
export function resolveFirefoxPath() {
  return resolveFrom("CAKE_FIREFOX", INSTALLED_FIREFOXES, "firefox", join("firefox", "firefox"));
}

function resolveFrom(envName, installedPaths, buildName, buildSubpath) {
  if (process.env[envName]) return process.env[envName];
  const installed = installedPaths.find((candidate) => existsSync(candidate));
  if (installed) return installed;
  const dir = join(BROWSERS_DIR, buildName);
  const versions = existsSync(dir) ? readdirSync(dir).filter((name) => name.startsWith("linux-")) : [];
  const newest = versions.sort((a, b) => a.localeCompare(b, undefined, { numeric: true })).at(-1);
  if (!newest) throw new Error(`no ${envName} set, nothing at ${installedPaths.join(", ")}, and none under ${dir}`);
  return join(dir, newest, buildSubpath);
}
