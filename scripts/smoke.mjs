import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

const executable = path.resolve(process.argv[2] ?? `src-tauri/target/release/sing-shell${process.platform === 'win32' ? '.exe' : ''}`);
const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'sing-shell-smoke-'));
let completed = false;
try {
  const child = spawn(executable, ['--smoke-test', '--test-data-dir', directory], { stdio: 'inherit', windowsHide: true });
  const timer = setTimeout(() => { child.kill(); }, 30_000);
  try {
    await new Promise((resolve, reject) => {
      child.on('error', reject);
      child.on('exit', (code, signal) => code === 0 ? resolve() : reject(new Error(`smoke exited: ${code ?? signal}`)));
    });
  } finally { clearTimeout(timer); }
  const report = JSON.parse(await fs.readFile(path.join(directory, 'smoke-result.json'), 'utf8'));
  assert.equal(report.completed, true);
  assert.equal(report.frontendReady, true);
  console.log(JSON.stringify(report));
  completed = true;
} finally {
  if (completed) {
    assert.ok(directory.startsWith(path.join(os.tmpdir(), 'sing-shell-smoke-')));
    await fs.rm(directory, { recursive: true, force: true });
  }
  else console.error(`Smoke diagnostics: ${directory}`);
}
