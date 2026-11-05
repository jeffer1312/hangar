//! Grupos de sessões (pareamento): arquivos em `.hangar-pair` e as regras sobre eles.
pub mod bridge;
pub mod context_cmd;
pub mod convert;
pub mod exit;
pub mod fed_exit;
pub mod fed_notice;
pub mod federation;
pub mod legacy;
pub mod link_return;
pub mod link_state;
pub mod local;
pub mod model;
pub mod orq;
pub mod outbox;
pub mod peers;
pub mod routes;
pub mod service;
pub mod store;
pub mod sweep;
pub mod text;

use std::path::PathBuf;
use std::sync::Arc;

use crate::list::discover_other::Dirs;
use service::{GroupService, OrqFacts};
use store::PairDir;

/// Serviço do processo sobre o `.hangar-pair` da conta padrão (`settings.projects_dir.parent`, a
/// pasta `claude` da lista). Arquivo de contratos vem do Python (`HANGAR_PAIR_ARCHIVE`) e o id da
/// máquina de quem monta o `AppState` (o `main` lê `HANGAR_SERVER_ID`); sem as pastas da lista não há serviço.
pub fn from_env(dirs: Option<&Dirs>, server_id: String, orq: Arc<dyn OrqFacts>, list: Arc<crate::list::bridge::ListBridge>, diary: crate::diag::DiagClient) -> Option<Arc<GroupService>> {
    let dirs = dirs?;
    let var = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    // Mesmo padrão do `pair._arquivo_dir`: o cofre `~/.hangar`, não a conta.
    let archive = var("HANGAR_PAIR_ARCHIVE").map(PathBuf::from).unwrap_or_else(|| dirs.home.join(".hangar").join("pair-arquivo"));
    let dir = PairDir::new(dirs.claude.join(".hangar-pair"), archive);
    // As outras máquinas guardam esta pelo id antigo; quem recusa as operações é quem as faz.
    match dir.identity_check(&server_id) {
        Ok(None) => {}
        Ok(Some(changed)) => tracing::error!(code = "erro_grupo_identificador_trocado", saved = %changed.saved, current = %changed.current,
            "groups: CP_SERVER_ID trocado com grupo entre máquinas vivo"),
        Err(error) => tracing::warn!(code = "groups_identity_unreadable", %error, "groups: a identidade gravada não foi lida"),
    }
    // Mudou o grupo: a lista relê o disco na hora, em vez de servir a descoberta de até 1 s atrás, e
    // o retorno da filha ligada confere se passou a ter filha para vigiar.
    let service = GroupService::new(dir, orq, server_id).with_diary(diary);
    Some(Arc::new(service.with_change_hook(Arc::new(move || { list.invalidate(); link_return::wake(); }))))
}

/// O livro das outras máquinas é resolvido no Rust, no mesmo lugar das rotas e do CLI.
pub fn peers_from_env() -> peers::PeerClient {
    let home = crate::accounts::environment::AccountEnvironment::capture().home;
    let backend = crate::session_open::cli::backend_dir();
    let path = match crate::user_data::path(&home, backend.as_deref(), "peers.json") {
        Ok(path) => path,
        Err(error) => { tracing::warn!(code="peers_unreadable", %error, "não consegui resolver o livro das máquinas"); None }
    };
    peers::PeerClient::new(peers::PeerBook::new(path))
}
