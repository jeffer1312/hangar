//! Persistência atômica do catálogo `<hangar_dir>/integrations/config.json`.

use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use hangar_core::fs::{chmod, lock_file, read_replaced, replace};

use crate::model::{
    AuthConfig, IntegrationConfig, PermissionPolicy, Transport, ValidationError, valid_tool_name,
};

const FORMAT_VERSION: u32 = 1;
static TEMP_WRITE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Erros de leitura e escrita do catálogo de integrações.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    Validation(ValidationError),
    AlreadyExists(String),
    NotFound(String),
    Corrupted(String),
    Io(String),
}

/// Campos opcionais para atualização parcial de uma integração existente.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntegrationUpdate {
    pub name: Option<String>,
    pub preset: Option<Option<String>>,
    pub enabled: Option<bool>,
    pub transport: Option<Transport>,
    pub auth: Option<AuthConfig>,
    pub policy: Option<PermissionPolicy>,
    pub pinned: Option<bool>,
    pub projects: Option<Vec<String>>,
    pub allowed_tools: Option<BTreeSet<String>>,
    pub updated_at: Option<f64>,
}

/// Catálogo persistido em `<hangar_dir>/integrations/config.json` sob trava exclusiva.
pub struct IntegrationRegistry {
    path: PathBuf,
    lock: Mutex<()>,
}

#[derive(Serialize, Deserialize)]
struct RegistryFile {
    version: u32,
    integrations: Vec<IntegrationConfig>,
}

impl IntegrationRegistry {
    /// Cria o manipulador de catálogo apontando para `<hangar_dir>/integrations/config.json`.
    pub fn new(hangar_dir: impl AsRef<Path>) -> Self {
        Self {
            path: config_path(hangar_dir.as_ref()),
            lock: Mutex::new(()),
        }
    }

    /// Retorna o caminho do arquivo `config.json`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Carrega todas as integrações cadastradas, ordenadas por `id`.
    ///
    /// # Errors
    ///
    /// Retorna [`RegistryError::Corrupted`] se `config.json` existir ilegível ou com versão
    /// desconhecida, ou [`RegistryError::Io`] em falhas de leitura do disco.
    pub fn load(&self) -> Result<Vec<IntegrationConfig>, RegistryError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| RegistryError::Io("trava interna indisponível".into()))?;
        let file = self.read_unlocked()?;
        Ok(file.integrations)
    }

    /// Lista as integrações cadastradas em ordem determinística por `id`.
    ///
    /// # Errors
    ///
    /// Propaga os erros de [`Self::load`].
    pub fn list(&self) -> Result<Vec<IntegrationConfig>, RegistryError> {
        self.load()
    }

    /// Busca uma integração pelo `id` sem alterar o catálogo.
    ///
    /// # Errors
    ///
    /// Propaga os erros de leitura ou corrupção de `config.json`.
    pub fn get(&self, id: &str) -> Result<Option<IntegrationConfig>, RegistryError> {
        let integrations = self.load()?;
        Ok(integrations.into_iter().find(|entry| entry.id == id))
    }

    /// Cadastra uma nova integração validada e persiste atomicamente em `config.json`.
    ///
    /// # Errors
    ///
    /// Retorna [`RegistryError::Validation`] se o registro for inválido,
    /// [`RegistryError::AlreadyExists`] se o `id` já existir, ou erros de corrupção/E/S.
    pub fn create(&self, mut config: IntegrationConfig) -> Result<IntegrationConfig, RegistryError> {
        config.normalize();
        config.validate()?;
        self.mutate(|file| {
            if file.integrations.iter().any(|entry| entry.id == config.id) {
                return Err(RegistryError::AlreadyExists(config.id.clone()));
            }
            file.integrations.push(config.clone());
            Ok((config, true))
        })
    }

    /// Atualiza os campos informados de uma integração existente e valida o resultado antes de gravar.
    ///
    /// # Errors
    ///
    /// Retorna [`RegistryError::NotFound`] se o `id` não existir, [`RegistryError::Validation`]
    /// se o estado atualizado violar regras de domínio, ou erros de corrupção/E/S.
    pub fn update(
        &self,
        id: &str,
        change: IntegrationUpdate,
    ) -> Result<IntegrationConfig, RegistryError> {
        self.mutate(|file| {
            let entry = file
                .integrations
                .iter_mut()
                .find(|entry| entry.id == id)
                .ok_or_else(|| RegistryError::NotFound(id.to_owned()))?;
            let mut next = entry.clone();
            if let Some(name) = change.name {
                next.name = name;
            }
            if let Some(preset) = change.preset {
                next.preset = preset;
            }
            if let Some(enabled) = change.enabled {
                next.enabled = enabled;
            }
            let mut destination_changed = false;
            if let Some(transport) = change.transport {
                destination_changed = transport_destination_changed(&entry.transport, &transport);
                next.transport = transport;
            }
            if let Some(auth) = change.auth {
                next.auth = auth;
            }
            if let Some(policy) = change.policy {
                next.policy = policy;
            }
            if let Some(pinned) = change.pinned {
                next.pinned = pinned;
            }
            if let Some(projects) = change.projects {
                next.projects = projects;
            }
            if let Some(allowed_tools) = change.allowed_tools {
                next.allowed_tools = allowed_tools;
            } else if destination_changed {
                // Troca de destino não herda liberações permanentes do servidor/comando anterior.
                next.allowed_tools.clear();
            }
            next.normalize();
            next.updated_at = change.updated_at.unwrap_or_else(now_secs);
            next.validate()?;
            let changed = *entry != next;
            *entry = next.clone();
            Ok((next, changed))
        })
    }

    /// Aplica um delta atômico em `allowed_tools` sob a trava do catálogo.
    ///
    /// Remove primeiro os itens de `remove` e depois insere os de `add`, de forma determinística
    /// caso uma mesma ferramenta apareça nas duas listas.
    ///
    /// # Errors
    ///
    /// Retorna [`RegistryError::Validation`] se algum nome em `add` for inválido,
    /// [`RegistryError::NotFound`] se a integração não existir, ou erros de corrupção/E/S.
    pub fn patch_allowed_tools(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<IntegrationConfig, RegistryError> {
        if add.iter().any(|tool| !valid_tool_name(tool)) {
            return Err(RegistryError::Validation(
                ValidationError::InvalidAllowedTool,
            ));
        }
        self.mutate(|file| {
            let entry = file
                .integrations
                .iter_mut()
                .find(|entry| entry.id == id)
                .ok_or_else(|| RegistryError::NotFound(id.to_owned()))?;
            let before = entry.allowed_tools.clone();
            for tool in remove {
                entry.allowed_tools.remove(tool.as_str());
            }
            for tool in add {
                entry.allowed_tools.insert(tool.clone());
            }
            let changed = entry.allowed_tools != before;
            if changed {
                entry.updated_at = now_secs();
            }
            Ok((entry.clone(), changed))
        })
    }

    /// Libera permanentemente uma ferramenta na integração (`allowed_tools`).
    ///
    /// # Errors
    ///
    /// Retorna [`RegistryError::Validation`] se `tool_name` for inválido,
    /// [`RegistryError::NotFound`] se a integração não existir, ou erros de corrupção/E/S.
    pub fn add_allowed_tool(
        &self,
        id: &str,
        tool_name: &str,
    ) -> Result<IntegrationConfig, RegistryError> {
        if !valid_tool_name(tool_name) {
            return Err(RegistryError::Validation(
                ValidationError::InvalidAllowedTool,
            ));
        }
        let tool = tool_name.to_owned();
        self.mutate(|file| {
            let entry = file
                .integrations
                .iter_mut()
                .find(|entry| entry.id == id)
                .ok_or_else(|| RegistryError::NotFound(id.to_owned()))?;
            let inserted = entry.allowed_tools.insert(tool);
            if inserted {
                entry.updated_at = now_secs();
            }
            Ok((entry.clone(), inserted))
        })
    }

    /// Remove uma ferramenta da lista `allowed_tools` da integração.
    ///
    /// # Errors
    ///
    /// Retorna [`RegistryError::NotFound`] se a integração não existir, ou erros de corrupção/E/S.
    pub fn remove_allowed_tool(
        &self,
        id: &str,
        tool_name: &str,
    ) -> Result<IntegrationConfig, RegistryError> {
        self.mutate(|file| {
            let entry = file
                .integrations
                .iter_mut()
                .find(|entry| entry.id == id)
                .ok_or_else(|| RegistryError::NotFound(id.to_owned()))?;
            let removed = entry.allowed_tools.remove(tool_name);
            if removed {
                entry.updated_at = now_secs();
            }
            Ok((entry.clone(), removed))
        })
    }

    /// Remove uma integração cadastrada pelo `id`.
    ///
    /// # Errors
    ///
    /// Retorna [`RegistryError::NotFound`] se o `id` não existir, ou erros de corrupção/E/S.
    pub fn delete(&self, id: &str) -> Result<IntegrationConfig, RegistryError> {
        self.mutate(|file| {
            let index = file
                .integrations
                .iter()
                .position(|entry| entry.id == id)
                .ok_or_else(|| RegistryError::NotFound(id.to_owned()))?;
            Ok((file.integrations.remove(index), true))
        })
    }

    fn read_unlocked(&self) -> Result<RegistryFile, RegistryError> {
        let bytes = match read_replaced(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(RegistryFile {
                    version: FORMAT_VERSION,
                    integrations: Vec::new(),
                });
            }
            Err(error) => return Err(RegistryError::Io(format!("{:?}", error.kind()))),
        };
        let mut file: RegistryFile = serde_json::from_slice(&bytes)
            .map_err(|error| RegistryError::Corrupted(format!("JSON inválido: {error}")))?;
        if file.version != FORMAT_VERSION {
            return Err(RegistryError::Corrupted(format!(
                "versão {} desconhecida",
                file.version
            )));
        }
        let mut seen = HashSet::with_capacity(file.integrations.len());
        for entry in &file.integrations {
            entry.validate().map_err(|error| {
                RegistryError::Corrupted(format!("integração '{}' inválida: {error}", entry.id))
            })?;
            if !seen.insert(entry.id.as_str()) {
                return Err(RegistryError::Corrupted(format!(
                    "id duplicado '{}' no catálogo",
                    entry.id
                )));
            }
        }
        file.integrations
            .sort_by(|left, right| left.id.cmp(&right.id));
        Ok(file)
    }

    fn mutate<T>(
        &self,
        change: impl FnOnce(&mut RegistryFile) -> Result<(T, bool), RegistryError>,
    ) -> Result<T, RegistryError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| RegistryError::Io("trava interna indisponível".into()))?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| RegistryError::Io("catálogo sem pasta pai".into()))?;
        std::fs::create_dir_all(parent)
            .map_err(|error| RegistryError::Io(format!("criar pasta: {:?}", error.kind())))?;
        chmod(parent, 0o700).map_err(|error| {
            RegistryError::Io(format!("permissão da pasta: {:?}", error.kind()))
        })?;
        let _file_lock = lock_file(&self.path.with_extension("lock"))
            .map_err(|error| RegistryError::Io(format!("trava do catálogo: {:?}", error.kind())))?;

        // Se o arquivo existir corrompido, falhamos antes de tocar nele para nunca apagar dados do usuário.
        let mut file = self.read_unlocked()?;
        let (value, changed) = change(&mut file)?;
        if changed {
            file.integrations
                .sort_by(|left, right| left.id.cmp(&right.id));
            let bytes = serde_json::to_vec_pretty(&file)
                .map_err(|error| RegistryError::Io(format!("serialização: {error}")))?;
            write_private_atomic(&self.path, &bytes).map_err(|error| {
                RegistryError::Io(format!("gravação atômica: {:?}", error.kind()))
            })?;
            chmod(&self.path, 0o600).map_err(|error| {
                RegistryError::Io(format!("permissão do arquivo: {:?}", error.kind()))
            })?;
        }
        Ok(value)
    }
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => write!(f, "{error}"),
            Self::AlreadyExists(id) => write!(f, "integração '{id}' já existe"),
            Self::NotFound(id) => write!(f, "integração '{id}' não encontrada"),
            Self::Corrupted(reason) => write!(f, "catálogo de integrações corrompido: {reason}"),
            Self::Io(reason) => write!(f, "erro de E/S no catálogo de integrações: {reason}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<ValidationError> for RegistryError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

/// Retorna `<hangar_dir>/integrations/config.json`.
pub fn config_path(hangar_dir: &Path) -> PathBuf {
    hangar_dir.join("integrations").join("config.json")
}

fn transport_destination_changed(old: &Transport, new: &Transport) -> bool {
    match (old, new) {
        (
            Transport::RemoteHttp { url: old_url, .. },
            Transport::RemoteHttp { url: new_url, .. },
        ) => old_url != new_url,
        (
            Transport::LocalStdio {
                command: old_cmd,
                cwd: old_cwd,
                ..
            },
            Transport::LocalStdio {
                command: new_cmd,
                cwd: new_cwd,
                ..
            },
        ) => old_cmd != new_cmd || old_cwd != new_cwd,
        _ => true,
    }
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "arquivo sem pasta pai"))?;
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = TEMP_WRITE_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".config-{}-{tick}-{seq}.tmp", std::process::id()));
    let mut created = false;
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        created = true;
        // Garante modo 0o600 no arquivo temporário antes do rename, preservando o arquivo atual em erro.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
        drop(file);
        replace(&temp, path)?;
        #[cfg(unix)]
        {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if created {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}
