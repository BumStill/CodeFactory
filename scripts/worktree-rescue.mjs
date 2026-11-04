// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { randomUUID } from 'node:crypto';

function git(cwd, args, env = {}) {
  return execFileSync('git', args, { cwd, env: { ...process.env, ...env }, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

/** Save the complete tree plus HEAD ancestry without touching the real index. */
export function rescueWorktree(dir) {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'codefactory-rescue-'));
  try {
    const env = { GIT_INDEX_FILE: path.join(temp, 'index'), GIT_AUTHOR_NAME: 'CodeFactory Rescue', GIT_AUTHOR_EMAIL: 'rescue@codefactory.invalid', GIT_COMMITTER_NAME: 'CodeFactory Rescue', GIT_COMMITTER_EMAIL: 'rescue@codefactory.invalid' };
    const head = git(dir, ['rev-parse', '--verify', 'HEAD']);
    git(dir, ['read-tree', head], env);
    git(dir, ['add', '-A'], env);
    const tree = git(dir, ['write-tree'], env);
    const commit = git(dir, ['commit-tree', tree, '-p', head, '-m', 'preserve before cleanup'], env);
    const ref = `refs/codefactory/rescue/cleanup/${randomUUID()}`;
    git(dir, ['update-ref', ref, commit]);
    if (git(dir, ['rev-parse', '--verify', ref]) !== commit) throw new Error('rescue ref verification failed');
    return ref;
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
}

/** Fail closed: inspect the removal subtree and save every worktree it contains.
 * Ordinary build caches without Git content do not snapshot the enclosing repo.
 * NUL-delimited porcelain preserves spaces, newlines and non-ASCII paths.
 */
export function rescueBeforeRemoval(target) {
  const root = fs.realpathSync(target);
  const repos = new Set();
  function visit(dir) {
    const entries = fs.readdirSync(dir, { withFileTypes: true });
    if (entries.some(e => e.name === '.git')) repos.add(dir);
    for (const entry of entries) {
      if (entry.isDirectory() && entry.name !== '.git') visit(path.join(dir, entry.name));
    }
  }
  visit(root);
  const targets = new Set(repos);
  for (const repo of repos) {
    const common = fs.realpathSync(git(repo, ['rev-parse', '--path-format=absolute', '--git-common-dir']));
    const commonRelative = path.relative(root, common);
    if (commonRelative === '' || (!commonRelative.startsWith(`..${path.sep}`) && commonRelative !== '..' && !path.isAbsolute(commonRelative))) {
      throw new Error('Git object database is inside removal subtree; preserve repository instead of deleting rescue refs');
    }
    for (const field of git(repo, ['worktree', 'list', '--porcelain', '-z']).split('\0')) {
      if (!field.startsWith('worktree ')) continue;
      const dir = fs.realpathSync(field.slice(9));
      const relative = path.relative(root, dir);
      if (relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative))) targets.add(dir);
    }
  }
  return [...targets].map(rescueWorktree);
}
