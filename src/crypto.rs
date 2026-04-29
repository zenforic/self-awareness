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

use crate::config::{app_dir, ImageFormat};

/// Magic bytes identifying an encrypted self-awareness file.
const MAGIC: [u8; 4] = *b"SAW1";

/// Offset of the format byte in the file header.
const FORMAT_OFFSET: usize = 4;
/// Offset where the nonce begins.
const NONCE_OFFSET: usize = 5;
/// Nonce length for AES-GCM (96 bits).
const NONCE_LEN: usize = 12;
/// GCM authentication tag length.
const TAG_LEN: usize = 16;
/// Total header size (magic + format + nonce). Everything after the ciphertext
/// up to TAG_LEN bytes is the auth tag.
const HEADER_LEN: usize = NONCE_OFFSET + NONCE_LEN;

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
pub fn encrypt_image(key: &[u8], plaintext: &[u8], format: ImageFormat) -> Result<Vec<u8>> {
    let (nonce, ciphertext) = encrypt_aes(key, plaintext)?;

    let format_byte = match format {
        ImageFormat::Webp => FORMAT_WEBP,
        ImageFormat::Jpeg => FORMAT_JPEG,
        ImageFormat::Png => FORMAT_PNG,
    };

    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.extend_from_slice(&MAGIC);
    out.push(format_byte);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt an encrypted file and return (plaintext_bytes, ImageFormat).
///
/// Validates the magic bytes and format byte. Returns an error if the
/// file is not a valid self-awareness encrypted file.
pub fn decrypt_image(key: &[u8], data: &[u8]) -> Result<(Vec<u8>, ImageFormat)> {
    if data.len() < HEADER_LEN {
        anyhow::bail!("Encrypted file too small ({}) — expected at least {} bytes", data.len(), HEADER_LEN);
    }

    // Validate magic
    if data[..4] != MAGIC {
        anyhow::bail!("Not a self-awareness encrypted file (bad magic bytes)");
    }

    // Decode format
    let format = match data[FORMAT_OFFSET] {
        FORMAT_WEBP => ImageFormat::Webp,
        FORMAT_JPEG => ImageFormat::Jpeg,
        FORMAT_PNG => ImageFormat::Png,
        other => anyhow::bail!("Unknown format byte: {}", other),
    };

    // Extract nonce and ciphertext
    let nonce = &data[NONCE_OFFSET..HEADER_LEN];
    let ciphertext = &data[HEADER_LEN..];

    let plaintext = decrypt_aes(key, nonce, ciphertext)?;
    Ok((plaintext, format))
}

/// Detect whether a file is an encrypted self-awareness file by checking
/// the first 4 bytes. Returns `true` if the magic bytes match.
pub fn is_encrypted_file(data: &[u8]) -> bool {
    data.len() >= 4 && data[..4] == MAGIC
}

/// The file extension used for encrypted images.
pub const ENCRYPTED_EXTENSION: &str = "enc";
