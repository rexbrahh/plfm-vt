//! Secrets encryption and decryption helpers.
//!
//! Implements envelope encryption for secret material:
//! - Data key: random per secret version
//! - Master key: operator-managed, loaded from env or file
//!
//! Cipher: AES-256-GCM for both payload and key wrapping.

use std::fs;

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};
use thiserror::Error;

const DATA_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const WRAP_AAD: &[u8] = b"plfm-secrets-wrap-v1";

#[derive(Debug, Error)]
pub enum SecretsCryptoError {
    #[error("missing secrets master key (set PLFM_SECRETS_MASTER_KEY or PLFM_SECRETS_MASTER_KEY_FILE)")]
    MissingMasterKey,
    #[error("invalid secrets master key encoding")]
    InvalidMasterKey,
    #[error("secret encryption failed")]
    EncryptFailed,
    #[error("secret decryption failed")]
    DecryptFailed,
    #[error("unknown master key id: {0}")]
    UnknownMasterKey(String),
}

#[derive(Debug, Clone)]
pub struct MasterKey {
    pub id: String,
    key_bytes: [u8; DATA_KEY_BYTES],
}

#[derive(Debug, Clone)]
pub struct EncryptedSecret {
    pub cipher: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub master_key_id: String,
    pub wrapped_data_key: Vec<u8>,
    pub wrapped_data_key_nonce: Vec<u8>,
    pub plaintext_size_bytes: i32,
}

fn load_master_key_bytes() -> Result<[u8; DATA_KEY_BYTES], SecretsCryptoError> {
    let key_source = std::env::var("PLFM_SECRETS_MASTER_KEY")
        .or_else(|_| std::env::var("GHOST_SECRETS_MASTER_KEY"))
        .ok();

    if let Some(raw) = key_source {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .map_err(|_| SecretsCryptoError::InvalidMasterKey)?;
        return bytes
            .as_slice()
            .try_into()
            .map_err(|_| SecretsCryptoError::InvalidMasterKey);
    }

    let key_path = std::env::var("PLFM_SECRETS_MASTER_KEY_FILE")
        .or_else(|_| std::env::var("GHOST_SECRETS_MASTER_KEY_FILE"))
        .ok();

    if let Some(path) = key_path {
        let contents = fs::read_to_string(path).map_err(|_| SecretsCryptoError::InvalidMasterKey)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(contents.trim())
            .map_err(|_| SecretsCryptoError::InvalidMasterKey)?;
        return bytes
            .as_slice()
            .try_into()
            .map_err(|_| SecretsCryptoError::InvalidMasterKey);
    }

    Err(SecretsCryptoError::MissingMasterKey)
}

fn master_key_id_for_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)[..8].to_string()
}

fn load_master_key() -> Result<MasterKey, SecretsCryptoError> {
    let key_bytes = load_master_key_bytes()?;
    let key_id = std::env::var("PLFM_SECRETS_MASTER_KEY_ID")
        .or_else(|_| std::env::var("GHOST_SECRETS_MASTER_KEY_ID"))
        .ok()
        .unwrap_or_else(|| master_key_id_for_bytes(&key_bytes));

    Ok(MasterKey {
        id: key_id,
        key_bytes,
    })
}

pub fn encrypt(plaintext: &[u8], aad: &[u8]) -> Result<EncryptedSecret, SecretsCryptoError> {
    let master = load_master_key()?;

    let mut data_key = [0u8; DATA_KEY_BYTES];
    rand::rng().fill_bytes(&mut data_key);

    let mut nonce_bytes = [0u8; NONCE_BYTES];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let cipher =
        Aes256Gcm::new_from_slice(&data_key).map_err(|_| SecretsCryptoError::EncryptFailed)?;
    let ciphertext = cipher
        .encrypt(nonce, Payload { msg: plaintext, aad })
        .map_err(|_| SecretsCryptoError::EncryptFailed)?;

    let mut wrap_nonce_bytes = [0u8; NONCE_BYTES];
    rand::rng().fill_bytes(&mut wrap_nonce_bytes);
    let wrap_nonce = Nonce::from_slice(&wrap_nonce_bytes);
    let wrap_cipher = Aes256Gcm::new_from_slice(&master.key_bytes)
        .map_err(|_| SecretsCryptoError::EncryptFailed)?;
    let wrapped_data_key = wrap_cipher
        .encrypt(
            wrap_nonce,
            Payload {
                msg: &data_key,
                aad: WRAP_AAD,
            },
        )
        .map_err(|_| SecretsCryptoError::EncryptFailed)?;

    Ok(EncryptedSecret {
        cipher: "aes-256-gcm".to_string(),
        nonce: nonce_bytes.to_vec(),
        ciphertext,
        master_key_id: master.id,
        wrapped_data_key,
        wrapped_data_key_nonce: wrap_nonce_bytes.to_vec(),
        plaintext_size_bytes: plaintext.len() as i32,
    })
}

pub fn decrypt(
    master_key_id: &str,
    nonce: &[u8],
    ciphertext: &[u8],
    wrapped_data_key: &[u8],
    wrapped_data_key_nonce: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, SecretsCryptoError> {
    let master = load_master_key()?;
    if master.id != master_key_id {
        return Err(SecretsCryptoError::UnknownMasterKey(master_key_id.to_string()));
    }

    let wrap_nonce = Nonce::from_slice(wrapped_data_key_nonce);
    let wrap_cipher = Aes256Gcm::new_from_slice(&master.key_bytes)
        .map_err(|_| SecretsCryptoError::DecryptFailed)?;
    let data_key = wrap_cipher
        .decrypt(
            wrap_nonce,
            Payload {
                msg: wrapped_data_key,
                aad: WRAP_AAD,
            },
        )
        .map_err(|_| SecretsCryptoError::DecryptFailed)?;

    let nonce = Nonce::from_slice(nonce);
    let cipher =
        Aes256Gcm::new_from_slice(&data_key).map_err(|_| SecretsCryptoError::DecryptFailed)?;
    cipher
        .decrypt(nonce, Payload { msg: ciphertext, aad })
        .map_err(|_| SecretsCryptoError::DecryptFailed)
}
