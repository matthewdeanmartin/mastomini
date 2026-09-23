//! Direct-message encryption (spec/05 "Direct messages").
//!
//! Every member has an X25519 key pair. The secret half is sealed twice:
//! with a key derived from the password (PBKDF2), and once per signed-in
//! device with a key derived from that device's bearer token (HKDF; tokens
//! are 256-bit random, so no stretching is needed). The server stores only
//! sealed copies, so without a participant's password or a live token of
//! theirs a direct message cannot be read, not even by an admin or from a
//! copy of the flash.
//!
//! A message is encrypted once with a random content key
//! (ChaCha20-Poly1305); the content key is wrapped for each participant
//! with an X25519 exchange against a fresh ephemeral key.

use crate::auth::fill_random;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// A member's unlocked secret key, only ever held for one request.
pub type SecretKey = Zeroizing<[u8; 32]>;

/// Ciphertext with its nonce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub nonce: [u8; 12],
    pub ct: Vec<u8>,
}

fn random<const N: usize>() -> [u8; N] {
    let mut out = [0; N];
    fill_random(&mut out);
    out
}

fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Sealed {
    let nonce = random::<12>();
    let ct = ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("ChaCha20-Poly1305 encryption cannot fail for small inputs");
    Sealed { nonce, ct }
}

fn open(key: &[u8; 32], aad: &[u8], sealed: &Sealed) -> Option<Zeroizing<Vec<u8>>> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(
            Nonce::from_slice(&sealed.nonce),
            Payload {
                msg: &sealed.ct,
                aad,
            },
        )
        .ok()
        .map(Zeroizing::new)
}

fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0; 32]);
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, out.as_mut())
        .expect("32 bytes is a valid HKDF length");
    out
}

fn as_key(bytes: &[u8]) -> Option<SecretKey> {
    let array: [u8; 32] = bytes.try_into().ok()?;
    Some(Zeroizing::new(array))
}

/// A new key pair: (secret, public).
pub fn new_keypair() -> (SecretKey, [u8; 32]) {
    let secret = Zeroizing::new(random::<32>());
    let public = PublicKey::from(&StaticSecret::from(*secret)).to_bytes();
    (secret, public)
}

pub fn new_salt() -> [u8; 16] {
    random()
}

fn aad(purpose: &str, account_id: u64) -> Vec<u8> {
    let mut out = purpose.as_bytes().to_vec();
    out.extend_from_slice(&account_id.to_be_bytes());
    out
}

fn password_key(password: &str, salt: &[u8; 16], rounds: u32) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0; 32]);
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, rounds.max(1), out.as_mut());
    out
}

/// Seal a secret key under a password.
pub fn seal_with_password(
    secret: &SecretKey,
    password: &str,
    salt: &[u8; 16],
    rounds: u32,
    account_id: u64,
) -> Sealed {
    let key = password_key(password, salt, rounds);
    seal(&key, &aad("mastomini key v1", account_id), secret.as_ref())
}

pub fn open_with_password(
    sealed: &Sealed,
    password: &str,
    salt: &[u8; 16],
    rounds: u32,
    account_id: u64,
) -> Option<SecretKey> {
    let key = password_key(password, salt, rounds);
    as_key(&open(&key, &aad("mastomini key v1", account_id), sealed)?)
}

fn token_key(token: &str) -> Zeroizing<[u8; 32]> {
    // Different from sha256(token), which is what the token record stores.
    hkdf(token.as_bytes(), b"mastomini", b"token wrap v1")
}

/// Seal a secret key for one signed-in device.
pub fn seal_with_token(secret: &SecretKey, token: &str, account_id: u64) -> Sealed {
    seal(
        &token_key(token),
        &aad("mastomini token v1", account_id),
        secret.as_ref(),
    )
}

pub fn open_with_token(sealed: &Sealed, token: &str, account_id: u64) -> Option<SecretKey> {
    as_key(&open(
        &token_key(token),
        &aad("mastomini token v1", account_id),
        sealed,
    )?)
}

/// One encrypted direct message, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub ephemeral: [u8; 32],
    pub body: Sealed,
    /// (account id, content key sealed for that account).
    pub wraps: Vec<(u64, Sealed)>,
}

fn wrap_key(shared: &[u8; 32], ephemeral: &[u8; 32], recipient: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(ephemeral);
    salt[32..].copy_from_slice(recipient);
    hkdf(shared, &salt, b"mastomini dm wrap v1")
}

fn body_aad(status_id: u64) -> Vec<u8> {
    aad("mastomini dm v1", status_id)
}

/// Encrypt `plaintext` for each (account id, public key). The status id is
/// bound in, so an envelope can't be moved to another status.
pub fn encrypt(plaintext: &[u8], status_id: u64, recipients: &[(u64, [u8; 32])]) -> Envelope {
    let content_key = Zeroizing::new(random::<32>());
    let ephemeral = StaticSecret::from(random::<32>());
    let ephemeral_public = PublicKey::from(&ephemeral).to_bytes();
    let wraps = recipients
        .iter()
        .map(|(account_id, public)| {
            let shared = ephemeral.diffie_hellman(&PublicKey::from(*public));
            let key = wrap_key(shared.as_bytes(), &ephemeral_public, public);
            let sealed = seal(
                &key,
                &aad("mastomini wrap v1", *account_id),
                content_key.as_ref(),
            );
            (*account_id, sealed)
        })
        .collect();
    Envelope {
        ephemeral: ephemeral_public,
        body: seal(&content_key, &body_aad(status_id), plaintext),
        wraps,
    }
}

/// Decrypt as `account_id`, holding `secret`. `None` if they aren't a
/// participant or anything doesn't authenticate.
pub fn decrypt(
    envelope: &Envelope,
    status_id: u64,
    account_id: u64,
    secret: &SecretKey,
) -> Option<Zeroizing<Vec<u8>>> {
    let (_, sealed) = envelope.wraps.iter().find(|(id, _)| *id == account_id)?;
    let static_secret = StaticSecret::from(**secret);
    let public = PublicKey::from(&static_secret).to_bytes();
    let shared = static_secret.diffie_hellman(&PublicKey::from(envelope.ephemeral));
    let key = wrap_key(shared.as_bytes(), &envelope.ephemeral, &public);
    let content_key = as_key(&open(&key, &aad("mastomini wrap v1", account_id), sealed)?)?;
    open(&content_key, &body_aad(status_id), &envelope.body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_participants_decrypt() {
        let (alice, alice_pub) = new_keypair();
        let (bob, bob_pub) = new_keypair();
        let (carol, _) = new_keypair();
        let env = encrypt(b"secret", 7, &[(1, alice_pub), (2, bob_pub)]);
        assert_eq!(decrypt(&env, 7, 1, &alice).unwrap().as_slice(), b"secret");
        assert_eq!(decrypt(&env, 7, 2, &bob).unwrap().as_slice(), b"secret");
        assert!(decrypt(&env, 7, 3, &carol).is_none(), "not a participant");
        assert!(
            decrypt(&env, 7, 2, &carol).is_none(),
            "wrong key for bob's slot"
        );
        assert!(
            decrypt(&env, 8, 1, &alice).is_none(),
            "bound to the status id"
        );
        assert!(!env.body.ct.windows(6).any(|w| w == b"secret"));
    }

    #[test]
    fn password_and_token_seals() {
        let (secret, _) = new_keypair();
        let salt = new_salt();
        let sealed = seal_with_password(&secret, "pw", &salt, 10, 5);
        assert_eq!(
            *open_with_password(&sealed, "pw", &salt, 10, 5).unwrap(),
            *secret
        );
        assert!(open_with_password(&sealed, "nope", &salt, 10, 5).is_none());
        assert!(open_with_password(&sealed, "pw", &salt, 10, 6).is_none());
        let t = seal_with_token(&secret, "token-a", 5);
        assert_eq!(*open_with_token(&t, "token-a", 5).unwrap(), *secret);
        assert!(open_with_token(&t, "token-b", 5).is_none());
    }
}
