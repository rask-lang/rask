// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Key management — struct.build/KM1-KM3.
//!
//! `rask keys generate` creates an Ed25519 keypair.
//! `rask keys list` shows key fingerprints.
//! `rask keys rotate` generates a new key signed by the old one.

use colored::Colorize;
use std::process;

use crate::output;

/// `rask keys generate` — generate a new Ed25519 signing key (KM1).
pub fn cmd_keys_generate() {
    use rask_resolve::signing;

    // Check if a key already exists
    if let Ok(existing) = signing::load_signing_key() {
        eprintln!("{}: signing key already exists: {}",
            output::error_label(), existing.fingerprint());
        eprintln!("  Use `rask keys rotate` to generate a new key.");
        process::exit(1);
    }

    let keypair = signing::KeyPair::generate();
    let fingerprint = keypair.fingerprint();

    match signing::save_signing_key(&keypair) {
        Ok(()) => {
            println!("  {} Ed25519 signing key", "Generated".green().bold());
            println!("  Fingerprint: {}", fingerprint);
            let path = signing::credentials_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "~/.rask/credentials".into());
            println!("  Stored in:   {}", path);
            println!();
            println!("  This key signs packages you publish.");
            println!("  Back it up — there's no recovery if lost.");
        }
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }
}

/// `rask keys list` — show key fingerprints (KM1).
pub fn cmd_keys_list() {
    use rask_resolve::signing;

    match signing::load_signing_key() {
        Ok(kp) => {
            let pubkey_hex = signing::hex_encode(kp.verifying_key().as_bytes());
            println!("  {} {}", "Fingerprint:".bold(), kp.fingerprint());
            println!("  {} {}", "Public key: ".bold(), pubkey_hex);
        }
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }
}

/// `rask keys rotate` — rotate signing key (KM2).
///
/// Generates a new key, signs it with the old key, and stores the
/// new key. The rotation record (old key's signature of new pubkey)
/// should be published to the registry so consumers can verify the
/// chain of trust.
pub fn cmd_keys_rotate() {
    use rask_resolve::signing;

    // Load old key
    let old_key = match signing::load_signing_key() {
        Ok(kp) => kp,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            eprintln!("  No existing key to rotate from.");
            process::exit(1);
        }
    };

    let old_fingerprint = old_key.fingerprint();

    // Generate new key
    let new_key = signing::KeyPair::generate();
    let new_fingerprint = new_key.fingerprint();

    // Sign rotation: old key signs new public key
    let rotation_sig = signing::sign_rotation(&old_key, &new_key);

    // Save new key (overwrites old)
    match signing::save_signing_key(&new_key) {
        Ok(()) => {
            println!("  {} Ed25519 signing key", "Rotated".green().bold());
            println!("  Old: {}", old_fingerprint);
            println!("  New: {}", new_fingerprint);
            println!();
            println!("  Rotation signature: {}", rotation_sig);
            println!("  Submit this to the registry to authorize the new key.");
        }
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }

    // Upload rotation record to registry
    let token = match load_auth_token_quiet() {
        Some(t) => t,
        None => {
            println!();
            println!("  {} no auth token found — rotation record not uploaded",
                "Warning:".yellow());
            println!("  Manually submit the rotation signature to the registry.");
            return;
        }
    };

    let reg_config = rask_resolve::registry::RegistryConfig::from_env();
    let old_pubkey = signing::hex_encode(old_key.verifying_key().as_bytes());
    let new_pubkey = signing::hex_encode(new_key.verifying_key().as_bytes());

    match reg_config.publish_key_rotation(&old_pubkey, &new_pubkey, &rotation_sig, &token) {
        Ok(()) => {
            println!("  {} rotation record to registry", "Uploaded".green().bold());
        }
        Err(e) => {
            println!();
            println!("  {} failed to upload rotation record: {}",
                "Warning:".yellow(), e);
            println!("  Manually submit the rotation signature to the registry.");
        }
    }
}

/// Try to load auth token without error messages.
fn load_auth_token_quiet() -> Option<String> {
    if let Ok(token) = std::env::var("RASK_REGISTRY_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }

    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(home).join(".rask").join("credentials");
    let content = std::fs::read_to_string(&path).ok()?;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if key == "token" && !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    None
}
