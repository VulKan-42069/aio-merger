// Downloads the FFmpeg runtime that the Windows installer bundles.
//
//   npm run fetch-ffmpeg
//
// Source: BtbN/FFmpeg-Builds (GPL, shared libraries, so ffmpeg and ffprobe
// share one set of DLLs). Extracts to ./ffmpeg/windows/.
// Linux builds use the system ffmpeg instead (the .deb/.rpm depend on it).

import { createWriteStream, existsSync, mkdirSync, readdirSync, rmSync, copyFileSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { pipeline } from "node:stream/promises";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const DEST = join(ROOT, "ffmpeg", "windows");
const URL = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n8.1-latest-win64-gpl-shared-8.1.zip";

if (existsSync(join(DEST, "ffmpeg.exe")) && !process.argv.includes("--force")) {
  console.log("ffmpeg/windows already present (use --force to refresh).");
  process.exit(0);
}

const work = join(tmpdir(), `aio-ffmpeg-${Date.now()}`);
mkdirSync(work, { recursive: true });
const zip = join(work, "ffmpeg.zip");

console.log("Downloading", URL);
const res = await fetch(URL);
if (!res.ok) throw new Error(`download failed: ${res.status}`);
await pipeline(res.body, createWriteStream(zip));

console.log("Extracting…");
const out = join(work, "x");
if (process.platform === "win32") {
  execFileSync("powershell", ["-NoProfile", "-Command", `Expand-Archive -LiteralPath '${zip}' -DestinationPath '${out}' -Force`], { stdio: "inherit" });
} else {
  execFileSync("unzip", ["-q", zip, "-d", out], { stdio: "inherit" });
}

const top = readdirSync(out).map((d) => join(out, d)).find((d) => statSync(d).isDirectory());
const bin = join(top, "bin");
rmSync(DEST, { recursive: true, force: true });
mkdirSync(DEST, { recursive: true });
for (const f of readdirSync(bin)) {
  if (f.endsWith(".dll") || f === "ffmpeg.exe" || f === "ffprobe.exe") copyFileSync(join(bin, f), join(DEST, f));
}
copyFileSync(join(top, "LICENSE.txt"), join(DEST, "FFMPEG-LICENSE.txt"));
rmSync(work, { recursive: true, force: true });
console.log("FFmpeg ready in", DEST);
