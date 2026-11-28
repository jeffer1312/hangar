//! Abertura de sessão única: `SessionOpenRequest` (o pedido no fio, o `CreateBody` do Python mais
//! `inherit`) → `resolve_open()` → `SessionSpec` (tudo decidido, gravado por sessão). Reabrir é o
//! spec gravado ⊕ `SessionChange`. Spec: `docs/superpowers/specs/2026-10-15-unified-session-open.md`.
pub mod adopt;
pub mod agents;
pub mod archive;
pub mod baton;
#[cfg(test)]
pub(crate) mod cases;
pub mod claude;
pub mod cli;
pub mod codex;
#[cfg(all(test, target_os = "linux"))]
mod contract;
pub mod facts;
pub mod fallen;
pub mod legacy_restore;
pub mod local;
pub mod open;
pub mod orphans;
pub mod record;
/// Reúne as regras que o servidor usa na abertura e em seus demais fluxos.
pub mod resolve {
    pub use hangar_session_open_policy::{
        CodexCatalogError, CodexLogin, CreatorFacts, OpenCaller, OpenFacts, ProviderCatalog, Resolved,
        ServerDefaults, choose_provider, resolve_open,
    };

    pub(crate) use hangar_session_open_policy::{
        creator_codex_mode, creator_mode, fit_name, folder_pattern, known_config_dir, valid_model,
    };
    #[cfg(test)]
    pub(crate) use hangar_session_open_policy::folder_base;
}
pub mod restore;
pub mod server;
pub mod terminal;
#[cfg(test)]
mod kimi_permission_tests;
#[cfg(test)]
mod tests;

pub use resolve::{OpenCaller, OpenFacts, Resolved, resolve_open};
pub use server::create;

pub use hangar_session_open_policy::{
    Account, AccountSource, AgentProvider, ChangeReason, ClaudeCustomizations, OpenError,
    PermissionMode, SPEC_VERSION, ServiceTier, SessionChange, SessionOpenRequest, SessionSpec,
    Surface, WorktreeSpec,
};

/// O vínculo pedido, decidido antes de abrir; `Err` recusa a abertura.
pub(crate) fn link_plan(request: &SessionOpenRequest, caller: &OpenCaller) -> Result<LinkPlan, OpenError> {
    if !request.link { return Ok(LinkPlan::No); }
    // A ligação sai da máquina da criadora; a filha vai a outra por `machine`, nunca por `srv::` no nome.
    if [request.name.as_deref(), request.creator.as_deref()].into_iter().flatten().any(crate::groups::local::is_remote) {
        return Err(OpenError::new(400, "erro_link_remoto", "a criadora é desta máquina; para a filha abrir noutra, use `machine`"));
    }
    // O convidado nunca entra no grupo de uma sessão do dono.
    if matches!(caller, OpenCaller::Guest { .. }) {
        return Err(OpenError::new(403, "erro_link_convidado", "convidado não liga sessões"));
    }
    Ok(match request.creator.as_deref() {
        Some(creator) => LinkPlan::Join(creator.to_owned()),
        None => LinkPlan::Unlinked("erro_link_sem_criador", "sem sessão criadora, a sessão abriu sem ligação"),
    })
}

/// O que fazer com o vínculo depois de abrir: nada, entrar no grupo da criadora, ou só avisar
/// (código e detalhe do `link_error`).
#[derive(Clone, Debug, PartialEq)]
pub enum LinkPlan { No, Join(String), Unlinked(&'static str, &'static str) }

impl LinkPlan {
    /// O pai cuja linha a filha recebe: só quando a ligação deve dar certo. `parent_orq` é o grupo do pai
    /// lido antes de abrir (`None` = grupos fora ou ilegíveis); grupo `orq` recusa filha ligada, e a
    /// linha prometeria um retorno que não vem.
    pub fn line_parent(&self, parent_orq: Option<bool>) -> Option<&str> {
        match (self, parent_orq) {
            (LinkPlan::Join(parent), Some(false)) => Some(parent),
            _ => None,
        }
    }
}

/// A filha ligada recebe a linha dela no começo do primeiro prompt; sem prompt não ganha um, porque
/// a linha sozinha abriria um turno que ninguém pediu. Só o provedor que devolve a resposta sozinho:
/// a Pi, omp e Kimi a linha mandaria não responder, e nada voltaria. Decidido depois da resolução,
/// porque o provedor omitido vem da criadora. Vai no prompt, não no `SessionStart`: o vínculo só é
/// gravado depois de o processo subir.
pub(crate) fn prefix_child_line(resolved: &mut Resolved, parent: &str) {
    if !crate::groups::link_return::returnable(resolved.spec.provider.name()) { return; }
    if let Some(prompt) = resolved.initial_prompt.as_mut().filter(|p| !p.is_empty()) {
        // Espaço, não quebra de linha: o prompt de uma linha continua de uma linha no pane.
        *prompt = format!("{} {prompt}", hangar_delegation::guidance::child_prefix(parent));
    }
}
