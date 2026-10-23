//! `POST /api/sessions/{name}/rename` no Rust, com um multiplexador de mentira que guarda as sessões
//! num arquivo. O esperado vem dos testes Python do rename (`test_rename_*`) e do `decisoes.md` da
//! migração (B2, B3, B7, B10, B14, B18).
#![cfg(unix)]
use crate::fake;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fake::{OWNER, SECRET, client};
use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::service::{GroupService, JoinOwned};
use hangar_server::groups::store::PairDir;
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, router};
use hangar_server::runtime::gateway::RuntimeRegistry;
use serde_json::{Value, json};

struct Srv { addr: SocketAddr, root: PathBuf, home: PathBuf, claude: PathBuf, pair: PathBuf, groups: Arc<GroupService>, _dir: tempfile::TempDir }

/// Multiplexador de mentira: `sessions` tem uma sessão por linha (`nome<TAB>dono-do-atalho<TAB>escondida`).
fn mux(root: &Path) -> PathBuf {
    let script = root.join("tmux");
    crate::write_executable(&script, format!(r#"#!/bin/sh
S='{s}'
echo "$*" >> '{log}'
[ -e '{fail}' ] && exit 2
strip() {{ printf '%s' "$1" | sed 's/^=//; s/:$//'; }}
has() {{ cut -f1 "$S" | grep -qx "$1"; }}
case "$1" in
  has-session) has "$(strip "$3")" ;;
  rename-session)
    old=$(strip "$3"); new=$4
    [ -e '{refuse}' ] && exit 1
    has "$old" || exit 1
    has "$new" && exit 1
    awk -F'\t' -v o="$old" -v n="$new" 'BEGIN{{OFS="\t"}} $1==o{{$1=n}} {{print}}' "$S" > "$S.tmp" && mv "$S.tmp" "$S" ;;
  show-options) name=$(strip "$4"); awk -F'\t' -v n="$name" '$1==n && $3=="1"{{f=1}} END{{exit !f}}' "$S" && echo 1 ;;
  list-sessions) cut -f1,2 "$S" ;;
  set-option)
    name=$(strip "$3"); owner=$5
    awk -F'\t' -v n="$name" -v w="$owner" 'BEGIN{{OFS="\t"}} $1==n{{$2=w}} {{print}}' "$S" > "$S.tmp" && mv "$S.tmp" "$S" ;;
  kill-session) name=$(strip "$3"); grep -v "^$name	" "$S" > "$S.tmp"; mv "$S.tmp" "$S" ;;
  display-message) echo 1700000000 ;;
  *) exit 0 ;;
esac
"#, s = root.join("sessions").display(), log = root.join("calls.log").display(), fail = root.join("fail").display(),
        refuse = root.join("refuse").display()));
    script
}

async fn server() -> Srv {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let home = root.join("home");
    let claude = home.join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(root.join("sessions"), "").unwrap();
    let script = mux(&root);
    let dirs = parse_dirs(&json!({"home": home, "claude": claude, "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let (_python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, ""));
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into())));
    let pair = claude.join(".hangar-pair");
    let groups = Arc::new(GroupService::new(PairDir::new(pair.clone(), root.join("arquivo")), Arc::new(PythonOrq::from_state(&state)), "casa".into()));
    state.groups = Some(groups.clone());
    let runtime = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "s".into(), "i".into()));
    assert!(state.state.runtime.set(runtime).is_ok());
    let state = Arc::new(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state);
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    Srv { addr, root, home, claude, pair, groups, _dir: dir }
}

impl Srv {
    async fn group(&self, name: &str, other: &str) {
        let harness = [(name.to_owned(), "claude".to_owned()), (other.to_owned(), "claude".to_owned())].into_iter().collect();
        self.groups.join(JoinOwned { name: name.into(), others: vec![other.into()], task: String::new(), replace_task: false, harness,
            orq: false, outside: Default::default(), linked: false }).await.ok().unwrap();
    }
    fn sessions(&self, lines: &[&str]) { std::fs::write(self.root.join("sessions"), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap(); }
    fn names(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("sessions")).unwrap().lines().map(|l| l.split('\t').next().unwrap().to_owned()).collect()
    }
    fn line(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join("sessions")).unwrap().lines().find(|l| l.split('\t').next() == Some(name)).map(str::to_owned)
    }
    fn calls(&self, sub: &str) -> usize {
        std::fs::read_to_string(self.root.join("calls.log")).unwrap_or_default().lines().filter(|l| l.split(' ').next() == Some(sub)).count()
    }
    fn write(&self, path: PathBuf, value: Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn read(&self, path: PathBuf) -> Option<Value> { std::fs::read(path).ok().map(|raw| serde_json::from_slice(&raw).unwrap()) }
    async fn rename(&self, name: &str, body: Value) -> (u16, Value) {
        let resp = client().post(format!("http://{}/api/sessions/{name}/rename", self.addr)).bearer_auth(OWNER)
            .header("content-type", "application/json").body(body.to_string()).send().await.unwrap();
        let status = resp.status().as_u16();
        (status, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_session_takes_the_new_name_with_queue_then_and_bastao() {
    let srv = server().await;
    srv.sessions(&["velho\t\t"]);
    srv.write(srv.claude.join(".hangar-queue/velho.jsonl"), json!({"id": "e1"}));
    srv.write(srv.claude.join(".hangar-chain/velho.json"), json!({"target": "outra"}));
    std::fs::create_dir_all(srv.claude.join(".hangar-bastao")).unwrap();
    std::fs::write(srv.claude.join(".hangar-bastao/velho.md"), "dossiê").unwrap();
    assert_eq!(srv.rename("velho", json!({"new": "novo"})).await, (200, json!({"ok": true, "name": "novo"})));
    assert_eq!(srv.names(), ["novo"]);
    for (gone, there) in [(".hangar-queue/velho.jsonl", ".hangar-queue/novo.jsonl"), (".hangar-chain/velho.json", ".hangar-chain/novo.json"),
        (".hangar-bastao/velho.md", ".hangar-bastao/novo.md")] {
        assert!(!srv.claude.join(gone).exists() && srv.claude.join(there).exists(), "{there}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn name_is_sanitized_and_answered_back() {
    let srv = server().await;
    srv.sessions(&["velho\t\t"]);
    assert_eq!(srv.rename("velho", json!({"new": "São Paulo"})).await, (200, json!({"ok": true, "name": "Sao-Paulo"})));
    assert_eq!(srv.names(), ["Sao-Paulo"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn same_name_changes_nothing() {
    let srv = server().await;
    srv.sessions(&["velho\t\t"]);
    assert_eq!(srv.rename("velho", json!({"new": "velho"})).await, (200, json!({"ok": true, "name": "velho"})));
    assert_eq!(srv.calls("rename-session"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_names_and_missing_session_refuse_before_any_effect() {
    let srv = server().await;
    srv.sessions(&["velho\t\t"]);
    let (status, body) = srv.rename("velho", json!({"new": "日本"})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (400, json!("erro_nome_invalido")));
    let (status, body) = srv.rename("velho", json!({"new": "a".repeat(65)})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (400, json!("erro_nome_invalido")));
    assert_eq!(srv.rename("velho", json!({"new": "a".repeat(64)})).await.0, 200, "64 cabe");
    let (status, body) = srv.rename("sumida", json!({"new": "x"})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (404, json!("erro_sessao_inexistente")));
    let (status, body) = srv.rename("x", json!({"novo": "y"})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (422, json!("erro_corpo_invalido")));
}

#[tokio::test(flavor = "multi_thread")]
async fn occupied_new_name_refuses_for_every_kind_of_session() {
    let srv = server().await;
    srv.sessions(&["velho\t\t", "tmux-viva\t\t"]);
    srv.write(srv.home.join(".hangar/codex-sessions/cx.json"), json!({"name": "cx", "key": "k", "headless": true}));
    srv.write(srv.home.join(".hangar/claude-headless/hl.json"), json!({"name": "hl", "key": "k2"}));
    for taken in ["tmux-viva", "cx", "hl"] {
        let (status, body) = srv.rename("velho", json!({"new": taken})).await;
        assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_nome_em_uso")), "{taken}");
    }
    assert_eq!(srv.names(), ["velho", "tmux-viva"]);
    assert_eq!(srv.read(srv.home.join(".hangar/codex-sessions/cx.json")).unwrap()["key"], "k", "o sidecar do outro fica intacto");
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_sidecar_moves_without_touching_the_multiplexer() {
    let srv = server().await;
    srv.write(srv.home.join(".hangar/claude-headless/hl.json"), json!({"name": "hl", "key": "chave", "session_id": "sid"}));
    srv.write(srv.home.join(".hangar/codex-sessions/cx.json"), json!({"name": "cx", "key": "k", "headless": true, "thread_id": "t"}));
    assert_eq!(srv.rename("hl", json!({"new": "hl2"})).await.0, 200);
    assert_eq!(srv.rename("cx", json!({"new": "cx2"})).await.0, 200);
    assert_eq!(srv.read(srv.home.join(".hangar/claude-headless/hl2.json")).unwrap(), json!({"name": "hl2", "key": "chave", "session_id": "sid"}));
    assert_eq!(srv.read(srv.home.join(".hangar/codex-sessions/cx2.json")).unwrap(), json!({"name": "cx2", "key": "k", "headless": true, "thread_id": "t"}));
    assert!(srv.read(srv.home.join(".hangar/claude-headless/hl.json")).is_none());
    assert_eq!(srv.calls("rename-session"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn group_follows_the_name() {
    let srv = server().await;
    srv.sessions(&["velho\t\t", "par\t\t"]);
    srv.group("velho", "par").await;
    assert_eq!(srv.rename("velho", json!({"new": "novo"})).await.0, 200);
    assert!(srv.read(srv.pair.join("velho.json")).is_none());
    assert_eq!(srv.read(srv.pair.join("par.json")).unwrap()["peers"], json!(["novo"]));
}

/// B2/B7: o destino da fila ocupado por sobra de outra vida aparece no meio; tudo volta ao nome antigo.
#[tokio::test(flavor = "multi_thread")]
async fn failure_in_the_middle_undoes_multiplexer_and_group() {
    let srv = server().await;
    srv.sessions(&["velho\t\t", "par\t\t"]);
    srv.group("velho", "par").await;
    srv.write(srv.claude.join(".hangar-queue/velho.jsonl"), json!({"id": "meu"}));
    srv.write(srv.claude.join(".hangar-queue/novo.jsonl"), json!({"id": "sobra"}));
    let (status, body) = srv.rename("velho", json!({"new": "novo"})).await;
    assert_eq!((status, body["detail"]["code"].clone(), body["detail"]["params"]["etapa"].clone()), (409, json!("erro_nome_em_uso"), json!("queue")));
    assert_eq!(body["detail"]["params"]["nao_desfeito"], json!([]));
    assert_eq!(srv.names(), ["velho", "par"]);
    assert_eq!(srv.read(srv.pair.join("par.json")).unwrap()["peers"], json!(["velho"]));
    assert_eq!(srv.read(srv.claude.join(".hangar-queue/velho.jsonl")).unwrap()["id"], "meu");
    assert_eq!(srv.read(srv.claude.join(".hangar-queue/novo.jsonl")).unwrap()["id"], "sobra");
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_multiplexer_rename_is_500_and_nothing_stays_renamed() {
    let srv = server().await;
    srv.sessions(&["velho\t\t"]);
    std::fs::write(srv.root.join("refuse"), "").unwrap();
    let (status, body) = srv.rename("velho", json!({"new": "novo"})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (500, json!("sessao_falha_renomear")));
    assert_eq!(srv.names(), ["velho"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_shell_and_shortcut_terminals_follow() {
    let srv = server().await;
    srv.sessions(&["velho\t\t", "term-velho\t\t1", "atalho-1\tvelho\t"]);
    assert_eq!(srv.rename("velho", json!({"new": "novo"})).await.0, 200);
    assert_eq!(srv.line("term-novo").as_deref(), Some("term-novo\t\t1"), "o shell escondido é o mesmo, com o nome novo");
    assert_eq!(srv.line("atalho-1").as_deref(), Some("atalho-1\tnovo\t"));
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_shell_of_the_new_name_already_there_kills_the_old_one() {
    let srv = server().await;
    srv.sessions(&["velho\t\t", "term-velho\t\t1", "term-novo\t\t1"]);
    assert_eq!(srv.rename("velho", json!({"new": "novo"})).await.0, 200);
    assert_eq!(srv.names(), ["novo", "term-novo"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_owner_follows_the_rename() {
    let srv = server().await;
    srv.sessions(&["velho\t\t"]);
    srv.write(srv.claude.join("guests.json"), json!({"guests": [], "sessions": {"velho": {"guest": "g1", "life": "t:1"}}}));
    assert_eq!(srv.rename("velho", json!({"new": "novo"})).await.0, 200);
    assert_eq!(srv.read(srv.claude.join("guests.json")).unwrap()["sessions"], json!({"novo": {"guest": "g1", "life": "t:1"}}));
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_in_progress_refuses() {
    let srv = server().await;
    srv.sessions(&["velho\t\t"]);
    let id = "11111111-2222-3333-4444-555555555555";
    let base = srv.home.join(".hangar/conversation-transfers");
    srv.write(base.join(format!("{id}.json")), json!({"id": id, "name": "velho", "phase": "source_stopped"}));
    let digest: String = ring::digest::digest(&ring::digest::SHA256, b"velho").as_ref().iter().map(|b| format!("{b:02x}")).collect();
    srv.write(base.join(format!("indexes/session-{digest}.json")), json!([id]));
    let (status, body) = srv.rename("velho", json!({"new": "novo"})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("session_transfer_busy")));
    assert_eq!(srv.names(), ["velho"]);
}
