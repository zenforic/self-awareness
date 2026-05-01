use anyhow::Result;
use std::path::{Path, PathBuf};
use chrono::{TimeZone, Local};
use sha2::{Sha256, Digest};

use crate::config::Config;
use crate::crypto;

pub struct ImageEntry {
    pub path: PathBuf,
    pub filename: String,
    pub timestamp_ms: Option<i64>,
    pub is_encrypted: bool,
    pub chain_valid: Option<bool>, // None = no chain, Some(true) = intact, Some(false) = broken
}

pub struct ViewerState {
    pub all_entries: Vec<ImageEntry>,
    pub filtered_indices: Vec<usize>,
    pub selected_index: usize,
    pub intact_count: usize,
    pub broken_count: usize,
    pub chain_status_msg: String,
    pub scroll_offset: usize,
    pub search_query: String,
    pub is_searching: bool,
}

impl ViewerState {
    pub fn new(config: &Config) -> Self {
        let mut state = ViewerState {
            all_entries: Vec::new(),
            filtered_indices: Vec::new(),
            selected_index: 0,
            intact_count: 0,
            broken_count: 0,
            chain_status_msg: String::new(),
            scroll_offset: 0,
            search_query: String::new(),
            is_searching: false,
        };
        state.refresh(config);
        state
    }

    pub fn refresh(&mut self, config: &Config) {
        self.all_entries.clear();
        self.intact_count = 0;
        self.broken_count = 0;

        let dir = Path::new(&config.output_dir);
        if !dir.exists() {
            self.chain_status_msg = "Output directory does not exist.".to_string();
            return;
        }

        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    files.push(path);
                }
            }
        }

        // Sort by name (which contains timestamp %Y%m%d_%H%M%S) so older is first
        files.sort();

        for path in files {
            let filename_opt = path.file_name().and_then(|s| s.to_str()).map(|s| s.to_string());
            if let Some(filename) = filename_opt {
                let is_enc = filename.ends_with(crypto::ENCRYPTED_EXTENSION);
                let is_img = filename.ends_with("webp") || filename.ends_with("jpg") || filename.ends_with("png");
                
                if !is_enc && !is_img { continue; }

                let timestamp_ms = parse_timestamp(&filename);
                self.all_entries.push(ImageEntry {
                    path,
                    filename,
                    timestamp_ms,
                    is_encrypted: is_enc,
                    chain_valid: None,
                });
            }
        }

        // Verify chain
        let mut prev_hash: [u8; 32] = Sha256::digest(b"self-awareness-genesis").into();
        
        for entry in &mut self.all_entries {
            if entry.is_encrypted {
                if let Ok(data) = std::fs::read(&entry.path) {
                    if let Ok((stored_hash_opt, current_file_hash)) = crypto::get_chain_info(&data) {
                        if let Some(stored_hash) = stored_hash_opt {
                            if let Some(ts) = entry.timestamp_ms {
                                let mut hasher = Sha256::new();
                                hasher.update(prev_hash);
                                hasher.update(current_file_hash);
                                hasher.update(ts.to_le_bytes());
                                let expected_hash: [u8; 32] = hasher.finalize().into();

                                if stored_hash == expected_hash {
                                    entry.chain_valid = Some(true);
                                    self.intact_count += 1;
                                    prev_hash = expected_hash;
                                } else {
                                    entry.chain_valid = Some(false);
                                    self.broken_count += 1;
                                    // Accept the broken hash as the new baseline to verify subsequent files
                                    prev_hash = stored_hash;
                                }
                            } else {
                                // Couldn't parse timestamp, chain broken
                                entry.chain_valid = Some(false);
                                self.broken_count += 1;
                                prev_hash = stored_hash;
                            }
                        }
                    }
                }
            }
        }

        if self.all_entries.is_empty() {
            self.chain_status_msg = "No images found.".to_string();
        } else if self.broken_count == 0 {
            self.chain_status_msg = format!("Chain intact: {} images.", self.intact_count);
        } else {
            self.chain_status_msg = format!("Chain broken: {} intact, {} broken.", self.intact_count, self.broken_count);
        }

        self.update_filter();
    }

    pub fn update_filter(&mut self) {
        self.filtered_indices.clear();
        let query = self.search_query.to_lowercase();
        for (i, entry) in self.all_entries.iter().enumerate() {
            if query.is_empty() || entry.filename.to_lowercase().contains(&query) {
                self.filtered_indices.push(i);
            }
        }
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    pub fn open_selected(&self, config: &Config) -> Result<()> {
        if self.filtered_indices.is_empty() || self.selected_index >= self.filtered_indices.len() {
            return Ok(());
        }

        let entry = &self.all_entries[self.filtered_indices[self.selected_index]];
        if !entry.is_encrypted {
            open_file(&entry.path)?;
            return Ok(());
        }

        // Decrypt to temp file and open
        let data = std::fs::read(&entry.path)?;
        let key = crypto::load_key(config.current_passphrase.as_deref())?;
        let (plaintext, format, _) = crypto::decrypt_image(&key, &data)?;

        let temp_dir = std::env::temp_dir().join("self-awareness");
        std::fs::create_dir_all(&temp_dir)?;
        
        let temp_path = temp_dir.join(format!("{}.{}", entry.filename, format.extension()));
        std::fs::write(&temp_path, plaintext)?;

        open_file(&temp_path)?;
        Ok(())
    }

    pub fn decrypt_all(&self, config: &Config) -> Result<PathBuf> {
        let dest_dir = crate::config::app_dir().join("decrypted_investigation");
        std::fs::create_dir_all(&dest_dir)?;

        let key = crypto::load_key(config.current_passphrase.as_deref())?;
        for entry in &self.all_entries {
            if entry.is_encrypted {
                if let Ok(data) = std::fs::read(&entry.path) {
                    if let Ok((plaintext, format, _)) = crypto::decrypt_image(&key, &data) {
                        let out_path = dest_dir.join(format!("{}.{}", entry.filename, format.extension()));
                        let _ = std::fs::write(out_path, plaintext);
                    }
                }
            } else {
                let _ = std::fs::copy(&entry.path, dest_dir.join(&entry.filename));
            }
        }

        open_file(&dest_dir)?;
        Ok(dest_dir)
    }

    pub fn next(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.filtered_indices.len();
            self.adjust_scroll();
        }
    }

    pub fn previous(&mut self) {
        if !self.filtered_indices.is_empty() {
            if self.selected_index == 0 {
                self.selected_index = self.filtered_indices.len() - 1;
            } else {
                self.selected_index -= 1;
            }
            self.adjust_scroll();
        }
    }

    pub fn page_up(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected_index = self.selected_index.saturating_sub(10);
            self.adjust_scroll();
        }
    }

    pub fn page_down(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected_index = (self.selected_index + 10).min(self.filtered_indices.len() - 1);
            self.adjust_scroll();
        }
    }

    fn adjust_scroll(&mut self) {
        // Assume about 10 items visible in the list area
        let visible = 10;
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + visible {
            self.scroll_offset = self.selected_index - visible + 1;
        }
    }
}

fn parse_timestamp(filename: &str) -> Option<i64> {
    let parts: Vec<&str> = filename.split('_').collect();
    if parts.len() < 3 { return None; }
    let date_str = format!("{}_{}", parts[0], parts[1]);
    let ms_str = parts[2].split('.').next()?;
    
    let naive = chrono::NaiveDateTime::parse_from_str(&date_str, "%Y%m%d_%H%M%S").ok()?;
    let local = match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) => t,
        chrono::LocalResult::Ambiguous(t, _) => t,
        chrono::LocalResult::None => return None,
    };
    
    let ms: i64 = ms_str.parse().ok()?;
    Some(local.timestamp() * 1000 + ms)
}

fn open_file(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(&["/C", "start", "", path.to_str().unwrap()])
            .spawn()?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Fallback for other OS if needed
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()?;
    }
    Ok(())
}
