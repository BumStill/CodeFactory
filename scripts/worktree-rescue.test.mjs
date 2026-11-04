import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { rescueBeforeRemoval } from './worktree-rescue.mjs';
const git = (cwd, ...args) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
test('M226 all CLI removal routes rescue before deleting', () => {
  for (const name of ['worktree-closeout.mjs', 'worktree-janitor.mjs', 'build-cache-budget.mjs']) {
    const source = fs.readFileSync(new URL(name, import.meta.url), 'utf8');
    assert.match(source, /rescueBeforeRemoval\(/, name);
  }
});
test('M226 nested work, untracked files and unpushed ancestry survive removal', () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'm226-'));
  try {
    const repo = path.join(tmp, 'repo'); fs.mkdirSync(repo);
    git(repo, 'init', '-b', 'main'); git(repo, 'config', 'user.name', 'Synthetic'); git(repo, 'config', 'user.email', 'synthetic@example.invalid');
    fs.writeFileSync(path.join(repo, '.gitignore'), '.claude/\n'); git(repo, 'add', '.'); git(repo, 'commit', '-m', 'base');
    const outer = path.join(tmp, 'task'); git(repo, 'worktree', 'add', '-b', 'task-a', outer);
    const nested = path.join(outer, '.claude', 'task b 中文'); fs.mkdirSync(path.dirname(nested), { recursive: true });
    git(repo, 'worktree', 'add', '-b', 'task-b', nested);
    fs.writeFileSync(path.join(nested, 'committed.txt'), 'local commit'); git(nested, 'add', '.'); git(nested, 'commit', '-m', 'unpushed');
    const head = git(nested, 'rev-parse', 'HEAD'); fs.writeFileSync(path.join(nested, 'new.txt'), 'untracked work');
    const refs = rescueBeforeRemoval(outer); assert.equal(refs.length, 2);
    const saved = refs.find(ref => { try { return git(repo, 'show', `${ref}:new.txt`) === 'untracked work'; } catch { return false; } });
    assert.ok(saved); assert.equal(git(repo, 'rev-parse', `${saved}^`), head);
    fs.rmSync(outer, { recursive: true });
    assert.equal(git(repo, 'show', `${saved}:new.txt`), 'untracked work');
    assert.equal(git(repo, 'show', `${saved}:committed.txt`), 'local commit');
  } finally { fs.rmSync(tmp, { recursive: true, force: true }); }
});
test('M226 malformed nested Git metadata prevents removal', () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'm226-invalid-'));
  try {
    fs.writeFileSync(path.join(tmp, '.git'), 'gitdir: missing\n');
    fs.writeFileSync(path.join(tmp, 'work.txt'), 'keep');
    assert.throws(() => rescueBeforeRemoval(tmp));
    assert.equal(fs.readFileSync(path.join(tmp, 'work.txt'), 'utf8'), 'keep');
  } finally { fs.rmSync(tmp, { recursive: true, force: true }); }
});
