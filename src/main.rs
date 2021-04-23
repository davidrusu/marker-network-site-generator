use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use remarkable_cloud_api::{reqwest, Client, ClientState};
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

async fn fetch(config: Config, client: Client, output_path: &Path) -> Result<()> {
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

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::from_args();
    let config = Config::load(&opt.config_path).context("Loading site config")?;

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
            fetch(config, client, &material_path)
                .await
                .context("Fetching site data")?;
        }
        Action::Gen {
            material_path,
            build_path,
        } => {
            let generator = Generator::prepare(config, material_path, build_path)
                .context("Preparing to generate site")?;

            generator.gen_index().context("Generating site")?;
        }
    };
    Ok(())
}
