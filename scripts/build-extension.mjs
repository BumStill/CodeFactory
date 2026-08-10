// SPDX-License-Identifier: Apache-2.0
//
// Assembles the loadable extension into extension/dist.
//
// This is the *development* path. Users never run it: the app compiles the same
// files into its binary (`browser::extension_package`) and writes them out with
// the pairing already filled in, which is what makes setup one click instead of
// a repository checkout. Keeping this script means the extension can be loaded
// from a working tree without building the app, and it is what
// `scripts/verify-extension-bridge.mjs` drives in a real browser.
//
// The copy step exists because the page script is shared with the desktop app and
// lives next to the Rust that injects it, but a Chrome extension can only load
// files inside its own directory.
//
//   pnpm ext:build     then load extension/dist as an unpacked extension

import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const source = join(root, "extension");
const dist = join(source, "dist");
const pageScript = join(root, "src-tauri", "src", "browser", "page.js");

const FILES = ["manifest.json", "background.js", "options.html", "options.js"];

await rm(dist, { recursive: true, force: true });
await mkdir(join(dist, "content"), { recursive: true });

for (const file of FILES) {
  await cp(join(source, file), join(dist, file));
}
await cp(pageScript, join(dist, "content", "page.js"));

// Fail loudly if the shared script ever stops being self-installing — the
// extension has no bundler, so an import would break only at runtime, inside a
// page, where nobody would see it.
const script = await readFile(pageScript, "utf8");
if (/^\s*(import|export)\s/m.test(script)) {
  throw new Error(
    "page.js must stay a standalone script: the extension loads it directly, with no bundler.",
  );
}

const manifest = JSON.parse(await readFile(join(source, "manifest.json"), "utf8"));
await writeFile(
  join(dist, "BUILD.txt"),
  `CodeFactory Browser Bridge ${manifest.version}\n` +
    "Load this directory via chrome://extensions → Developer mode → Load unpacked.\n" +
    "Development build. An installed CodeFactory writes its own copy, with pairing\n" +
    "already filled in, from Settings → Browser — nothing to type there.\n",
);

console.log(`extension built: ${dist}`);
