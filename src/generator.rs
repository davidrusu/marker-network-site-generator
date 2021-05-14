use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use anyhow::{Context, Result};
use remarkable_cloud_api::Uuid;
use serde_json::json;

use crate::config::Config;
use crate::manifest::{Manifest, Posts};
use crate::theme::Theme;

pub struct Generator {
    root: PathBuf,
    prefix: PathBuf,
    config: Config,
    manifest: Manifest,
    theme: Theme,
    svgs: BTreeMap<Uuid, Vec<PathBuf>>, // Rendered notebook pages
    build_nonce: String,
}

impl Generator {
    pub fn prepare(
        config: Config,
        material_path: PathBuf,
        root: PathBuf,
        prefix: PathBuf,
    ) -> Result<Self> {
        std::fs::create_dir_all(&root).context("creating the generated site directory")?;

        let manifest = Manifest::load(&material_path).context("Loading manifest")?;
        println!("Loaded manifest {:#?}", manifest);

        let theme = config.theme().context("Loading theme from config")?;
	let mut gen = Self {
            root,
            prefix,
            config,
            manifest,
            theme,
            svgs: Default::default(),
            build_nonce: chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string(),
        };
        gen.svgs = gen.render_all_svgs(&material_path).context("Rendering svg's")?;

        Ok(gen)
    }

    fn title(&self) -> &str {
        &self.config.title
    }

    fn logo_svg(&self) -> &Path {
        self.doc_first_page(self.manifest.logo.id)
    }

    fn home_pages(&self) -> &[PathBuf] {
        self.doc_pages(self.manifest.home.id)
    }

    fn doc_first_page(&self, id: Uuid) -> &Path {
        &self.doc_pages(id)[0]
    }

    /// Panics if Doc ID does not exist.
    fn doc_pages(&self, id: Uuid) -> &[PathBuf] {
        &self.svgs[&id]
    }

    fn relative_to_root(&self, path: &Path) -> Result<PathBuf> {
        Ok(self.prefix.join(
            path.strip_prefix(&self.root)
                .with_context(|| format!("Stripping root from path {:?}", path))?,
        ))
    }

    pub fn gen_index(&self) -> Result<()> {
        let mut docs: Vec<(String, Uuid, PathBuf)> = Vec::new();
        let mut sub_folders: Vec<(String, PathBuf)> = Vec::new();

        let posts_path = self.root.join("posts");
        std::fs::create_dir_all(&posts_path)
            .context("Creating posts directory in generated site root")?;

        let breadcrumbs = &[("Home".to_string(), PathBuf::from("/index.html"))];
        for doc in self.manifest.posts.documents.values() {
            let doc_path = self
                .gen_doc(breadcrumbs, &posts_path, &doc.name, doc.id)
                .context("Generating a top level document")?;
            docs.push((doc.name.clone(), doc.id, doc_path));
        }

        for (sub_folder_name, sub_folder_posts) in self.manifest.posts.folders.iter() {
            let sub_folder_path = self
                .gen_folder(breadcrumbs, &posts_path, sub_folder_name, sub_folder_posts)
                .context("Generating a top-level folder")?;
            sub_folders.push((sub_folder_name.to_string(), sub_folder_path));
        }

        self.theme
            .render_index(
                &json!({
                    "build_nonce": self.build_nonce,
		    "prefix": self.prefix,
                    "title": self.title(),
                    "logo": self.logo_svg(),
                    "name": "Home",
                    "pages": self.home_pages(),
                    "documents": docs.into_iter().map(|(name, id, link)| json!({
                        "name": name,
                        "svg": self.doc_first_page(id),
                        "link": link,
                    })).collect::<Vec<_>>(),
                    "folders": sub_folders.into_iter().map(|(name, link)| json!({
                        "name": name,
                        "link": link,
                    })).collect::<Vec<_>>(),
                }),
                &self.root,
            )
            .context("Rendering index.html")?;

        self.theme.render_css(&self.root).context("Rendering css")?;

        Ok(())
    }

    fn gen_doc(
        &self,
        breadcrumbs: &[(String, PathBuf)],
        parent: &Path,
        name: &str,
        id: Uuid,
    ) -> Result<PathBuf> {
        let sanitized_name = sanitize(name);
        // TODO: replace this with a breadcrumbs_to_path method on the Site
        let doc_path = parent.join(format!("{}.html", sanitized_name));

        self.theme
            .render_document(
                &json!({
                    "build_nonce": self.build_nonce,
		    "prefix": self.prefix,
                    "title": self.title(),
                    "name": name,
                    "breadcrumbs": breadcrumbs
                        .iter()
                        .map(|(crumb, link)| json!({"name": crumb, "link": link}))
                        .collect::<Vec<_>>(),
                    "logo": self.logo_svg(),
                    "back_link": breadcrumbs.iter().last().map(|(_, link)| link).unwrap(),
                    "pages": self.doc_pages(id),
                }),
                &doc_path,
            )
            .context("Rendering document html")?;

        self.relative_to_root(&doc_path)
    }

    fn gen_folder(
        &self,
        breadcrumbs: &[(String, PathBuf)],
        parent: &Path,
        folder: &str,
        posts: &Posts,
    ) -> Result<PathBuf> {
        let sanitized_folder = sanitize(folder);
        let folder_path = parent.join(&sanitized_folder);
        std::fs::create_dir_all(&folder_path)
            .context("Creating folder directory before generating html")?;

        let folder_html_path = parent.join(format!("{}.html", sanitized_folder));
        let folder_link = self.relative_to_root(&folder_html_path)?;

        let mut docs: Vec<(String, Uuid, PathBuf)> = Vec::new();
        let mut sub_folders: Vec<(String, PathBuf)> = Vec::new();

        let mut breadcrumbs_for_children = breadcrumbs.to_vec();
        breadcrumbs_for_children.push((folder.to_string(), folder_link.clone()));
        for doc in posts.documents.values() {
            let doc_path = self
                .gen_doc(&breadcrumbs_for_children, &folder_path, &doc.name, doc.id)
                .context("Generating a doc inside a folder")?;
            docs.push((doc.name.clone(), doc.id, doc_path));
        }

        for (sub_folder_name, sub_folder_posts) in posts.folders.iter() {
            let sub_folder_path = self
                .gen_folder(
                    &breadcrumbs_for_children,
                    &folder_path,
                    sub_folder_name,
                    sub_folder_posts,
                )
                .context("Generating a sub-folder inside a folder")?;
            sub_folders.push((sub_folder_name.to_string(), sub_folder_path));
        }

        self.theme
            .render_folder(
                &json!({
                    "build_nonce": self.build_nonce,
		    "prefix": self.prefix,
                    "title": self.title(),
                    "name": folder,
                    "logo": self.logo_svg(),
                    "breadcrumbs": breadcrumbs
                        .iter()
                        .map(|(name, link)| json!({"name": name, "link": link}))
                        .collect::<Vec<_>>(),
                    "back_link": breadcrumbs.iter().last().map(|(_, link)| link).unwrap(),
                    "documents": docs.into_iter().map(|(name, id, link)| json!({
                        "name": name,
                        "svg": self.doc_first_page(id),
                        "link": link,
                    })).collect::<Vec<_>>(),
                    "folders": sub_folders.into_iter().map(|(name, link)| json!({
                        "name": name,
                        "link": link,
                    })).collect::<Vec<_>>(),
                }),
                &folder_html_path,
            )
            .context("Rendering folder html")?;

        Ok(folder_link)
    }

    fn render_all_svgs(
	&self,
        material_root: &Path,
    ) -> Result<BTreeMap<Uuid, Vec<PathBuf>>> {
        let zip_dir = material_root.join("zip");

        let mut doc_svgs: BTreeMap<Uuid, Vec<PathBuf>> = Default::default();

        doc_svgs.insert(
            self.manifest.home.id,
            self.render_notebook_zip(
                self.manifest.home.id,
                &zip_dir.join(format!("{}.zip", self.manifest.home.id)),
                false,
            )
            .context("Rendering index svg")?,
        );

        doc_svgs.insert(
            self.manifest.logo.id,
            self.render_notebook_zip(
                self.manifest.logo.id,
                &zip_dir.join(format!("{}.zip", self.manifest.logo.id)),
                true,
            )
            .context("Rendering logo svg")?,
        );

        doc_svgs.extend(
            self.manifest
                .posts
                .docs()
                .par_iter()
                .map(|doc| {
                    Ok((
                        doc.id,
                        self.render_notebook_zip(
                            doc.id,
                            &zip_dir.join(format!("{}.zip", doc.id)),
                            false,
                        )
                        .context("Rendering document svg")?,
                    ))
                })
                .collect::<Result<Vec<_>>>()
                .context("Rendering at least one document")?,
        );

        Ok(doc_svgs)
    }

    fn render_notebook_zip(
	&self,
        id: Uuid,
        zip_path: &Path,
        auto_crop: bool,
    ) -> Result<Vec<PathBuf>> {
        let notebook_root = self.root.join("svg").join(format!("{}", id));
        std::fs::create_dir_all(&notebook_root).context("Creating notebook svg directory")?;

        let zip_file = std::fs::File::open(zip_path).context("Opening zip file")?;
        let mut zip = zip::ZipArchive::new(zip_file).context("Reading ZipArchive")?;
        let mut rendered_svgs = Vec::new();
        for i in 0..zip.len() {
            let mut file = zip
                .by_index(i)
                .context("Attempting to index into the zip files")?;
            if file.name().ends_with(".rm") {
                let lines =
                    lines_are_rusty::LinesData::parse(&mut file).context("Parsing .rm file")?;
                // file name has pattern <uuid>/<page-num>.rm, we just want the page-num.
                let page_number = file
                    .name()
                    .trim_start_matches(&format!("{}/", id))
                    .trim_end_matches(".rm");
                println!("Rendering {} p{} svg", id, page_number);

                let output_path = notebook_root.join(format!("{}.svg", page_number));
                let mut output =
                    std::fs::File::create(&output_path).context("Creating output file for svg")?;
                let debug = false;
                lines_are_rusty::render_svg(
                    &mut output,
                    &lines.pages[0],
                    auto_crop,
                    &Default::default(),
                    debug,
                )
                .context("Rendering document page svg")?;

                rendered_svgs.push(
                    self.prefix.join(
                        output_path
                            .strip_prefix(&self.root)
                            .context("Stripping site root form svg paths")?
                            .to_path_buf(),
                    ),
                );
            }
        }

        Ok(rendered_svgs)
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
