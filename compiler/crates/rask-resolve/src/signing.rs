// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Package signing — struct.build/SG1-SG7, KM1-KM3.
//!
//! Ed25519 signing with TOFU (Trust On First Use). Keys live in
//! `~/.rask/credentials`. First publish of a package records the
//! signing key; subsequent publishes must match or use explicit
//! rotation.

use std::path::{Path, PathBuf};

use ed25519_dalek::{
    Signature, Signer, SigningKey, Verifier, VerifyingKey,
    SECRET_KEY_LENGTH,
};
use rand::rngs::OsRng;
use sha2::{Sha256, Digest};

/// Hex-encoded Ed25519 public key fingerprint prefix.
/// Format: `ed25519:<first-32-hex-chars-of-sha256(pubkey)>`
const FINGERPRINT_PREFIX: &str = "ed25519:";

/// A signing keypair loaded from credentials.
#[derive(Debug)]
pub struct KeyPair {
    signing_key: SigningKey,
}

impl KeyPair {
    /// Generate a new random Ed25519 keypair.
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        KeyPair { signing_key }
    }

    /// Load from raw secret key bytes (32 bytes).
    pub fn from_secret_bytes(bytes: &[u8; SECRET_KEY_LENGTH]) -> Self {
        let signing_key = SigningKey::from_bytes(bytes);
        KeyPair { signing_key }
    }

    /// The raw 32-byte secret key.
    pub fn secret_bytes(&self) -> &[u8; SECRET_KEY_LENGTH] {
        self.signing_key.as_bytes()
    }

    /// The public verifying key.
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// Fingerprint of the public key: `ed25519:<hex>`.
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.verifying_key())
    }

    /// Sign a byte slice, returning hex-encoded signature.
    pub fn sign(&self, data: &[u8]) -> String {
        let sig = self.signing_key.sign(data);
        hex::encode(&sig.to_bytes())
    }
}

/// Compute a fingerprint from a verifying (public) key.
/// Format: `ed25519:<first 32 hex chars of SHA-256(pubkey bytes)>`
pub fn fingerprint_of(key: &VerifyingKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let hash = hasher.finalize();
    // First 16 bytes = 32 hex chars — enough to be collision-resistant
    // for TOFU fingerprint comparison.
    let hex_str: String = hash.iter()
        .take(16)
        .map(|b| format!("{:02x}", b))
        .collect();
    format!("{}{}", FINGERPRINT_PREFIX, hex_str)
}

/// Verify a signature against data and a public key fingerprint.
/// `signature_hex` is hex-encoded Ed25519 signature.
/// `pubkey_hex` is hex-encoded 32-byte public key (NOT the fingerprint).
pub fn verify_signature(
    data: &[u8],
    signature_hex: &str,
    pubkey_hex: &str,
) -> Result<(), SigningError> {
    let sig_bytes = hex::decode(signature_hex)
        .map_err(|_| SigningError::InvalidSignature("bad hex in signature".into()))?;

    let sig = Signature::from_slice(&sig_bytes)
        .map_err(|_| SigningError::InvalidSignature("malformed signature".into()))?;

    let key_bytes = hex::decode(pubkey_hex)
        .map_err(|_| SigningError::InvalidSignature("bad hex in public key".into()))?;

    if key_bytes.len() != 32 {
        return Err(SigningError::InvalidSignature(
            format!("public key is {} bytes, expected 32", key_bytes.len()),
        ));
    }

    let mut key_arr = [0u8; 32];
    key_arr.copy_from_slice(&key_bytes);

    let verifying_key = VerifyingKey::from_bytes(&key_arr)
        .map_err(|_| SigningError::InvalidSignature("invalid public key".into()))?;

    verifying_key.verify(data, &sig)
        .map_err(|_| SigningError::VerificationFailed)
}

/// Compute fingerprint from hex-encoded public key bytes.
pub fn fingerprint_from_pubkey_hex(pubkey_hex: &str) -> Result<String, SigningError> {
    let key_bytes = hex::decode(pubkey_hex)
        .map_err(|_| SigningError::InvalidSignature("bad hex in public key".into()))?;

    if key_bytes.len() != 32 {
        return Err(SigningError::InvalidSignature(
            format!("public key is {} bytes, expected 32", key_bytes.len()),
        ));
    }

    let mut key_arr = [0u8; 32];
    key_arr.copy_from_slice(&key_bytes);

    let verifying_key = VerifyingKey::from_bytes(&key_arr)
        .map_err(|_| SigningError::InvalidSignature("invalid public key".into()))?;

    Ok(fingerprint_of(&verifying_key))
}

// --- Credentials file management (KM1) ---

/// Path to the credentials file: `~/.rask/credentials`.
pub fn credentials_path() -> Result<PathBuf, SigningError> {
    let home = std::env::var("HOME")
        .map_err(|_| SigningError::NoHome)?;
    Ok(PathBuf::from(home).join(".rask").join("credentials"))
}

/// Load the signing keypair from `~/.rask/credentials`.
///
/// Credentials format (one key=value per line):
/// ```text
/// token = "registry-api-token"
/// signing-key = "hex-encoded-32-byte-secret"
/// ```
pub fn load_signing_key() -> Result<KeyPair, SigningError> {
    let path = credentials_path()?;
    load_signing_key_from(&path)
}

/// Load from an explicit path (for testing).
pub fn load_signing_key_from(path: &Path) -> Result<KeyPair, SigningError> {
    let content = std::fs::read_to_string(path)
        .map_err(|_| SigningError::NoCredentials(path.display().to_string()))?;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if key == "signing-key" && !value.is_empty() {
                let bytes = hex::decode(value)
                    .map_err(|_| SigningError::InvalidKeyFormat)?;
                if bytes.len() != SECRET_KEY_LENGTH {
                    return Err(SigningError::InvalidKeyFormat);
                }
                let mut arr = [0u8; SECRET_KEY_LENGTH];
                arr.copy_from_slice(&bytes);
                return Ok(KeyPair::from_secret_bytes(&arr));
            }
        }
    }

    Err(SigningError::NoSigningKey(path.display().to_string()))
}

/// Save a signing key to the credentials file.
/// Preserves existing content (token, comments). Overwrites
/// any existing `signing-key` line.
pub fn save_signing_key(keypair: &KeyPair) -> Result<(), SigningError> {
    let path = credentials_path()?;
    save_signing_key_to(keypair, &path)
}

/// Save to an explicit path (for testing).
pub fn save_signing_key_to(keypair: &KeyPair, path: &Path) -> Result<(), SigningError> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SigningError::Io(e.to_string()))?;
    }

    let hex_secret = hex::encode(keypair.secret_bytes());
    let new_line = format!("signing-key = \"{}\"", hex_secret);

    // Read existing content, replace or append
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some((key, _)) = trimmed.split_once('=') {
            if key.trim() == "signing-key" {
                lines.push(new_line.clone());
                replaced = true;
                continue;
            }
        }
        lines.push(line.to_string());
    }

    if !replaced {
        if !lines.is_empty() && !lines.last().map(|l| l.is_empty()).unwrap_or(true) {
            lines.push(String::new());
        }
        lines.push(new_line);
    }

    let output = lines.join("\n") + "\n";
    std::fs::write(path, &output)
        .map_err(|e| SigningError::Io(e.to_string()))?;

    // Restrict permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }

    Ok(())
}

/// Sign a rotation record: new key signed by old key.
/// Returns the hex-encoded signature of the new public key bytes
/// by the old signing key.
pub fn sign_rotation(old_key: &KeyPair, new_key: &KeyPair) -> String {
    let new_pubkey_bytes = new_key.verifying_key().as_bytes().to_vec();
    old_key.sign(&new_pubkey_bytes)
}

// --- Hex encoding (tiny, no extra dep) ---

/// Hex-encode bytes. Public for use in publish command.
pub fn hex_encode(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, ()> {
        if s.len() % 2 != 0 {
            return Err(());
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
            .collect()
    }
}

// --- Errors ---

#[derive(Debug)]
pub enum SigningError {
    NoHome,
    NoCredentials(String),
    NoSigningKey(String),
    InvalidKeyFormat,
    InvalidSignature(String),
    VerificationFailed,
    Io(String),
}

impl std::fmt::Display for SigningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SigningError::NoHome => write!(f, "cannot determine home directory"),
            SigningError::NoCredentials(path) => write!(
                f, "no credentials file found at {}\n  run `rask keys generate` to create a signing key", path
            ),
            SigningError::NoSigningKey(path) => write!(
                f, "no signing key in {}\n  run `rask keys generate` to create one", path
            ),
            SigningError::InvalidKeyFormat => write!(
                f, "signing key in credentials file is malformed (expected 64 hex chars)"
            ),
            SigningError::InvalidSignature(msg) => write!(f, "invalid signature: {}", msg),
            SigningError::VerificationFailed => write!(
                f, "signature verification failed — signature does not match known key"
            ),
            SigningError::Io(msg) => write!(f, "I/O error: {}", msg),
        }
    }
}

impl std::error::Error for SigningError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_sign_verify() {
        let kp = KeyPair::generate();
        let data = b"hello world";
        let sig = kp.sign(data);
        let pubkey_hex = hex::encode(kp.verifying_key().as_bytes());

        assert!(verify_signature(data, &sig, &pubkey_hex).is_ok());
        assert!(verify_signature(b"wrong data", &sig, &pubkey_hex).is_err());
    }

    #[test]
    fn fingerprint_deterministic() {
        let kp = KeyPair::generate();
        let fp1 = kp.fingerprint();
        let fp2 = kp.fingerprint();
        assert_eq!(fp1, fp2);
        assert!(fp1.starts_with("ed25519:"));
        assert_eq!(fp1.len(), "ed25519:".len() + 32); // 16 bytes = 32 hex chars
    }

    #[test]
    fn fingerprint_from_hex() {
        let kp = KeyPair::generate();
        let pubkey_hex = hex::encode(kp.verifying_key().as_bytes());
        let fp = fingerprint_from_pubkey_hex(&pubkey_hex).unwrap();
        assert_eq!(fp, kp.fingerprint());
    }

    #[test]
    fn roundtrip_credentials() {
        let dir = std::env::temp_dir().join("rask-signing-test");
        let _ = std::fs::create_dir_all(&dir);
        let cred_path = dir.join("credentials");

        // Start with a token
        std::fs::write(&cred_path, "token = \"my-token\"\n").unwrap();

        let kp = KeyPair::generate();
        save_signing_key_to(&kp, &cred_path).unwrap();

        let loaded = load_signing_key_from(&cred_path).unwrap();
        assert_eq!(loaded.fingerprint(), kp.fingerprint());

        // Token should still be there
        let content = std::fs::read_to_string(&cred_path).unwrap();
        assert!(content.contains("token = \"my-token\""));
        assert!(content.contains("signing-key = "));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_signature() {
        let old = KeyPair::generate();
        let new = KeyPair::generate();
        let rotation_sig = sign_rotation(&old, &new);

        // Verify rotation: old key signed new pubkey bytes
        let old_pubkey_hex = hex::encode(old.verifying_key().as_bytes());
        let new_pubkey_bytes = new.verifying_key().as_bytes().to_vec();
        assert!(verify_signature(&new_pubkey_bytes, &rotation_sig, &old_pubkey_hex).is_ok());
    }

    #[test]
    fn hex_roundtrip() {
        let data = [0u8, 1, 255, 128, 64];
        let encoded = hex::encode(&data);
        assert_eq!(encoded, "0001ff8040");
        let decoded = hex::decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }
}
