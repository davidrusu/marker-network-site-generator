use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use remarkable_cloud_api::{Documents, Parent, Uuid};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub home: Uuid,
    pub logo: Uuid,
    pub posts: Posts,
}

impl Manifest {
    pub fn build(root_folder: String, docs: Documents) -> Result<Self> {
        let site_root = if let Ok(id) = Uuid::parse_str(&root_folder) {
            let root_doc = docs
                .get(&id)
                .ok_or_else(|| anyhow!("No document with ID {}", id))?;

            if root_doc.doc_type != "CollectionType" {
                return Err(anyhow!("Site root must be a folder: {}", root_doc.doc_type));
            }

            root_doc.to_owned()
        } else {
            let root_nodes = docs.children(Parent::Root);
            let mut site_roots: Vec<_> = root_nodes
                .iter()
                .filter(|d| d.doc_type == "CollectionType")
                .filter(|d| d.visible_name == root_folder)
                .collect();

            if site_roots.len() != 1 {
                return Err(anyhow!(
                "Make sure to have one folder named '{}' on your remarkable. And make sure you are synced with rM cloud, found {} folders",
                root_folder,
                site_roots.len()
            ));
            }

            site_roots.pop().unwrap().to_owned()
        };

        let home = Self::root_doc_by_name("Home", site_root.id, &docs)
            .context("Looking for 'Home' notebook")?;
        let logo = Self::root_doc_by_name("Logo", site_root.id, &docs)
            .context("Looking for 'Logo' notebook")?;
        let posts = Posts::build(site_root.id, &docs).context("Looking for 'Posts' folder")?;

        Ok(Manifest { home, logo, posts })
    }

    pub fn load(material_root: &Path) -> Result<Self> {
        let manifest_file = std::fs::File::open(&material_root.join("manifest.json"))
            .context("Opening material manifest file")?;
        let manifest = serde_json::from_reader(manifest_file).context("Parsing manifest file")?;
        Ok(manifest)
    }

    pub fn save(&self, material_root: &Path) -> Result<()> {
        let manifest_file = std::fs::File::create(&material_root.join("manifest.json"))
            .context("Creating manifest file")?;
        serde_json::to_writer_pretty(manifest_file, &self).context("Writing manifest file")?;
        Ok(())
    }

    pub fn doc_ids(&self) -> Vec<Uuid> {
        std::iter::once(self.home)
            .chain(std::iter::once(self.logo))
            .chain(self.posts.doc_ids())
            .collect()
    }

    fn root_doc_by_name(doc_name: &str, root_id: Uuid, docs: &Documents) -> Result<Uuid> {
        let mut matching_docs = docs
            .children(Parent::Node(root_id))
            .into_iter()
            .filter(|d| d.visible_name == doc_name && d.doc_type == "DocumentType");

        match (matching_docs.next(), matching_docs.next()) {
            (Some(doc), None) => Ok(doc.id),
            (None, None) => Err(anyhow!("Missing '{}' notebook in site root", doc_name)),
            (Some(_), Some(_)) => Err(anyhow!("Multiple '{}' notebooks in site root", doc_name)),
            (None, Some(_)) => panic!("Impossible!"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Posts {
    pub documents: BTreeMap<String, Uuid>,
    pub folders: BTreeMap<String, Posts>,
}

impl Posts {
    pub fn doc_ids(&self) -> Vec<Uuid> {
        self.documents
            .values()
            .copied()
            .chain(self.folders.values().flat_map(|f| f.doc_ids()))
            .collect()
    }

    fn build(root_id: Uuid, docs: &Documents) -> Result<Posts> {
        let mut matching_docs = docs
            .children(Parent::Node(root_id))
            .into_iter()
            .filter(|d| d.visible_name == "Posts" && d.doc_type == "CollectionType");

        let posts_folder = match (matching_docs.next(), matching_docs.next()) {
            (Some(posts_folder), None) => posts_folder,
            (None, None) => return Err(anyhow!("Missing 'Posts' folder in site root")),
            (Some(_), Some(_)) => return Err(anyhow!("Multiple 'Posts' folders in site root")),
            (None, Some(_)) => panic!("Impossible!"),
        };

        let posts = Self::build_posts_hierarchy(posts_folder.id, docs);
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
                    Self::build_posts_hierarchy(d.id, all_docs),
                )
            })
            .collect();
        Posts { documents, folders }
    }
}
