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
        let config = serde_json::from_reader(file).context("Parsing config file")?;
        Ok(config)
    }

    pub fn theme(&self) -> Result<Theme> {
        let theme_dir = PathBuf::from("themes").join(&self.theme);
        Theme::load(&theme_dir)
    }
}
