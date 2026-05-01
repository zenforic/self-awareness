use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub interval_seconds: u64,
    pub max_disk_mb: u64,
    pub output_dir: String,
    pub image_format: ImageFormat,
    pub retention_days: u64,
    pub start_on_boot: bool,
    /// Whether screenshots are encrypted at rest with AES-256-GCM.
    /// Defaults to `false` for existing configs (backward compatible),
    /// `true` for new configs.
    #[serde(default)]
    pub encrypt_images: bool,
    /// Whether to embed a hash chain for tamper detection
    #[serde(default = "default_true")]
    pub hash_chain: bool,
    /// Argon2 password hash for TUI login, if set.
    pub tui_passphrase_hash: Option<String>,
    #[serde(skip)]
    pub current_passphrase: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Webp,
    Jpeg,
    Png,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            interval_seconds: 60,
            max_disk_mb: 500,
            output_dir: default_output_dir(),
            image_format: ImageFormat::Webp,
            retention_days: 7,
            start_on_boot: false,
            encrypt_images: true,
            hash_chain: true,
            tui_passphrase_hash: None,
            current_passphrase: None,
        }
    }
}

impl Default for ImageFormat {
    fn default() -> Self {
        ImageFormat::Webp
    }
}

fn default_output_dir() -> String {
    dirs::picture_dir()
        .map(|p| p.join("self-awareness").to_string_lossy().into_owned())
        .unwrap_or_else(|| "screenshots".to_string())
}

impl Config {
    pub fn load() -> Result<Self, anyhow::Error> {
        let config_path = config_file_path();
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = serde_json::from_str(&content)?;
            // Existing configs without encrypt_images get `false` from #[serde(default)]
            // — backward compatible. New configs get `true` from Default.
            Ok(config)
        } else {
            let config = Config::default();
            config.save()?;
            Ok(config)
        }
    }

    pub fn save(&self) -> Result<(), anyhow::Error> {
        let config_path = config_file_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }
}

pub fn config_file_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("self-awareness")
        .join("config.json")
}

pub fn daemon_log_path() -> PathBuf {
    app_dir().join("daemon.log")
}

pub fn daemon_pid_path() -> PathBuf {
    app_dir().join("daemon.pid")
}

pub fn app_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("self-awareness")
}

impl ImageFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            ImageFormat::Webp => "webp",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
        }
    }

    pub fn all() -> &'static [ImageFormat] {
        &[ImageFormat::Webp, ImageFormat::Jpeg, ImageFormat::Png]
    }

    pub fn label(&self) -> &'static str {
        match self {
            ImageFormat::Webp => "WebP",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Png => "PNG",
        }
    }
}
