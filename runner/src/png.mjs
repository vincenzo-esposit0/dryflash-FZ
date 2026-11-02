// SPDX-License-Identifier: GPL-3.0-or-later
// Reads a golden screen back as dark/light pixels, whatever made the PNG: this emulator at any scale, or
// an image editor. 8-bit grey, RGB and RGBA, not interlaced (PNG spec, 2nd edition, §9, §11.2.2).
import { inflateSync } from "node:zlib";

const W = 128;
const H = 64;

function paeth(a, b, c) {
  const p = a + b - c;
  const [pa, pb, pc] = [Math.abs(p - a), Math.abs(p - b), Math.abs(p - c)];
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
}

/** @returns {{width: number, height: number, channels: number, data: Uint8Array}} */
export function decodePng(file) {
  if (file.subarray(0, 8).toString("latin1") !== "\x89PNG\r\n\x1a\n") throw new Error("not a PNG");
  let at = 8;
  let header = null;
  const idat = [];
  while (at < file.length) {
    const n = file.readUInt32BE(at);
    const type = file.subarray(at + 4, at + 8).toString("latin1");
    const body = file.subarray(at + 8, at + 8 + n);
    if (type === "IHDR") {
      header = { width: body.readUInt32BE(0), height: body.readUInt32BE(4), depth: body[8], color: body[9], interlace: body[12] };
    }
    if (type === "IDAT") idat.push(body);
    at += 12 + n;
  }
  if (!header) throw new Error("a PNG with no header");
  const channels = { 0: 1, 2: 3, 6: 4 }[header.color];
  if (header.depth !== 8 || !channels || header.interlace) {
    throw new Error("only 8-bit grey, RGB and RGBA PNGs, not interlaced, can be goldens");
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = header.width * channels;
  const data = new Uint8Array(stride * header.height);
  for (let y = 0; y < header.height; y++) {
    const filter = raw[y * (stride + 1)];
    for (let x = 0; x < stride; x++) {
      const v = raw[y * (stride + 1) + 1 + x];
      const a = x >= channels ? data[y * stride + x - channels] : 0;
      const b = y > 0 ? data[(y - 1) * stride + x] : 0;
      const c = x >= channels && y > 0 ? data[(y - 1) * stride + x - channels] : 0;
      const predicted = [0, a, b, (a + b) >> 1, paeth(a, b, c)][filter];
      if (predicted === undefined) throw new Error(`PNG filter ${filter} does not exist`);
      data[y * stride + x] = (v + predicted) & 0xff;
    }
  }
  return { width: header.width, height: header.height, channels, data };
}

/** A golden as a frame: 128×64, one bit per pixel, 1 = dark, as `Emulator.frame`. */
export function pngToFrame(file) {
  const { width, height, channels, data } = decodePng(file);
  const scale = width / W;
  if (!Number.isInteger(scale) || height !== H * scale) throw new Error(`a golden must be 128×64 or a whole multiple; this is ${width}×${height}`);
  const stride = width * channels;
  const luma = (x, y) => {
    const i = y * stride + x * channels;
    return channels === 1 ? data[i] : 0.299 * data[i] + 0.587 * data[i + 1] + 0.114 * data[i + 2];
  };
  let [lo, hi] = [255, 0];
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      const l = luma(x * scale, y * scale);
      lo = Math.min(lo, l);
      hi = Math.max(hi, l);
    }
  }
  const frame = new Uint8Array(1024);
  // One shade all over is a blank screen, whichever colour its backlight has.
  if (hi - lo < 32) return frame;
  const middle = (lo + hi) / 2;
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      if (luma(x * scale, y * scale) < middle) frame[(y * W + x) >> 3] |= 0x80 >> ((y * W + x) & 7);
    }
  }
  return frame;
}
