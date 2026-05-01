//! Cryptographic utilities for encrypting screenshots at rest.
//!
//! Uses AES-256-GCM for authenticated encryption and Windows DPAPI for
//! master key protection. The master key is tied to the current user
//! account — no other user can decrypt it without the user's password.
//!
//! Encrypted file format:
//! ```text
//! [4 bytes]  Magic: "SAW1"
//! [1 byte]   Format: 0=WebP, 1=JPEG, 2=PNG
//! [12 bytes] Nonce: random per-file
//! [N bytes]  Ciphertext: AES-256-GCM encrypted image data
//! [16 bytes] Tag: GCM authentication tag
//! ```

use anyhow::Result;
use aes_gcm::aead::AeadInPlace;
use aes_gcm::{KeyInit, Key, Nonce, Aes256Gcm};
use rand::RngCore;
use std::path::PathBuf;
use sha2::{Sha256, Digest};

use crate::config::{app_dir, ImageFormat};

/// Magic bytes identifying an encrypted self-awareness file.
const MAGIC: [u8; 4] = *b"SAW1";

/// Offset of the format byte in the file header.
const FORMAT_OFFSET: usize = 4;
/// Offset where the nonce begins.
const NONCE_OFFSET: usize = 5;
/// Nonce length for AES-GCM (96 bits).
const NONCE_LEN: usize = 12;
/// Chain hash length.
const CHAIN_HASH_LEN: usize = 32;
/// Format byte flag indicating the presence of a hash chain.
const FLAG_HASH_CHAIN: u8 = 0x80;

/// Get the expected header length based on the format byte.
fn header_len(format_byte: u8) -> usize {
    if (format_byte & FLAG_HASH_CHAIN) != 0 {
        NONCE_OFFSET + NONCE_LEN + CHAIN_HASH_LEN
    } else {
        NONCE_OFFSET + NONCE_LEN
    }
}

/// Format byte constants.
const FORMAT_WEBP: u8 = 0;
const FORMAT_JPEG: u8 = 1;
const FORMAT_PNG: u8 = 2;

/// Path to the DPAPI-protected master key file.
fn key_path() -> PathBuf {
    app_dir().join("key.bin")
}

// ---------------------------------------------------------------------------
// Master key management (DPAPI)
// ---------------------------------------------------------------------------

/// Load the master key from disk, decrypting it with DPAPI.
/// If the key file does not exist, generates a new one.
pub fn load_key() -> Result<Vec<u8>> {
    let path = key_path();
    if path.exists() {
        let protected = std::fs::read(&path)?;
        Ok(decrypt_dpapi(&protected)?)
    } else {
        let key = generate_key();
        save_key(&key)?;
        Ok(key)
    }
}

/// Generate a random 256-bit (32-byte) master key.
fn generate_key() -> Vec<u8> {
    let mut key = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

/// Encrypt the master key with DPAPI and write it to disk.
fn save_key(key: &[u8]) -> Result<()> {
    let path = key_path();
    let protected = encrypt_dpapi(key, None)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, protected)?;
    Ok(())
}

/// Encrypt data with Windows DPAPI (user scope).
#[cfg(target_os = "windows")]
fn encrypt_dpapi(data: &[u8], _description: Option<&str>) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };

    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };

    unsafe {
        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;

        let result = (0..output.cbData)
            .map(|i| *output.pbData.add(i as usize))
            .collect::<Vec<u8>>();

        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(output.pbData as *mut _));
        Ok(result)
    }
}

/// Decrypt data protected by Windows DPAPI (user scope).
#[cfg(target_os = "windows")]
fn decrypt_dpapi(data: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };

    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };

    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            0,
            &mut output,
        )?;

        let result = (0..output.cbData)
            .map(|i| *output.pbData.add(i as usize))
            .collect::<Vec<u8>>();

        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(output.pbData as *mut _));
        Ok(result)
    }
}

// Stub implementations for non-Windows platforms (won't be used).
#[cfg(not(target_os = "windows"))]
fn encrypt_dpapi(_data: &[u8], _description: Option<&str>) -> Result<Vec<u8>> {
    anyhow::bail!("DPAPI is not available on this platform")
}

#[cfg(not(target_os = "windows"))]
fn decrypt_dpapi(_data: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("DPAPI is not available on this platform")
}

// ---------------------------------------------------------------------------
// AES-256-GCM encryption / decryption
// ---------------------------------------------------------------------------

/// Encrypt plaintext with AES-256-GCM using the provided key.
/// Returns (nonce, ciphertext_with_tag).
fn encrypt_aes(key: &[u8], plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    let nonce = Nonce::from_slice(&nonce);

    let mut buffer = plaintext.to_vec();
    cipher.encrypt_in_place(nonce, b"", &mut buffer).map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok((nonce.to_vec(), buffer))
}

/// Decrypt ciphertext with AES-256-GCM using the provided key and nonce.
fn decrypt_aes(key: &[u8], nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

    let nonce = Nonce::from_slice(nonce);
    let mut buffer = ciphertext.to_vec();
    cipher.decrypt_in_place(nonce, b"", &mut buffer).map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(buffer)
}

// ---------------------------------------------------------------------------
// Public API — encrypt / decrypt full image files
// ---------------------------------------------------------------------------

/// Encrypt raw image bytes and return the complete file contents
/// (header + ciphertext + tag).
///
/// The format byte records which image codec was used so the viewer
/// knows how to decode the plaintext.
pub fn encrypt_image(
    key: &[u8],
    plaintext: &[u8],
    format: ImageFormat,
    hash_chain_info: Option<(&[u8; 32], i64)>,
) -> Result<(Vec<u8>, Option<[u8; 32]>)> {
    let (nonce, ciphertext) = encrypt_aes(key, plaintext)?;

    let mut format_byte = match format {
        ImageFormat::Webp => FORMAT_WEBP,
        ImageFormat::Jpeg => FORMAT_JPEG,
        ImageFormat::Png => FORMAT_PNG,
    };

    let mut new_chain_hash = None;

    if let Some((prev_hash, timestamp_ms)) = hash_chain_info {
        format_byte |= FLAG_HASH_CHAIN;
        let current_file_hash = Sha256::digest(&ciphertext);
        let mut hasher = Sha256::new();
        hasher.update(prev_hash);
        hasher.update(current_file_hash);
        hasher.update(timestamp_ms.to_le_bytes());
        let result: [u8; 32] = hasher.finalize().into();
        new_chain_hash = Some(result);
    }

    let header_size = header_len(format_byte);

    let mut out = Vec::with_capacity(header_size + ciphertext.len());
    out.extend_from_slice(&MAGIC);
    out.push(format_byte);
    out.extend_from_slice(&nonce);
    if let Some(h) = new_chain_hash {
        out.extend_from_slice(&h);
    }
    out.extend_from_slice(&ciphertext);
    Ok((out, new_chain_hash))
}

/// Decrypt an encrypted file and return (plaintext_bytes, ImageFormat, chain_hash).
pub fn decrypt_image(key: &[u8], data: &[u8]) -> Result<(Vec<u8>, ImageFormat, Option<[u8; 32]>)> {
    if data.len() < NONCE_OFFSET + NONCE_LEN {
        anyhow::bail!("Encrypted file too small");
    }

    if data[..4] != MAGIC {
        anyhow::bail!("Not a self-awareness encrypted file (bad magic bytes)");
    }

    let raw_format = data[FORMAT_OFFSET];
    let has_chain = (raw_format & FLAG_HASH_CHAIN) != 0;
    let format_byte = raw_format & !FLAG_HASH_CHAIN;

    let format = match format_byte {
        FORMAT_WEBP => ImageFormat::Webp,
        FORMAT_JPEG => ImageFormat::Jpeg,
        FORMAT_PNG => ImageFormat::Png,
        other => anyhow::bail!("Unknown format byte: {}", other),
    };

    let hl = header_len(raw_format);
    if data.len() < hl {
        anyhow::bail!("Encrypted file too small for header");
    }

    let nonce = &data[NONCE_OFFSET .. NONCE_OFFSET + NONCE_LEN];
    let mut chain_hash = None;
    if has_chain {
        let mut h = [0u8; 32];
        h.copy_from_slice(&data[NONCE_OFFSET + NONCE_LEN .. hl]);
        chain_hash = Some(h);
    }

    let ciphertext = &data[hl..];

    let plaintext = decrypt_aes(key, nonce, ciphertext)?;
    Ok((plaintext, format, chain_hash))
}

/// Extracts the chain hash and computes the current file hash for verification.
/// Returns `(stored_chain_hash, computed_current_file_hash)`
pub fn get_chain_info(data: &[u8]) -> Result<(Option<[u8; 32]>, [u8; 32])> {
    if data.len() < NONCE_OFFSET + NONCE_LEN {
        anyhow::bail!("File too small");
    }
    if data[..4] != MAGIC {
        anyhow::bail!("Invalid magic");
    }
    let raw_format = data[FORMAT_OFFSET];
    let hl = header_len(raw_format);
    if data.len() < hl {
        anyhow::bail!("File too small for header");
    }

    let has_chain = (raw_format & FLAG_HASH_CHAIN) != 0;
    let mut chain_hash = None;
    if has_chain {
        let mut h = [0u8; 32];
        h.copy_from_slice(&data[NONCE_OFFSET + NONCE_LEN .. hl]);
        chain_hash = Some(h);
    }

    let ciphertext = &data[hl..];
    let current_file_hash = Sha256::digest(ciphertext).into();

    Ok((chain_hash, current_file_hash))
}

/// Detect whether a file is an encrypted self-awareness file by checking
/// the first 4 bytes. Returns `true` if the magic bytes match.
pub fn is_encrypted_file(data: &[u8]) -> bool {
    data.len() >= 4 && data[..4] == MAGIC
}

/// The file extension used for encrypted images.
pub const ENCRYPTED_EXTENSION: &str = "enc";

/// Scan the output directory to find the latest valid chain hash.
/// Returns a genesis hash if no valid encrypted files with chain hashes are found.
pub fn get_latest_chain_hash(output_dir: &str) -> Result<[u8; 32]> {
    let mut files = std::fs::read_dir(output_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| p.extension().map(|e| e == "enc").unwrap_or(false))
        .collect::<Vec<_>>();

    files.sort_by_key(|f| {
        f.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    for file in files.into_iter().rev() {
        let data = match std::fs::read(&file) {
            Ok(d) => d,
            Err(_) => continue,
        };
        
        if !is_encrypted_file(&data) {
            continue;
        }

        let raw_format = data[FORMAT_OFFSET];
        let has_chain = (raw_format & FLAG_HASH_CHAIN) != 0;
        if has_chain {
            let hl = header_len(raw_format);
            if data.len() >= hl {
                let mut h = [0u8; 32];
                h.copy_from_slice(&data[NONCE_OFFSET + NONCE_LEN .. hl]);
                return Ok(h);
            }
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(b"self-awareness-genesis");
    Ok(hasher.finalize().into())
}
