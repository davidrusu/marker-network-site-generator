use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub site_root: String,
    pub title: String,
    pub theme: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).context("Opening config file")?;
        Self::from_reader(file)
    }

    pub fn from_reader<R: std::io::Read>(r: R) -> Result<Self> {
        let config = serde_json::from_reader(r).context("Parsing config file")?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let temp_path = path.with_extension("json.tmp");
        let config_file = std::fs::File::create(&temp_path).context("Creating manifest file")?;

        serde_json::to_writer_pretty(&config_file, self).context("Parsing config file")?;

        std::fs::rename(temp_path, path).context("Renaming tempfile to config file")?;
        Ok(())
    }

    pub fn theme(&self) -> Result<Theme> {
        let theme_dir = PathBuf::from("themes").join(&self.theme);
        Theme::load(&theme_dir)
    }
}
