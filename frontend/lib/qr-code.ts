/*
 * QR Code encoder (byte mode) for the in-site wallet checkout.
 *
 * Ported to TypeScript from Project Nayuki's "QR Code generator library"
 * (https://www.nayuki.io/page/qr-code-generator-library), reduced to byte-mode
 * segments and a plain boolean module matrix.
 *
 * Copyright (c) Project Nayuki. (MIT License)
 * https://www.nayuki.io/page/qr-code-generator-library
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy of
 * this software and associated documentation files (the "Software"), to deal in
 * the Software without restriction, including without limitation the rights to
 * use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
 * the Software, and to permit persons to whom the Software is furnished to do so,
 * subject to the following conditions:
 * - The above copyright notice and this permission notice shall be included in
 *   all copies or substantial portions of the Software.
 * - The Software is provided "as is", without warranty of any kind, express or
 *   implied, including but not limited to the warranties of merchantability,
 *   fitness for a particular purpose and noninfringement. In no event shall the
 *   authors or copyright holders be liable for any claim, damages or other
 *   liability, whether in an action of contract, tort or otherwise, arising from,
 *   out of or in connection with the Software or the use or other dealings in the
 *   Software.
 */

export type QrErrorCorrection = "L" | "M" | "Q" | "H";

export type QrCode = {
  /** 1–40. */
  version: number;
  /** Modules per side: 17 + 4 × version. */
  size: number;
  /** The mask pattern that was applied (0–7). */
  mask: number;
  /** `modules[y][x]` — true is a dark module. */
  modules: boolean[][];
};

export type QrEncodeOptions = {
  ecc?: QrErrorCorrection;
  minVersion?: number;
  maxVersion?: number;
  /** Force a mask (0–7); by default the lowest-penalty mask is chosen. */
  mask?: number;
};

export const QR_MIN_VERSION = 1;
export const QR_MAX_VERSION = 40;

const ECC_ORDINAL: Record<QrErrorCorrection, number> = { L: 0, M: 1, Q: 2, H: 3 };
const ECC_FORMAT_BITS: Record<QrErrorCorrection, number> = { L: 1, M: 0, Q: 3, H: 2 };

// Index 0 is padding; rows are L, M, Q, H.
const ECC_CODEWORDS_PER_BLOCK: readonly (readonly number[])[] = [
  [-1, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
  [-1, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28],
  [-1, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30, 30, 30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
  [-1, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
];

const NUM_ERROR_CORRECTION_BLOCKS: readonly (readonly number[])[] = [
  [-1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25],
  [-1, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49],
  [-1, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29, 34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68],
  [-1, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32, 35, 37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81],
];

const PENALTY_N1 = 3;
const PENALTY_N2 = 3;
const PENALTY_N3 = 40;
const PENALTY_N4 = 10;

function getBit(value: number, index: number): boolean {
  return ((value >>> index) & 1) !== 0;
}

/** Data and ECC modules available in a version, after function patterns. */
export function qrRawDataModules(version: number): number {
  let result = (16 * version + 128) * version + 64;
  if (version >= 2) {
    const numAlign = Math.floor(version / 7) + 2;
    result -= (25 * numAlign - 10) * numAlign - 55;
    if (version >= 7) result -= 36;
  }
  return result;
}

/** Number of 8-bit data codewords for a version and ECC level. */
export function qrDataCodewords(version: number, ecc: QrErrorCorrection): number {
  const o = ECC_ORDINAL[ecc];
  return Math.floor(qrRawDataModules(version) / 8)
    - ECC_CODEWORDS_PER_BLOCK[o][version] * NUM_ERROR_CORRECTION_BLOCKS[o][version];
}

function byteModeCountBits(version: number): number {
  return version < 10 ? 8 : 16;
}

/** Maximum byte-mode payload length (in bytes) for a version and ECC level. */
export function qrByteCapacity(version: number, ecc: QrErrorCorrection): number {
  const bits = qrDataCodewords(version, ecc) * 8 - 4 - byteModeCountBits(version);
  return Math.min(Math.floor(bits / 8), (1 << byteModeCountBits(version)) - 1);
}

// ---- Reed–Solomon over GF(2^8 / 0x11D) ----

function rsMultiply(x: number, y: number): number {
  let z = 0;
  for (let i = 7; i >= 0; i--) {
    z = (z << 1) ^ ((z >>> 7) * 0x11d);
    z ^= ((y >>> i) & 1) * x;
  }
  return z & 0xff;
}

function rsComputeDivisor(degree: number): number[] {
  const result: number[] = new Array(degree).fill(0);
  result[degree - 1] = 1;
  let root = 1;
  for (let i = 0; i < degree; i++) {
    for (let j = 0; j < result.length; j++) {
      result[j] = rsMultiply(result[j], root);
      if (j + 1 < result.length) result[j] ^= result[j + 1];
    }
    root = rsMultiply(root, 0x02);
  }
  return result;
}

function rsComputeRemainder(data: readonly number[], divisor: readonly number[]): number[] {
  const result: number[] = divisor.map(() => 0);
  for (const b of data) {
    const factor = b ^ (result.shift() as number);
    result.push(0);
    divisor.forEach((coef, i) => { result[i] ^= rsMultiply(coef, factor); });
  }
  return result;
}

// ---- Symbol construction ----

class QrBuilder {
  readonly size: number;
  readonly modules: boolean[][];
  private readonly isFunction: boolean[][];

  constructor(readonly version: number, readonly ecc: QrErrorCorrection) {
    this.size = version * 4 + 17;
    this.modules = Array.from({ length: this.size }, () => new Array<boolean>(this.size).fill(false));
    this.isFunction = Array.from({ length: this.size }, () => new Array<boolean>(this.size).fill(false));
  }

  private setFunctionModule(x: number, y: number, dark: boolean) {
    this.modules[y][x] = dark;
    this.isFunction[y][x] = true;
  }

  alignmentPatternPositions(): number[] {
    if (this.version === 1) return [];
    const numAlign = Math.floor(this.version / 7) + 2;
    const step = Math.floor((this.version * 8 + numAlign * 3 + 5) / (numAlign * 4 - 4)) * 2;
    const result = [6];
    for (let pos = this.size - 7; result.length < numAlign; pos -= step) result.splice(1, 0, pos);
    return result;
  }

  drawFunctionPatterns() {
    for (let i = 0; i < this.size; i++) {
      this.setFunctionModule(6, i, i % 2 === 0);
      this.setFunctionModule(i, 6, i % 2 === 0);
    }
    this.drawFinderPattern(3, 3);
    this.drawFinderPattern(this.size - 4, 3);
    this.drawFinderPattern(3, this.size - 4);
    const positions = this.alignmentPatternPositions();
    const last = positions.length - 1;
    for (let i = 0; i <= last; i++) {
      for (let j = 0; j <= last; j++) {
        if (!((i === 0 && j === 0) || (i === 0 && j === last) || (i === last && j === 0))) {
          this.drawAlignmentPattern(positions[i], positions[j]);
        }
      }
    }
    this.drawFormatBits(0);
    this.drawVersion();
  }

  drawFormatBits(mask: number) {
    const data = (ECC_FORMAT_BITS[this.ecc] << 3) | mask;
    let rem = data;
    for (let i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
    const bits = ((data << 10) | rem) ^ 0x5412;
    for (let i = 0; i <= 5; i++) this.setFunctionModule(8, i, getBit(bits, i));
    this.setFunctionModule(8, 7, getBit(bits, 6));
    this.setFunctionModule(8, 8, getBit(bits, 7));
    this.setFunctionModule(7, 8, getBit(bits, 8));
    for (let i = 9; i < 15; i++) this.setFunctionModule(14 - i, 8, getBit(bits, i));
    for (let i = 0; i < 8; i++) this.setFunctionModule(this.size - 1 - i, 8, getBit(bits, i));
    for (let i = 8; i < 15; i++) this.setFunctionModule(8, this.size - 15 + i, getBit(bits, i));
    this.setFunctionModule(8, this.size - 8, true); // The always-dark module.
  }

  private drawVersion() {
    if (this.version < 7) return;
    let rem = this.version;
    for (let i = 0; i < 12; i++) rem = (rem << 1) ^ ((rem >>> 11) * 0x1f25);
    const bits = (this.version << 12) | rem;
    for (let i = 0; i < 18; i++) {
      const dark = getBit(bits, i);
      const a = this.size - 11 + (i % 3);
      const b = Math.floor(i / 3);
      this.setFunctionModule(a, b, dark);
      this.setFunctionModule(b, a, dark);
    }
  }

  private drawFinderPattern(x: number, y: number) {
    for (let dy = -4; dy <= 4; dy++) {
      for (let dx = -4; dx <= 4; dx++) {
        const dist = Math.max(Math.abs(dx), Math.abs(dy));
        const xx = x + dx;
        const yy = y + dy;
        if (xx >= 0 && xx < this.size && yy >= 0 && yy < this.size) {
          this.setFunctionModule(xx, yy, dist !== 2 && dist !== 4);
        }
      }
    }
  }

  private drawAlignmentPattern(x: number, y: number) {
    for (let dy = -2; dy <= 2; dy++) {
      for (let dx = -2; dx <= 2; dx++) {
        this.setFunctionModule(x + dx, y + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
      }
    }
  }

  addEccAndInterleave(data: readonly number[]): number[] {
    const o = ECC_ORDINAL[this.ecc];
    const numBlocks = NUM_ERROR_CORRECTION_BLOCKS[o][this.version];
    const blockEccLen = ECC_CODEWORDS_PER_BLOCK[o][this.version];
    const rawCodewords = Math.floor(qrRawDataModules(this.version) / 8);
    const numShortBlocks = numBlocks - (rawCodewords % numBlocks);
    const shortBlockLen = Math.floor(rawCodewords / numBlocks);
    const divisor = rsComputeDivisor(blockEccLen);
    const blocks: number[][] = [];
    for (let i = 0, k = 0; i < numBlocks; i++) {
      const dat = data.slice(k, k + shortBlockLen - blockEccLen + (i < numShortBlocks ? 0 : 1));
      k += dat.length;
      const ecc = rsComputeRemainder(dat, divisor);
      if (i < numShortBlocks) dat.push(0);
      blocks.push(dat.concat(ecc));
    }
    const result: number[] = [];
    for (let i = 0; i < blocks[0].length; i++) {
      blocks.forEach((block, j) => {
        if (i !== shortBlockLen - blockEccLen || j >= numShortBlocks) result.push(block[i]);
      });
    }
    return result;
  }

  drawCodewords(data: readonly number[]) {
    let i = 0;
    for (let right = this.size - 1; right >= 1; right -= 2) {
      if (right === 6) right = 5;
      for (let vert = 0; vert < this.size; vert++) {
        for (let j = 0; j < 2; j++) {
          const x = right - j;
          const upward = ((right + 1) & 2) === 0;
          const y = upward ? this.size - 1 - vert : vert;
          if (!this.isFunction[y][x] && i < data.length * 8) {
            this.modules[y][x] = getBit(data[i >>> 3], 7 - (i & 7));
            i++;
          }
        }
      }
    }
  }

  applyMask(mask: number) {
    for (let y = 0; y < this.size; y++) {
      for (let x = 0; x < this.size; x++) {
        let invert: boolean;
        switch (mask) {
          case 0: invert = (x + y) % 2 === 0; break;
          case 1: invert = y % 2 === 0; break;
          case 2: invert = x % 3 === 0; break;
          case 3: invert = (x + y) % 3 === 0; break;
          case 4: invert = (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0; break;
          case 5: invert = ((x * y) % 2) + ((x * y) % 3) === 0; break;
          case 6: invert = (((x * y) % 2) + ((x * y) % 3)) % 2 === 0; break;
          case 7: invert = (((x + y) % 2) + ((x * y) % 3)) % 2 === 0; break;
          default: throw new RangeError("QR mask must be 0–7");
        }
        if (!this.isFunction[y][x] && invert) this.modules[y][x] = !this.modules[y][x];
      }
    }
  }

  penaltyScore(): number {
    let result = 0;
    const size = this.size;
    const m = this.modules;
    for (let y = 0; y < size; y++) {
      let runColor = false;
      let runX = 0;
      const runHistory = [0, 0, 0, 0, 0, 0, 0];
      for (let x = 0; x < size; x++) {
        if (m[y][x] === runColor) {
          runX++;
          if (runX === 5) result += PENALTY_N1;
          else if (runX > 5) result++;
        } else {
          this.finderPenaltyAddHistory(runX, runHistory);
          if (!runColor) result += this.finderPenaltyCountPatterns(runHistory) * PENALTY_N3;
          runColor = m[y][x];
          runX = 1;
        }
      }
      result += this.finderPenaltyTerminateAndCount(runColor, runX, runHistory) * PENALTY_N3;
    }
    for (let x = 0; x < size; x++) {
      let runColor = false;
      let runY = 0;
      const runHistory = [0, 0, 0, 0, 0, 0, 0];
      for (let y = 0; y < size; y++) {
        if (m[y][x] === runColor) {
          runY++;
          if (runY === 5) result += PENALTY_N1;
          else if (runY > 5) result++;
        } else {
          this.finderPenaltyAddHistory(runY, runHistory);
          if (!runColor) result += this.finderPenaltyCountPatterns(runHistory) * PENALTY_N3;
          runColor = m[y][x];
          runY = 1;
        }
      }
      result += this.finderPenaltyTerminateAndCount(runColor, runY, runHistory) * PENALTY_N3;
    }
    for (let y = 0; y < size - 1; y++) {
      for (let x = 0; x < size - 1; x++) {
        const color = m[y][x];
        if (color === m[y][x + 1] && color === m[y + 1][x] && color === m[y + 1][x + 1]) result += PENALTY_N2;
      }
    }
    let dark = 0;
    for (const row of m) dark = row.reduce((sum, color) => sum + (color ? 1 : 0), dark);
    const total = size * size;
    const k = Math.ceil(Math.abs(dark * 20 - total * 10) / total) - 1;
    result += k * PENALTY_N4;
    return result;
  }

  private finderPenaltyCountPatterns(runHistory: readonly number[]): number {
    const n = runHistory[1];
    const core = n > 0 && runHistory[2] === n && runHistory[3] === n * 3 && runHistory[4] === n && runHistory[5] === n;
    return (core && runHistory[0] >= n * 4 && runHistory[6] >= n ? 1 : 0)
      + (core && runHistory[6] >= n * 4 && runHistory[0] >= n ? 1 : 0);
  }

  private finderPenaltyTerminateAndCount(currentRunColor: boolean, currentRunLength: number, runHistory: number[]): number {
    let length = currentRunLength;
    if (currentRunColor) {
      this.finderPenaltyAddHistory(length, runHistory);
      length = 0;
    }
    length += this.size;
    this.finderPenaltyAddHistory(length, runHistory);
    return this.finderPenaltyCountPatterns(runHistory);
  }

  private finderPenaltyAddHistory(currentRunLength: number, runHistory: number[]) {
    let length = currentRunLength;
    if (runHistory[0] === 0) length += this.size;
    runHistory.pop();
    runHistory.unshift(length);
  }
}

/** Encode raw bytes as a byte-mode QR Code symbol. Throws if the data does not fit. */
export function encodeQrBytes(data: Uint8Array | readonly number[], options: QrEncodeOptions = {}): QrCode {
  const ecc = options.ecc ?? "M";
  const minVersion = options.minVersion ?? QR_MIN_VERSION;
  const maxVersion = options.maxVersion ?? QR_MAX_VERSION;
  if (!(QR_MIN_VERSION <= minVersion && minVersion <= maxVersion && maxVersion <= QR_MAX_VERSION)
    || !Number.isInteger(minVersion) || !Number.isInteger(maxVersion)) {
    throw new RangeError("Invalid QR version range");
  }
  const mask = options.mask ?? -1;
  if (mask < -1 || mask > 7 || !Number.isInteger(mask)) throw new RangeError("QR mask must be 0–7");
  const bytes = Array.from(data, (value) => value & 0xff);

  let version = minVersion;
  for (; ; version++) {
    if (bytes.length <= qrByteCapacity(version, ecc)) break;
    if (version >= maxVersion) {
      throw new RangeError(`QR data too long: ${bytes.length} bytes exceeds version ${maxVersion}-${ecc} capacity of ${qrByteCapacity(maxVersion, ecc)} bytes`);
    }
  }

  // Bit stream: mode indicator, character count, payload, terminator, padding.
  const bits: number[] = [];
  const append = (value: number, length: number) => {
    for (let i = length - 1; i >= 0; i--) bits.push((value >>> i) & 1);
  };
  append(0x4, 4);
  append(bytes.length, byteModeCountBits(version));
  for (const b of bytes) append(b, 8);
  const capacityBits = qrDataCodewords(version, ecc) * 8;
  append(0, Math.min(4, capacityBits - bits.length));
  append(0, (8 - (bits.length % 8)) % 8);
  for (let pad = 0xec; bits.length < capacityBits; pad ^= 0xec ^ 0x11) append(pad, 8);
  const codewords: number[] = new Array(bits.length / 8).fill(0);
  bits.forEach((bit, i) => { codewords[i >>> 3] |= bit << (7 - (i & 7)); });

  const builder = new QrBuilder(version, ecc);
  builder.drawFunctionPatterns();
  builder.drawCodewords(builder.addEccAndInterleave(codewords));
  let chosen = mask;
  if (chosen === -1) {
    let minPenalty = Infinity;
    for (let i = 0; i < 8; i++) {
      builder.applyMask(i);
      builder.drawFormatBits(i);
      const penalty = builder.penaltyScore();
      if (penalty < minPenalty) {
        chosen = i;
        minPenalty = penalty;
      }
      builder.applyMask(i); // XOR again to undo.
    }
  }
  builder.applyMask(chosen);
  builder.drawFormatBits(chosen);
  return { version, size: builder.size, mask: chosen, modules: builder.modules };
}

/** Encode a string (as UTF-8) as a byte-mode QR Code symbol. */
export function encodeQrText(text: string, options: QrEncodeOptions = {}): QrCode {
  return encodeQrBytes(new TextEncoder().encode(text), options);
}
