//! PWB1 envelope golden：布局、AAD、坏 header → Corrupted、已知向量往返。
//!
//! 向量参数（与 `tests/golden/pwb1_valid.hex` 锁定）：
//! - key = 0x11 × 32
//! - nonce = 0x00..0x17
//! - key_version = 1
//! - scope = (provider-golden, session-golden)
//! - logical ref = pblob_golden
//! - plaintext = `reasoning-secret-that-must-never-appear-on-disk`

#![cfg(feature = "protected")]

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use pawork_blob_store::{
    open_pwb1_envelope, parse_pwb1_envelope, pwb1_aad, AeadKey, BlobScope, ProtectedBlobRef,
    ProviderId, SessionId, PWB1_ALGORITHM, PWB1_HEADER_LEN, PWB1_MAGIC, PWB1_NONCE_LEN,
    PWB1_VERSION,
};

const GOLDEN_HEX: &str = include_str!("golden/pwb1_valid.hex");
const GOLDEN_KEY: [u8; 32] = [0x11; 32];
const GOLDEN_NONCE: [u8; 24] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
    0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
];
const GOLDEN_KEY_VERSION: u32 = 1;
const GOLDEN_PLAINTEXT: &[u8] = b"reasoning-secret-that-must-never-appear-on-disk";

fn decode_hex(text: &str) -> Vec<u8> {
    let hex: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(hex.len() % 2 == 0, "golden hex must have even length");
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex digit"))
        .collect()
}

fn golden_scope() -> BlobScope {
    BlobScope::new(
        ProviderId::from("provider-golden"),
        SessionId::from("session-golden"),
    )
}

fn golden_ref() -> ProtectedBlobRef {
    ProtectedBlobRef::from("pblob_golden")
}

fn push_len_prefixed(target: &mut Vec<u8>, value: &[u8]) {
    let length = u32::try_from(value.len()).expect("aad field fits u32");
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(value);
}

fn expected_aad() -> Vec<u8> {
    let mut value = b"pawork.protected-blob.v1\0".to_vec();
    push_len_prefixed(&mut value, b"provider-golden");
    push_len_prefixed(&mut value, b"session-golden");
    push_len_prefixed(&mut value, b"pblob_golden");
    value.extend_from_slice(&GOLDEN_KEY_VERSION.to_be_bytes());
    value
}

fn hand_seal() -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new_from_slice(&GOLDEN_KEY).expect("key");
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(GOLDEN_NONCE),
            Payload {
                msg: GOLDEN_PLAINTEXT,
                aad: &expected_aad(),
            },
        )
        .expect("encrypt");
    let mut envelope = Vec::with_capacity(PWB1_HEADER_LEN + ciphertext.len());
    envelope.extend_from_slice(PWB1_MAGIC);
    envelope.push(PWB1_VERSION);
    envelope.push(PWB1_ALGORITHM);
    envelope.extend_from_slice(&GOLDEN_KEY_VERSION.to_be_bytes());
    envelope.extend_from_slice(&GOLDEN_NONCE);
    envelope.extend_from_slice(&ciphertext);
    envelope
}

#[test]
fn pwb1_valid_hex_matches_known_key_nonce_frame() {
    let golden = decode_hex(GOLDEN_HEX);
    let constructed = hand_seal();
    assert_eq!(golden, constructed, "committed hex must match known-vector seal");
    assert_eq!(golden.len(), PWB1_HEADER_LEN + (golden.len() - PWB1_HEADER_LEN));
    assert_eq!(&golden[..4], PWB1_MAGIC);
    assert_eq!(golden[4], PWB1_VERSION);
    assert_eq!(golden[5], PWB1_ALGORITHM);
    assert_eq!(&golden[6..10], &GOLDEN_KEY_VERSION.to_be_bytes());
    assert_eq!(&golden[10..10 + PWB1_NONCE_LEN], &GOLDEN_NONCE);
    assert!(
        !golden
            .windows(GOLDEN_PLAINTEXT.len())
            .any(|window| window == GOLDEN_PLAINTEXT),
        "ciphertext window must not contain plaintext"
    );
}

#[test]
fn pwb1_valid_hex_opens_to_plaintext() {
    let golden = decode_hex(GOLDEN_HEX);
    let opened = open_pwb1_envelope(
        &golden,
        &golden_scope(),
        &golden_ref(),
        &AeadKey::new(GOLDEN_KEY),
    )
    .expect("open golden");
    assert_eq!(opened.expose(), GOLDEN_PLAINTEXT);

    let (version, nonce, _ciphertext) =
        parse_pwb1_envelope(&golden, &golden_ref()).expect("parse golden");
    assert_eq!(version, GOLDEN_KEY_VERSION);
    assert_eq!(nonce, &GOLDEN_NONCE);
}

#[test]
fn pwb1_aad_is_context_plus_len_prefixed_scope_and_key_version() {
    let aad = pwb1_aad(&golden_scope(), &golden_ref(), GOLDEN_KEY_VERSION);
    assert_eq!(aad, expected_aad());
    assert!(aad.starts_with(b"pawork.protected-blob.v1\0"));
}

#[test]
fn pwb1_bad_magic_version_alg_are_corrupted() {
    let golden = decode_hex(GOLDEN_HEX);
    let blob_ref = golden_ref();

    let mut bad_magic = golden.clone();
    bad_magic[0] = b'X';
    let error = parse_pwb1_envelope(&bad_magic, &blob_ref).expect_err("bad magic");
    assert!(error.is_corrupted());

    let mut bad_version = golden.clone();
    bad_version[4] = 2;
    let error = parse_pwb1_envelope(&bad_version, &blob_ref).expect_err("bad version");
    assert!(error.is_corrupted());

    let mut bad_alg = golden.clone();
    bad_alg[5] = 2;
    let error = parse_pwb1_envelope(&bad_alg, &blob_ref).expect_err("bad alg");
    assert!(error.is_corrupted());

    let error = open_pwb1_envelope(
        &bad_magic,
        &golden_scope(),
        &blob_ref,
        &AeadKey::new(GOLDEN_KEY),
    )
    .expect_err("open bad magic");
    assert!(error.is_corrupted());
}
