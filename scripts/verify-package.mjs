import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { verifyBinary } from './verify-binary.mjs';

const version = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json')).version;
const target = process.env.EXPECTED_RUST_TARGET;
assert.ok(target, 'set EXPECTED_RUST_TARGET');
assert.equal(target, `${process.arch === 'arm64' ? 'aarch64' : 'x86_64'}-${{ win32: 'pc-windows-msvc', linux: 'unknown-linux-gnu', darwin: 'apple-darwin' }[process.platform]}`);
const directory = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'sing-shell-package-')));
const bundle = path.resolve('src-tauri/target/release/bundle');
const report = {
  version, target, source: process.env.GITHUB_SHA ?? execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
  run: process.env.GITHUB_RUN_ID ?? null, passed: false, packages: [], verified: [],
  skipped: ['publisher code signing', 'macOS notarization', 'TUN and system proxy'],
};

function run(command, args, env = {}) {
  const result = spawnSync(command, args, { stdio: 'inherit', windowsHide: true, timeout: 180_000, env: { ...process.env, ...env } });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${command}: ${result.status}/${result.signal}`);
}
function hash(file) { return createHash('sha256').update(fs.readFileSync(file)).digest('hex'); }
function one(files, label) {
  assert.equal(files.length, 1, `expected one ${label}, found ${JSON.stringify(files)}`);
  return files[0];
}
function packageFile(extension) {
  const relative = one(fs.readdirSync(bundle, { recursive: true }).filter(file => file.endsWith(extension)), extension);
  const file = path.join(bundle, relative);
  report.packages.push({ name: path.basename(file), size: fs.statSync(file).size, sha256: hash(file) });
  return file;
}
function smoke(executable, label, native = true) {
  if (native) verifyBinary(executable, target);
  run(process.execPath, ['scripts/smoke.mjs', executable], { APPIMAGE_EXTRACT_AND_RUN: '1', WEBKIT_DISABLE_DMABUF_RENDERER: '1' });
  report.verified.push(label);
}

try {
  if (process.platform === 'win32') {
    const install = path.join(directory, 'installed');
    run(packageFile('-setup.exe'), ['/S', `/D=${install}`]);
    try { smoke(path.join(install, 'sing-shell.exe'), 'NSIS installed executable: native architecture, WebView, IPC and exit'); }
    finally { run(path.join(install, 'uninstall.exe'), ['/S', `_?=${install}`]); }
  } else if (process.platform === 'linux') {
    const extracted = path.join(directory, 'deb');
    run('dpkg-deb', ['-x', packageFile('.deb'), extracted]);
    smoke(path.join(extracted, 'usr/bin/sing-shell'), 'DEB extracted executable: native architecture, WebView, IPC and exit');
    const appimage = packageFile('.AppImage');
    fs.chmodSync(appimage, 0o755);
    smoke(appimage, 'AppImage self-extraction, WebView, IPC and exit', false);
  } else {
    const mount = path.join(directory, 'mounted');
    run('hdiutil', ['attach', packageFile('.dmg'), '-nobrowse', '-readonly', '-mountpoint', mount]);
    try { smoke(path.join(mount, 'Sing Shell.app/Contents/MacOS/sing-shell'), 'DMG mounted executable: native architecture, WebView, IPC and exit'); }
    finally { run('hdiutil', ['detach', mount]); }
    const archives = fs.readdirSync(bundle, { recursive: true }).filter(file => file.endsWith('.app.tar.gz'));
    if (archives.length) {
      const archive = packageFile('.app.tar.gz');
      const extracted = path.join(directory, 'app'); fs.mkdirSync(extracted);
      run('tar', ['-xzf', archive, '-C', extracted]);
      smoke(path.join(extracted, 'Sing Shell.app/Contents/MacOS/sing-shell'), 'app archive: native architecture, WebView, IPC and exit');
    }
  }

  const coreVersion = '1.14.4';
  const corePlatform = { win32: 'windows', linux: 'linux', darwin: 'darwin' }[process.platform];
  const coreArch = process.arch === 'arm64' ? 'arm64' : 'amd64';
  const upstream = `sing-box-${coreVersion}-${corePlatform}-${coreArch}`;
  const archive = `${upstream}.${process.platform === 'win32' ? 'zip' : 'tar.gz'}`;
  const expected = {
    'windows-amd64': '99185d75d314a07dbba4a130c8ad986cbeb629a07927975636b341ca61f991df',
    'windows-arm64': '8cd0a4d0cd8cd480be5731ac31f17038969a42d12add9feb6e031fd43d85bbae',
    'linux-amd64': '699a1b510aa4396b2176d32d1181920399ebf1e267835da6f3b598d0f7c24968',
    'linux-arm64': '436441f76ee2f7fe21b5b2d426d3cb261a480455a704865789ad533e4273ec26',
    'darwin-arm64': '0d8c7f5ed5d587b0f1a5e9772a708ec3bfa4ac4c9329604b01d8d9c2ba406838',
  }[`${corePlatform}-${coreArch}`];
  run('gh', ['release', 'download', `v${coreVersion}`, '--repo', 'SagerNet/sing-box', '--pattern', archive, '--dir', directory]);
  const coreArchive = path.join(directory, archive);
  assert.equal(hash(coreArchive), expected, 'official core archive digest mismatch');
  run('tar', ['-xf', coreArchive, '-C', directory]);
  const executable = path.join(directory, upstream, process.platform === 'win32' ? 'sing-box.exe' : 'sing-box');
  verifyBinary(executable, target);
  run('cargo', ['test', '--manifest-path', 'src-tauri/Cargo.toml', '--locked', '--test', 'real_core', '--', '--ignored'], { SING_SHELL_TEST_CORE: executable });
  report.officialCore = { version: coreVersion, archive, sha256: expected };
  report.verified.push('official sing-box: config check, start, SOCKS5 TCP roundtrip, stop and port release');
  report.passed = true;
} catch (error) {
  report.error = String(error.stack ?? error);
  throw error;
} finally {
  fs.mkdirSync('artifacts/verification', { recursive: true });
  fs.writeFileSync(`artifacts/verification/${target}.json`, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify(report, null, 2));
  // Preserve failed-run diagnostics; the disposable runner cleans its own temp files.
}
