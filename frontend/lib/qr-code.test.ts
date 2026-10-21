import assert from "node:assert/strict";
import test from "node:test";
import { encodeQrBytes, encodeQrText, qrByteCapacity, qrDataCodewords } from "./qr-code";

const TRON_ADDRESS = "TQ4ziQJ3ZBRuNLQgsHaVaT1pBq5Bzd7Uvk";

function bytes(length: number) {
  return Array.from({ length }, (_, i) => (i * 37 + 11) & 0xff);
}

test("symbol size is 17 + 4 × version", () => {
  for (const length of [1, 20, 60, 120, 200]) {
    const qr = encodeQrBytes(bytes(length));
    assert.equal(qr.size, 17 + 4 * qr.version);
    assert.equal(qr.modules.length, qr.size);
    for (const row of qr.modules) assert.equal(row.length, qr.size);
  }
});

test("byte-mode ECC M capacities match the standard table", () => {
  assert.equal(qrDataCodewords(1, "M"), 16);
  assert.deepEqual([1, 2, 3, 4, 5, 10].map((v) => qrByteCapacity(v, "M")), [14, 26, 42, 62, 84, 213]);
});

test("version is chosen by input length at ECC M boundaries", () => {
  assert.equal(encodeQrBytes(bytes(14)).version, 1);
  assert.equal(encodeQrBytes(bytes(15)).version, 2);
  assert.equal(encodeQrBytes(bytes(26)).version, 2);
  assert.equal(encodeQrBytes(bytes(27)).version, 3);
  assert.equal(encodeQrBytes(bytes(42)).version, 3);
  assert.equal(encodeQrBytes(bytes(43)).version, 4);
  // A 34-character TRON address fits version 3-M.
  assert.equal(encodeQrText(TRON_ADDRESS).version, 3);
});

test("finder patterns and separators are present in three corners", () => {
  const { modules, size } = encodeQrText(TRON_ADDRESS);
  const corners: [number, number][] = [[0, 0], [size - 7, 0], [0, size - 7]];
  for (const [x0, y0] of corners) {
    for (let dy = 0; dy < 7; dy++) {
      for (let dx = 0; dx < 7; dx++) {
        const ring = Math.max(Math.abs(dx - 3), Math.abs(dy - 3));
        assert.equal(modules[y0 + dy][x0 + dx], ring !== 2, `finder module (${x0 + dx}, ${y0 + dy})`);
      }
    }
  }
  for (let i = 0; i < 8; i++) {
    assert.equal(modules[7][i], false, "top-left horizontal separator");
    assert.equal(modules[i][7], false, "top-left vertical separator");
    assert.equal(modules[7][size - 1 - i], false, "top-right separator");
    assert.equal(modules[size - 8][i], false, "bottom-left separator");
  }
});

test("timing patterns alternate and the dark module is set", () => {
  for (const text of ["a", TRON_ADDRESS, "x".repeat(150)]) {
    const { modules, size } = encodeQrText(text);
    for (let i = 8; i < size - 8; i++) {
      assert.equal(modules[6][i], i % 2 === 0, `horizontal timing at ${i}`);
      assert.equal(modules[i][6], i % 2 === 0, `vertical timing at ${i}`);
    }
    assert.equal(modules[size - 8][8], true, "dark module at (x=8, y=size-8)");
  }
});

test("alignment pattern centre is present from version 2", () => {
  const qr = encodeQrBytes(bytes(20));
  assert.equal(qr.version, 2);
  // Version 2 has one alignment pattern centred at (18, 18).
  assert.equal(qr.modules[18][18], true);
  assert.equal(qr.modules[17][18], false);
  assert.equal(qr.modules[16][18], true);
});

test("format information copies agree", () => {
  const { modules, size } = encodeQrText(TRON_ADDRESS);
  const first: boolean[] = [];
  for (let i = 0; i <= 5; i++) first.push(modules[i][8]);
  first.push(modules[7][8], modules[8][8], modules[8][7]);
  for (let i = 9; i < 15; i++) first.push(modules[8][14 - i]);
  const second: boolean[] = [];
  for (let i = 0; i < 8; i++) second.push(modules[8][size - 1 - i]);
  for (let i = 8; i < 15; i++) second.push(modules[size - 15 + i][8]);
  assert.deepEqual(first, second);
});

test("encoding is deterministic and the chosen mask is reported", () => {
  const a = encodeQrText(TRON_ADDRESS);
  const b = encodeQrText(TRON_ADDRESS);
  assert.deepEqual(a, b);
  assert.ok(a.mask >= 0 && a.mask <= 7);
  const forced = encodeQrText(TRON_ADDRESS, { mask: (a.mask + 1) % 8 });
  assert.equal(forced.mask, (a.mask + 1) % 8);
  assert.notDeepEqual(forced.modules, a.modules);
});

test("UTF-8 text is encoded as bytes", () => {
  // 5 CJK characters are 15 UTF-8 bytes, which no longer fit version 1-M.
  assert.equal(encodeQrText("钱包充值啊").version, 2);
});

test("overlong input throws a range error", () => {
  assert.throws(() => encodeQrBytes(bytes(15), { maxVersion: 1 }), RangeError);
  assert.throws(() => encodeQrBytes(bytes(3000)), /too long/);
});
