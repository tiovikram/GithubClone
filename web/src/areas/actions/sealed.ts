// libsodium `crypto_box_seal` in the browser, for Actions / environment / org secrets:
// ciphertext = ephemeral_pk || crypto_box(msg, nonce = blake2b(ephemeral_pk || recipient_pk, 24),
// recipient_pk, ephemeral_sk). X25519 + XSalsa20-Poly1305 come from tweetnacl (`nacl.box`, the
// same construction as libsodium's crypto_box_easy); BLAKE2b is implemented below.
import nacl from "tweetnacl";

const MASK = (1n << 64n) - 1n;
const IV = [
  0x6a09e667f3bcc908n, 0xbb67ae8584caa73bn, 0x3c6ef372fe94f82bn, 0xa54ff53a5f1d36f1n,
  0x510e527fade682d1n, 0x9b05688c2b3e6c1fn, 0x1f83d9abfb41bd6bn, 0x5be0cd19137e2179n,
];
const SIGMA = [
  [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
  [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
  [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
  [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
  [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
  [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
  [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
  [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
  [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
  [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

const rotr = (x: bigint, n: bigint) => ((x >> n) | (x << (64n - n))) & MASK;

function compress(h: bigint[], block: Uint8Array, t: bigint, last: boolean) {
  const m: bigint[] = [];
  const dv = new DataView(block.buffer, block.byteOffset, 128);
  for (let i = 0; i < 16; i++) m.push(dv.getBigUint64(i * 8, true));
  const v = [...h, ...IV];
  v[12] ^= t & MASK;
  v[13] ^= t >> 64n;
  if (last) v[14] ^= MASK;
  const g = (a: number, b: number, c: number, d: number, x: bigint, y: bigint) => {
    v[a] = (v[a] + v[b] + x) & MASK;
    v[d] = rotr(v[d] ^ v[a], 32n);
    v[c] = (v[c] + v[d]) & MASK;
    v[b] = rotr(v[b] ^ v[c], 24n);
    v[a] = (v[a] + v[b] + y) & MASK;
    v[d] = rotr(v[d] ^ v[a], 16n);
    v[c] = (v[c] + v[d]) & MASK;
    v[b] = rotr(v[b] ^ v[c], 63n);
  };
  for (let r = 0; r < 12; r++) {
    const s = SIGMA[r % 10];
    g(0, 4, 8, 12, m[s[0]], m[s[1]]);
    g(1, 5, 9, 13, m[s[2]], m[s[3]]);
    g(2, 6, 10, 14, m[s[4]], m[s[5]]);
    g(3, 7, 11, 15, m[s[6]], m[s[7]]);
    g(0, 5, 10, 15, m[s[8]], m[s[9]]);
    g(1, 6, 11, 12, m[s[10]], m[s[11]]);
    g(2, 7, 8, 13, m[s[12]], m[s[13]]);
    g(3, 4, 9, 14, m[s[14]], m[s[15]]);
  }
  for (let i = 0; i < 8; i++) h[i] ^= v[i] ^ v[i + 8];
}

/** Unkeyed BLAKE2b with an `outLen`-byte digest (1..64). */
export function blake2b(input: Uint8Array, outLen: number): Uint8Array {
  const h = [...IV];
  h[0] ^= 0x01010000n ^ BigInt(outLen);
  const blocks = Math.max(1, Math.ceil(input.length / 128));
  for (let i = 0; i < blocks; i++) {
    const block = new Uint8Array(128);
    block.set(input.subarray(i * 128, (i + 1) * 128));
    const last = i === blocks - 1;
    compress(h, block, BigInt(last ? input.length : (i + 1) * 128), last);
  }
  const out = new Uint8Array(64);
  const dv = new DataView(out.buffer);
  h.forEach((x, i) => dv.setBigUint64(i * 8, x, true));
  return out.slice(0, outLen);
}

function fromB64(s: string): Uint8Array {
  const bin = atob(s);
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

function toB64(b: Uint8Array): string {
  let s = "";
  for (const x of b) s += String.fromCharCode(x);
  return btoa(s);
}

/** Seal `value` (UTF-8) for the base64 X25519 public key; returns base64 ciphertext. */
export function sealSecret(publicKeyB64: string, value: string): string {
  const pk = fromB64(publicKeyB64);
  const eph = nacl.box.keyPair();
  const nonceIn = new Uint8Array(64);
  nonceIn.set(eph.publicKey, 0);
  nonceIn.set(pk, 32);
  const nonce = blake2b(nonceIn, 24);
  const boxed = nacl.box(new TextEncoder().encode(value), nonce, pk, eph.secretKey);
  const out = new Uint8Array(32 + boxed.length);
  out.set(eph.publicKey, 0);
  out.set(boxed, 32);
  return toB64(out);
}
