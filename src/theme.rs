use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use handlebars::Handlebars;

#[derive(Debug)]
pub struct Theme {
    handlebars: Handlebars<'static>,
    css: PathBuf,
}

impl Theme {
    pub fn load(theme: &Path) -> Result<Self> {
        let mut handlebars = Handlebars::new();
        handlebars
            .register_template_file("index", &theme.join("index.html"))
            .context("Registering index template")?;
        handlebars
            .register_template_file("document", &theme.join("document.html"))
            .context("Registering document template")?;
        handlebars
            .register_template_file("folder", &theme.join("folder.html"))
            .context("Registering folder template")?;
        let css = theme.join("style.css");
        if !css.exists() {
            return Err(anyhow!("Missing theme css: {:?}", css));
        }
        Ok(Self { handlebars, css })
    }

    pub fn render_index(&self, params: &handlebars::JsonValue, gen_root: &Path) -> Result<()> {
        let f_out =
            std::fs::File::create(&gen_root.join("index.html")).context("Creating index.html")?;
        self.handlebars
            .render_to_write("index", params, f_out)
            .context("Rendering index.html")?;
        Ok(())
    }

    pub fn render_document(&self, params: &handlebars::JsonValue, out: &Path) -> Result<()> {
        let f_out = std::fs::File::create(&out).context("Creating document file for rendering")?;
        self.handlebars
            .render_to_write("document", params, f_out)
            .context("Rendering document template")?;
        Ok(())
    }

    pub fn render_folder(&self, params: &handlebars::JsonValue, out: &Path) -> Result<()> {
        let f_out = std::fs::File::create(&out).context("Creating folder file for rendering")?;
        self.handlebars
            .render_to_write("folder", params, f_out)
            .context("Cendering folder template")?;
        Ok(())
    }

    pub fn render_css(&self, gen_root: &Path) -> Result<()> {
        std::fs::copy(&self.css, &gen_root.join("style.css"))
            .context("Copying theme css into generated site")?;
        Ok(())
    }
}
