// M233: executable startup wiring regression, independent of Rust build.
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';
const source = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8');
test('M233 setup does not synchronously rebuild spend or settle historical sessions', () => {
  const setup = source.slice(source.indexOf('.setup(|app| {'), source.indexOf('.invoke_handler(tauri::generate_handler!['));
  const boundary = setup.indexOf('startup_recovery::spawn');
  assert.ok(boundary >= 0, 'recovery must leave the window setup path');
  const foreground = setup.slice(0, boundary);
  assert.ok(!foreground.includes('daily_spend_live::rebuild('));
  assert.ok(!foreground.includes('thin_derive::settle_merged_sessions('));
});
test('M233 startup database connection defers expensive knowledge FTS backfill', () => {
  const db = readFileSync(new URL('../src-tauri/src/storage/db.rs', import.meta.url), 'utf8');
  assert.ok(source.includes('storage::db::connect_startup('));
  assert.ok(db.includes('ensure_schema_for_startup'));
});
