//! Grupos de sessões (pareamento): arquivos em `.hangar-pair` e as regras sobre eles.
pub mod bridge;
pub mod deliver;
pub mod exit;
pub mod legacy;
pub mod local;
pub mod model;
pub mod orq;
pub mod peers;
pub mod routes;
pub mod service;
pub mod store;
pub mod sweep;

use std::path::PathBuf;
use std::sync::Arc;

use crate::list::discover_other::Dirs;
use service::{GroupService, OrqFacts};
use store::PairDir;

/// Serviço do processo sobre o `.hangar-pair` da conta padrão (`settings.projects_dir.parent`, a
/// pasta `claude` da lista). Arquivo de contratos e id da máquina vêm do Python
/// (`HANGAR_PAIR_ARCHIVE`, `HANGAR_SERVER_ID`); sem as pastas da lista não há serviço.
pub fn from_env(dirs: Option<&Dirs>, orq: Arc<dyn OrqFacts>, list: Arc<crate::list::bridge::ListBridge>) -> Option<Arc<GroupService>> {
    let dirs = dirs?;
    let var = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    // Mesmo padrão do `pair._arquivo_dir`: o cofre `~/.hangar`, não a conta.
    let archive = var("HANGAR_PAIR_ARCHIVE").map(PathBuf::from).unwrap_or_else(|| dirs.home.join(".hangar").join("pair-arquivo"));
    let dir = PairDir::new(dirs.claude.join(".hangar-pair"), archive);
    // Mudou o grupo: a lista relê o disco na hora, em vez de servir a descoberta de até 1 s atrás.
    let service = GroupService::new(dir, orq, var("HANGAR_SERVER_ID").unwrap_or_default());
    Some(Arc::new(service.with_change_hook(Arc::new(move || list.invalidate()))))
}

/// Outras máquinas pelo `peers.json` que o Python indica (`HANGAR_PEERS_FILE`); sem ele, nenhuma.
pub fn peers_from_env() -> peers::PeerClient {
    let path = std::env::var_os("HANGAR_PEERS_FILE").filter(|v| !v.is_empty()).map(PathBuf::from);
    peers::PeerClient::new(peers::PeerBook::new(path))
}
