//! Assistente de instalação (spec `2026-10-06-instalador-grafico-design.md`): a entrada que acha o Hangar deste computador e
//! as seis telas que o instalam pelos scripts de sempre no modo `--app`. Só Linux e Windows; no macOS o app segue no cartão
//! de endereço + token.
use super::*;

pub(crate) mod agent;
mod app_copy;
mod askpass;
mod codes;
mod entry;
mod failure;
mod flow;
mod local;
mod marks;
mod phone;
mod precheck;
mod repo;
mod report;
pub(crate) mod run;
mod screens;
pub(crate) mod system;
mod tail;
mod wizard;

pub(super) use wizard::SetupWizard;
pub(crate) use wizard::{stop_agent_process, sweep_group};
pub(crate) use app_copy::target as installed_app_target;

/// O assistente existe onde há script de instalação com modo `--app`.
pub(super) fn supported() -> bool { cfg!(any(target_os = "linux", target_os = "windows")) }

/// De onde o assistente foi aberto: a entrada já sabe a pasta; o menu procura de novo; reabrir retoma o que estava rodando.
pub(super) enum Origin {
    Entry(Option<local::LocalInstall>),
    Menu,
    Resume(run::SetupState),
}

/// O cartão da entrada enquanto não há conexão salva (spec "Entrada").
pub(super) enum Entry {
    Probing,
    Found(local::Found),
}

/// O assistente que estava rodando quando o app fechou (`state.json`): reabrir o app retoma nele.
pub(super) fn saved_run() -> Option<run::SetupState> { if supported() { run::load_state() } else { None } }

/// Um conserto do agente ficou sem desfazer (o app caiu ou saiu enquanto desfazia): o assistente abre para devolver a
/// pasta e avisar. A anotação só sai depois de desfazer sem erro.
pub(super) fn interrupted_fix() -> bool { supported() && run::state_dir().is_some_and(|dir| repo::saved_at(&dir)) }

/// O app abriu o assistente sozinho ao iniciar: a recuperação de um conserto interrompido fica mais cautelosa (não desfaz
/// uma pasta que já seguiu em frente). Pelo menu, a pessoa pediu: desfaz como sempre.
static OPENED_AT_LAUNCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn opening_at_launch() { OPENED_AT_LAUNCH.store(true, std::sync::atomic::Ordering::Relaxed); }

fn take_launch() -> bool { OPENED_AT_LAUNCH.swap(false, std::sync::atomic::Ordering::Relaxed) }

/// Só para provar telas: `HANGAR_SETUP_DEMO` abre o assistente ao iniciar.
pub(super) fn demo() -> bool { supported() && std::env::var_os("HANGAR_SETUP_DEMO").is_some() }
