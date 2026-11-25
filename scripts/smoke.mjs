import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

const executable = path.resolve(process.argv[2] ?? `src-tauri/target/release/sing-shell${process.platform === 'win32' ? '.exe' : ''}`);
const ownedDirectory=!process.env.SING_SHELL_TEST_DATA;
const directory = process.env.SING_SHELL_TEST_DATA ?? await fs.mkdtemp(path.join(os.tmpdir(), 'sing-shell-smoke-'));
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
  assert.equal(report.closeRequestIntercepted, true);
  if(!process.env.EXPECTED_LEGACY) assert.equal(report.settings.schemaVersion,2);
  if(process.env.EXPECTED_VERSION) assert.equal(report.version,process.env.EXPECTED_VERSION);
  if(process.env.EXPECTED_RUST_TARGET) assert.equal(report.target,process.env.EXPECTED_RUST_TARGET);
  if(process.env.EXPECTED_CONFIG) assert.equal(report.settings.configPath,process.env.EXPECTED_CONFIG);
  console.log(JSON.stringify(report));
  completed = true;
} finally {
  if (completed && ownedDirectory) {
    assert.ok(directory.startsWith(path.join(os.tmpdir(), 'sing-shell-smoke-')));
    await fs.rm(directory, { recursive: true, force: true });
  }
  else if(!completed) console.error(`Smoke diagnostics: ${directory}`);
}
