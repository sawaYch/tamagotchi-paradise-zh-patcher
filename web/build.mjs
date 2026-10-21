import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const web = dirname(fileURLToPath(import.meta.url));
const root = resolve(web, "..");
const dist = join(web, "dist");
const wasm = join(
  root,
  "target",
  "wasm32-unknown-unknown",
  "release",
  "paradise-gui.wasm",
);

function run(cmd, args, options = {}) {
  const r = spawnSync(cmd, args, { cwd: root, stdio: "inherit", ...options });
  if (r.error || r.status !== 0) {
    console.error(`Failed: ${cmd} ${args.join(" ")}`);
    process.exit(r.status || 1);
  }
}

rmSync(dist, { recursive: true, force: true });
mkdirSync(join(dist, "pkg"), { recursive: true });

run("cargo", [
  "build",
  "--release",
  "--locked",
  "--target",
  "wasm32-unknown-unknown",
  "-p",
  "paradise-gui",
]);

run("wasm-bindgen", [
  "--target",
  "web",
  "--no-typescript",
  "--out-name",
  "paradise",
  "--out-dir",
  join(dist, "pkg"),
  wasm,
]);

for (const f of ["index.html", "_headers"]) {
  cpSync(join(web, f), join(dist, f));
}
console.log(`Built ${dist}`);
