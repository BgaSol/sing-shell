import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { verifyBinary } from './verify-binary.mjs';

test('native package verification rejects an x86_64 binary on an ARM64 target', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'sing-shell-binary-'));
  const file = path.join(directory, 'binary');
  try {
    const pe = Buffer.alloc(128);
    pe.write('MZ'); pe.writeUInt32LE(64, 0x3c); pe.write('PE\0\0', 64); pe.writeUInt16LE(0x8664, 68);
    fs.writeFileSync(file, pe);
    verifyBinary(file, 'x86_64-pc-windows-msvc');
    assert.throws(() => verifyBinary(file, 'aarch64-pc-windows-msvc'), /wrong PE architecture/);
    const elf = Buffer.alloc(64);
    elf.write('\x7fELF'); elf[4] = 2; elf[5] = 1; elf.writeUInt16LE(62, 18);
    fs.writeFileSync(file, elf);
    verifyBinary(file, 'x86_64-unknown-linux-gnu');
    assert.throws(() => verifyBinary(file, 'aarch64-unknown-linux-gnu'), /wrong ELF architecture/);
    const macho = Buffer.alloc(32);
    macho.writeUInt32LE(0xfeedfacf, 0); macho.writeUInt32LE(0x100000c, 4);
    fs.writeFileSync(file, macho);
    verifyBinary(file, 'aarch64-apple-darwin');
    macho.writeUInt32LE(0x1000007, 4);
    fs.writeFileSync(file, macho);
    assert.throws(() => verifyBinary(file, 'aarch64-apple-darwin'), /ARM64 Mach-O/);
  } finally {
    fs.unlinkSync(file);
    fs.rmdirSync(directory);
  }
});
