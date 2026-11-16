//! Pergunta lateral (`/btw`): os casos de `backend/tests/test_btw.py` da rota, do histórico e do fork,
//! que responde com e sem terminal.
use hangar_server::session_write::btw::*;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};

fn code(result: Result<Value, BtwError>) -> (u16, String) { let e = result.unwrap_err(); (e.status, e.code) }

// ── Rota: corpo, decisão, histórico ─────────────────────────────────────────────────────────────

/// B6 (`api.py:1919`): `question` de 1 a 4000 caracteres (code points), nada a mais, com o 422 do FastAPI.
#[test]
fn body_is_the_strict_python_model() {
    assert_eq!(parse_body(br#"{"question":"oi"}"#).unwrap(), "oi");
    assert_eq!(parse_body(format!(r#"{{"question":"{}"}}"#, "é".repeat(4000)).as_bytes()).unwrap().chars().count(), 4000);
    let kind = |body: &[u8]| parse_body(body).unwrap_err()["detail"][0]["type"].as_str().unwrap().to_owned();
    assert_eq!(kind(format!(r#"{{"question":"{}"}}"#, "é".repeat(4001)).as_bytes()), "string_too_long");
    assert_eq!(kind(br#"{"question":""}"#), "string_too_short");
    assert_eq!(kind(br#"{}"#), "missing");
    assert_eq!(kind(br#"{"question":3}"#), "string_type");
    assert_eq!(kind(br#"{"question":null}"#), "string_type");
    assert_eq!(kind(br#"{"question":"q","x":1}"#), "extra_forbidden");
    assert_eq!(kind(b"[]"), "model_attributes_type");
    assert_eq!(kind(b"nao json"), "json_invalid");
    assert_eq!(parse_body(br#"{"question":"q","x":1}"#).unwrap_err()["detail"][0]["loc"], json!(["body", "x"]));
}

/// B1, B3, B4 (`api.py:5348`, `:5351`, `:5338`): sessão que não existe, sem terminal e outro provedor.
#[test]
fn route_follows_provider_terminal_and_existence() {
    use axum::http::StatusCode;
    assert_eq!(route("claude", true), Ok(Route::Terminal));
    assert_eq!(route("claude", false), Ok(Route::Headless));
    for other in ["codex", "pi", "omp", "kimi"] {
        for terminal in [true, false] { assert_eq!(route(other, terminal), Err((StatusCode::BAD_REQUEST, "erro_btw_so_claude"))); }
    }
    assert_eq!(route_of_row(None), Err((StatusCode::NOT_FOUND, "erro_sessao_inexistente")));
    let row = |provider: &str, headless: bool| serde_json::from_value::<hangar_api::session::SessionRow>(json!({"name":"s","provider":provider,"headless":headless})).unwrap();
    assert_eq!(route_of_row(Some(&row("claude", true))), Ok(Route::Headless));
    assert_eq!(route_of_row(Some(&row("claude", false))), Ok(Route::Unowned));
    assert_eq!(route_of_row(Some(&row("pi", false))), Err((StatusCode::BAD_REQUEST, "erro_btw_so_claude")));
}

/// B24 (`test_historico_guarda_so_os_ultimos`): as últimas 50, no formato do `json.dumps` do Python.
#[test]
fn history_keeps_only_the_last_ones_in_the_python_format() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..55 { assert!(record(dir.path(), "s/x", &json!({"question": i.to_string(), "answer": "á", "fonte": "buffer", "ts": i})).unwrap()); }
    let items = history(dir.path(), "s/x").unwrap();
    assert_eq!(items.len(), 50);
    assert_eq!(items[0]["question"], "5");
    let file = dir.path().join(".hangar-btw/s-x.jsonl");
    let text = std::fs::read_to_string(file).unwrap();
    assert_eq!(text.lines().count(), 50);
    assert_eq!(text.lines().next().unwrap(), r#"{"question": "5", "answer": "á", "fonte": "buffer", "ts": 5}"#);
}

/// B24 (`api.py:5360`): disco que falha não vira erro: `salvo: false`.
#[test]
fn history_that_cannot_be_written_is_false_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".hangar-btw"), "arquivo no lugar da pasta").unwrap();
    assert!(!record(dir.path(), "s", &json!({"question": "q"})).unwrap());
}

/// B25 (`btw.py:370`): sem arquivo é `[]` e a pasta nasce; linha inválida é pulada.
#[test]
fn history_without_file_is_empty_and_skips_bad_lines() {
    let dir = tempfile::tempdir().unwrap();
    assert!(history(dir.path(), "s").unwrap().is_empty());
    assert!(dir.path().join(".hangar-btw").is_dir());
    std::fs::write(dir.path().join(".hangar-btw/s.jsonl"), "{\"question\": \"a\"}\nnao json\n\n   \n{\"question\": \"b\"}\n").unwrap();
    assert_eq!(history(dir.path(), "s").unwrap(), [json!({"question":"a"}), json!({"question":"b"})]);
}

// ── Sem terminal: o fork ─────────────────────────────────────────────────────────────────────────

/// B31 (`test_sem_terminal_*`): argv do fork, modelo, motor e `--` antes da pergunta.
#[test]
fn fork_argv_matches_the_python() {
    let argv = fork_argv(&json!({"session_id":"sid-antigo"}), "novo", "quanto é 2+2?");
    assert_eq!(argv[..2], ["claude", "-p"]);
    assert_eq!(argv[argv.iter().position(|a| a == "--resume").unwrap() + 1], "sid-antigo");
    assert!(argv.contains(&"--fork-session".into()));
    assert!(argv[argv.iter().position(|a| a == "--disallowed-tools").unwrap() + 1].contains("Bash"));
    assert_eq!(argv[argv.iter().position(|a| a == "--session-id").unwrap() + 1], "novo");
    assert_eq!(argv[argv.len() - 2..], ["--", "quanto é 2+2?"]);
    assert!(!argv.contains(&"--model".into()));
    let flag = fork_argv(&json!({"session_id":"s"}), "n", "--dangerously-skip-permissions");
    assert_eq!(flag[flag.len() - 2..], ["--", "--dangerously-skip-permissions"]);
    let model = fork_argv(&json!({"session_id":"s","model":"haiku"}), "n", "q");
    assert_eq!(model[model.iter().position(|a| a == "--model").unwrap() + 1], "haiku");
    let engine = fork_argv(&json!({"session_id":"s","engine":"kimi","model":"k3","context_window":256000}), "n", "q");
    assert_eq!(engine[..7], ["hangar-engine", "--exec", "kimi", "--model", "k3", "--context", "256000"]);
    assert_eq!(engine[engine.iter().position(|a| a == "--").unwrap() + 1], "claude");
    let no_model = fork_argv(&json!({"session_id":"s","engine":"kimi","model":"","context_window":256000}), "n", "q");
    assert_eq!(no_model[..4], ["hangar-engine", "--exec", "kimi", "--"]);
}

/// B32: `is_error`, resposta vazia e saída que não é JSON.
#[test]
fn fork_answer_refuses_errors_and_blank() {
    assert_eq!(fork_answer(br#"{"result": "  4\n"}"#).unwrap(), "4");
    assert_eq!(fork_answer(br#"{"result":"Credit balance is too low","is_error":true}"#).unwrap_err().code, "erro_btw_fork_falhou");
    for blank in [&br#"{"result":""}"#[..], br#"{"result":null}"#, b"", b"nao json", br#"[1]"#] {
        assert_eq!(fork_answer(blank).unwrap_err().code, "erro_btw_ilegivel", "{}", String::from_utf8_lossy(blank));
    }
}

#[cfg(unix)]
mod fork {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Claude falso: grava argv, cwd e conta, cria o transcript do fork como a CLI e responde `out`.
    fn fake_claude(dir: &Path, out: &str, code: i32, sleep: u32) -> std::path::PathBuf {
        let script = dir.join("claude-falso");
        let log = dir.join("rodou");
        std::fs::write(&script, format!(r#"#!/bin/bash
printf '%s\n' "$@" > '{log}.argv'; pwd > '{log}.cwd'; printf '%s' "$CLAUDE_CONFIG_DIR" > '{log}.conta'
while [ $# -gt 0 ]; do [ "$1" = --session-id ] && sid=$2; shift; done
for d in "$CLAUDE_CONFIG_DIR"/projects/*/; do : > "$d$sid.jsonl"; done
sleep {sleep}
printf '%s' '{out}'
exit {code}
"#, log = log.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }
    struct Case { dir: tempfile::TempDir, meta: Value }
    fn case(with_transcript: bool, extra: Value) -> Case {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let conta = dir.path().join("conta");
        let project = conta.join("projects").join(hangar_workspace::worktrees::sanitize_cwd(cwd.to_str().unwrap()));
        std::fs::create_dir_all(&project).unwrap();
        if with_transcript { std::fs::write(project.join("sid-antigo.jsonl"), "{}\n").unwrap(); }
        let mut meta = json!({"name":"s1","cwd":cwd,"session_id":"sid-antigo","config_dir":conta});
        for (k, v) in extra.as_object().unwrap() { meta[k] = v.clone(); }
        Case { dir, meta }
    }
    fn project(c: &Case) -> std::path::PathBuf { std::fs::read_dir(c.dir.path().join("conta/projects")).unwrap().next().unwrap().unwrap().path() }
    async fn run(c: &Case, question: &str, out: &str, code: i32, sleep: u32, timeout: Duration) -> Result<Value, BtwError> {
        let claude = fake_claude(c.dir.path(), out, code, sleep);
        ask_headless(&c.meta, question, &c.dir.path().join("padrao/projects"), &Programs { claude: claude.clone(), engine: claude }, timeout).await
    }
    fn ran(c: &Case, what: &str) -> Option<String> { std::fs::read_to_string(c.dir.path().join(format!("rodou.{what}"))).ok() }

    /// B31, B32 (`test_sem_terminal_forka_a_conversa_e_apaga_o_transcript_do_fork`).
    #[tokio::test]
    async fn forks_the_conversation_and_deletes_the_fork_transcript() {
        let c = case(true, json!({"model":"haiku"}));
        let r = run(&c, "quanto   é 2+2?", r#"{"result": "4"}"#, 0, 0, FORK_TIMEOUT).await.unwrap();
        assert_eq!((r["question"].as_str(), r["answer"].as_str(), r["fonte"].as_str()), (Some("quanto é 2+2?"), Some("4"), Some("fork")));
        let argv: Vec<String> = ran(&c, "argv").unwrap().lines().map(str::to_owned).collect();
        assert_eq!(argv.last().unwrap(), "quanto é 2+2?");
        assert_eq!(argv[argv.iter().position(|a| a == "--model").unwrap() + 1], "haiku");
        assert_eq!(ran(&c, "conta").unwrap(), c.meta["config_dir"].as_str().unwrap());
        // O processo vê o caminho real: no macOS o temporário passa por um link (`/var` → `/private/var`).
        let cwd = std::fs::canonicalize(c.meta["cwd"].as_str().unwrap()).unwrap();
        assert_eq!(Path::new(ran(&c, "cwd").unwrap().trim()), cwd);
        let left: Vec<_> = std::fs::read_dir(project(&c)).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(left, ["sid-antigo.jsonl"], "só a conversa original fica");
    }

    /// B32 (`test_sem_terminal_sem_conversa_nao_roda_nada`): 409 sem transcript, 404 sem conversa.
    #[tokio::test]
    async fn without_a_conversation_nothing_runs() {
        let c = case(false, json!({}));
        assert_eq!(code(run(&c, "q", "{}", 0, 0, FORK_TIMEOUT).await), (409, "erro_btw_sem_conversa".into()));
        assert!(ran(&c, "argv").is_none());
        let c = case(true, json!({"session_id": null}));
        assert_eq!(code(run(&c, "q", "{}", 0, 0, FORK_TIMEOUT).await), (404, "erro_btw_sem_conversa".into()));
        assert_eq!(code(ask_headless(&Value::Null, "q", c.dir.path(), &Programs::default(), FORK_TIMEOUT).await), (404, "erro_btw_sem_conversa".into()));
        assert_eq!(code(ask_headless(&Value::Null, "  ", c.dir.path(), &Programs::default(), FORK_TIMEOUT).await), (400, "erro_btw_vazia".into()));
    }

    /// B32 (`test_sem_terminal_falha_do_cli_nao_vira_resposta`): código ≠ 0 é 502 e o fork some igual.
    #[tokio::test]
    async fn cli_failure_is_not_an_answer_and_still_cleans_up() {
        let c = case(true, json!({}));
        assert_eq!(code(run(&c, "q", "", 1, 0, FORK_TIMEOUT).await), (502, "erro_btw_fork_falhou".into()));
        assert_eq!(std::fs::read_dir(project(&c)).unwrap().count(), 1);
    }

    /// B32 (`test_sem_terminal_resposta_de_erro_nao_passa_por_resposta`).
    #[tokio::test]
    async fn error_reply_is_not_an_answer() {
        let c = case(true, json!({}));
        assert_eq!(code(run(&c, "q", r#"{"result": "Credit balance is too low", "is_error": true}"#, 0, 0, FORK_TIMEOUT).await), (502, "erro_btw_fork_falhou".into()));
    }

    /// B32 (`test_sem_terminal_que_nao_responde_a_tempo`): 504, e o fork some igual.
    #[tokio::test]
    async fn fork_that_does_not_answer_in_time_is_504() {
        let c = case(true, json!({}));
        assert_eq!(code(run(&c, "q", "{}", 0, 5, Duration::from_millis(300)).await).0, 504);
        assert_eq!(std::fs::read_dir(project(&c)).unwrap().count(), 1);
    }

    /// B31 (`test_sem_terminal_com_motor_passa_pelo_hangar_engine`): o motor recebe o argv inteiro.
    #[tokio::test]
    async fn engine_runs_through_hangar_engine() {
        let c = case(true, json!({"engine":"kimi","model":"k3","context_window":256000}));
        run(&c, "q", r#"{"result":"4"}"#, 0, 0, FORK_TIMEOUT).await.unwrap();
        let argv: Vec<String> = ran(&c, "argv").unwrap().lines().map(str::to_owned).collect();
        assert_eq!(argv[..6], ["--exec", "kimi", "--model", "k3", "--context", "256000"]);
        assert_eq!(argv[argv.iter().position(|a| a == "--").unwrap() + 1], "claude");
    }

    /// Claude com terminal responde pelo mesmo fork, na conversa do pane e na conta dele, sem tecla no pane:
    /// digitado, o /btw caía no plugin e a rota esperava um overlay que não abria.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_terminal_session_answers_through_the_fork_without_touching_the_pane() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let registry = Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new("instance-test".into()));
        let log = crate::session_write_control::open_terminal(&registry, dir.path(), "t").await;
        std::fs::create_dir_all(dir.path().join("projects/p")).unwrap();
        std::fs::write(dir.path().join("projects/p/sid.jsonl"), "{}\n").unwrap();
        let claude = fake_claude(dir.path(), r#"{"result":"4"}"#, 0, 0);
        let (_python, upstream) = crate::fake::spawn_fake().await;
        let mut state = hangar_server::routes::AppState::new(crate::fake::config(upstream, "127.0.0.1"));
        state.btw_programs = Programs { claude: claude.clone(), engine: claude };
        assert!(state.state.runtime.set(registry).is_ok());
        let server = crate::fake::spawn_state(state).await;
        let (status, reply) = crate::session_write_control::send(server, "POST", "t/btw", r#"{"question":"q"}"#).await;
        assert_eq!((status, reply["answer"].as_str(), reply["fonte"].as_str()), (200, Some("4"), Some("fork")), "{reply}");
        let argv: Vec<String> = std::fs::read_to_string(dir.path().join("rodou.argv")).unwrap().lines().map(str::to_owned).collect();
        assert_eq!(argv[argv.iter().position(|a| a == "--resume").unwrap() + 1], "sid");
        assert_eq!(std::fs::read_to_string(dir.path().join("rodou.conta")).unwrap(), dir.path().to_str().unwrap());
        assert_eq!(crate::session_write_control::keys_sent(&log), 0);
    }
}

// ── Entradas externas: nenhuma derruba o servidor ───────────────────────────────────────────────

proptest::proptest! {
    #[test]
    fn question_text_never_panics(screen in "\\PC*", raw in proptest::collection::vec(proptest::prelude::any::<char>(), 0..400)) {
        let raw: String = raw.into_iter().collect();
        for text in [screen.as_str(), raw.as_str()] {
            let _ = normalize(text);
        }
    }
    #[test]
    fn fork_output_and_body_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..600), text in "\\PC*") {
        let _ = fork_answer(&bytes);
        let _ = parse_body(&bytes);
        let _ = fork_answer(text.as_bytes());
        let _ = parse_body(format!(r#"{{"question":{}}}"#, Value::String(text.clone())).as_bytes());
    }
}
