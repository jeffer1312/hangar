//! Catálogo de conectores do servidor (`~/.hangar/connectors.json`): privado, trocado de forma
//! atômica sob trava. Nenhuma operação daqui inicia processo.
use crate::error::AdminError;
use crate::manifest::Manifest;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(flatten)]
    pub manifest: Manifest,
    pub enabled: bool,
}

impl std::ops::Deref for Entry {
    type Target = Manifest;
    fn deref(&self) -> &Manifest { &self.manifest }
}

#[derive(Default, Serialize, Deserialize)]
struct CatalogFile {
    version: u32,
    connectors: Vec<Entry>,
}

pub struct Registry { path: PathBuf }

impl Registry {
    pub fn new(path: PathBuf) -> Self { Self { path } }

    /// O catálogo do servidor: `<home>/.hangar/connectors.json`.
    pub fn default_path(home: &Path) -> PathBuf { home.join(".hangar").join("connectors.json") }

    pub fn path(&self) -> &Path { &self.path }

    pub fn list(&self) -> Result<Vec<Entry>, AdminError> { Ok(self.read()?.connectors) }

    pub fn get(&self, id: &str) -> Result<Entry, AdminError> {
        self.read()?.connectors.into_iter().find(|entry| entry.id == id).ok_or_else(|| AdminError::not_found(id))
    }

    /// Cadastra. O mesmo manifesto de novo não muda nada (nem religa); um diferente com o mesmo
    /// `id` é recusado: trocar o comando é remover (o que para o processo) e cadastrar de novo.
    pub fn add(&self, manifest: Manifest) -> Result<Entry, AdminError> {
        self.update(|file| {
            if let Some(existing) = file.connectors.iter().find(|entry| entry.id == manifest.id) {
                return if existing.manifest == manifest {
                    Ok((existing.clone(), false))
                } else {
                    Err(AdminError::new("connector_exists", format!("já existe outro conector {:?}; remova antes de cadastrar outro", manifest.id)))
                };
            }
            let entry = Entry { manifest, enabled: true };
            file.connectors.push(entry.clone());
            file.connectors.sort_by(|a, b| a.id.cmp(&b.id));
            Ok((entry, true))
        })
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<Entry, AdminError> {
        self.update(|file| {
            let entry = file.connectors.iter_mut().find(|entry| entry.id == id).ok_or_else(|| AdminError::not_found(id))?;
            let changed = entry.enabled != enabled;
            entry.enabled = enabled;
            Ok((entry.clone(), changed))
        })
    }

    pub fn remove(&self, id: &str) -> Result<Entry, AdminError> {
        self.update(|file| {
            let index = file.connectors.iter().position(|entry| entry.id == id).ok_or_else(|| AdminError::not_found(id))?;
            Ok((file.connectors.remove(index), true))
        })
    }

    fn read(&self) -> Result<CatalogFile, AdminError> {
        let bytes = match hangar_core::fs::read_replaced(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(CatalogFile { version: FORMAT_VERSION, connectors: Vec::new() }),
            Err(error) => return Err(AdminError::io("leitura do catálogo", &error)),
        };
        let file: CatalogFile = serde_json::from_slice(&bytes)
            .map_err(|_| AdminError::new("registry_invalid", "catálogo de conectores ilegível"))?;
        if file.version != FORMAT_VERSION {
            return Err(AdminError::new("registry_invalid", format!("catálogo na versão {} desconhecida", file.version)));
        }
        Ok(file)
    }

    fn update<T>(&self, change: impl FnOnce(&mut CatalogFile) -> Result<(T, bool), AdminError>) -> Result<T, AdminError> {
        let parent = self.path.parent().ok_or_else(|| AdminError::new("registry_unavailable", "catálogo sem pasta"))?;
        std::fs::create_dir_all(parent).map_err(|error| AdminError::io("pasta do catálogo", &error))?;
        let _lock = lock(&self.path.with_extension("lock")).map_err(|error| AdminError::io("trava do catálogo", &error))?;
        let mut file = self.read()?;
        let (value, changed) = change(&mut file)?;
        if changed {
            let bytes = serde_json::to_vec_pretty(&file).map_err(|_| AdminError::new("registry_unavailable", "catálogo não serializou"))?;
            hangar_core::fs::atomic_write(&self.path, &bytes).map_err(|error| AdminError::io("gravação do catálogo", &error))?;
        }
        Ok(value)
    }
}

fn lock(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let file = options.open(path)?;
    file.lock()?;
    Ok(file)
}
