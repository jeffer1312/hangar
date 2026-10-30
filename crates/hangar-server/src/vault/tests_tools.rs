//! O uso pelo agente: saída mascarada, arquivo 0600 que some, negação, orçamento, nova tentativa na
//! falha de rede e o pedido cancelado quando a chamada larga a espera.
use super::*;
use crate::{auth::TrustedHosts, config::Config, routes::AppState};
use base64::Engine;
use exec::{MASK, Owner, Pending, WaitFail, settle};
use futures_util::future::BoxFuture;
use remote::WaitStatus;
use requests::{Answer, Decision, RequestError};
use std::{sync::Mutex, time::Duration};
use tokio::time::Instant;
use wire::{AskReq, UseKind, UseReq};

const VALUE: &str = "s3nh@ Fo+/x?=é";

#[derive(Default)]
struct Keys(Mutex<Option<[u8; 32]>>);

impl KeyStore for Keys {
    fn read(&self) -> BoxFuture<'_, Result<Option<Key>, KeyError>> {
        let key = self.0.lock().unwrap().map(Zeroizing::new);
        Box::pin(async move { Ok(key) })
    }
    fn write(&self, key: Key) -> BoxFuture<'_, Result<(), KeyError>> {
        *self.0.lock().unwrap() = Some(*key);
        Box::pin(async { Ok(()) })
    }
}

async fn owner_state() -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config { listen: "127.0.0.1:9".parse().unwrap(), upstream: "127.0.0.1:9".parse().unwrap(),
        auth_token: "dono".into(), internal_secret: "privado".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
    state.hangar_dir = dir.path().join("hangar");
    std::fs::create_dir_all(&state.hangar_dir).unwrap();
    state.server_id = "dono".into();
    state.vault = Arc::new(Vault::new(Arc::new(Keys::default())));
    state.vault.create(&state.hangar_dir).await.unwrap();
    (Arc::new(state), dir)
}

fn use_req(entry: &str) -> UseReq {
    UseReq { origin_server: "dono".into(), session_key: "k1".into(), session: "s1".into(), entry: entry.into(), kind: UseKind::Exec,
        reason: "deploy".into(), target: "wrangler whoami".into() }
}

fn ask_req() -> AskReq {
    AskReq { origin_server: "dono".into(), session_key: "k1".into(), session: "s1".into(), label: "token".into(), reason: "deploy".into(),
        placeholder: None }
}

#[test]
fn the_value_is_masked_literal_in_base64_and_url_encoded() {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    let forms = [VALUE.to_owned(), STANDARD.encode(VALUE), STANDARD_NO_PAD.encode(VALUE), URL_SAFE.encode(VALUE), URL_SAFE_NO_PAD.encode(VALUE),
        percent_encoding::utf8_percent_encode(VALUE, percent_encoding::NON_ALPHANUMERIC).to_string(),
        "s3nh%40%20Fo%2B%2Fx%3F%3D%C3%A9".to_owned()];
    for form in &forms {
        let out = exec::mask(&format!("antes {form} depois"), VALUE);
        assert_eq!(out.as_str(), format!("antes {MASK} depois"), "forma {form}");
    }
    assert_eq!(exec::mask("nada aqui", VALUE).as_str(), "nada aqui");
    // Formulário com `+`, hex minúsculo e JSON (com e sem `\u` no não-ASCII).
    for form in ["s3nh%40+Fo%2B%2Fx%3F%3D%C3%A9", "s3nh%40%20Fo%2b%2fx%3f%3d%c3%a9", "s3nh@ Fo+/x?=\\u00e9"] {
        assert_eq!(exec::mask(&format!("[{form}]"), VALUE).as_str(), format!("[{MASK}]"), "forma {form}");
    }
    assert_eq!(exec::mask("{\"k\":\"a\\\"b\\\\c\"}", "a\"b\\c").as_str(), format!("{{\"k\":\"{MASK}\"}}"));
}

/// Basic auth: o base64 de `usuario:senha` não começa com a senha, e o miolo dela é achado em
/// qualquer alinhamento.
#[test]
fn base64_of_the_value_inside_other_content_is_masked() {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    let value = "token-secreto-9Qz";
    for prefix in ["u:", "us:", "usr:", "joao.silva:"] {
        for encoded in [STANDARD.encode(format!("{prefix}{value}")), URL_SAFE_NO_PAD.encode(format!("{prefix}{value}xyz"))] {
            let out = exec::mask(&format!("Basic {encoded}"), value);
            let (before, after) = out.strip_prefix("Basic ").and_then(|o| o.split_once(MASK)).expect("mascarado");
            // Antes do miolo, só o prefixo (e um caractere misto); depois, o resto do bloco e o sufixo.
            assert!(before.len() <= prefix.len() * 4 / 3 + 2 && after.len() <= 6, "{prefix}: {before} | {after}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_command_never_inherits_session_or_server_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let value = Zeroizing::new(VALUE.to_owned());
    let env = vec![("PATH".into(), std::env::var_os("PATH").unwrap_or_default()), ("TYPESAFE_API_KEY".into(), "jev".into()),
        ("HANGAR_CANO_KEY".into(), "cano".into()), ("HANGAR_INTERNAL_SECRET".into(), "srv".into()), ("FICA".into(), "sim".into())];
    let (_, out) = exec::run("printf '%s|%s|%s|%s' \"${TYPESAFE_API_KEY:-x}\" \"${HANGAR_CANO_KEY:-x}\" \"${HANGAR_INTERNAL_SECRET:-x}\" \"$FICA\"",
        dir.path(), env, "TOKEN", &value, Duration::from_secs(20)).await;
    assert_eq!(out, "x|x|x|sim");
}

/// Chamada largada no meio: o neto que segura o valor morre junto.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_dropped_exec_kills_the_whole_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("neto.pid");
    let value = Zeroizing::new(VALUE.to_owned());
    let env = vec![("PATH".into(), std::env::var_os("PATH").unwrap_or_default())];
    let command = format!("sleep 300 & echo $! > {}; wait", pidfile.display());
    let run = exec::run(&command, dir.path(), env, "TOKEN", &value, Duration::from_secs(300));
    assert!(tokio::time::timeout(Duration::from_millis(500), run).await.is_err());
    let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_owned();
    for _ in 0..50 {
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if state.is_empty() || state.contains(") Z") { return; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("o neto {pid} sobreviveu à chamada largada");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_runs_with_the_value_in_the_env_and_masks_the_output() {
    let dir = tempfile::tempdir().unwrap();
    let value = Zeroizing::new(VALUE.to_owned());
    let env = vec![("PATH".into(), std::env::var_os("PATH").unwrap_or_default()), ("CP_AUTH_TOKEN".into(), "dono".into())];
    let (exit, out) = exec::run("printf '%s|%s' \"$TOKEN\" \"${CP_AUTH_TOKEN:-sem}\"; printf '%s' \"$TOKEN\" | base64 >&2; exit 3",
        dir.path(), env, "TOKEN", &value, Duration::from_secs(20)).await;
    assert_eq!(exit, 3);
    assert!(!out.contains(VALUE) && !out.contains(&base64::engine::general_purpose::STANDARD.encode(VALUE)), "{out}");
    assert!(out.starts_with(&format!("{MASK}|sem")), "segredo do servidor não chega ao comando: {out}");
    let (code, out) = exec::run("sleep 30", dir.path(), Vec::new(), "TOKEN", &value, Duration::from_millis(200)).await;
    assert_eq!((code, out.as_str()), (124, "timeout 0s"));
}

#[test]
fn a_file_goes_at_close_and_in_the_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("vault-files");
    let mine = files::write(&base, "k1", "s1", VALUE.as_bytes()).unwrap();
    let other = files::write(&base, "k2", "s2", VALUE.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&mine).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&base).unwrap().permissions().mode() & 0o777, 0o700);
    }
    assert_eq!(std::fs::read(&mine).unwrap(), VALUE.as_bytes());
    files::close(&base, "renomeada", Some("k1"));
    assert!(!mine.exists(), "a chave da sessão fecha o arquivo dela mesmo com outro nome");
    assert!(other.exists());
    // Viva na lista: fica; fora dela ou com mais de 24 h: sai.
    let alive: &dyn Fn(&files::Meta) -> bool = &|m| m.session == "s2";
    files::sweep(&base, now(), Some(alive));
    assert!(other.exists());
    files::sweep(&base, now() + files::LIFE.as_secs_f64() + 1.0, Some(alive));
    assert!(!other.exists(), "24 h");
    let third = files::write(&base, "k3", "s3", VALUE.as_bytes()).unwrap();
    files::sweep(&base, now(), Some(alive));
    assert!(!third.exists(), "sessão fora da lista");
    std::fs::write(base.join("0123"), VALUE).unwrap();
    files::sweep(&base, now(), None);
    assert!(!base.join("0123").exists(), "arquivo sem ficha sai");
    assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0, "nenhuma ficha sobra");
}

#[tokio::test]
async fn a_denied_use_is_erro_cofre_negado() {
    let (st, _dir) = owner_state().await;
    st.vault.add(&st.hangar_dir, NewEntry { name: "cf".into(), username: None, password: Zeroizing::new(VALUE.into()), url: None,
        notes: None, policy: Policy::Ask }).await.unwrap();
    let mut feed = st.vault_requests.feed();
    let owner = Owner::Local(st.clone());
    let asked = tokio::spawn(async move { exec::secret(&owner, use_req("cf"), Instant::now() + exec::BUDGET).await });
    let card = feed.wait_for(|f| !f.requests.is_empty()).await.unwrap().requests[0].clone();
    let deny = Answer { interaction_id: card.interaction_id, decision: Decision::Deny, password: None, keep: None, name: None, policy: None };
    st.vault_requests.answer(&st.vault, &st.hangar_dir, &card.id, deny).await.unwrap();
    let text = asked.await.unwrap().err().unwrap();
    assert!(text.starts_with("erro_cofre_negado:"), "{text}");
    assert!(!text.contains(VALUE));
}

#[tokio::test]
async fn one_session_opens_at_most_three_cards_and_waits_after_a_refusal() {
    let (st, _dir) = owner_state().await;
    let ids: Vec<String> = (0..3).map(|_| requests::ask_entry(&st, ask_req()).unwrap()).collect();
    assert_eq!(requests::ask_entry(&st, ask_req()).err(), Some(RequestError::TooMany));
    assert!(requests::ask_entry(&st, AskReq { session_key: "k2".into(), ..ask_req() }).is_ok(), "outra sessão abre");
    let card = st.vault_requests.feed().borrow().requests.iter().find(|c| c.id == ids[0]).unwrap().clone();
    let decline = Answer { interaction_id: card.interaction_id, decision: Decision::Decline, password: None, keep: None, name: None, policy: None };
    st.vault_requests.answer(&st.vault, &st.hangar_dir, &card.id, decline).await.unwrap();
    assert_eq!(requests::ask_entry(&st, ask_req()).err(), Some(RequestError::Cooldown), "recusada há pouco");
}

#[tokio::test(start_paused = true)]
async fn the_budget_ends_the_wait_as_timed_out() {
    let started = Instant::now();
    let pending = || async { tokio::time::sleep(wire::WAIT_HOLD).await; Ok::<_, WaitFail>(WaitStatus::Pending) };
    let status = settle(started + exec::BUDGET, pending).await.unwrap();
    assert!(matches!(status, WaitStatus::TimedOut));
    assert!(started.elapsed() >= exec::BUDGET && started.elapsed() < exec::BUDGET + wire::WAIT_HOLD);
}

#[tokio::test(start_paused = true)]
async fn a_network_failure_in_the_wait_is_tried_again_within_60s() {
    let calls = std::sync::atomic::AtomicU32::new(0);
    let flaky = || {
        let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async move {
            match n {
                0 | 1 => Err(WaitFail::Retry("erro_cofre_dona_fora: sem rede".into())),
                2 => Ok(WaitStatus::Pending),
                _ => Ok(WaitStatus::Allowed(Zeroizing::new(VALUE.into()))),
            }
        }
    };
    assert!(matches!(settle(Instant::now() + exec::BUDGET, flaky).await, Ok(WaitStatus::Allowed(_))));
    // Rede caída por mais de 60 s depois da última espera boa: desiste com o motivo.
    let down = || async { Err::<WaitStatus, _>(WaitFail::Retry("erro_cofre_dona_fora: sem rede".into())) };
    let started = Instant::now();
    let text = settle(started + exec::BUDGET, down).await.err().unwrap();
    assert!(text.starts_with("erro_cofre_dona_fora:") && started.elapsed() >= exec::RETRY_WINDOW, "{text}");
    let stop = || async { Err::<WaitStatus, _>(WaitFail::Stop("erro_cofre_pedido_perdido: x".into())) };
    assert_eq!(settle(Instant::now() + exec::BUDGET, stop).await.err().as_deref(), Some("erro_cofre_pedido_perdido: x"));
}

#[tokio::test]
async fn a_dropped_wait_cancels_the_request_at_the_owner() {
    let (st, _dir) = owner_state().await;
    let id = requests::ask_entry(&st, ask_req()).unwrap();
    let wait = Pending::new(Owner::Local(st.clone()), id.clone()).settle(Instant::now() + exec::BUDGET);
    assert!(tokio::time::timeout(Duration::from_millis(50), wait).await.is_err(), "ainda sem resposta");
    assert!(matches!(st.vault_requests.wait(&id, Duration::ZERO).await, Err(RequestError::Lost)), "o futuro largado cancela o pedido");
    assert!(st.vault_requests.feed().borrow().requests.is_empty(), "o cartão some");
    // Orçamento estourado também cancela.
    let id = requests::ask_entry(&st, ask_req()).unwrap();
    let status = Pending::new(Owner::Local(st.clone()), id.clone()).settle(Instant::now() + Duration::from_millis(50)).await.unwrap();
    assert!(matches!(status, WaitStatus::TimedOut));
    assert!(matches!(st.vault_requests.wait(&id, Duration::ZERO).await, Err(RequestError::Lost)));
}

#[test]
fn a_session_born_without_the_long_deadline_is_detected_where_readable() {
    let env = |pairs: &[(&str, &str)]| -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    };
    assert!(exec::lacks_deadline("claude", Some(&env(&[("PATH", "/bin")])), None));
    assert!(!exec::lacks_deadline("claude", Some(&env(&[(exec::CLAUDE_IDLE_VAR, exec::CLAUDE_IDLE_MS)])), None));
    assert!(!exec::lacks_deadline("claude", None, None), "ambiente não lido do processo segue");
    let registered = "[mcp_servers.hangar]\nurl = \"http://127.0.0.1:8765/mcp/\"\ntool_timeout_sec = 660\n";
    assert!(!exec::lacks_deadline("codex", None, Some(registered)));
    assert!(exec::lacks_deadline("codex", None, Some("[mcp_servers.hangar]\nurl = \"http://127.0.0.1:8765/mcp/\"\n")));
    assert!(exec::lacks_deadline("codex", None, Some("[mcp_servers.hangar]\ntool_timeout_sec = 60\n")), "o padrão do Codex");
    assert!(!exec::lacks_deadline("codex", None, None), "config.toml não lido segue");
    assert!(exec::env_name_ok("CF_TOKEN") && !exec::env_name_ok("1X") && !exec::env_name_ok("A-B") && !exec::env_name_ok("CP_AUTH_TOKEN"));
}
