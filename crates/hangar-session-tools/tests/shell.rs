use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use futures_util::future::BoxFuture;
use hangar_session_tools::shell::{Mux, Output, Service};
use serde_json::json;

#[derive(Clone)]
struct Session {
    cwd: String,
    hidden: bool,
    running: String,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<String, Session>,
    commands: Vec<Vec<String>>,
    warnings: Vec<String>,
    display_fails: bool,
    display_empty: bool,
    replace_on_display: Option<Session>,
    mark_fails: bool,
    kill_fails: bool,
    create_fails: bool,
    create_race: bool,
    scope_lost: bool,
    scope_rejects_create: bool,
    scope_timeout: bool,
}
#[derive(Default)]
struct FakeMux(Mutex<State>);
impl Mux for FakeMux {
    fn scope(&self) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async { vec!["scope".into(), "--".into()] })
    }
    fn warning(&self, message: &str) {
        self.0.lock().unwrap().warnings.push(message.into());
    }
    fn run(&self, args: Vec<String>) -> BoxFuture<'_, Output> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            state.commands.push(args.clone());
            if args == ["scope", "--", "true"] {
                return Output {
                    code: Some(i32::from(state.scope_lost)),
                    ..Default::default()
                };
            }
            if args.starts_with(&["scope".into(), "--".into()])
                && (state.scope_lost || state.scope_rejects_create || state.scope_timeout)
            {
                return Output {
                    code: if state.scope_timeout { None } else { Some(1) },
                    ..Default::default()
                };
            }
            let start = args.iter().position(|arg| arg == "mux").unwrap() + 1;
            let args = &args[start..];
            let target = args
                .iter()
                .position(|arg| arg == "-t")
                .map(|i| {
                    args[i + 1]
                        .trim_start_matches('=')
                        .trim_end_matches(':')
                        .to_owned()
                })
                .unwrap_or_default();
            let mut code = 0;
            let mut stdout = String::new();
            match args[0].as_str() {
                "has-session" => {
                    code = i32::from(!state.sessions.contains_key(&target));
                }
                "show-options" => {
                    stdout = if state.sessions.get(&target).is_some_and(|s| s.hidden) {
                        "1"
                    } else {
                        "0"
                    }
                    .into();
                }
                "display" => {
                    if state.display_fails {
                        code = 1;
                    } else if !state.display_empty {
                        stdout = state.sessions.get(&target).unwrap().cwd.clone();
                    }
                    if let Some(replacement) = state.replace_on_display.take() {
                        state.sessions.insert(target.clone(), replacement);
                    }
                }
                "kill-session" => {
                    if !state.kill_fails {
                        state.sessions.remove(&target);
                    }
                }
                "new-session" => {
                    let target = args[args.iter().position(|arg| arg == "-s").unwrap() + 1].clone();
                    let cwd = args[args.iter().position(|arg| arg == "-c").unwrap() + 1].clone();
                    if state.create_race {
                        state.sessions.insert(
                            target,
                            Session {
                                cwd,
                                hidden: true,
                                running: "outro criador".into(),
                            },
                        );
                        code = 1;
                    } else if state.create_fails {
                        code = 1;
                    } else {
                        state.sessions.insert(
                            target,
                            Session {
                                cwd,
                                hidden: false,
                                running: "shell".into(),
                            },
                        );
                    }
                }
                "set-option" => {
                    if state.mark_fails {
                        code = 1;
                    } else {
                        state.sessions.get_mut(&target).unwrap().hidden = true;
                    }
                }
                other => panic!("comando não implementado no multiplexador de teste: {other}"),
            }
            Output {
                code: Some(code),
                stdout,
                stderr: String::new(),
            }
        })
    }
}
fn service(windows: bool) -> (Arc<FakeMux>, Service) {
    let mux = Arc::new(FakeMux::default());
    let service = Service {
        mux: mux.clone(),
        program: "mux".into(),
        windows,
    };
    (mux, service)
}
fn existing(mux: &FakeMux, cwd: &str, hidden: bool) {
    mux.0.lock().unwrap().sessions.insert(
        "term-agente".into(),
        Session {
            cwd: cwd.into(),
            hidden,
            running: "servidor em execução".into(),
        },
    );
}

#[tokio::test]
async fn shell_created_separately_hidden_in_requested_directory() {
    let (mux, service) = service(false);
    assert_eq!(
        service.open("agente", "/repo").await.unwrap(),
        json!({"ok": true, "shell": "term-agente"})
    );
    let state = mux.0.lock().unwrap();
    let session = &state.sessions["term-agente"];
    assert_eq!(session.cwd, "/repo");
    assert!(session.hidden);
    assert!(state.commands.iter().any(|args| args
        == &[
            "scope",
            "--",
            "mux",
            "new-session",
            "-d",
            "-s",
            "term-agente",
            "-c",
            "/repo"
        ]));
    assert!(state.commands.iter().any(|args| args
        == &[
            "mux",
            "set-option",
            "-t",
            "=term-agente:",
            "@cp_hidden",
            "1"
        ]));
}

#[tokio::test]
async fn shell_reuses_birth_directory_without_interrupting_running_work() {
    let (mux, service) = service(false);
    existing(&mux, "/repo", true);
    service.open("agente", "/repo/.").await.unwrap();
    let state = mux.0.lock().unwrap();
    assert_eq!(
        state.sessions["term-agente"].running,
        "servidor em execução"
    );
    assert!(
        !state
            .commands
            .iter()
            .flatten()
            .any(|arg| arg == "kill-session"
                || arg == "new-session"
                || arg == "-A"
                || arg == "#{pane_current_path}")
    );
    assert!(
        state
            .commands
            .iter()
            .flatten()
            .any(|arg| arg == "#{session_path}")
    );
}

#[tokio::test]
async fn shell_refuses_unmarked_occupant_without_mutating_it() {
    let (mux, service) = service(false);
    existing(&mux, "/terceiro", false);
    let error = service.open("agente", "/repo").await.unwrap_err();
    assert_eq!((error.status, error.code), (409, "erro_sessao_tmux_em_uso"));
    assert_eq!(error.params["nome"], "term-agente");
    let state = mux.0.lock().unwrap();
    assert_eq!(
        state.sessions["term-agente"].running,
        "servidor em execução"
    );
    assert!(
        !state
            .commands
            .iter()
            .flatten()
            .any(|arg| arg == "kill-session" || arg == "new-session" || arg == "set-option")
    );
}

#[tokio::test]
async fn shell_recreates_orphan_in_new_directory() {
    let (mux, service) = service(false);
    existing(&mux, "/antigo", true);
    service.open("agente", "/novo").await.unwrap();
    let state = mux.0.lock().unwrap();
    assert_eq!(state.sessions["term-agente"].cwd, "/novo");
    assert!(state.sessions["term-agente"].hidden);
    assert!(
        state
            .commands
            .iter()
            .any(|args| args == &["mux", "kill-session", "-t", "=term-agente"])
    );
}

#[tokio::test]
async fn shell_rejects_old_directory_when_kill_reports_success_but_session_survives() {
    let (mux, service) = service(false);
    existing(&mux, "/antigo", true);
    mux.0.lock().unwrap().kill_fails = true;
    let error = service.open("agente", "/novo").await.unwrap_err();
    assert_eq!(
        (error.status, error.code),
        (500, "erro_shell_criacao_falhou")
    );
    assert_eq!(mux.0.lock().unwrap().sessions["term-agente"].cwd, "/antigo");
}

#[tokio::test]
async fn shell_reuses_existing_work_with_warning_when_directory_read_fails() {
    let (mux, service) = service(false);
    existing(&mux, "/antigo", true);
    mux.0.lock().unwrap().display_fails = true;
    service.open("agente", "/novo").await.unwrap();
    let state = mux.0.lock().unwrap();
    assert!(!state.warnings.is_empty());
    assert_eq!(
        state.sessions["term-agente"].running,
        "servidor em execução"
    );
}

#[tokio::test]
async fn shell_reuses_existing_work_when_directory_response_is_empty() {
    let (mux, service) = service(false);
    existing(&mux, "/antigo", true);
    mux.0.lock().unwrap().display_empty = true;
    service.open("agente", "/novo").await.unwrap();
    assert_eq!(
        mux.0.lock().unwrap().sessions["term-agente"].running,
        "servidor em execução"
    );
}

#[tokio::test]
async fn shell_accepts_concurrent_creator_without_marking_its_shell_again() {
    let (mux, service) = service(false);
    mux.0.lock().unwrap().create_race = true;
    service.open("agente", "/repo").await.unwrap();
    let state = mux.0.lock().unwrap();
    assert_eq!(state.sessions["term-agente"].running, "outro criador");
    assert!(
        !state
            .commands
            .iter()
            .flatten()
            .any(|arg| arg == "set-option")
    );
}

#[tokio::test]
async fn shell_reports_creation_failure_when_no_concurrent_shell_exists() {
    let (mux, service) = service(false);
    mux.0.lock().unwrap().create_fails = true;
    assert_eq!(
        service.open("agente", "/repo").await.unwrap_err().code,
        "erro_shell_criacao_falhou"
    );
    assert!(mux.0.lock().unwrap().sessions.is_empty());
}

#[tokio::test]
async fn shell_removes_only_newly_created_session_when_mark_fails() {
    let (mux, service) = service(false);
    mux.0.lock().unwrap().mark_fails = true;
    assert_eq!(
        service.open("agente", "/repo").await.unwrap_err().code,
        "erro_shell_criacao_falhou"
    );
    assert!(mux.0.lock().unwrap().sessions.is_empty());
}

#[tokio::test]
async fn shell_keeps_existing_work_when_mark_fails() {
    let (mux, service) = service(false);
    existing(&mux, "/repo", true);
    mux.0.lock().unwrap().mark_fails = true;
    assert_eq!(
        service.open("agente", "/repo").await.unwrap_err().code,
        "erro_shell_criacao_falhou"
    );
    assert_eq!(
        mux.0.lock().unwrap().sessions["term-agente"].running,
        "servidor em execução"
    );
}

#[tokio::test]
async fn shell_windows_kill_uses_plain_name_and_normalizes_without_case_folding() {
    let (mux, service) = service(true);
    existing(&mux, "C:\\Repo\\sub\\..", true);
    service.open("agente", "C:/Repo/.").await.unwrap();
    assert_eq!(
        mux.0.lock().unwrap().sessions["term-agente"].running,
        "servidor em execução"
    );
    service.open("agente", "C:/repo").await.unwrap();
    let state = mux.0.lock().unwrap();
    assert_eq!(state.sessions["term-agente"].cwd, "C:/repo");
    assert!(
        state
            .commands
            .iter()
            .any(|args| args == &["mux", "kill-session", "-t", "term-agente"])
    );
}

#[tokio::test]
async fn shell_normalizes_unc_paths_without_resolving_above_share() {
    let (mux, service) = service(true);
    existing(&mux, "\\\\servidor\\share\\pasta\\..", true);
    service.open("agente", "//servidor/share/.").await.unwrap();
    assert_eq!(
        mux.0.lock().unwrap().sessions["term-agente"].running,
        "servidor em execução"
    );
}

proptest::proptest! {
    #[test]
    fn shell_arbitrary_directory_never_panics(directory in ".*") {
        let (mux, service) = service(false);
        existing(&mux, &directory, true);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result = runtime.block_on(service.open("agente", &directory));
        proptest::prop_assert!(result.is_ok());
    }
}

#[tokio::test]
async fn shell_retries_without_scope_only_when_probe_confirms_scope_lost() {
    let (mux, service) = service(false);
    mux.0.lock().unwrap().scope_lost = true;
    assert_eq!(
        service.open("agente", "/repo").await.unwrap()["shell"],
        "term-agente"
    );
    let state = mux.0.lock().unwrap();
    assert!(state.sessions["term-agente"].hidden);
    assert!(
        state
            .commands
            .iter()
            .any(|args| args == &["scope", "--", "true"])
    );
    assert!(state.commands.iter().any(|args| args
        == &[
            "mux",
            "new-session",
            "-d",
            "-s",
            "term-agente",
            "-c",
            "/repo"
        ]));
    assert!(!state.warnings.is_empty());
}

#[tokio::test]
async fn shell_does_not_retry_create_when_scope_probe_succeeds() {
    let (mux, service) = service(false);
    mux.0.lock().unwrap().scope_rejects_create = true;
    assert_eq!(
        service.open("agente", "/repo").await.unwrap_err().code,
        "erro_shell_criacao_falhou"
    );
    let state = mux.0.lock().unwrap();
    assert!(
        state
            .commands
            .iter()
            .any(|args| args == &["scope", "--", "true"])
    );
    assert!(
        !state
            .commands
            .iter()
            .any(|args| args.starts_with(&["mux".into(), "new-session".into()]))
    );
    assert!(state.sessions.is_empty());
}

#[tokio::test]
async fn shell_does_not_probe_or_retry_create_after_scope_timeout() {
    let (mux, service) = service(false);
    mux.0.lock().unwrap().scope_timeout = true;
    assert_eq!(
        service.open("agente", "/repo").await.unwrap_err().code,
        "erro_shell_criacao_falhou"
    );
    let state = mux.0.lock().unwrap();
    assert!(
        !state
            .commands
            .iter()
            .any(|args| args == &["scope", "--", "true"])
    );
    assert!(
        !state
            .commands
            .iter()
            .any(|args| args.starts_with(&["mux".into(), "new-session".into()]))
    );
    assert!(state.sessions.is_empty());
}

#[tokio::test]
async fn shell_refuses_replacement_that_loses_hidden_mark_during_directory_read() {
    for requested in ["/novo", "/antigo"] {
        let (mux, service) = service(false);
        existing(&mux, "/antigo", true);
        mux.0.lock().unwrap().replace_on_display = Some(Session {
            cwd: "/outra-abertura".into(),
            hidden: false,
            running: "trabalho da outra abertura".into(),
        });

        let result = service.open("agente", requested).await;
        let state = mux.0.lock().unwrap();
        let session = &state.sessions["term-agente"];
        assert_eq!(session.cwd, "/outra-abertura");
        assert_eq!(session.running, "trabalho da outra abertura");
        assert!(!session.hidden);
        assert!(
            !state
                .commands
                .iter()
                .flatten()
                .any(|arg| arg == "set-option" || arg == "kill-session")
        );
        let error = result.unwrap_err();
        assert_eq!((error.status, error.code), (409, "erro_sessao_tmux_em_uso"));
        assert_eq!(error.params["nome"], "term-agente");
    }
}
