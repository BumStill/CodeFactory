// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, symlinkSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
const validator = path.resolve('src-tauri/src/agent/trusted_python_ast.py');
const temp = mkdtempSync(path.join(tmpdir(), 'cf-m198-ast-'));
const root = path.join(temp, 'workspace');
mkdirSync(root);
const check = script => spawnSync('python3', ['-I', validator, root], { input: script, encoding: 'utf8' });
try {
  writeFileSync(path.join(root, 'source.rs'), 'old');
  for (let i = 0; i < 19; i++) {
    const script = `p='source.rs'\ns=open(p).read()\ns=s.replace('old','new-${i}')\nopen(p,'w').write(s)\n`;
    assert.equal(check(script).status, 0);
    assert.equal(readFileSync(path.join(root, 'source.rs'), 'utf8'), 'old', 'validator must not execute');
  }
  const helper = `p='source.rs'\ns=open(p).read()\norig=s\ndef sub(old,new,count=1):\n    global s\n    assert s.count(old)>=1, 'MISSING: '+old[:80]\n    s=s.replace(old,new,count)\nsub('old', '''new isn't shell; \u0060read\u0060''')\nopen(p,'w').write(s)\n`;
  assert.equal(check(helper).status, 0, check(helper).stderr);
  for (const script of [
    "open('../outside','w').write('bad')", "open('/tmp/outside','w').write('bad')",
    "open('.git/config','w').write('bad')", "open('.env.local').read()",
    "import os\nos.system('git push origin main')", "import socket\nsocket.create_connection(('example.invalid',80))",
    "__import__('os').system('git reset --hard')", "exec('pass')", "eval('1')",
    "p=open('source.rs').read()\nopen(p,'w').write('bad')",
    "open('source.rs','w',opener=eval).write('bad')", "open=lambda x: x",
    "def hidden():\n    import os\n    os.system('git push')",
    "class Evil:\n    pass", "for x in range(10):\n    pass",
    "open('C:/outside','w').write('bad')", "open('id_ed25519').read()",
  ]) assert.notEqual(check(script).status, 0, script);
  if (process.platform !== 'win32') {
    writeFileSync(path.join(temp, 'outside'), 'unchanged');
    symlinkSync(path.join(temp, 'outside'), path.join(root, 'escape'));
    assert.notEqual(check("open('escape','w').write('bad')").status, 0);
    assert.equal(readFileSync(path.join(temp, 'outside'), 'utf8'), 'unchanged');
  }
  console.log('M198 AST: generated local edits, original helper shape, no execution, unsafe effects and symlink escape verified');
} finally { rmSync(temp, { recursive: true, force: true }); }
