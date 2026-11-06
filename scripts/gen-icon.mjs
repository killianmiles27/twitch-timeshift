// Generates assets/icon-512.png: flat violet rounded square, white clock
// ring + hands, red "live" dot. No external deps (node:zlib only).
// Output resolves relative to this script, not the caller's CWD.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import zlib from 'node:zlib';

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const S = 512;
const px = new Uint8Array(S * S * 4);
const set = (x, y, r, g, b, a) => {
  if (x < 0 || y < 0 || x >= S || y >= S) return;
  const i = (y * S + x) * 4;
  // simple alpha-over onto existing pixel
  const ia = a / 255, oa = px[i + 3] / 255, na = ia + oa * (1 - ia);
  if (na === 0) return;
  px[i] = Math.round((r * ia + px[i] * oa * (1 - ia)) / na);
  px[i + 1] = Math.round((g * ia + px[i + 1] * oa * (1 - ia)) / na);
  px[i + 2] = Math.round((b * ia + px[i + 2] * oa * (1 - ia)) / na);
  px[i + 3] = Math.round(na * 255);
};
const distSeg = (x, y, x1, y1, x2, y2) => {
  const dx = x2 - x1, dy = y2 - y1;
  const t = Math.max(0, Math.min(1, ((x - x1) * dx + (y - y1) * dy) / (dx * dx + dy * dy)));
  return Math.hypot(x - (x1 + t * dx), y - (y1 + t * dy));
};
// supersampled coverage for smooth edges
const SS = 3;
const cover = (fn) => {
  let n = 0;
  for (let sy = 0; sy < SS; sy++) for (let sx = 0; sx < SS; sx++) if (fn(sx, sy)) n++;
  return n / (SS * SS);
};

const R = 104; // corner radius
const insideRoundRect = (x, y) => {
  const cx = Math.max(R, Math.min(S - R, x)), cy = Math.max(R, Math.min(S - R, y));
  return Math.hypot(x - cx, y - cy) <= R || (x >= R && x < S - R) || (y >= R && y < S - R);
};
const C = 248, CR = 138, RING = 25; // clock center/radius/ring width
const dotA = Math.PI / 4, dotX = C + CR * Math.cos(dotA), dotY = C + CR * Math.sin(dotA);

for (let y = 0; y < S; y++) for (let x = 0; x < S; x++) {
  const bgA = cover((sx, sy) => insideRoundRect(x + (sx + .5) / SS, y + (sy + .5) / SS));
  if (bgA > 0) set(x, y, 124, 58, 237, Math.round(bgA * 255)); // flat violet #7C3AED
  const ringA = cover((sx, sy) => {
    const d = Math.hypot(x + (sx + .5) / SS - C, y + (sy + .5) / SS - C);
    return Math.abs(d - CR) <= RING / 2;
  });
  if (ringA > 0) set(x, y, 255, 255, 255, Math.round(ringA * 255));
  const handA = cover((sx, sy) => {
    const fx = x + (sx + .5) / SS, fy = y + (sy + .5) / SS;
    return distSeg(fx, fy, C, C, C - 62, C - 72) <= 13 || distSeg(fx, fy, C, C, C + 84, C - 34) <= 13;
  });
  if (handA > 0) set(x, y, 255, 255, 255, Math.round(handA * 255));
  const hubA = cover((sx, sy) => Math.hypot(x + (sx + .5) / SS - C, y + (sy + .5) / SS - C) <= 17);
  if (hubA > 0) set(x, y, 255, 255, 255, Math.round(hubA * 255));
  // red live dot with white rim, on the ring at 45deg
  const rimA = cover((sx, sy) => Math.hypot(x + (sx + .5) / SS - dotX, y + (sy + .5) / SS - dotY) <= 40);
  if (rimA > 0) set(x, y, 255, 255, 255, Math.round(rimA * 255));
  const dotAc = cover((sx, sy) => Math.hypot(x + (sx + .5) / SS - dotX, y + (sy + .5) / SS - dotY) <= 31);
  if (dotAc > 0) set(x, y, 235, 4, 0, Math.round(dotAc * 255));
}

// PNG encode (RGBA, filter 0)
const raw = Buffer.alloc(S * (S * 4 + 1));
for (let y = 0; y < S; y++) {
  raw[y * (S * 4 + 1)] = 0;
  Buffer.from(px.buffer, y * S * 4, S * 4).copy(raw, y * (S * 4 + 1) + 1);
}
const chunk = (type, data) => {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type), data]);
  const crcTable = chunk.t ??= Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
  let crc = 0xFFFFFFFF;
  for (const b of body) crc = crcTable[(crc ^ b) & 0xFF] ^ (crc >>> 8);
  const crcBuf = Buffer.alloc(4); crcBuf.writeUInt32BE((crc ^ 0xFFFFFFFF) >>> 0);
  return Buffer.concat([len, body, crcBuf]);
};
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0); ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; ihdr[9] = 6; // 8-bit RGBA
const png = Buffer.concat([
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  chunk('IHDR', ihdr),
  chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
]);
const outDir = path.join(projectRoot, 'assets');
fs.mkdirSync(outDir, { recursive: true });
const outFile = path.join(outDir, 'icon-512.png');
fs.writeFileSync(outFile, png);
console.log('wrote', outFile, png.length, 'bytes');
