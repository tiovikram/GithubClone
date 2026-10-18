// libsodium `crypto_box_seal` (what GitHub's secrets APIs expect), built from tweetnacl's box and
// a BLAKE2b nonce: ephemeral X25519 key pair, nonce = blake2b(epk || pk, 24 bytes),
// output = epk || box(message, nonce, pk, esk).
import nacl from "tweetnacl";
import { blake2b } from "blakejs";

const fromB64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const toB64 = (b: Uint8Array) => btoa(String.fromCharCode(...b));

/** Seal `plaintext` for the base64 X25519 public key `keyB64`; returns base64. */
export function sealBase64(keyB64: string, plaintext: string): string {
  const pk = fromB64(keyB64);
  const eph = nacl.box.keyPair();
  const input = new Uint8Array(64);
  input.set(eph.publicKey, 0);
  input.set(pk, 32);
  const nonce = blake2b(input, undefined, 24);
  const boxed = nacl.box(new TextEncoder().encode(plaintext), nonce, pk, eph.secretKey);
  const out = new Uint8Array(32 + boxed.length);
  out.set(eph.publicKey, 0);
  out.set(boxed, 32);
  return toB64(out);
}
