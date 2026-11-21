// A made-up extension, to try "Your custom extensions" (docs/extensions.md) on this PC.
//   npm run extension-example             write it, then keep it up to date every 2 seconds
//   npm run extension-example -- --once   write it once (it shows as out of date after a minute)
//   npm run extension-example -- --remove remove it again
// It writes %LOCALAPPDATA%\Trayward\extensions\example.json the way a real extension should: to a
// temporary file first, then renamed over the old one, so Trayward never reads half a file.
// Then: Settings → Extensions → Your custom extensions on, and the card "Example" on its page.
import { mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

if (!process.env.LOCALAPPDATA) {
  console.error("✗ No LOCALAPPDATA: Trayward's extensions folder is in it (Windows only).");
  process.exit(1);
}
const dir = join(process.env.LOCALAPPDATA, "Trayward", "extensions");
const file = join(dir, "example.json");

if (process.argv.includes("--remove")) {
  rmSync(file, { force: true });
  console.log(`Removed ${file}`);
  process.exit(0);
}

/** A time with its offset from UTC, as the format wants it: 2026-11-18T23:00:00+01:00. */
function zoned(date) {
  const pad = (n) => String(Math.abs(n)).padStart(2, "0");
  const offset = -date.getTimezoneOffset();
  const sign = offset < 0 ? "-" : "+";
  const day = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
  return `${day}T${time}${sign}${pad(Math.trunc(offset / 60))}:${pad(offset % 60)}`;
}

const started = new Date();
const nextRun = new Date(started.getTime() + 25 * 60 * 1000);

/** The file at step `n`: a copy job that fills up, then starts over. */
function extension(n) {
  const done = (n * 3) % 103;
  const copied = Math.round((done / 100) * 640);
  let state;
  if (done >= 100) state = "ok";
  else if (done >= 80) state = "warn";
  return {
    version: 1,
    id: "example",
    label: "Example",
    description: "Made up by npm run extension-example",
    tooltip: [
      { label: "Copying", type: "progress", value: Math.min(done, 100), state },
      { label: "Next run", type: "time", value: zoned(nextRun) },
    ],
    flyout: [
      { label: "Photos", type: "progress", value: copied, max: 640, text: `${copied} of 640 GB` },
      { label: "Documents", type: "text", value: "Done", state: "ok" },
      { label: "Videos", type: "text", value: "Share offline", state: "bad" },
      { label: "Started", type: "time", value: zoned(started) },
      { label: "Next run", type: "time", value: zoned(nextRun) },
    ],
    stale_after: 60,
  };
}

/** Writes the file whole: a temporary file, renamed over the old one. */
function write(data) {
  mkdirSync(dir, { recursive: true });
  const temp = `${file}.tmp`;
  writeFileSync(temp, JSON.stringify(data, null, 2) + "\n", "utf8");
  renameSync(temp, file);
}

write(extension(0));
console.log(`Wrote ${file}`);
console.log(
  "In Trayward: Settings → Extensions → Your custom extensions on, then switch on Example.",
);
if (process.argv.includes("--once")) {
  process.exit(0);
}
console.log("Updating it every 2 seconds; Ctrl+C stops (a minute later it shows as out of date).");
let n = 0;
setInterval(() => write(extension(++n)), 2000);
