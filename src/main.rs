use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use anyhow::{anyhow, Context, Result};
use handlebars::Handlebars;
use remarkable_cloud_api::{reqwest, Client, ClientState, Document, Documents, Parent, Uuid};
use serde::{Deserialize, Serialize};
use serde_json::json;
use structopt::StructOpt;

#[derive(Debug, Serialize, Deserialize)]
struct SiteConfig {
    site_root: String,
    title: String,
    theme: String,
}

#[derive(Debug)]
struct Theme {
    handlebars: Handlebars<'static>,
    css: PathBuf,
}

impl Theme {
    fn load(theme: &Path) -> Result<Self> {
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

    fn render_index(&self, params: &handlebars::JsonValue, gen_root: &Path) -> Result<()> {
        let f_out =
            std::fs::File::create(&gen_root.join("index.html")).context("Creating index.html")?;
        self.handlebars
            .render_to_write("index", params, f_out)
            .context("Rendering index.html")?;
        Ok(())
    }

    fn render_document(&self, params: &handlebars::JsonValue, out: &Path) -> Result<()> {
        let f_out = std::fs::File::create(&out).context("Creating document file for rendering")?;
        self.handlebars
            .render_to_write("document", params, f_out)
            .context("Rendering document template")?;
        Ok(())
    }

    fn render_folder(&self, params: &handlebars::JsonValue, out: &Path) -> Result<()> {
        let f_out = std::fs::File::create(&out).context("Creating folder file for rendering")?;
        self.handlebars
            .render_to_write("folder", params, f_out)
            .context("Cendering folder template")?;
        Ok(())
    }

    fn render_css(&self, gen_root: &Path) -> Result<()> {
        std::fs::copy(&self.css, &gen_root.join("style.css"))
            .context("Copying theme css into generated site")?;
        Ok(())
    }
}

impl SiteConfig {
    fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).context("Opening config file")?;
        let config = serde_json::from_reader(file).context("Parsing config file")?;
        Ok(config)
    }

    fn theme(&self) -> Result<Theme> {
        let theme_dir = PathBuf::from("themes").join(&self.theme);
        Theme::load(&theme_dir)
    }
}

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(parse(from_os_str))]
    site_config_path: PathBuf,
    #[structopt(subcommand)]
    action: Action,
}

#[derive(Debug, StructOpt)]
#[structopt(about = "First fetch the raw site material from rM cloud, then generate the site")]
enum Action {
    Fetch {
        device_token: String,
        #[structopt(parse(from_os_str))]
        material_path: PathBuf,
    },
    Gen {
        #[structopt(parse(from_os_str))]
        material_path: PathBuf,
        #[structopt(parse(from_os_str))]
        build_path: PathBuf,
    },
}

fn find_index_nb<'a>(docs: &[&'a Document]) -> Result<&'a Document> {
    let mut matching_docs = docs
        .iter()
        .filter(|d| d.visible_name == "Index" && d.doc_type == "DocumentType");

    match (matching_docs.next(), matching_docs.next()) {
        (Some(index), None) => Ok(index),
        (None, None) => Err(anyhow!("Missing 'Index' notebook in site root")),
        (Some(a), Some(b)) => Err(anyhow!(
            "Multiple 'Index' notebooks in site root: {:?} {:?}",
            a,
            b
        )),
        (None, Some(_)) => panic!("Impossible!"),
    }
}

fn find_logo_nb<'a>(docs: &[&'a Document]) -> Result<&'a Document> {
    let mut matching_docs = docs
        .iter()
        .filter(|d| d.visible_name == "Logo" && d.doc_type == "DocumentType");

    match (matching_docs.next(), matching_docs.next()) {
        (Some(logo), None) => Ok(logo),
        (None, None) => Err(anyhow!("Missing 'Logo' notebook in site root")),
        (Some(_), Some(_)) => Err(anyhow!("Multiple 'Logo' notebooks in site root")),
        (None, Some(_)) => panic!("Impossible!"),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    index: Uuid,
    logo: Uuid,
    posts: Posts,
}

impl Manifest {
    fn build(site_root: String, docs: Documents) -> Result<Self> {
        let root_nodes = docs.children(Parent::Root);
        let site_roots: Vec<_> = root_nodes
            .iter()
            .filter(|d| d.visible_name == site_root)
            .collect();

        if site_roots.len() != 1 {
            return Err(anyhow!(
                "Make sure to have one folder named '{}' on your remarkable you are synced with rM cloud, found {} folders",
                site_root,
                site_roots.len()
            ));
        }

        let site_root_docs = docs.children(Parent::Node(site_roots[0].id));
        let index = find_index_nb(&site_root_docs)
            .context("Finding index notebook")?
            .id;
        let logo = find_logo_nb(&site_root_docs)
            .context("Finding logo notebook")?
            .id;
        let posts = find_posts(&site_root_docs, &docs).context("Finding Posts")?;

        Ok(Manifest { index, logo, posts })
    }

    fn load(material_root: &Path) -> Result<Self> {
        let manifest_file = std::fs::File::open(&material_root.join("manifest.json"))
            .context("Opening material manifest file")?;
        let manifest = serde_json::from_reader(manifest_file).context("Parsing manifest file")?;
        Ok(manifest)
    }

    fn save(&self, material_root: &Path) -> Result<()> {
        let manifest_file = std::fs::File::create(&material_root.join("manifest.json"))
            .context("Creating manifest file")?;
        serde_json::to_writer_pretty(manifest_file, &self).context("Writing manifest file")?;
        Ok(())
    }

    fn doc_ids(&self) -> Vec<Uuid> {
        std::iter::once(self.index)
            .chain(std::iter::once(self.logo))
            .chain(self.posts.doc_ids())
            .collect()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Posts {
    documents: BTreeMap<String, Uuid>,
    folders: BTreeMap<String, Posts>,
}

impl Posts {
    fn doc_ids(&self) -> Vec<Uuid> {
        self.documents
            .values()
            .copied()
            .chain(self.folders.values().flat_map(|f| f.doc_ids()))
            .collect()
    }
}

fn find_posts<'a>(root_docs: &[&'a Document], all_docs: &'a Documents) -> Result<Posts> {
    let mut matching_docs = root_docs
        .iter()
        .filter(|d| d.visible_name == "Posts" && d.doc_type == "CollectionType");

    let posts_folder = match (matching_docs.next(), matching_docs.next()) {
        (Some(posts_folder), None) => posts_folder,
        (None, None) => return Err(anyhow!("Missing 'Posts' folder in site root")),
        (Some(_), Some(_)) => return Err(anyhow!("Multiple 'Posts' folders in site root")),
        (None, Some(_)) => panic!("Impossible!"),
    };
    let posts = build_posts_hierarchy(posts_folder.id, all_docs);
    println!("{:#?}", posts);
    Ok(posts)
}

fn build_posts_hierarchy(folder: Uuid, all_docs: &Documents) -> Posts {
    let items = all_docs.children(Parent::Node(folder));
    let documents = items
        .iter()
        .filter(|d| d.doc_type == "DocumentType")
        .map(|d| (d.visible_name.clone(), d.id))
        .collect();
    let folders = items
        .iter()
        .filter(|d| d.doc_type == "CollectionType")
        .map(|d| {
            (
                d.visible_name.clone(),
                build_posts_hierarchy(d.id, all_docs),
            )
        })
        .collect();
    Posts { documents, folders }
}

async fn fetch(config: SiteConfig, client: Client, output_path: &Path) -> Result<()> {
    std::fs::create_dir_all(&output_path).context("Creating material output directory")?;

    let archives_dir = output_path.join("zip");
    std::fs::create_dir_all(&archives_dir).context("Creating zip archives directory")?;

    let documents = client
        .all_documents(false)
        .await
        .context("Fetching all document metadata from rM Cloud")?;

    let manifest =
        Manifest::build(config.site_root, documents).context("Building Manifest from documents")?;

    manifest
        .save(&output_path)
        .context("Saving the generated Manifest")?;

    for doc_id in manifest.doc_ids() {
        println!("Downloading {}", doc_id);
        let zip = client
            .download_zip(doc_id)
            .await
            .context("Downloading document zip")?;
        let bytes = zip.into_inner().into_inner();
        let mut file = std::fs::File::create(&archives_dir.join(format!("{}.zip", doc_id)))
            .context("Creating file for document zip")?;
        file.write_all(&bytes)
            .context("Writing document zip to disk")?;
    }

    Ok(())
}

fn render_all_svgs(
    manifest: &Manifest,
    material_root: &Path,
    site_root: &Path,
) -> Result<BTreeMap<Uuid, Vec<PathBuf>>> {
    let zip_dir = material_root.join("zip");

    let mut doc_svgs: BTreeMap<Uuid, Vec<PathBuf>> = Default::default();

    doc_svgs.insert(
        manifest.index,
        render_zip(
            manifest.index,
            &zip_dir.join(format!("{}.zip", manifest.index)),
            &site_root,
            false,
        )
        .context("Rendering index svg")?,
    );

    doc_svgs.insert(
        manifest.logo,
        render_zip(
            manifest.logo,
            &zip_dir.join(format!("{}.zip", manifest.logo)),
            &site_root,
            true,
        )
        .context("Rendering logo svg")?,
    );

    doc_svgs.extend(
        manifest
            .posts
            .doc_ids()
            .par_iter()
            .map(|doc_id| {
                Ok((
                    *doc_id,
                    render_zip(
                        *doc_id,
                        &zip_dir.join(format!("{}.zip", doc_id)),
                        &site_root,
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

fn render_zip(
    id: Uuid,
    zip_path: &Path,
    site_root: &Path,
    auto_crop: bool,
) -> Result<Vec<PathBuf>> {
    let svg_root = site_root.join("svg");
    let zip_file = std::fs::File::open(zip_path).context("Opening zip file")?;
    let mut zip = zip::ZipArchive::new(zip_file).context("Reading ZipArchive")?;
    let mut rendered_svgs = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip
            .by_index(i)
            .context("Attempting to index into the zip files")?;
        if file.name().ends_with(".rm") {
            let lines = lines_are_rusty::LinesData::parse(&mut file).context("Parsing .rm file")?;
            // file name has pattern <uuid>/<page-num>.rm, we just want the page-num.
            let page_number = file
                .name()
                .trim_start_matches(&format!("{}/", id))
                .trim_end_matches(".rm");
            println!("Rendering {} p{} svg", id, page_number);

            let output_path = svg_root.join(format!("{}-{}.svg", id, page_number));
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
                PathBuf::from("/").join(
                    output_path
                        .strip_prefix(site_root)
                        .context("Stripping site root form svg paths")?
                        .to_path_buf(),
                ),
            );
        }
    }

    Ok(rendered_svgs)
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

struct Site {
    root: PathBuf,
    theme: Theme,
    manifest: Manifest,
    config: SiteConfig,
    svgs: BTreeMap<Uuid, Vec<PathBuf>>, // Rendered notebook pages
}

impl Site {
    fn title(&self) -> &str {
        &self.config.title
    }

    fn logo_svg(&self) -> &Path {
        self.doc_first_page(self.manifest.logo)
    }

    fn index_pages(&self) -> &[PathBuf] {
        self.doc_pages(self.manifest.index)
    }

    fn doc_first_page(&self, id: Uuid) -> &Path {
        &self.doc_pages(id)[0]
    }

    /// Panics if Doc ID does not exist.
    fn doc_pages(&self, id: Uuid) -> &[PathBuf] {
        &self.svgs[&id]
    }

    fn relative_to_root(&self, path: &Path) -> Result<PathBuf> {
        Ok(PathBuf::from("/").join(
            path.strip_prefix(&self.root)
                .with_context(|| format!("Stripping root from path {:?}", path))?,
        ))
    }
}

fn gen_doc(
    site: &Site,
    breadcrumbs: &[(String, PathBuf)],
    parent: &Path,
    name: &str,
    id: Uuid,
) -> Result<PathBuf> {
    let sanitized_name = sanitize(name);
    // TODO: replace this with a breadcrumbs_to_path method on the Site
    let doc_path = parent.join(format!("{}.html", sanitized_name));

    site.theme
        .render_document(
            &json!({
                "title": site.title(),
                "name": name,
                "breadcrumbs": breadcrumbs
                    .iter()
                    .map(|(crumb, link)| json!({"name": crumb, "link": link}))
                    .collect::<Vec<_>>(),
                "logo": site.logo_svg(),
                "back_link": breadcrumbs.iter().last().map(|(_, link)| link).unwrap(),
                "pages": site.doc_pages(id),
            }),
            &doc_path,
        )
        .context("Rendering document html")?;

    site.relative_to_root(&doc_path)
}

fn gen_folder(
    site: &Site,
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
    let folder_link = site.relative_to_root(&folder_html_path)?;

    let mut docs: Vec<(String, Uuid, PathBuf)> = Vec::new();
    let mut sub_folders: Vec<(String, PathBuf)> = Vec::new();

    let mut breadcrumbs_for_children = breadcrumbs.to_vec();
    breadcrumbs_for_children.push((folder.to_string(), folder_link.clone()));
    for (doc_name, doc_id) in posts.documents.iter() {
        let doc_path = gen_doc(
            site,
            &breadcrumbs_for_children,
            &folder_path,
            doc_name,
            *doc_id,
        )
        .context("Generating a doc inside a folder")?;
        docs.push((doc_name.to_string(), *doc_id, doc_path));
    }

    for (sub_folder_name, sub_folder_posts) in posts.folders.iter() {
        let sub_folder_path = gen_folder(
            site,
            &breadcrumbs_for_children,
            &folder_path,
            sub_folder_name,
            sub_folder_posts,
        )
        .context("Generating a sub-folder inside a folder")?;
        sub_folders.push((sub_folder_name.to_string(), sub_folder_path));
    }

    site.theme
        .render_folder(
            &json!({
                "title": site.title(),
                "name": folder,
                "logo": site.logo_svg(),
                "breadcrumbs": breadcrumbs
                    .iter()
                    .map(|(name, link)| json!({"name": name, "link": link}))
                    .collect::<Vec<_>>(),
                "back_link": breadcrumbs.iter().last().map(|(_, link)| link).unwrap(),
                "documents": docs.into_iter().map(|(name, id, link)| json!({
                    "name": name,
                    "svg": site.doc_first_page(id),
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

fn gen_index(site: &Site) -> Result<()> {
    let mut docs: Vec<(String, Uuid, PathBuf)> = Vec::new();
    let mut sub_folders: Vec<(String, PathBuf)> = Vec::new();

    let posts_path = site.root.join("posts");
    std::fs::create_dir_all(&posts_path)
        .context("Creating posts directory in generated site root")?;

    let breadcrumbs = &[("Home".to_string(), PathBuf::from("/index.html"))];
    for (doc_name, doc_id) in site.manifest.posts.documents.iter() {
        let doc_path = gen_doc(site, breadcrumbs, &posts_path, doc_name, *doc_id)
            .context("Generating a top level document")?;
        docs.push((doc_name.to_string(), *doc_id, doc_path));
    }

    for (sub_folder_name, sub_folder_posts) in site.manifest.posts.folders.iter() {
        let sub_folder_path = gen_folder(
            site,
            breadcrumbs,
            &posts_path,
            sub_folder_name,
            sub_folder_posts,
        )
        .context("Generating a top-level folder")?;
        sub_folders.push((sub_folder_name.to_string(), sub_folder_path));
    }

    site.theme
        .render_index(
            &json!({
                "title": site.title(),
                "logo": site.logo_svg(),
                "name": "Home",
                "pages": site.index_pages(),
                "documents": docs.into_iter().map(|(name, id, link)| json!({
                    "name": name,
                    "svg": site.doc_first_page(id), // TODO: Site::doc_first_page(id)
                    "link": link,
                })).collect::<Vec<_>>(),
                "folders": sub_folders.into_iter().map(|(name, link)| json!({
                    "name": name,
                    "link": link,
                })).collect::<Vec<_>>(),
            }),
            &site.root,
        )
        .context("Rendering index.html")?;
    Ok(())
}

async fn gen(config: SiteConfig, material_path: PathBuf, root: PathBuf) -> Result<()> {
    let manifest = Manifest::load(&material_path).context("Loading manifest")?;
    println!("Loaded manifest {:#?}", manifest);

    let svg_root = root.join("svg");
    std::fs::create_dir_all(&root).context("creating the generated site directory")?;
    std::fs::create_dir_all(&svg_root).context("creating the generated site svg directory")?;

    let svgs = render_all_svgs(&manifest, &material_path, &root).context("Rendering svg's")?;

    let theme = config.theme().context("Loading theme from config")?;
    theme.render_css(&root).context("Rendering css")?;

    let site = Site {
        root,
        theme,
        manifest,
        config,
        svgs,
    };

    gen_index(&site).context("Generating index page")?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::from_args();
    let site_config = SiteConfig::load(&opt.site_config_path).context("Loading site config")?;

    match opt.action {
        Action::Fetch {
            device_token,
            material_path,
        } => {
            let mut client = Client::new(
                ClientState {
                    device_token,
                    ..Default::default()
                },
                reqwest::Client::builder()
                    .user_agent("rm-site-gen")
                    .build()
                    .context("Building reqwest client")?,
            );
            client
                .refresh_state()
                .await
                .context("Refreshing rM Cloud auth tokens")?;
            fetch(site_config, client, &material_path)
                .await
                .context("Fetching site data")?;
        }
        Action::Gen {
            material_path,
            build_path,
        } => {
            gen(site_config, material_path, build_path)
                .await
                .context("Generating site")?;
        }
    };
    Ok(())
}
