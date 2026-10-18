//! libsodium `crypto_box_seal` / `crypto_box_seal_open` (X25519 + XSalsa20-Poly1305), the
//! format GitHub clients use to encrypt secrets against a `.../public-key` endpoint.
//!
//! sealed = epk (32) || tag (16) || ciphertext, where the box is keyed with
//! HSalsa20(X25519(esk, pk), 0) and the nonce is BLAKE2b-192(epk || pk).
use blake2::digest::consts::U24;
use blake2::{Blake2b, Digest};
use curve25519_dalek::montgomery::MontgomeryPoint;
use poly1305::universal_hash::KeyInit;
use poly1305::Poly1305;
use salsa20::cipher::consts::U10;
use salsa20::cipher::{KeyIvInit, StreamCipher};
use salsa20::XSalsa20;

pub const SEAL_BYTES: usize = 32 + 16;

/// A fresh X25519 key pair (secret, public).
pub fn keypair() -> ([u8; 32], [u8; 32]) {
    let sk: [u8; 32] = rand::random();
    (sk, MontgomeryPoint::mul_base_clamped(sk).to_bytes())
}

fn nonce(epk: &[u8; 32], pk: &[u8; 32]) -> [u8; 24] {
    let mut h = Blake2b::<U24>::new();
    h.update(epk);
    h.update(pk);
    h.finalize().into()
}

/// crypto_box_beforenm: HSalsa20 of the X25519 shared secret.
fn shared_key(sk: &[u8; 32], pk: &[u8; 32]) -> [u8; 32] {
    let s = MontgomeryPoint(*pk).mul_clamped(*sk).to_bytes();
    salsa20::hsalsa::<U10>(&s.into(), &[0u8; 16].into()).into()
}

/// XSalsa20 keystream XOR of `32 zero bytes || data`: returns (poly1305 key, transformed data).
fn xsalsa(k: &[u8; 32], n: &[u8; 24], data: &[u8]) -> ([u8; 32], Vec<u8>) {
    let mut buf = vec![0u8; 32 + data.len()];
    buf[32..].copy_from_slice(data);
    XSalsa20::new(k.into(), n.into()).apply_keystream(&mut buf);
    let mut pk = [0u8; 32];
    pk.copy_from_slice(&buf[..32]);
    (pk, buf.split_off(32))
}

fn tag(poly_key: &[u8; 32], c: &[u8]) -> [u8; 16] {
    Poly1305::new(poly_key.into()).compute_unpadded(c).into()
}

/// Seals `m` for the holder of `pk` (anonymous sender).
pub fn seal(m: &[u8], pk: &[u8; 32]) -> Vec<u8> {
    let (esk, epk) = keypair();
    let n = nonce(&epk, pk);
    let k = shared_key(&esk, pk);
    let (pkey, c) = xsalsa(&k, &n, m);
    let mut out = Vec::with_capacity(SEAL_BYTES + m.len());
    out.extend_from_slice(&epk);
    out.extend_from_slice(&tag(&pkey, &c));
    out.extend_from_slice(&c);
    out
}

/// Opens a sealed box; `None` when it is malformed or was not sealed for this key pair.
pub fn open(sealed: &[u8], sk: &[u8; 32], pk: &[u8; 32]) -> Option<Vec<u8>> {
    if sealed.len() < SEAL_BYTES {
        return None;
    }
    let epk: [u8; 32] = sealed[..32].try_into().ok()?;
    let t: [u8; 16] = sealed[32..48].try_into().ok()?;
    let c = &sealed[48..];
    let n = nonce(&epk, pk);
    let k = shared_key(sk, &epk);
    // Recompute the Poly1305 key from the first keystream block, verify, then decrypt.
    let (pkey, m) = xsalsa(&k, &n, c);
    let expect = tag(&pkey, c);
    // constant-time compare
    let diff = expect.iter().zip(t.iter()).fold(0u8, |a, (x, y)| a | (x ^ y));
    (diff == 0).then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tamper() {
        let (sk, pk) = keypair();
        let sealed = seal(b"hunter2", &pk);
        assert_eq!(sealed.len(), SEAL_BYTES + 7);
        assert_eq!(open(&sealed, &sk, &pk).as_deref(), Some(&b"hunter2"[..]));
        let mut bad = sealed.clone();
        bad[50] ^= 1;
        assert!(open(&bad, &sk, &pk).is_none());
        let (sk2, pk2) = keypair();
        assert!(open(&sealed, &sk2, &pk2).is_none());
        assert!(open(&sealed[..40], &sk, &pk).is_none());
    }

    /// NaCl's crypto_box test vector (tests/box.c): alice -> bob, as a check that the box
    /// construction (X25519, HSalsa20, XSalsa20-Poly1305) matches libsodium byte for byte.
    #[test]
    fn nacl_box_vector() {
        let alicesk: [u8; 32] = hex::decode("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a").unwrap().try_into().unwrap();
        let bobpk: [u8; 32] = hex::decode("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f").unwrap().try_into().unwrap();
        let n: [u8; 24] = hex::decode("69696ee955b62b73cd62bda875fc73d68219e0036b7a0b37").unwrap().try_into().unwrap();
        let k = shared_key(&alicesk, &bobpk);
        assert_eq!(hex::encode(k), "1b27556473e985d462cd51197a9a46c76009549eac6474f206c4ee0844f68389");
        let m = hex::decode(concat!(
            "be075fc53c81f2d5cf141316ebeb0c7b5228c52a4c62cbd44b66849b64244ffc",
            "e5ecbaaf33bd751a1ac728d45e6c61296cdc3c01233561f41db66cce314adb31",
            "0e3be8250c46f06dceea3a7fa1348057e2f6556ad6b1318a024a838f21af1fde",
            "048977eb48f59ffd4924ca1c60902e52f0a089bc76897040e082f93776384864",
            "5e0705"
        ))
        .unwrap();
        let (pkey, c) = xsalsa(&k, &n, &m);
        assert_eq!(hex::encode(tag(&pkey, &c)), "f3ffc7703f9400e52a7dfb4b3d3305d9");
        assert_eq!(&hex::encode(&c)[..32], "8e993b9f48681273c29650ba32fc76ce");
    }
}
