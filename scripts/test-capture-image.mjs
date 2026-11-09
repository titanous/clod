#!/usr/bin/env node
// Evaluate only the pure PNG helper: do not import the SDK capture driver.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { deflateSync, inflateSync } from 'node:zlib';
const source = fs.readFileSync(new URL('./capture.mjs', import.meta.url), 'utf8');
const helper = source.slice(source.indexOf('function image64()'), source.indexOf('\nconst root ='));
assert(helper.startsWith('function image64()'));
const png = Buffer.from(vm.runInNewContext(helper + '\nimage64()', { Buffer, deflateSync }), 'base64');
assert.equal(png.subarray(0, 8).toString('hex'), '89504e470d0a1a0a');
const crc32 = data => {
  let crc = 0xffffffff;
  for (const byte of data) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
};
let offset = 8;
const chunks = [];
while (offset < png.length) {
  const size = png.readUInt32BE(offset);
  const type = png.subarray(offset + 4, offset + 8).toString();
  const data = png.subarray(offset + 8, offset + 8 + size);
  assert.equal(crc32(png.subarray(offset + 4, offset + 8 + size)), png.readUInt32BE(offset + 8 + size));
  chunks.push({ type, data });
  offset += 12 + size;
}
assert.equal(offset, png.length);
assert.deepEqual(chunks.map(c => c.type), ['IHDR', 'IDAT', 'IEND']);
assert.equal(chunks[0].data.readUInt32BE(0), 64);
assert.equal(chunks[0].data.readUInt32BE(4), 64);
assert.equal(chunks[0].data[8], 8);
assert.equal(chunks[0].data[9], 2);
const pixels = inflateSync(chunks[1].data);
assert.equal(pixels.length, 64 * 193);
for (let y = 0; y < 64; y++) {
  assert.equal(pixels[y * 193], 0);
  for (let x = 0; x < 64; x++) assert.deepEqual([...pixels.subarray(y * 193 + 1 + x * 3, y * 193 + 4 + x * 3)], x < 32 ? [255, 0, 0] : [0, 0, 255]);
}
assert(source.includes('data:image64()'));
console.log('PASS capture PNG dimensions, CRCs, decompression, and red/blue pixels (no SDK import)');
