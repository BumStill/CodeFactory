#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// M144 / P0: a SQLite write transaction must never be held across external
// I/O. On Windows a held write lock blocks every later message until the app
// restarts, so this check fails the build when a transaction scope reaches a
// subprocess, network client, or model transport.
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";

const root = process.argv.slice(2).find((arg) => !arg.startsWith("--")) ?? "src-tauri/src";
const OPEN = /\b(begin_write\(|\.begin\(\)\.await)/;
const CLOSE = /\btx\.(commit|rollback)\(\)\.await|\bconn\.(commit|rollback)/;
const EXTERNAL = [
  /\bCommand::new\b/,
  /\breqwest::/,
  /\bClient::(new|builder)\b/,
  /\bmodel_transport\b/,
  /\bprovider\.(stream|complete|chat)\b/,
  /\.spawn\(\)\.await/,
];
const findings = [];

function walk(dir) {
  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    const stat = statSync(full);
    if (stat.isDirectory()) walk(full);
    else if (entry.endsWith(".rs")) scan(full);
  }
}

function scan(file) {
  const lines = readFileSync(file, "utf8").split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    if (!OPEN.test(lines[index])) continue;
    const start = index;
    let depth = 0;
    let opened = false;
    for (let cursor = index; cursor < Math.min(lines.length, start + 160); cursor += 1) {
      const line = lines[cursor];
      if (!opened && line.includes("{")) opened = true;
      if (opened) {
        depth += (line.match(/\{/g) ?? []).length;
        depth -= (line.match(/\}/g) ?? []).length;
      }
      if (CLOSE.test(line) || (opened && depth <= 0 && cursor > start)) break;
      if (cursor > start && EXTERNAL.some((pattern) => pattern.test(line))) {
        findings.push(`${file}:${cursor + 1}: external I/O inside a write transaction opened at ${start + 1}: ${line.trim()}`);
      }
    }
  }
}

walk(root);
if (findings.length > 0) {
  console.error("write transaction scope violations:\n" + findings.join("\n"));
  process.exit(1);
}
console.log(`write-transaction-scope: OK under ${root}`);
