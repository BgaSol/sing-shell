import assert from 'node:assert/strict';
import fs from 'node:fs';

export function verifyBinary(file, target) {
  const bytes = fs.readFileSync(file);
  const arm = target.startsWith('aarch64-');
  if (target.endsWith('-pc-windows-msvc')) {
    assert.equal(bytes.toString('ascii', 0, 2), 'MZ', 'expected PE executable');
    const offset = bytes.readUInt32LE(0x3c);
    assert.equal(bytes.toString('ascii', offset, offset + 4), 'PE\0\0');
    assert.equal(bytes.readUInt16LE(offset + 4), arm ? 0xaa64 : 0x8664, 'wrong PE architecture');
  } else if (target.endsWith('-unknown-linux-gnu')) {
    assert.equal(bytes.toString('ascii', 0, 4), '\x7fELF', 'expected ELF executable');
    assert.equal(bytes[4], 2, 'expected ELF64');
    assert.equal(bytes[5], 1, 'expected little-endian ELF');
    assert.equal(bytes.readUInt16LE(18), arm ? 183 : 62, 'wrong ELF architecture');
  } else {
    assert.equal(target, 'aarch64-apple-darwin', 'unsupported native target');
    assert.equal(bytes.readUInt32LE(0), 0xfeedfacf, 'expected Mach-O64');
    assert.equal(bytes.readUInt32LE(4), 0x100000c, 'expected ARM64 Mach-O');
  }
}
