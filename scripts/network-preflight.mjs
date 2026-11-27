import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import net from 'node:net';
import dgram from 'node:dgram';
import {createHash} from 'node:crypto';
import {spawn, execFileSync} from 'node:child_process';
import {echoPair} from './network-echo.mjs';

// Only run in disposable native runners: the routes cover two test addresses, never the default route.
assert.equal(process.env.SING_SHELL_NETWORK_PREFLIGHT, '1', 'explicit isolated network preflight required');
const target = process.argv[2];
assert.ok(target, 'native target required');
const version = process.env.CORE_VERSION??'1.14.4';
const platform = process.platform;
assert.equal(target.startsWith('aarch64-'), process.arch === 'arm64', 'native architecture required');
const suffix = platform === 'win32' ? `windows-${process.arch === 'arm64' ? 'arm64' : 'amd64'}.zip` : platform === 'darwin' ? 'darwin-arm64.tar.gz' : `linux-${process.arch === 'arm64' ? 'arm64' : 'amd64'}.tar.gz`;
const name = `sing-box-${version}-${suffix}`;
const directory = path.resolve('artifacts/network-preflight', target);
await fs.mkdir(directory, {recursive: true});
const report = {target, platform, arch: process.arch, version, passed: false, verified: [], skipped: ['interactive system authorization and installed application identity'], kernel: os.release()};
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
function command(executable, args) { return execFileSync(executable, args, {encoding: 'utf8', windowsHide: true, timeout: 30_000}).trim(); }
async function fetchChecked(url, api = false) {
  const headers = {'User-Agent': 'Sing-Shell-network-preflight'};
  if (api && process.env.GH_TOKEN) headers.Authorization = `Bearer ${process.env.GH_TOKEN}`;
  const response = await fetch(url, {headers, signal: AbortSignal.timeout(60_000)});
  assert.ok(response.ok, `${url}: ${response.status}`);
  return response;
}
let child, tcp, udp;
let output = '';
const iface = platform === 'darwin' ? 'utun223' : 'ss-preflight';
function interfacePresent() {
  try {
    if (platform === 'win32') return command('powershell', ['-NoProfile', '-Command', `@(Get-NetIPAddress -InterfaceAlias '${iface}' -ErrorAction SilentlyContinue).Count`]) !== '0';
    if (platform === 'darwin') return command('/sbin/ifconfig', [iface]).includes(iface);
    return JSON.parse(command('ip', ['-j', 'address', 'show', 'dev', iface])).length > 0;
  } catch { return false; }
}
async function tcpRoundtrip(host, port) {
  await new Promise((resolve, reject) => {
    const socket = net.connect({host, port});
    socket.setTimeout(8000, () => socket.destroy(Error('TCP test route timeout')));
    const chunks = [];
    socket.on('error', reject);
    socket.once('connect', () => socket.write('sing-shell-tun-probe'));
    socket.on('data', bytes => {
      chunks.push(bytes);
      if (Buffer.concat(chunks).toString() === 'sing-shell-tun-probe') { socket.end(); resolve(); }
    });
  });
}
async function udpRoundtrip(host, port) {
  const socket = dgram.createSocket(net.isIP(host) === 6 ? 'udp6' : 'udp4');
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(Error('UDP test route timeout')), 8000);
      socket.on('error', e => { clearTimeout(timer); reject(e); });
      socket.once('message', bytes => { clearTimeout(timer); try { assert.equal(bytes.toString(), 'sing-shell-tun-udp'); resolve(); } catch (e) { reject(e); } });
      socket.send('sing-shell-tun-udp', port, host, error => { if (error) { clearTimeout(timer); reject(error); } });
    });
  } finally { socket.close(); }
}
try {
  assert.equal(interfacePresent(), false, 'pre-existing test interface must not be changed');
  const release = await (await fetchChecked(`https://api.github.com/repos/SagerNet/sing-box/releases/tags/v${version}`, true)).json();
  const asset = release.assets.find(a => a.name === name);
  assert.ok(asset?.digest?.startsWith('sha256:'));
  const archive = path.join(directory, name);
  const bytes = Buffer.from(await (await fetchChecked(asset.browser_download_url)).arrayBuffer());
  assert.equal(`sha256:${createHash('sha256').update(bytes).digest('hex')}`, asset.digest);
  await fs.writeFile(archive, bytes);
  const entries = command('tar', ['-tf', archive]).split(/\r?\n/);
  assert.ok(entries.length < 128);
  for (const entry of entries) assert.ok(!path.isAbsolute(entry) && !entry.split(/[\\/]/).includes('..'), 'safe upstream archive path');
  command('tar', ['-xf', archive, '-C', directory]);
  const coreEntry = entries.find(p => p.endsWith(platform === 'win32' ? '/sing-box.exe' : '/sing-box'));
  assert.ok(coreEntry);
  const core = path.join(directory, coreEntry);
  report.archive = {name, digest: asset.digest};
  report.resources = entries.filter(p => /\.(dll|so|dylib)$/i.test(p));
  assert.ok(command(core, ['version']).includes(version));
  const echo = await echoPair();
  tcp = echo.tcp; udp = echo.udp;
  const port = echo.port;
  const addresses = ['198.51.100.1', 'fdfe:dbca:199::99'];
  const config = {
    log: {level: 'info'},
    inbounds: [{type: 'tun', tag: 'test-tun', interface_name: iface, address: ['172.29.255.1/30', 'fdfe:dbca:199::1/126'], auto_route: true, dns_mode: 'disabled', route_address: [`${addresses[0]}/32`, `${addresses[1]}/128`], stack: 'mixed', mtu: 1400}],
    outbounds: [{type: 'direct', tag: 'direct'}],
    route: {rules: [{ip_cidr: [`${addresses[0]}/32`, `${addresses[1]}/128`], action: 'route', outbound: 'direct', override_address: '127.0.0.1', override_port: port}], final: 'direct'},
  };
  const configFile = path.join(directory, 'tun-test.json');
  await fs.writeFile(configFile, JSON.stringify(config, null, 2));
  command(core, ['check', '-c', configFile]);
  report.verified.push('official archive identity, native executable and controlled TUN configuration');
  const env = {...process.env};
  for (const key of Object.keys(env)) if (/token|secret|password|private.key/i.test(key)) delete env[key];
  child = spawn(core, ['run', '-c', configFile], {cwd: path.dirname(core), env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe']});
  child.stdout.on('data', bytes => { output = (output + bytes).slice(-65536); });
  child.stderr.on('data', bytes => { output = (output + bytes).slice(-65536); });
  child.on('error', e => { output += String(e); });
  const deadline = Date.now() + 60_000;
  while (!interfacePresent()) {
    assert.ok(child.exitCode === null, `core exited before TUN became ready: ${output}`);
    assert.ok(Date.now() < deadline, `TUN interface timeout: ${output}`);
    await delay(200);
  }
  report.verified.push('actual privileged TUN interface created');
  for (const address of addresses) {
    await tcpRoundtrip(address, port);
    report.verified.push(`TCP ${net.isIP(address) === 6 ? 'IPv6' : 'IPv4'} through controlled TUN route`);
    await udpRoundtrip(address, port);
    report.verified.push(`UDP ${net.isIP(address) === 6 ? 'IPv6' : 'IPv4'} through controlled TUN route`);
  }
  const exited = new Promise(resolve => child.once('exit', resolve));
  child.kill('SIGTERM');
  await Promise.race([exited, delay(10_000).then(() => { throw Error('core did not stop'); })]);
  child = null;
  const cleanupDeadline = Date.now() + 10_000;
  while (interfacePresent()) {
    assert.ok(Date.now() < cleanupDeadline, 'TUN interface remained after stop');
    await delay(200);
  }
  report.verified.push('TUN interface released after actual process exit');
  report.passed = true;
} catch (error) {
  report.error = String(error);
  process.exitCode = 1;
} finally {
  if (child) { child.kill('SIGKILL'); await delay(1000); }
  if (tcp) await new Promise(resolve => tcp.close(resolve));
  if (udp) udp.close();
  report.log = output;
  await fs.writeFile(path.join(directory, 'report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report, null, 2));
}
