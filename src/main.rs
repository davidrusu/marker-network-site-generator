use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use async_recursion::async_recursion;
use remarkable_cloud_api::{reqwest, Client, ClientState, Parent, Uuid};
use structopt::StructOpt;

mod config;
mod generator;
mod manifest;
mod theme;

use config::Config;
use generator::Generator;
use manifest::Manifest;

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(parse(from_os_str))]
    config_path: PathBuf,
    #[structopt(long)]
    no_cache: bool,
    #[structopt(subcommand)]
    action: Action,
}

#[derive(Debug, StructOpt)]
#[structopt(about = "First fetch the raw site material from rM cloud, then generate the site")]
enum Action {
    Init {
        device_token: String,
        folder: String,
    },
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

async fn init(client: Client, folder_name: String, config_path: PathBuf) -> Result<()> {
    if let Some(config_parent) = config_path.parent() {
        std::fs::create_dir_all(&config_parent).context("Ensuring config path parent exists")?;
    }
    let docs = client
        .all_documents(false)
        .await
        .context("Fetching all document metadata from rM Cloud")?;

    let folders_with_given_name = docs
        .children(Parent::Root)
        .into_iter()
        .filter(|d| d.visible_name == folder_name && d.doc_type == "CollectionType")
        .count();

    if folders_with_given_name > 0 {
        return Err(anyhow!(
            "Choose a unique folder name:  {} folder(s) with the name '{}'",
            folders_with_given_name,
            folder_name
        ));
    }
    println!("Creating folder on remarkable {:?}", folder_name);
    let folder_id = client
        .create_folder(Uuid::new_v4(), folder_name.clone(), Parent::Root)
        .await
        .context("Creating site folder on remarkable")?;

    upload_directory(&client, &Path::new("starter"), folder_id).await?;

    let config = Config {
        site_root: folder_id.to_string(),
        title: folder_name,
        theme: "marker".to_string(),
    };

    println!("Saving config file");
    config.save(&config_path).context("Saving config")?;

    Ok(())
}

#[async_recursion]
async fn upload_directory(client: &Client, dir: &Path, rm_folder_id: Uuid) -> Result<()> {
    println!("Uploading {:?}", dir);
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let folder_name = path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .map(String::from)
                .ok_or_else(|| anyhow!("Couldn't get sub folder file name: {:?}", path))?;
            let sub_folder_id = client
                .create_folder(Uuid::new_v4(), folder_name, Parent::Node(rm_folder_id))
                .await
                .context("Creating folder on remarkable")?;

            upload_directory(&client, &path, sub_folder_id).await?;
        } else {
            assert_eq!(
                path.extension().and_then(std::ffi::OsStr::to_str),
                Some("zip")
            );

            let notebook_name = path
                .file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .map(String::from)
                .ok_or_else(|| anyhow!("Zip file has no name: {:?}", path))?;

            let zip_file = std::fs::File::open(path).context("Opening notebook zip file")?;
            let mut zip = zip::ZipArchive::new(zip_file).context("Reading ZipArchive")?;
            client
                .upload_notebook(
                    Uuid::new_v4(),
                    notebook_name,
                    Parent::Node(rm_folder_id),
                    &mut zip,
                )
                .await
                .context("Creating notebook on remarkable")?;
        }
    }
    Ok(())
}

async fn fetch(config: Config, client: Client, output_path: &Path) -> Result<()> {
    std::fs::create_dir_all(&output_path).context("Creating material output directory")?;

    let archives_dir = output_path.join("zip");
    std::fs::create_dir_all(&archives_dir).context("Creating zip archives directory")?;

    let documents = client
        .all_documents(false)
        .await
        .context("Fetching all document metadata from rM Cloud")?;

    let existing_docs: BTreeMap<Uuid, manifest::DocumentMeta> =
        if let Ok(existing_manifest) = Manifest::load(&output_path).context("Loading manifest") {
            existing_manifest
                .docs()
                .into_iter()
                .map(|d| (d.id, d.clone()))
                .collect()
        } else {
            Default::default()
        };

    let manifest =
        Manifest::build(config.site_root, documents).context("Building Manifest from documents")?;

    for doc in manifest.docs() {
        if let Some(existing_doc) = existing_docs.get(&doc.id) {
            if existing_doc.modified_client >= doc.modified_client {
                println!("Nothing new from {}", doc.id);
                continue;
            }
        }
        println!("Downloading {}", doc.id);

        let zip = client
            .download_zip(doc.id)
            .await
            .context("Downloading document zip")?;
        let bytes = zip.into_inner().into_inner();
        let mut file = std::fs::File::create(&archives_dir.join(format!("{}.zip", doc.id)))
            .context("Creating file for document zip")?;
        file.write_all(&bytes)
            .context("Writing document zip to disk")?;
    }

    manifest
        .save(&output_path)
        .context("Saving the generated Manifest")?;

    Ok(())
}

async fn build_rm_client(device_token: String) -> Result<Client> {
    let mut client = Client::new(
        ClientState {
            device_token,
            ..Default::default()
        },
        reqwest::Client::builder()
            .user_agent("marker-network-site-generator-cli")
            .build()
            .context("Building reqwest client")?,
    );

    client
        .refresh_state()
        .await
        .context("Refreshing rM Cloud auth tokens")?;

    Ok(client)
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::from_args();

    match opt.action {
        Action::Init {
            device_token,
            folder,
        } => {
            let client = build_rm_client(device_token)
                .await
                .context("Building rM Client")?;
            init(client, folder, opt.config_path)
                .await
                .context("Initializing site")?;
        }
        Action::Fetch {
            device_token,
            material_path,
        } => {
            let config = Config::load(&opt.config_path).context("Loading site config")?;
            let client = build_rm_client(device_token)
                .await
                .context("Building rM Client")?;
            fetch(config, client, &material_path)
                .await
                .context("Fetching site data")?;
        }
        Action::Gen {
            material_path,
            build_path,
        } => {
            let config = Config::load(&opt.config_path).context("Loading site config")?;
            let generator = Generator::prepare(
                config,
                material_path,
                build_path,
                PathBuf::from("/"),
                opt.no_cache,
            )
            .context("Preparing to generate site")?;

            generator.gen_index().context("Generating site")?;
        }
    };
    Ok(())
}
