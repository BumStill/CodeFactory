#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Verify local chat attachments can be loaded through Tauri's asset protocol.
// Without this scope, convertFileSrc(path) produces an asset:// URL that the
// WebView rejects, so image previews render as a blank/broken tile.

import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const confPath = path.join(root, "src-tauri", "tauri.conf.json");
const conf = JSON.parse(fs.readFileSync(confPath, "utf8"));
const assetProtocol = conf?.app?.security?.assetProtocol;

function fail(message) {
  console.error(message);
  process.exit(1);
}

if (!assetProtocol || assetProtocol.enable !== true) {
  fail("app.security.assetProtocol.enable must be true for chat image previews");
}

const scope = assetProtocol.scope;
if (!Array.isArray(scope)) {
  fail("app.security.assetProtocol.scope must be an array");
}

const hasAttachmentScope = scope.some((entry) =>
  typeof entry === "string" && entry.includes(".codefactory/attachments") && entry.endsWith("/**"),
);
if (!hasAttachmentScope) {
  fail("asset protocol scope must allow project .codefactory/attachments/** paths");
}

console.log("attachment asset protocol scope: ok");
