// Builds and runs Trayward on this PC, saying each step as it goes.
//   node scripts/run.mjs start   stop the running Trayward, build the release, start it
//   node scripts/run.mjs demo    build and start the demo (made-up data, next to the real one)
//   node scripts/run.mjs stop    stop the running Trayward (not the demo)
//   node scripts/run.mjs build   build the release
// Stopping is the same as Exit in its menu: its hidden control window is asked to close, and it
// saves its state. (taskkill without /F doesn't reach that window: it would wait for nothing.)
import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join, relative } from "node:path";
import { root } from "./tools.mjs";

const release = join(root, "target", "release", "trayward.exe");
// Windows' own programs by their full path, not whatever of that name PATH finds first
const system32 = join(process.env.SystemRoot ?? String.raw`C:\Windows`, "System32");
const powershell = join(system32, "WindowsPowerShell", "v1.0", "powershell.exe");
const taskkill = join(system32, "taskkill.exe");
const debug = join(root, "target", "debug", "trayward.exe");

const started = Date.now();
const seconds = (ms) => `${(ms / 1000).toFixed(1)} s`;
/** A step, numbered: "[1/3] Building the release". */
let step = 0;
let steps = 0;
const say = (text) => console.log(`\n[${++step}/${steps}] ${text}`);
const info = (text) => console.log(`      ${text}`);
const shown = (path) => relative(process.cwd(), path) || path;

/** Runs a command in the repository, showing it and its output; ends the script if it fails. */
function run(cmd, args) {
  info(`> ${cmd} ${args.join(" ")}`);
  const begin = Date.now();
  const r = spawnSync(cmd, args, { cwd: root, stdio: "inherit" });
  if (r.status !== 0) {
    console.error(`\n✗ ${cmd} ${args.join(" ")} failed (exit code ${r.status ?? "none"}).`);
    process.exit(r.status ?? 1);
  }
  info(`done in ${seconds(Date.now() - begin)}`);
}

/** The running Trayward processes (not the demo, not a hook): id and exe. */
function running() {
  const ps = spawnSync(
    powershell,
    [
      "-NoProfile",
      "-Command",
      "Get-CimInstance Win32_Process -Filter \"Name='trayward.exe'\" | " +
        "Where-Object { $_.CommandLine -notmatch '--demo|trayward-hook' } | " +
        'ForEach-Object { "$($_.ProcessId)`t$($_.ExecutablePath)" }',
    ],
    { encoding: "utf8" },
  );
  return ps.stdout
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [id, exe] = line.split("\t");
      return { id, exe };
    });
}

/**
 * Posts WM_CLOSE to Trayward's control window (not the demo's, which has its own class).
 * Returns whether the window was there.
 */
function askToClose() {
  const ps = spawnSync(
    powershell,
    [
      "-NoProfile",
      "-Command",
      "Add-Type -Namespace Trayward -Name Win -MemberDefinition '" +
        '[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string t);' +
        '[DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);' +
        "'; $h = [Trayward.Win]::FindWindowW('Trayward.Control', [NullString]::Value); " +
        "if ($h -eq [IntPtr]::Zero) { 'none' } else { [void][Trayward.Win]::PostMessageW($h, 0x10, [IntPtr]::Zero, [IntPtr]::Zero); 'sent' }",
    ],
    { encoding: "utf8" },
  );
  return ps.stdout.trim() === "sent";
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
/** Waits until `done()` says so or `ms` have passed, looking 5 times a second; `tick(waited)` in between. */
function waitFor(done, ms, tick) {
  const begin = Date.now();
  return new Promise((resolve) => {
    const timer = setInterval(() => {
      const waited = Date.now() - begin;
      if (done() || waited >= ms) {
        clearInterval(timer);
        resolve();
      } else {
        tick(waited);
      }
    }, 200);
  });
}
/** How long Trayward gets to close by itself. */
const CLOSE_WAIT_MS = 10_000;

/** Asks the running Trayward to close (it saves its state), and waits for it. */
async function stop() {
  say("Closing the running Trayward");
  const found = running();
  if (found.length === 0) {
    info("none is running");
    return;
  }
  for (const p of found) {
    info(`process ${p.id}: ${p.exe ? shown(p.exe) : "trayward.exe"}`);
  }
  info("asking it to close (as Exit in its menu: it saves its state)…");
  if (!askToClose()) {
    info("its window wasn't found (still starting?)");
  }
  const begin = Date.now();
  let lastNote = 0;
  await waitFor(
    () => running().length === 0,
    CLOSE_WAIT_MS,
    (waited) => {
      if (waited - lastNote >= 2000) {
        lastNote = waited;
        info(`still closing, waited ${seconds(waited)} of ${seconds(CLOSE_WAIT_MS)}…`);
      }
    },
  );
  const left = running();
  if (left.length > 0) {
    info(`it didn't close within ${seconds(CLOSE_WAIT_MS)}: ending it (taskkill /F)`);
    spawnSync(taskkill, ["/F", ...left.flatMap((p) => ["/PID", p.id])], { stdio: "ignore" });
    await sleep(500);
  }
  if (running().length > 0) {
    console.error("\n✗ Trayward is still running; close it from its menu (right-click → Exit).");
    process.exit(1);
  }
  info(`closed in ${seconds(Date.now() - begin)}`);
}

/** Starts `exe` on its own: it keeps running after this script ends. */
function launch(exe, args = []) {
  say(`Starting ${shown(exe)}${args.length ? " " + args.join(" ") : ""}`);
  if (!existsSync(exe)) {
    console.error(`\n✗ ${shown(exe)} doesn't exist.`);
    process.exit(1);
  }
  const child = spawn(exe, args, { cwd: root, detached: true, stdio: "ignore" });
  child.unref();
  info(`running as process ${child.pid}; it stays when this window closes`);
}

const command = process.argv[2] ?? "start";
console.log(`Trayward: ${command}`);
switch (command) {
  case "start":
    steps = 3;
    await stop();
    say("Building the release");
    run("cargo", ["build", "--release", "--locked"]);
    launch(release);
    break;
  case "demo":
    steps = 2;
    say("Building the debug build");
    run("cargo", ["build", "--locked"]);
    launch(debug, ["--demo"]);
    info("the demo runs next to the real one; it reads nothing of yours");
    break;
  case "stop":
    steps = 1;
    await stop();
    break;
  case "build":
    steps = 1;
    say("Building the release");
    run("cargo", ["build", "--release", "--locked"]);
    info(`→ ${shown(release)}`);
    break;
  default:
    console.error(`Unknown command: ${command} (start, demo, stop, build)`);
    process.exit(2);
}
console.log(`\n✓ Done in ${seconds(Date.now() - started)}.`);
