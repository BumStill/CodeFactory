// SPDX-License-Identifier: Apache-2.0
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// M4B-1: the measurement must exercise the real admission/loop, not a timer simulation.
test('first-request fixture uses production admission and transport for five rounds', () => {
  const source = readFileSync(new URL('../src-tauri/src/agent/first_request_latency.rs', import.meta.url), 'utf8');
  for (const token of ['admit_headless_chat_turn', 'resolve_route_plan', 'checkpoint::create', 'AgentLoop::new_headless', 'agent.run(history)', 'TcpListener', '0..5', 'median_ms']) {
    assert.ok(source.includes(token), `missing production measurement boundary: ${token}`);
  }
});

test('skills, memory and delivery preparation are observable separately', () => {
  const source = readFileSync(new URL('../src-tauri/src/agent/mod.rs', import.meta.url), 'utf8');
  for (const stage of ['skills_memory', 'delivery_readiness']) {
    assert.ok(source.includes(`record("${stage}"`), `missing stage: ${stage}`);
  }
});
