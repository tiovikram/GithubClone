// libsodium `crypto_box_seal` (what GitHub's secrets APIs expect), built from tweetnacl's
// X25519 + XSalsa20-Poly1305 box and a BLAKE2b-derived nonce: ephemeral_pk || box(m, nonce, pk, esk)
// with nonce = BLAKE2b-192(ephemeral_pk || pk).
import { blake2b } from "blakejs";
import nacl from "tweetnacl";

function b64decode(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function b64encode(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i++) s += String.fromCharCode(b[i]);
  return btoa(s);
}

/** Encrypt `plaintext` for the base64 `publicKey`; returns base64 for `encrypted_value`. */
export function sealSecret(publicKey: string, plaintext: string): string {
  const pk = b64decode(publicKey);
  if (pk.length !== nacl.box.publicKeyLength) throw new Error("Invalid public key");
  const eph = nacl.box.keyPair();
  const nonceInput = new Uint8Array(eph.publicKey.length + pk.length);
  nonceInput.set(eph.publicKey);
  nonceInput.set(pk, eph.publicKey.length);
  const nonce = blake2b(nonceInput, undefined, nacl.box.nonceLength);
  const boxed = nacl.box(new TextEncoder().encode(plaintext), nonce, pk, eph.secretKey);
  const out = new Uint8Array(eph.publicKey.length + boxed.length);
  out.set(eph.publicKey);
  out.set(boxed, eph.publicKey.length);
  return b64encode(out);
}
