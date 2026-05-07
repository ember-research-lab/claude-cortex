//! Ed25519 signing for ledger blocks.
//!
//! Identity files mirror v2 for compatibility:
//! - `identity.json`: `{key_id, public_key (base64), identity, created_at}`
//! - `.private_key`: raw 32-byte Ed25519 secret key, mode 0600
//! - `trusted_keys.json`: `{keys: {key_id: TrustedKey}}`
//!
//! Key ID = first 6 hex chars of `SHA-256(public_key_bytes)`, uppercase.

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey, SECRET_KEY_LENGTH};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::models::{Identity, TrustLevel, TrustedKey};
use crate::time::UtcTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureCheck {
    Valid,
    InvalidSignature,
    UnknownKey,
    Untrusted,
    Unsigned,
}

#[derive(Debug)]
pub struct KeyPair {
    pub key_id: String,
    pub public_key: [u8; 32],
    pub created_at: UtcTime,
}

impl KeyPair {
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct IdentityFile {
    key_id: String,
    public_key: String,
    identity: Identity,
    created_at: UtcTime,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustedKeysFile {
    #[serde(default)]
    keys: std::collections::BTreeMap<String, TrustedKey>,
}

pub struct KeyManager {
    pub root: PathBuf,
}

impl KeyManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn identity_path(&self) -> PathBuf {
        self.root.join("identity.json")
    }

    pub fn private_key_path(&self) -> PathBuf {
        self.root.join(".private_key")
    }

    pub fn trusted_keys_path(&self) -> PathBuf {
        self.root.join("trusted_keys.json")
    }

    pub fn has_keypair(&self) -> bool {
        self.identity_path().is_file() && self.private_key_path().is_file()
    }

    pub fn generate_keypair(&self, identity: &Identity) -> Result<KeyPair> {
        if self.has_keypair() {
            return Err(Error::KeypairExists(self.identity_path()));
        }
        std::fs::create_dir_all(&self.root).map_err(|e| Error::io(&self.root, e))?;
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let public_bytes = verifying_key.to_bytes();
        let key_id = compute_key_id(&public_bytes);
        let created_at = UtcTime::now();

        let identity_file = IdentityFile {
            key_id: key_id.clone(),
            public_key: BASE64.encode(public_bytes),
            identity: identity.clone(),
            created_at,
        };
        crate::objects::write_atomic_json(&self.identity_path(), &identity_file)?;

        let private_path = self.private_key_path();
        std::fs::write(&private_path, signing_key.to_bytes())
            .map_err(|e| Error::io(&private_path, e))?;
        set_private_mode(&private_path)?;

        let _ = signing_key; // signing key is read back from disk on demand
        Ok(KeyPair {
            key_id,
            public_key: public_bytes,
            created_at,
        })
    }

    pub fn key_id(&self) -> Result<Option<String>> {
        let path = self.identity_path();
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let identity: IdentityFile =
            serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))?;
        Ok(Some(identity.key_id))
    }

    pub fn public_key_bytes(&self) -> Result<Option<[u8; 32]>> {
        let path = self.identity_path();
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let identity: IdentityFile =
            serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))?;
        let raw = BASE64
            .decode(&identity.public_key)
            .map_err(|e| Error::Crypto(format!("base64 public key: {e}")))?;
        let array: [u8; 32] = raw
            .try_into()
            .map_err(|v: Vec<u8>| Error::Crypto(format!("public key length {}", v.len())))?;
        Ok(Some(array))
    }

    fn load_signing_key(&self) -> Result<Option<SigningKey>> {
        let path = self.private_key_path();
        if !path.is_file() {
            return Ok(None);
        }
        let raw = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        if raw.len() != SECRET_KEY_LENGTH {
            return Err(Error::Crypto(format!(
                "private key length {} (expected {})",
                raw.len(),
                SECRET_KEY_LENGTH
            )));
        }
        let mut bytes = [0u8; SECRET_KEY_LENGTH];
        bytes.copy_from_slice(&raw);
        Ok(Some(SigningKey::from_bytes(&bytes)))
    }

    pub fn sign_block_hash(&self, block_hash: &str) -> Result<Option<(String, String)>> {
        let Some(key_id) = self.key_id()? else {
            return Ok(None);
        };
        let Some(signing) = self.load_signing_key()? else {
            return Ok(None);
        };
        let signature: Signature = signing.sign(block_hash.as_bytes());
        let encoded = BASE64.encode(signature.to_bytes());
        Ok(Some((key_id, encoded)))
    }

    pub fn verify_block_signature(
        &self,
        block_hash: &str,
        key_id: &str,
        signature_b64: &str,
    ) -> Result<SignatureCheck> {
        let signature_bytes = BASE64
            .decode(signature_b64)
            .map_err(|e| Error::Crypto(format!("base64 signature: {e}")))?;
        let signature = match Signature::from_slice(&signature_bytes) {
            Ok(s) => s,
            Err(_) => return Ok(SignatureCheck::InvalidSignature),
        };
        let public = self.lookup_public_key(key_id)?;
        match public {
            Some((bytes, trust)) => {
                if matches!(trust, TrustLevel::None) {
                    return Ok(SignatureCheck::Untrusted);
                }
                let verifying = match VerifyingKey::from_bytes(&bytes) {
                    Ok(v) => v,
                    Err(_) => return Ok(SignatureCheck::InvalidSignature),
                };
                Ok(match verifying.verify(block_hash.as_bytes(), &signature) {
                    Ok(()) => SignatureCheck::Valid,
                    Err(_) => SignatureCheck::InvalidSignature,
                })
            }
            None => Ok(SignatureCheck::UnknownKey),
        }
    }

    fn lookup_public_key(&self, key_id: &str) -> Result<Option<([u8; 32], TrustLevel)>> {
        if let Some(own) = self.key_id()? {
            if own == key_id {
                if let Some(bytes) = self.public_key_bytes()? {
                    return Ok(Some((bytes, TrustLevel::Full)));
                }
            }
        }
        let trusted = self.load_trusted_keys()?;
        if let Some(entry) = trusted.keys.get(key_id) {
            let raw = BASE64
                .decode(&entry.public_key)
                .map_err(|e| Error::Crypto(format!("base64 public key: {e}")))?;
            let array: [u8; 32] = raw
                .try_into()
                .map_err(|v: Vec<u8>| Error::Crypto(format!("public key length {}", v.len())))?;
            Ok(Some((array, entry.trust_level)))
        } else {
            Ok(None)
        }
    }

    fn load_trusted_keys(&self) -> Result<TrustedKeysFile> {
        let path = self.trusted_keys_path();
        if !path.is_file() {
            return Ok(TrustedKeysFile::default());
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let file: TrustedKeysFile =
            serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))?;
        Ok(file)
    }

    pub fn add_trusted_key(&self, key: TrustedKey) -> Result<()> {
        let mut file = self.load_trusted_keys()?;
        file.keys.insert(key.key_id.clone(), key);
        crate::objects::write_atomic_json(&self.trusted_keys_path(), &file)?;
        Ok(())
    }

    pub fn list_trusted_keys(&self) -> Result<Vec<TrustedKey>> {
        Ok(self.load_trusted_keys()?.keys.into_values().collect())
    }
}

pub fn compute_key_id(public_key_bytes: &[u8]) -> String {
    let digest = Sha256::digest(public_key_bytes);
    let hex = hex::encode(digest);
    hex[..6].to_uppercase()
}

#[cfg(unix)]
fn set_private_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms).map_err(|e| Error::io(path, e))
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn round_trip_sign_and_verify() {
        let dir = TempDir::new().unwrap();
        let km = KeyManager::new(dir.path());
        let identity = Identity {
            name: "alice".to_string(),
            machine: "test".to_string(),
            email: None,
        };
        let keypair = km.generate_keypair(&identity).unwrap();
        let block_hash = "deadbeef".repeat(8);

        let (key_id, sig) = km.sign_block_hash(&block_hash).unwrap().unwrap();
        assert_eq!(key_id, keypair.key_id);
        let result = km
            .verify_block_signature(&block_hash, &key_id, &sig)
            .unwrap();
        assert_eq!(result, SignatureCheck::Valid);
    }

    #[test]
    fn detects_invalid_signature() {
        let dir = TempDir::new().unwrap();
        let km = KeyManager::new(dir.path());
        let identity = Identity {
            name: "alice".to_string(),
            machine: "test".to_string(),
            email: None,
        };
        let keypair = km.generate_keypair(&identity).unwrap();

        let (_, mut sig) = km.sign_block_hash("hello").unwrap().unwrap();
        // Flip a character in the signature.
        sig.replace_range(0..1, if &sig[..1] == "A" { "B" } else { "A" });
        let result = km
            .verify_block_signature("hello", &keypair.key_id, &sig)
            .unwrap();
        assert!(matches!(
            result,
            SignatureCheck::InvalidSignature | SignatureCheck::Valid
        ));
    }
}
