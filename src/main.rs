use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
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

    println!("Created folder {:?}", folder_id);

    println!("Creating Posts folder {:?}/Posts", folder_name);
    let posts_folder_id = client
        .create_folder(Uuid::new_v4(), "Posts".to_string(), Parent::Node(folder_id))
        .await
        .context("Creating Posts folder on remarkable")?;
    println!("Created Posts folder {:?}", posts_folder_id);

    println!("Uploading Home starter {:?}/Home", folder_name);
    let starter_path = Path::new("starter");
    let home_zip_file =
        std::fs::File::open(starter_path.join("Home.zip")).context("Opening Home zip file")?;
    let mut zip = zip::ZipArchive::new(home_zip_file).context("Reading Home ZipArchive")?;

    client
        .upload_notebook(
            Uuid::new_v4(),
            "Home".to_string(),
            Parent::Node(folder_id),
            &mut zip,
        )
        .await
        .context("Creating Home notebook on remarkable")?;
    println!("Created Home notebook {:?}", posts_folder_id);

    println!("Uploading Logo starter {:?}/Logo", folder_name);
    let logo_zip_file =
        std::fs::File::open(starter_path.join("Logo.zip")).context("Opening Logo zip file")?;
    let mut zip = zip::ZipArchive::new(logo_zip_file).context("Reading Logo ZipArchive")?;

    client
        .upload_notebook(
            Uuid::new_v4(),
            "Logo".to_string(),
            Parent::Node(folder_id),
            &mut zip,
        )
        .await
        .context("Creating Logo notebook on remarkable")?;
    println!("Created Logo notebook {:?}", posts_folder_id);

    println!("Uploading Sarmale starter {:?}/Posts/Sarmale", folder_name);
    let sarmale_zip_file = std::fs::File::open(starter_path.join("Posts").join("Sarmale.zip"))
        .context("Opening Sarmale zip file")?;
    let mut zip = zip::ZipArchive::new(sarmale_zip_file).context("Reading Sarmale ZipArchive")?;

    client
        .upload_notebook(
            Uuid::new_v4(),
            "Sarmale (Cabbage Rolls)".to_string(),
            Parent::Node(posts_folder_id),
            &mut zip,
        )
        .await
        .context("Creating Sarmale notebook on remarkable")?;
    println!("Created Sarmale notebook {:?}", posts_folder_id);

    let config = Config {
        site_root: folder_id.to_string(),
        title: folder_name,
        theme: "marker".to_string(),
    };

    println!("Saving config file");
    config.save(&config_path).context("Saving config")?;

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
            let generator =
                Generator::prepare(config, material_path, build_path, PathBuf::from("/"))
                    .context("Preparing to generate site")?;

            generator.gen_index().context("Generating site")?;
        }
    };
    Ok(())
}
