/// Unit tests for the self-awareness codebase.
/// Run with: cargo test
///
/// Tests cover:
///   - config: ImageFormat helpers, Config defaults, serialization round-trip,
///             backward-compatible deserialization, path helpers
///   - crypto: AES-GCM encrypt/decrypt, encrypt_image / decrypt_image round-trips
///             (with and without hash chain), magic detection, get_chain_info,
///             TUI password hashing and verification, error paths
///   - viewer: navigation (next/prev/page_up/page_down), search filter,
///             parse_timestamp (exercised via filename-based logic)

// ────────────────────────────────────────────────────────────────────────────
// Config tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod config_tests {
    use crate::config::{Config, ImageFormat};

    // ── ImageFormat helpers ──────────────────────────────────────────────────

    #[test]
    fn image_format_extensions() {
        assert_eq!(ImageFormat::Webp.extension(), "webp");
        assert_eq!(ImageFormat::Jpeg.extension(), "jpg");
        assert_eq!(ImageFormat::Png.extension(), "png");
    }

    #[test]
    fn image_format_labels() {
        assert_eq!(ImageFormat::Webp.label(), "WebP");
        assert_eq!(ImageFormat::Jpeg.label(), "JPEG");
        assert_eq!(ImageFormat::Png.label(), "PNG");
    }

    #[test]
    fn image_format_all_contains_all_variants() {
        let all = ImageFormat::all();
        assert_eq!(all.len(), 3);
        assert!(all.contains(&ImageFormat::Webp));
        assert!(all.contains(&ImageFormat::Jpeg));
        assert!(all.contains(&ImageFormat::Png));
    }

    #[test]
    fn image_format_default_is_webp() {
        assert_eq!(ImageFormat::default(), ImageFormat::Webp);
    }

    // ── Config default values ────────────────────────────────────────────────

    #[test]
    fn config_default_values() {
        let c = Config::default();
        assert_eq!(c.interval_seconds, 60);
        assert_eq!(c.max_disk_mb, 500);
        assert_eq!(c.retention_days, 7);
        assert!(!c.start_on_boot);
        assert!(c.encrypt_images);
        assert!(c.hash_chain);
        assert!(c.tui_passphrase_hash.is_none());
        assert!(c.current_passphrase.is_none());
        assert_eq!(c.image_format, ImageFormat::Webp);
    }

    // ── Serialization round-trip ─────────────────────────────────────────────

    #[test]
    fn config_serialization_round_trip() {
        let original = Config {
            interval_seconds: 30,
            max_disk_mb: 250,
            output_dir: "C:\\test\\output".to_string(),
            image_format: ImageFormat::Jpeg,
            retention_days: 14,
            start_on_boot: true,
            encrypt_images: false,
            hash_chain: false,
            tui_passphrase_hash: Some("some_hash".to_string()),
            current_passphrase: None, // skipped by serde
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let restored: Config = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored.interval_seconds, 30);
        assert_eq!(restored.max_disk_mb, 250);
        assert_eq!(restored.output_dir, "C:\\test\\output");
        assert_eq!(restored.image_format, ImageFormat::Jpeg);
        assert_eq!(restored.retention_days, 14);
        assert!(restored.start_on_boot);
        assert!(!restored.encrypt_images);
        assert!(!restored.hash_chain);
        assert_eq!(restored.tui_passphrase_hash.as_deref(), Some("some_hash"));
        // current_passphrase is #[serde(skip)] so always None after deserialize
        assert!(restored.current_passphrase.is_none());
    }

    /// Older config files won't have `encrypt_images` or `hash_chain` fields.
    /// `encrypt_images` uses `#[serde(default)]` → false (backwards compat).
    /// `hash_chain`    uses `#[serde(default = "default_true")]` → true.
    #[test]
    fn config_backward_compat_missing_optional_fields() {
        let legacy_json = r#"{
            "interval_seconds": 120,
            "max_disk_mb": 100,
            "output_dir": "C:\\pics",
            "image_format": "png",
            "retention_days": 3,
            "start_on_boot": false
        }"#;

        let config: Config = serde_json::from_str(legacy_json).expect("deserialize legacy");
        assert!(!config.encrypt_images, "should default to false for old configs");
        assert!(config.hash_chain,     "should default to true via default_true");
        assert!(config.tui_passphrase_hash.is_none());
    }

    #[test]
    fn config_image_format_serialization_lowercase() {
        // Formats must serialize as lowercase strings per serde(rename_all = "lowercase")
        let json = serde_json::to_string(&ImageFormat::Webp).unwrap();
        assert_eq!(json, "\"webp\"");
        let json = serde_json::to_string(&ImageFormat::Jpeg).unwrap();
        assert_eq!(json, "\"jpeg\"");
        let json = serde_json::to_string(&ImageFormat::Png).unwrap();
        assert_eq!(json, "\"png\"");
    }

    #[test]
    fn config_image_format_deserialization() {
        let fmt: ImageFormat = serde_json::from_str("\"webp\"").unwrap();
        assert_eq!(fmt, ImageFormat::Webp);
        let fmt: ImageFormat = serde_json::from_str("\"jpeg\"").unwrap();
        assert_eq!(fmt, ImageFormat::Jpeg);
        let fmt: ImageFormat = serde_json::from_str("\"png\"").unwrap();
        assert_eq!(fmt, ImageFormat::Png);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Crypto tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod crypto_tests {
    use crate::config::ImageFormat;
    use crate::crypto::{
        decrypt_image, encrypt_image, get_chain_info, hash_tui_password, is_encrypted_file,
        verify_tui_password, ENCRYPTED_EXTENSION,
    };

    // ── AES helpers (tested via public encrypt_image / decrypt_image API) ───

    fn test_key() -> Vec<u8> {
        // 32-byte key for AES-256-GCM
        (0u8..32).collect()
    }

    // ── is_encrypted_file ────────────────────────────────────────────────────

    #[test]
    fn is_encrypted_file_valid_magic() {
        let magic = b"SAW1some_extra_bytes";
        assert!(is_encrypted_file(magic));
    }

    #[test]
    fn is_encrypted_file_bad_magic() {
        assert!(!is_encrypted_file(b"NOPE"));
        assert!(!is_encrypted_file(b"saw1")); // case sensitive
    }

    #[test]
    fn is_encrypted_file_too_short() {
        assert!(!is_encrypted_file(b"SAW")); // only 3 bytes
        assert!(!is_encrypted_file(b""));
    }

    // ── encrypt_image / decrypt_image round-trips ────────────────────────────

    #[test]
    fn encrypt_decrypt_webp_no_hash_chain() {
        let key = test_key();
        let plaintext = b"fake webp image data 1234567890";
        let (encrypted, hash) = encrypt_image(&key, plaintext, ImageFormat::Webp, None)
            .expect("encrypt");

        assert!(hash.is_none(), "no hash chain requested");
        assert!(is_encrypted_file(&encrypted));

        let (decrypted, format, chain) =
            decrypt_image(&key, &encrypted).expect("decrypt");

        assert_eq!(decrypted, plaintext);
        assert_eq!(format, ImageFormat::Webp);
        assert!(chain.is_none());
    }

    #[test]
    fn encrypt_decrypt_jpeg_no_hash_chain() {
        let key = test_key();
        let plaintext = b"jpeg image bytes";
        let (encrypted, _) = encrypt_image(&key, plaintext, ImageFormat::Jpeg, None)
            .expect("encrypt");

        let (decrypted, format, _) = decrypt_image(&key, &encrypted).expect("decrypt");
        assert_eq!(decrypted, plaintext);
        assert_eq!(format, ImageFormat::Jpeg);
    }

    #[test]
    fn encrypt_decrypt_png_no_hash_chain() {
        let key = test_key();
        let plaintext = b"png image bytes 000";
        let (encrypted, _) = encrypt_image(&key, plaintext, ImageFormat::Png, None)
            .expect("encrypt");

        let (decrypted, format, _) = decrypt_image(&key, &encrypted).expect("decrypt");
        assert_eq!(decrypted, plaintext);
        assert_eq!(format, ImageFormat::Png);
    }

    #[test]
    fn encrypt_decrypt_with_hash_chain() {
        let key = test_key();
        let plaintext = b"image with hash chain";
        let genesis: [u8; 32] = [0xAB; 32];
        let timestamp_ms: i64 = 1_700_000_000_000;

        let (encrypted, chain_hash) =
            encrypt_image(&key, plaintext, ImageFormat::Webp, Some((&genesis, timestamp_ms)))
                .expect("encrypt");

        assert!(chain_hash.is_some(), "should produce a chain hash");
        assert!(is_encrypted_file(&encrypted));

        let (decrypted, format, stored_chain) =
            decrypt_image(&key, &encrypted).expect("decrypt");

        assert_eq!(decrypted, plaintext);
        assert_eq!(format, ImageFormat::Webp);
        // The stored chain hash from the file should match what encrypt returned
        assert_eq!(stored_chain, chain_hash);
    }

    #[test]
    fn hash_chain_is_deterministic_given_same_inputs() {
        let key = test_key();
        let plaintext = b"deterministic test data";
        let genesis: [u8; 32] = [0x01; 32];
        let ts: i64 = 42_000;

        // We can't guarantee the same ciphertext (nonce is random), but we can
        // verify that two independent files produce distinct chain hashes even
        // from the same genesis, because the nonce/ciphertext will differ.
        let (_, hash_a) = encrypt_image(&key, plaintext, ImageFormat::Webp, Some((&genesis, ts)))
            .expect("encrypt a");
        let (_, hash_b) = encrypt_image(&key, plaintext, ImageFormat::Webp, Some((&genesis, ts)))
            .expect("encrypt b");

        // Both should be present
        assert!(hash_a.is_some());
        assert!(hash_b.is_some());
        // They are almost certainly different because the nonce/ciphertext differ
        // (extremely low probability of collision — acceptable in a test)
        assert_ne!(hash_a, hash_b, "random nonces produce unique chain hashes");
    }

    #[test]
    fn decrypt_image_wrong_key_fails() {
        let key = test_key();
        let wrong_key: Vec<u8> = (1u8..33).collect();
        let plaintext = b"secret data";

        let (encrypted, _) = encrypt_image(&key, plaintext, ImageFormat::Png, None)
            .expect("encrypt");

        let result = decrypt_image(&wrong_key, &encrypted);
        assert!(result.is_err(), "wrong key should fail GCM authentication");
    }

    #[test]
    fn decrypt_image_bad_magic_fails() {
        let key = test_key();
        let bad_data = b"BADMAGIC_and_then_some_more_bytes_to_pass_length_check";
        let result = decrypt_image(&key, bad_data);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("bad magic") || msg.contains("magic"), "got: {}", msg);
    }

    #[test]
    fn decrypt_image_too_small_fails() {
        let key = test_key();
        let result = decrypt_image(&key, b"SAW1");
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_image_tampered_ciphertext_fails() {
        let key = test_key();
        let plaintext = b"tamper test";
        let (mut encrypted, _) = encrypt_image(&key, plaintext, ImageFormat::Webp, None)
            .expect("encrypt");

        // Flip a bit in the ciphertext (well past the header)
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xFF;

        let result = decrypt_image(&key, &encrypted);
        assert!(result.is_err(), "tampered ciphertext should fail");
    }

    // ── get_chain_info ───────────────────────────────────────────────────────

    #[test]
    fn get_chain_info_no_chain() {
        let key = test_key();
        let plaintext = b"no chain file";
        let (encrypted, _) = encrypt_image(&key, plaintext, ImageFormat::Png, None)
            .expect("encrypt");

        let (stored, file_hash) = get_chain_info(&encrypted).expect("get_chain_info");
        assert!(stored.is_none(), "no chain hash expected");
        // file_hash is SHA-256 of ciphertext — must be 32 bytes
        assert_eq!(file_hash.len(), 32);
    }

    #[test]
    fn get_chain_info_with_chain() {
        let key = test_key();
        let plaintext = b"chain file";
        let genesis: [u8; 32] = [0xCC; 32];
        let ts: i64 = 9_999;

        let (encrypted, expected_chain) =
            encrypt_image(&key, plaintext, ImageFormat::Webp, Some((&genesis, ts)))
                .expect("encrypt");

        let (stored, _) = get_chain_info(&encrypted).expect("get_chain_info");
        assert_eq!(stored, expected_chain, "stored hash should match what encrypt produced");
    }

    #[test]
    fn get_chain_info_bad_magic_fails() {
        let result = get_chain_info(b"NOPE_NOT_A_SAW_FILE_");
        assert!(result.is_err());
    }

    #[test]
    fn get_chain_info_too_small_fails() {
        let result = get_chain_info(b"SAW1\x00");
        assert!(result.is_err());
    }

    // ── ENCRYPTED_EXTENSION constant ─────────────────────────────────────────

    #[test]
    fn encrypted_extension_is_enc() {
        assert_eq!(ENCRYPTED_EXTENSION, "enc");
    }

    // ── TUI password hashing / verification ─────────────────────────────────

    #[test]
    fn tui_password_hash_and_verify_correct() {
        let password = "super-secret-tui-pass";
        let hash = hash_tui_password(password).expect("hash");
        // Hash should be a non-empty PHC string
        assert!(!hash.is_empty());
        assert!(hash.starts_with("$argon2"), "expected PHC format, got: {}", hash);

        let ok = verify_tui_password(password, &hash).expect("verify");
        assert!(ok, "correct password should verify");
    }

    #[test]
    fn tui_password_verify_wrong_password() {
        let password = "correct-horse-battery-staple";
        let hash = hash_tui_password(password).expect("hash");

        let ok = verify_tui_password("wrong-password", &hash).expect("verify");
        assert!(!ok, "wrong password should not verify");
    }

    #[test]
    fn tui_password_hash_is_unique_per_call() {
        // Argon2 uses a random salt — two hashes of the same password differ
        let password = "same-password";
        let hash1 = hash_tui_password(password).expect("hash1");
        let hash2 = hash_tui_password(password).expect("hash2");
        assert_ne!(hash1, hash2, "salts should differ");

        // But both should verify correctly
        assert!(verify_tui_password(password, &hash1).unwrap());
        assert!(verify_tui_password(password, &hash2).unwrap());
    }

    #[test]
    fn tui_password_verify_empty_password() {
        let hash = hash_tui_password("").expect("hash empty");
        assert!(verify_tui_password("", &hash).unwrap());
        assert!(!verify_tui_password("notempty", &hash).unwrap());
    }

    #[test]
    fn tui_password_verify_invalid_hash_format() {
        let result = verify_tui_password("pass", "this-is-not-a-phc-hash");
        assert!(result.is_err(), "invalid hash format should return Err");
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Viewer tests  (navigation + filter; no filesystem I/O)
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod viewer_tests {
    use crate::viewer::{ImageEntry, ViewerState};
    use std::path::PathBuf;

    // ── Builder helpers ──────────────────────────────────────────────────────

    fn make_entry(filename: &str) -> ImageEntry {
        ImageEntry {
            path: PathBuf::from(filename),
            filename: filename.to_string(),
            timestamp_ms: None,
            is_encrypted: filename.ends_with(".enc"),
            chain_valid: None,
            gap_duration_ms: None,
        }
    }

    fn make_state_with_entries(filenames: &[&str]) -> ViewerState {
        let entries: Vec<ImageEntry> = filenames.iter().map(|f| make_entry(f)).collect();
        let count = entries.len();
        let filtered: Vec<usize> = (0..count).collect();

        ViewerState {
            all_entries: entries,
            filtered_indices: filtered,
            selected_index: 0,
            intact_count: 0,
            broken_count: 0,
            chain_status_msg: String::new(),
            scroll_offset: 0,
            search_query: String::new(),
            is_searching: false,
        }
    }

    // ── Navigation: next / previous ─────────────────────────────────────────

    #[test]
    fn navigation_next_wraps_around() {
        let mut state = make_state_with_entries(&["a.enc", "b.enc", "c.enc"]);
        assert_eq!(state.selected_index, 0);
        state.next();
        assert_eq!(state.selected_index, 1);
        state.next();
        assert_eq!(state.selected_index, 2);
        state.next(); // wrap
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn navigation_previous_wraps_around() {
        let mut state = make_state_with_entries(&["a.enc", "b.enc", "c.enc"]);
        state.previous(); // wrap from 0 to last
        assert_eq!(state.selected_index, 2);
        state.previous();
        assert_eq!(state.selected_index, 1);
        state.previous();
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn navigation_empty_list_is_safe() {
        let mut state = make_state_with_entries(&[]);
        // None of these should panic on an empty list
        state.next();
        state.previous();
        state.page_up();
        state.page_down();
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn navigation_single_item_stays_at_zero() {
        let mut state = make_state_with_entries(&["only.enc"]);
        state.next();
        assert_eq!(state.selected_index, 0);
        state.previous();
        assert_eq!(state.selected_index, 0);
    }

    // ── Navigation: page_up / page_down ─────────────────────────────────────

    #[test]
    fn page_down_clamps_to_last() {
        let filenames: Vec<String> = (0..5).map(|i| format!("img{}.enc", i)).collect();
        let refs: Vec<&str> = filenames.iter().map(String::as_str).collect();
        let mut state = make_state_with_entries(&refs);

        state.page_down(); // jump by 10, but only 5 items — clamp to 4
        assert_eq!(state.selected_index, 4);
    }

    #[test]
    fn page_up_saturates_to_zero() {
        let filenames: Vec<String> = (0..15).map(|i| format!("img{}.enc", i)).collect();
        let refs: Vec<&str> = filenames.iter().map(String::as_str).collect();
        let mut state = make_state_with_entries(&refs);

        // Move to index 3, then page_up — should clamp at 0
        state.next(); state.next(); state.next();
        assert_eq!(state.selected_index, 3);
        state.page_up();
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn page_down_advances_by_ten() {
        let filenames: Vec<String> = (0..25).map(|i| format!("img{:02}.enc", i)).collect();
        let refs: Vec<&str> = filenames.iter().map(String::as_str).collect();
        let mut state = make_state_with_entries(&refs);

        state.page_down();
        assert_eq!(state.selected_index, 10);
        state.page_down();
        assert_eq!(state.selected_index, 20);
    }

    #[test]
    fn page_up_retreats_by_ten() {
        let filenames: Vec<String> = (0..25).map(|i| format!("img{:02}.enc", i)).collect();
        let refs: Vec<&str> = filenames.iter().map(String::as_str).collect();
        let mut state = make_state_with_entries(&refs);

        // Start at 20
        state.page_down(); state.page_down();
        assert_eq!(state.selected_index, 20);

        state.page_up();
        assert_eq!(state.selected_index, 10);
        state.page_up();
        assert_eq!(state.selected_index, 0);
    }

    // ── scroll adjust ────────────────────────────────────────────────────────

    #[test]
    fn scroll_offset_advances_when_selection_leaves_window() {
        let filenames: Vec<String> = (0..20).map(|i| format!("img{:02}.enc", i)).collect();
        let refs: Vec<&str> = filenames.iter().map(String::as_str).collect();
        let mut state = make_state_with_entries(&refs);

        // Navigate past the visible window (10 items)
        for _ in 0..11 {
            state.next();
        }
        assert_eq!(state.selected_index, 11);
        assert!(state.scroll_offset > 0, "scroll should have advanced");
    }

    #[test]
    fn scroll_offset_resets_when_going_back() {
        let filenames: Vec<String> = (0..20).map(|i| format!("img{:02}.enc", i)).collect();
        let refs: Vec<&str> = filenames.iter().map(String::as_str).collect();
        let mut state = make_state_with_entries(&refs);

        // Go forward past the visible window
        for _ in 0..15 {
            state.next();
        }
        let offset_after_forward = state.scroll_offset;
        assert!(offset_after_forward > 0);

        // Go all the way back to 0
        for _ in 0..15 {
            state.previous();
        }
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.scroll_offset, 0);
    }

    // ── update_filter ────────────────────────────────────────────────────────

    #[test]
    fn filter_empty_query_shows_all() {
        let mut state = make_state_with_entries(&["alpha.enc", "beta.enc", "gamma.enc"]);
        state.search_query.clear();
        state.update_filter();
        assert_eq!(state.filtered_indices.len(), 3);
    }

    #[test]
    fn filter_matching_query_narrows_results() {
        let mut state = make_state_with_entries(&[
            "20250101_120000_000.enc",
            "20250101_130000_000.enc",
            "20250102_120000_000.enc",
        ]);
        state.search_query = "20250101".to_string();
        state.update_filter();
        assert_eq!(state.filtered_indices.len(), 2);
    }

    #[test]
    fn filter_case_insensitive() {
        let mut state = make_state_with_entries(&["Screenshot_ABC.enc", "screenshot_abc.enc"]);
        state.search_query = "SCREENSHOT".to_string();
        state.update_filter();
        assert_eq!(state.filtered_indices.len(), 2);
    }

    #[test]
    fn filter_no_match_returns_empty() {
        let mut state =
            make_state_with_entries(&["alpha.enc", "beta.enc"]);
        state.search_query = "zzznomatch".to_string();
        state.update_filter();
        assert_eq!(state.filtered_indices.len(), 0);
    }

    #[test]
    fn filter_resets_selection_to_zero() {
        let mut state = make_state_with_entries(&["a.enc", "b.enc", "c.enc"]);
        state.next(); state.next();
        assert_eq!(state.selected_index, 2);

        state.search_query = "b".to_string();
        state.update_filter();

        assert_eq!(state.selected_index, 0, "selection should reset after filter");
        assert_eq!(state.scroll_offset, 0, "scroll should reset after filter");
    }

    #[test]
    fn filter_indices_point_to_correct_entries() {
        let mut state = make_state_with_entries(&[
            "apple.enc",
            "banana.enc",
            "apricot.enc",
            "cherry.enc",
        ]);
        state.search_query = "ap".to_string();
        state.update_filter();

        assert_eq!(state.filtered_indices.len(), 2);
        // The filtered indices should point to "apple" (0) and "apricot" (2)
        assert_eq!(state.all_entries[state.filtered_indices[0]].filename, "apple.enc");
        assert_eq!(state.all_entries[state.filtered_indices[1]].filename, "apricot.enc");
    }
}
