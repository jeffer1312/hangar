//! `hangar-server codex-tui`: o lançador da sessão Codex com terminal, o comando que o pane roda.
//! Prepara a conta pela API do servidor, atualiza o Codex e os marketplaces, sobe o `codex app-server`
//! numa porta de loopback, aceita os hooks sincronizados pelo Hangar e roda a TUI em primeiro plano,
//! filha deste processo: é o que deixa o lançador derrubar o app-server quando a TUI sai. Grava e
//! apaga o arquivo da sessão (`~/.hangar/codex-sessions/<nome>.json`) com a mesma trava do servidor.
//!
//! Mora no `hangar-server` e não num crate próprio porque reaproveita o que só o servidor tem: o
//! arquivo da sessão (`runtime::codex_sidecar`), a política e a identidade da subida
//! (`runtime::codex_launch`), a pré-confiança da pasta (`session_open::codex`), a decisão dos hooks
//! (`codex_setup::trust`) e a resolução da conta (`accounts`).
//!
//! Toda linha de andamento vai ao stderr do pane com o prefixo `hangar-codex-tui: `: a lista do
//! servidor lê esse prefixo enquanto a sessão ainda não tem conversa.
mod app_server;
mod args;
pub mod close;
mod hooks;
mod marketplaces;
mod plan;
mod prepare;
mod session;
mod threads;
mod update;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use crate::accounts::environment::{Account, AccountEnvironment};
use crate::accounts::{AccountService, Provider};
use crate::runtime::codex_sidecar;

pub(crate) use marketplaces::failed_lines as marketplaces_failed_lines;

/// O prefixo que a lista do servidor e as telas leem no pane.
pub(crate) const PREFIX: &str = "hangar-codex-tui: ";
/// A linha que deixa o pane aberto depois de uma abertura que falhou; a lista a reconhece.
pub(crate) const ENTER: &str = "Pressione Enter para fechar esta sessão.";

/// Onde as linhas de andamento aparecem: o pane, ou a lista de um teste.
pub(crate) trait Screen: Sync {
    fn say(&self, line: &str);
}

/// O stderr do pane, linha a linha.
pub(crate) struct Pane;

impl Screen for Pane {
    fn say(&self, line: &str) { eprintln!("{PREFIX}{line}"); }
}

/// O desfecho de uma vida do lançador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Exit {
    pub code: i32,
    /// A sessão não chegou a abrir a TUI: o pane fica com o erro até o Enter.
    pub opening: bool,
}

impl Exit {
    fn opening(code: i32) -> Self { Self { code, opening: true } }
    fn done(code: i32) -> Self { Self { code, opening: false } }
}

/// `args` sem o nome do subcomando.
pub fn cli(args: &[String]) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{PREFIX}não deu para iniciar o lançador ({:?})", error.kind());
            return ExitCode::from(1);
        }
    };
    let code = runtime.block_on(run(args));
    // A escuta de threads que ainda espera o app-server morto não segura a saída.
    runtime.shutdown_timeout(Duration::from_millis(200));
    ExitCode::from((code & 0xff) as u8)
}

/// O ambiente do pane em texto: variável fora de UTF-8 não é do Hangar e não muda nada aqui.
fn pane_environment() -> Vec<(String, String)> {
    std::env::vars_os().filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?))).collect()
}

fn value_of<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().rev().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
}

async fn run(raw: &[String]) -> i32 {
    let env = pane_environment();
    let mut life = session::Life::default();
    let exit = tokio::select! {
        exit = flow(raw, &env, &mut life) => exit,
        code = signals() => Exit::done(code),
    };
    life.finish(&Pane).await;
    if exit.opening && exit.code != 0 {
        // O pane fechado (SIGHUP) ou o `kill-session` não esperam o Enter.
        tokio::select! {
            () = wait_enter() => {}
            code = signals() => return code,
        }
    }
    exit.code
}

/// `SIGTERM`/`SIGHUP` viram 143 e a limpeza roda: `tmux kill-session` derruba o pane por sinal, e sem
/// isso o app-server ficaria escutando sem dono. No Windows o fechamento do console faz o mesmo.
async fn signals() -> i32 {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut hup), Ok(mut int)) =
            (signal(SignalKind::terminate()), signal(SignalKind::hangup()), signal(SignalKind::interrupt())) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = term.recv() => 143,
            _ = hup.recv() => 143,
            _ = int.recv() => 130,
        }
    }
    #[cfg(windows)]
    {
        match tokio::signal::windows::ctrl_close() {
            Ok(mut close) => {
                close.recv().await;
                143
            }
            Err(_) => std::future::pending().await,
        }
    }
}

/// O pane fica com o erro na tela até alguém ler: sem isto ele morre e a sessão some da lista. Sem
/// terminal na entrada (fim de arquivo), segue na hora.
async fn wait_enter() {
    eprintln!("{ENTER}");
    let _ = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }).await;
}

/// A abertura inteira, da leitura dos argumentos até a TUI sair.
async fn flow(raw: &[String], env: &[(String, String)], life: &mut session::Life) -> Exit {
    let screen = Pane;
    let args = match args::parse(raw, value_of(env, "CP_SESSION_NAME")) {
        Ok(args::Parsed::Help) => {
            print!("{}", args::HELP);
            return Exit::done(0);
        }
        Ok(args::Parsed::Run(args)) => args,
        Err(refusal) => {
            eprint!("{}", refusal.render());
            return Exit::opening(2);
        }
    };
    let home = AccountEnvironment::from_pairs(env.iter().cloned()).home;
    let sidecars = codex_sidecar::dir(&home);
    let existing = codex_sidecar::load(&codex_sidecar::path(&sidecars, &args.name));
    let here = std::env::current_dir().unwrap_or_default();
    let facts = plan::Facts {
        session_key: value_of(env, "CP_SESSION_KEY"),
        here: &here,
        inherited_codex_home: value_of(env, "CODEX_HOME"),
        home: &home,
    };
    let plan = match plan::decide(args, existing.as_ref(), &facts) {
        Ok(plan) => plan,
        Err(refusal) => {
            eprint!("{}", refusal.render());
            return Exit::opening(2);
        }
    };
    if let Some(warning) = plan.mode_warning() {
        screen.say(&warning);
    }
    // A conta tem de ser do Hangar, a mesma recusa da abertura pelo servidor.
    let service = AccountService::new(AccountEnvironment::from_pairs(env.iter().cloned()));
    if let Err(error) = service.resolve(Provider::Codex, &plan.account) {
        screen.say(&format!("a conta Codex {} não é uma conta do Hangar ({}); nenhuma sessão foi aberta", plan.account, error.code));
        return Exit::opening(1);
    }
    let child_env = child_environment(env, &plan);
    if plan.prepares(sync_enabled(&home)) {
        let target = prepare::Target {
            account: &plan.account,
            cwd: &plan.cwd,
            codex_home: &plan.codex_home,
            server: prepare::server_of(env, &home),
        };
        if let Err(reason) = prepare::run(&target, &screen).await {
            screen.say(&format!("a conta não abriu porque o preparo não foi confirmado ({reason})"));
            return Exit::opening(1);
        }
    }
    let endpoint = match app_server::free_endpoint() {
        Ok(endpoint) => endpoint,
        Err(error) => {
            screen.say(&format!("não deu para reservar uma porta local ({:?})", error.kind()));
            return Exit::opening(1);
        }
    };
    update::Update::new(&home, &child_env).run(&screen).await;
    marketplaces::run(&child_env, &screen).await;
    let diary = app_server::Diary::new(&plan.name, hangar_core::paths::log_base());
    let Some(codex) = crate::runtime::process::resolve_program("codex", &child_env) else {
        diary.failure(&app_server::Failure::new("criar_processo", "abrir_app_server").kind("FileNotFoundError"), &mut app_server::Tail::none());
        eprintln!("{PREFIX}nao achei o executavel `codex` no PATH deste processo. Instale o Codex CLI (npm install -g @openai/codex) e reabra a sessao.");
        return Exit::opening(1);
    };
    // A chave do spec que o Rust pôs no pane (na importada, a da conversa): o registro é por ela.
    let key = plan.imported_key.as_deref().or(value_of(env, "CP_SESSION_KEY")).filter(|key| !key.is_empty());
    let shared = Arc::new(session::Session::new(sidecars, &plan, endpoint.clone(), key));
    life.start(shared.clone(), codex.clone(), child_env.clone(), diary);
    if let Err(error) = life.spawn_server(&plan).await {
        screen.say(&format!("não deu para subir o app-server do Codex ({:?})", error.kind()));
        return Exit::opening(1);
    }
    if let Some(pid) = life.server_pid() {
        shared.set_app_pid(pid);
    }
    if let Some(resume) = plan.resume.as_deref() {
        // Retomando, a identidade já é conhecida e o `thread/started` não vem: grava agora.
        match session::rollout_of(&plan.codex_home, resume) {
            Some(rollout) => shared.save_thread(resume, &rollout.to_string_lossy(), None, &screen),
            None => screen.say(&format!("nao achei o rollout da conversa {resume}")),
        }
    }
    life.listen(&plan.cwd);
    screen.say("subindo o app-server do Codex…");
    if !life.wait_port(app_server::PORT_WAIT, &screen).await {
        return Exit::opening(1);
    }
    for line in hooks::lines(&hooks::trust(&endpoint, &plan.cwd, &home, &plan.codex_home).await) {
        screen.say(&line);
    }
    let argv = plan::tui_argv(&codex.to_string_lossy(), &plan, &endpoint, shared.selected_tier().as_deref());
    let mut tui = match tokio::process::Command::new(&argv[0]).args(&argv[1..]).env_clear().envs(&child_env).spawn() {
        Ok(tui) => tui,
        Err(error) => {
            screen.say(&format!("não deu para abrir a TUI do Codex ({:?})", error.kind()));
            return Exit::opening(1);
        }
    };
    if let Some(pid) = tui.id() {
        shared.tui_started(pid, &screen);
    }
    Exit::done(life.watch(&mut tui, &plan, &screen).await)
}

/// `CP_CODEX_SYNC_ENABLED` do `.env` do servidor, o mesmo que decide o limite das instruções no pane.
fn sync_enabled(home: &Path) -> bool {
    let backend = crate::session_open::cli::backend_dir_for_home(home);
    match hangar_core::user_data::env_value(home, backend.as_deref(), "CP_CODEX_SYNC_ENABLED") {
        Ok(Some(value)) => !matches!(value.to_lowercase().as_str(), "0" | "false" | "no"),
        _ => true,
    }
}

/// O ambiente de todos os filhos (app-server, TUI, npm, marketplaces): o do pane com a identidade
/// da sessão, filtrado pela conta. A adicional não herda identidade da principal.
fn child_environment(env: &[(String, String)], plan: &plan::Plan) -> BTreeMap<String, String> {
    let mut pairs: Vec<(String, String)> = env.to_vec();
    if let Some(key) = &plan.imported_key {
        // A conversa importada é outra sessão: nada do pane de quem a abriu passa.
        pairs.retain(|(name, _)| !matches!(name.as_str(), "CP_SESSION_KEY" | "TMUX" | "TMUX_PANE" | "HANGAR_CANO_KEY"));
        pairs.push(("CP_SESSION_KEY".into(), key.clone()));
    }
    pairs.push(("CP_SESSION_NAME".into(), plan.name.clone()));
    pairs.push(("CODEX_HOME".into(), plan.codex_home.to_string_lossy().into_owned()));
    let account = Account { id: plan.account.clone(), home: plan.codex_home.clone(), is_default: plan.account == "default" };
    AccountEnvironment::from_pairs(pairs).codex(&account)
}

#[cfg(test)]
mod tests;
