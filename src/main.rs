use std::collections::BTreeMap;
use std::io::Write;

use rayon::prelude::*;

use anyhow::{anyhow, Result};
use handlebars::Handlebars;
use remarkable_cloud_api::{reqwest, Client, ClientState, Document, Documents, Parent, Uuid};
use serde::{Deserialize, Serialize};
use serde_json::json;
use structopt::StructOpt;
use zip;

#[derive(Debug, Serialize, Deserialize)]
struct SiteConfig {
    site_root: String,
    title: String,
    css: std::path::PathBuf,
    templates: Templates,
}

#[derive(Debug, Serialize, Deserialize)]
struct Templates {
    index: std::path::PathBuf,
    document: std::path::PathBuf,
    folder: std::path::PathBuf,
}

impl Templates {
    fn build_handlebars(&self) -> Result<Handlebars> {
        let mut reg = Handlebars::new();
        reg.register_template_file("index", &self.index)?;
        reg.register_template_file("document", &self.document)?;
        reg.register_template_file("folder", &self.folder)?;
        Ok(reg)
    }
}

impl SiteConfig {
    fn load(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        // let reader = BufReader::new(file);
        let config = serde_json::from_reader(file)?;
        Ok(config)
    }
}

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(parse(from_os_str))]
    site_config_path: std::path::PathBuf,
    #[structopt(subcommand)]
    action: Action,
}

#[derive(Debug, StructOpt)]
#[structopt(about = "First fetch the raw site material from rM cloud, then generate the site")]
enum Action {
    Fetch {
        device_token: String,
        #[structopt(parse(from_os_str))]
        material_path: std::path::PathBuf,
    },
    Gen {
        #[structopt(parse(from_os_str))]
        material_path: std::path::PathBuf,
        #[structopt(parse(from_os_str))]
        build_path: std::path::PathBuf,
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
    fn doc_ids(&self) -> Vec<Uuid> {
        std::iter::once(self.index)
            .chain(std::iter::once(self.logo))
            .chain(self.posts.doc_ids())
            .collect()
    }
}

fn build_manifest<'a>(root_docs: &[&'a Document], all_docs: &'a Documents) -> Result<Manifest> {
    let index = find_index_nb(&root_docs)?.id;
    let logo = find_logo_nb(&root_docs)?.id;
    let posts = find_posts(&root_docs, &all_docs)?;
    Ok(Manifest { index, logo, posts })
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

async fn fetch(config: SiteConfig, client: Client, output_path: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(&output_path)?;
    let archives_dir = output_path.join("zip");
    std::fs::create_dir_all(&archives_dir)?;

    let documents = client.all_documents(false).await?;
    let root_nodes = documents.children(Parent::Root);
    let site_roots: Vec<_> = root_nodes
        .iter()
        .filter(|d| d.visible_name == config.site_root)
        .collect();

    if site_roots.len() != 1 {
        println!(
            "Make sure to have one folder named '{}' on your remarkable",
            config.site_root
        )
    }

    let site_root_id = site_roots[0].id;
    let site_root_docs = documents.children(Parent::Node(site_root_id));

    let manifest = build_manifest(&site_root_docs, &documents)?;
    let manifest_file = std::fs::File::create(&output_path.join("manifest.json"))?;
    serde_json::to_writer_pretty(manifest_file, &manifest)?;

    for doc_id in manifest.doc_ids() {
        let zip = client.download_zip(doc_id).await?;
        let bytes = zip.into_inner().into_inner();
        let mut file = std::fs::File::create(&archives_dir.join(format!("{}.zip", doc_id)))?;
        file.write_all(&bytes)?;
    }

    Ok(())
}

fn render_zip(
    id: Uuid,
    zip_path: &std::path::Path,
    svg_root_path: &std::path::Path,
    auto_crop: bool,
) -> Result<Vec<std::path::PathBuf>> {
    println!(
        "Rendering zip {:?} {:?} {:?} {:?}",
        id, zip_path, svg_root_path, auto_crop
    );
    let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    let mut rendered_svgs = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.name().ends_with(".rm") {
            let lines = lines_are_rusty::LinesData::parse(&mut file)?;
            // file name has pattern <uuid>/<page-num>.rm, we just want the page number.
            let page_number = file
                .name()
                .trim_start_matches(&format!("{}/", id))
                .trim_end_matches(".rm");

            let output_path = svg_root_path.join(format!("{}-{:0>3}.svg", id, page_number));
            let mut output = std::fs::File::create(&output_path)?;
            let debug = false;
            lines_are_rusty::render_svg(
                &mut output,
                &lines.pages[0],
                auto_crop,
                &Default::default(),
                debug,
            )?;
            rendered_svgs.push(output_path.to_path_buf());
        }
    }

    Ok(rendered_svgs)
}

fn sanitize(folder: &str) -> String {
    folder
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn gen_doc(
    config: &SiteConfig,
    handlebars: &Handlebars,
    manifest: &Manifest,
    root: &std::path::Path,
    breadcrumbs: &[(String, std::path::PathBuf)],
    parent: &std::path::Path,
    name: &str,
    id: Uuid,
    svgs: &BTreeMap<Uuid, Vec<std::path::PathBuf>>,
) -> Result<std::path::PathBuf> {
    let sanitized_name = sanitize(name);
    let doc_path = parent.join(format!("{}.html", sanitized_name));
    let doc_html = std::fs::File::create(&doc_path)?;

    handlebars.render_to_write(
        "document",
        &json!({
            "title": config.title,
            "name": name,
            "breadcrumbs": breadcrumbs
                .iter()
                .map(|(name, link)| json!({"name": name, "link": link}))
                .collect::<Vec<_>>(),
            "logo": svgs[&manifest.logo][0],
            "back_link": breadcrumbs.iter().last().map(|(_, link)| link).unwrap(),
            "pages": svgs[&id],
        }),
        doc_html,
    )?;

    Ok(std::path::PathBuf::from("/").join(doc_path.strip_prefix(root)?.to_path_buf()))
}

fn gen_folder(
    config: &SiteConfig,
    handlebars: &Handlebars,
    manifest: &Manifest,
    root: &std::path::Path,
    breadcrumbs: &[(String, std::path::PathBuf)],
    parent: &std::path::Path,
    folder: &str,
    posts: &Posts,
    svgs: &BTreeMap<Uuid, Vec<std::path::PathBuf>>,
) -> Result<std::path::PathBuf> {
    let sanitized_folder = sanitize(folder);
    let folder_path = parent.join(&sanitized_folder);
    std::fs::create_dir_all(&folder_path)?;

    let folder_html_path = parent.join(format!("{}.html", sanitized_folder));
    let folder_link =
        std::path::PathBuf::from("/").join(folder_html_path.strip_prefix(root)?.to_path_buf());

    let mut docs: Vec<(String, Uuid, std::path::PathBuf)> = Vec::new();
    let mut sub_folders: Vec<(String, std::path::PathBuf)> = Vec::new();

    let mut breadcrumbs_for_children = breadcrumbs.to_vec();
    breadcrumbs_for_children.push((folder.to_string(), folder_link.clone()));
    for (doc_name, doc_id) in posts.documents.iter() {
        let doc_path = gen_doc(
            config,
            handlebars,
            manifest,
            root,
            &breadcrumbs_for_children,
            &folder_path,
            doc_name,
            *doc_id,
            svgs,
        )?;
        docs.push((doc_name.to_string(), *doc_id, doc_path));
    }

    for (sub_folder_name, sub_folder_posts) in posts.folders.iter() {
        let sub_folder_path = gen_folder(
            config,
            handlebars,
            manifest,
            root,
            &breadcrumbs_for_children,
            &folder_path,
            sub_folder_name,
            sub_folder_posts,
            svgs,
        )?;
        sub_folders.push((sub_folder_name.to_string(), sub_folder_path));
    }

    let folder_html = std::fs::File::create(&folder_html_path)?;
    handlebars.render_to_write(
        "folder",
        &json!({
            "title": config.title,
            "name": folder,
            "logo": svgs[&manifest.logo][0],
            "breadcrumbs": breadcrumbs
                .iter()
                .map(|(name, link)| json!({"name": name, "link": link}))
                .collect::<Vec<_>>(),
            "back_link": breadcrumbs.iter().last().map(|(_, link)| link).unwrap(),
            "documents": docs.into_iter().map(|(name, id, link)| json!({
                "name": name,
                "svg": svgs[&id][0],
                "link": link,
            })).collect::<Vec<_>>(),
            "folders": sub_folders.into_iter().map(|(name, link)| json!({
                "name": name,
                "link": link,
            })).collect::<Vec<_>>(),
        }),
        folder_html,
    )?;

    Ok(folder_link)
}

fn gen_index(
    config: &SiteConfig,
    handlebars: &Handlebars,
    manifest: &Manifest,
    root: &std::path::Path,
    svgs: &BTreeMap<Uuid, Vec<std::path::PathBuf>>,
) -> Result<()> {
    let mut docs: Vec<(String, Uuid, std::path::PathBuf)> = Vec::new();
    let mut sub_folders: Vec<(String, std::path::PathBuf)> = Vec::new();

    let posts_path = root.join("posts");
    std::fs::create_dir_all(&posts_path)?;

    let breadcrumbs = &[("Home".to_string(), std::path::PathBuf::from("/index.html"))];
    for (doc_name, doc_id) in manifest.posts.documents.iter() {
        let doc_path = gen_doc(
            config,
            handlebars,
            manifest,
            root,
            breadcrumbs,
            &posts_path,
            doc_name,
            *doc_id,
            svgs,
        )?;
        docs.push((doc_name.to_string(), *doc_id, doc_path));
    }

    for (sub_folder_name, sub_folder_posts) in manifest.posts.folders.iter() {
        let sub_folder_path = gen_folder(
            config,
            handlebars,
            manifest,
            root,
            breadcrumbs,
            &posts_path,
            sub_folder_name,
            sub_folder_posts,
            svgs,
        )?;
        sub_folders.push((sub_folder_name.to_string(), sub_folder_path));
    }

    let index_html = std::fs::File::create(root.join("index.html"))?;
    handlebars.render_to_write(
        "index",
        &json!({
            "title": config.title,
            "logo": svgs[&manifest.logo][0],
            "name": "Home",
            "pages": svgs[&manifest.index],
            "documents": docs.into_iter().map(|(name, id, link)| json!({
                "name": name,
                "svg": svgs[&id][0],
                "link": link,
            })).collect::<Vec<_>>(),
            "folders": sub_folders.into_iter().map(|(name, link)| json!({
                "name": name,
                "link": link,
            })).collect::<Vec<_>>(),
        }),
        index_html,
    )?;
    Ok(())
}

async fn gen(
    config: SiteConfig,
    material_path: &std::path::Path,
    build_path: &std::path::Path,
) -> Result<()> {
    let zip_dir = &material_path.join("zip");
    let manifest_file = std::fs::File::open(&material_path.join("manifest.json"))?;
    let manifest: Manifest = serde_json::from_reader(manifest_file)?;
    println!("Loaded manifest {:#?}", manifest);

    let svg_root = build_path.join("svg");
    std::fs::create_dir_all(&build_path)?;
    std::fs::create_dir_all(&svg_root)?;

    let mut doc_svgs: BTreeMap<Uuid, Vec<std::path::PathBuf>> = Default::default();
    doc_svgs.insert(
        manifest.index,
        render_zip(
            manifest.index,
            &zip_dir.join(format!("{}.zip", manifest.index)),
            &svg_root,
            false,
        )?,
    );
    doc_svgs.insert(
        manifest.logo,
        render_zip(
            manifest.logo,
            &zip_dir.join(format!("{}.zip", manifest.logo)),
            &svg_root,
            true,
        )?,
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
                        &svg_root,
                        false,
                    )?,
                ))
            })
            .collect::<Result<Vec<_>>>()?,
    );

    // fix svg paths to be relative to build_path
    for (_, svgs) in doc_svgs.iter_mut() {
        for svg_path in svgs.iter_mut() {
            *svg_path = std::path::PathBuf::from("/")
                .join(svg_path.strip_prefix(build_path)?.to_path_buf());
        }
    }

    let handlebars = config.templates.build_handlebars()?;
    gen_index(&config, &handlebars, &manifest, &build_path, &doc_svgs)?;
    std::fs::copy(config.css, build_path.join("style.css"))?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::from_args();
    let site_config = SiteConfig::load(&opt.site_config_path)?;

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
                    .build()?,
            );
            client.refresh_state().await?;
            fetch(site_config, client, &material_path).await?;
        }
        Action::Gen {
            material_path,
            build_path,
        } => {
            gen(site_config, &material_path, &build_path).await?;
        }
    };
    Ok(())
}
