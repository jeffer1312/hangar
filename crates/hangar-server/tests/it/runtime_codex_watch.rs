//! Vigia do Codex com terminal (`codex_terminal::Watcher`): as rodadas chamadas direto, sobre um tmux falso
//! (lista de panes num arquivo), sidecars numa pasta temporária e app-servers falsos. Nenhuma sessão viva.
use crate::runtime_open::{app_server_process,fake_app_server,registry};
use hangar_server::accounts::environment::AccountEnvironment;
use hangar_server::list::mux::Mux;
use hangar_server::runtime::codex_relaunch::PermissionRead;
use hangar_server::runtime::codex_terminal::{WatchConfig,Watcher};
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{ClockSample,RuntimeError};
use hangar_server::runtime::queue::Action;
use serde_json::{Value,json};
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::time::{Duration,Instant};

pub(crate) struct Fixture { _dir:tempfile::TempDir, home:PathBuf, queue:PathBuf, panes:PathBuf, tmux:PathBuf, root:PathBuf }

/// `list-panes` lê o arquivo (com `.down` ao lado, o multiplexador não responde); `has-session` procura o
/// nome nele e `kill-session` o tira.
pub(crate) fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let (home,queue,panes,tmux) = (root.join("home"),root.join("claude").join(".hangar-queue"),root.join("panes"),root.join("fake-tmux"));
    std::fs::write(&panes,"").unwrap();
    let p = panes.display();
    crate::write_executable(&tmux,format!("#!/bin/sh\nP='{p}'\ncase \"$1\" in\n  list-panes) [ -f \"$P.down\" ] && exit 2; cat \"$P\";;\n  \
        has-session) n=\"${{3#=}}\"; grep -q \"^$n\t\" \"$P\";;\n  \
        kill-session) n=\"${{3#=}}\"; grep -v \"^$n\t\" \"$P\" > \"$P.new\"; mv \"$P.new\" \"$P\";;\n  \
        display-message) printf 'cx-proof\t%%7\t1700000000\\n';;\n  \
        capture-pane) cat \"$P.screen\";;\n  \
        show-environment) [ -f \"$P.env\" ] && cat \"$P.env\" || exit 1;;\n  \
        send-keys) echo \"$@\" >> \"$P.keys\"; if [ -f \"$P.after\" ]; then cp \"$P.after\" \"$P.screen\"; fi;;\n  *) exit 1;;\nesac\n"));
    Fixture { _dir:dir,home,queue,panes,tmux,root }
}

impl Fixture {
    pub(crate) fn watcher(&self) -> Arc<Watcher> {
        Arc::new(Watcher::new(WatchConfig { home:self.home.clone(),queue_dir:self.queue.clone(),
            mux:Mux::with_program(&self.tmux,Duration::from_secs(5)),mux_program:self.tmux.clone().into_os_string() })
            .with_death_after(DEATH_AFTER))
    }
    fn sidecar_dir(&self) -> PathBuf { self.home.join(".hangar").join("codex-sessions") }
    fn sidecar_path(&self,name:&str) -> PathBuf { self.sidecar_dir().join(format!("{name}.json")) }
    /// Grava por rename, como o Python: a pasta muda e a leitura em cache da lista vê a versão nova.
    pub(crate) fn put_sidecar(&self,name:&str,meta:Value) {
        std::fs::create_dir_all(self.sidecar_dir()).unwrap();
        let tmp = self.sidecar_dir().join(format!("{name}.tmp"));
        std::fs::write(&tmp,meta.to_string()).unwrap();
        std::fs::rename(tmp,self.sidecar_path(name)).unwrap();
    }
    fn sidecar(&self,name:&str) -> Option<Value> { std::fs::read(self.sidecar_path(name)).ok().map(|raw|serde_json::from_slice(&raw).unwrap()) }
    pub(crate) fn panes(&self,names:&[&str]) {
        let lines:String = names.iter().map(|name|format!("{name}\t1\t4242\t/w\t%7\t\tcodex\t1700000000\t0\t0\n")).collect();
        std::fs::write(&self.panes,lines).unwrap();
    }
    /// Panes com id e nascimento escolhidos: (sessão, pane, nascimento).
    fn panes_born(&self,panes:&[(&str,&str,u64)]) {
        let lines:String = panes.iter().map(|(name,id,born)|format!("{name}\t1\t4242\t/w\t{id}\t\tcodex\t{born}\t0\t0\n")).collect();
        std::fs::write(&self.panes,lines).unwrap();
    }
    /// A `CP_SESSION_KEY` que o lançador põe no ambiente da sessão do multiplexador.
    fn pane_key(&self,key:&str) { std::fs::write(self.panes.with_extension("env"),format!("CP_SESSION_KEY={key}\n")).unwrap(); }
    fn queue_json(&self,key:&str) -> Value { serde_json::from_slice(&std::fs::read(self.queue_state(key)).unwrap()).unwrap() }
    fn queue_state(&self,key:&str) -> PathBuf { self.queue.join("runtime").join(format!("{key}.json")) }
    pub(crate) fn meta(&self,name:&str,endpoint:&str,app_pid:Option<u32>,tui:&Child) -> Value {
        json!({"name":name,"provider":"codex","thread_id":"thread-1","rollout_path":null,"cwd":self.root,"model":null,"effort":null,
            "endpoint":endpoint,"app_pid":app_pid,"tui_pid":tui.id(),"codex_home":self.root.join("codex-home")})
    }
}

/// Prazo de ausência da vigia nos testes.
const DEATH_AFTER:Duration = Duration::from_millis(300);
const CLOCK:ClockSample = ClockSample { monotonic_s:0.0,epoch_s:1000.0 };

pub(crate) fn tui() -> Child { std::process::Command::new("sh").args(["-c","sleep 120; true","codex-tui"]).spawn().unwrap() }

fn exited(child:&mut Child) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() { return true; }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

pub(crate) fn finish(children:&mut [&mut Child]) { for child in children { let _ = child.kill(); let _ = child.wait(); } }

fn key_of(meta:&Value) -> String { meta["key"].as_str().expect("chave gravada na adoção").to_owned() }

#[tokio::test]
async fn a_round_adopts_follows_a_new_app_server_and_buries_the_dead_pane() {
    let fx = fixture();
    let (first,second) = (fake_app_server(true).await,fake_app_server(true).await);
    let (mut app1,mut app2,mut tui) = (app_server_process(&first.endpoint),app_server_process(&second.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&first.endpoint,Some(app1.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    watcher.round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-proof"),"pane vivo e sidecar pronto: adotado sem o Python");
    first.wait_method(1,"initialize").await;
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    let state:Value = serde_json::from_slice(&std::fs::read(fx.queue_state(&key)).unwrap()).unwrap();
    assert_eq!(state["generation"],1);
    assert_eq!(state["runtime_state"]["codex_life"]["tui_pid"],tui.id());
    watcher.round(&registry).await;
    assert_eq!(first.connections.load(std::sync::atomic::Ordering::SeqCst),1,"a mesma vida não reabre");

    // O lançador ressubiu o app-server: mesma chave, geração nova, e o antigo não é morto pela vigia.
    let mut meta = fx.meta("cx-proof",&second.endpoint,Some(app2.id()),&tui);
    meta["key"] = json!(key);
    fx.put_sidecar("cx-proof",meta);
    watcher.round(&registry).await;
    assert!(registry.handle(&key,1).await.is_err(),"a vida antiga saiu");
    assert!(registry.handle(&key,2).await.is_ok(),"a vida nova abriu na mesma chave");
    second.wait_method(1,"initialize").await;
    assert!(app1.try_wait().unwrap().is_none());

    // Pane morto e TUI morta pelo prazo, depois ator, app-server, sidecar e fila desta vida.
    fx.panes(&[]);
    tui.kill().unwrap(); tui.wait().unwrap();
    watcher.round(&registry).await;
    watcher.round(&registry).await;
    assert!(registry.handle(&key,2).await.is_ok(),"rodadas seguidas (o `wake`) não encerram antes do prazo");
    tokio::time::sleep(DEATH_AFTER + Duration::from_millis(50)).await;
    watcher.round(&registry).await;
    assert!(registry.handle(&key,2).await.is_err());
    assert!(!registry.owns_codex_terminal("cx-proof"));
    assert!(exited(&mut app2),"o app-server desta vida recebeu o SIGTERM");
    assert!(fx.sidecar("cx-proof").is_none(),"sidecar da vida apagado");
    assert!(!fx.queue_state(&key).exists(),"fila da vida apagada");
    finish(&mut [&mut app1,&mut app2]);
}

#[tokio::test]
async fn a_message_typed_before_the_adoption_is_kept_in_the_queue() {
    // A TUI ainda abria: o Python gravou a mensagem na fila pelo nome. Adotar não pode apagá-la.
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    std::fs::create_dir_all(&fx.queue).unwrap();
    std::fs::write(fx.queue.join("cx-proof.jsonl"),format!("{}\n",json!({"id":"early","text":"oi antes da TUI","ts":1.0e12,"delivered":false}))).unwrap();
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    fx.watcher().round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-proof"));
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    let rows = fx.queue_json(&key)["rows"].clone();
    assert!(rows.as_array().unwrap().iter().any(|row|row["id"] == "early" && row["text"] == "oi antes da TUI"),"{rows}");
    let projection = std::fs::read_to_string(fx.queue.join("cx-proof.jsonl")).unwrap();
    assert!(projection.contains("early"),"a projeção regravada mantém a mensagem: {projection}");
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn reconnecting_follows_the_conversation_the_session_file_moved_to() {
    // O socket caiu e, nessa janela, a TUI abriu outra conversa (`/new`): o lançador gravou a nova no
    // arquivo da sessão, e a reconexão assina ela, não a antiga (o `ensure_running` do Python).
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    fx.watcher().round(&registry).await;
    server.wait_method(1,"thread/resume").await;
    let mut meta = fx.sidecar("cx-proof").unwrap();
    meta["thread_id"] = json!("thread-2");
    meta["rollout_path"] = json!(fx.root.join("rollout-2.jsonl"));
    fx.put_sidecar("cx-proof",meta);
    server.drop_connection(1);
    let resume = server.wait_method(2,"thread/resume").await;
    assert_eq!(resume["params"]["threadId"],"thread-2");
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    let handle = registry.handle(&key,1).await.unwrap();
    assert_eq!(handle.view().await.unwrap()["thread_id"],"thread-2");
    finish(&mut [&mut app,&mut tui]);
}

#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_queue_projection_refuses_the_adoption_instead_of_overwriting_it() {
    use std::os::unix::fs::PermissionsExt;
    if std::fs::metadata("/proc/self").map(|m|std::os::unix::fs::MetadataExt::uid(&m)==0).unwrap_or(false) { return; }
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    std::fs::create_dir_all(&fx.queue).unwrap();
    let projection = fx.queue.join("cx-proof.jsonl");
    let typed = format!("{}\n",json!({"id":"early","text":"oi antes da TUI","ts":1.0e12,"delivered":false}));
    std::fs::write(&projection,&typed).unwrap();
    std::fs::set_permissions(&projection,std::fs::Permissions::from_mode(0o000)).unwrap();
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    let adopted = watcher.adopt_now(&registry,"cx-proof").await;
    std::fs::set_permissions(&projection,std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(adopted.map(|r|r.map_err(|e|e.code)),Some(Err("codex_queue_projection_unreadable".into())));
    assert_eq!(std::fs::read_to_string(&projection).unwrap(),typed,"a mensagem digitada antes não foi apagada");
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn a_terminal_stuck_launching_stops_going_to_python() {
    // `launching` é a TUI abrindo: a escrita vai ao Python (que enfileira) só por um tempo.
    let fx = fixture();
    fx.put_sidecar("cx-proof",json!({"name":"cx-proof","provider":"codex","launching":true,"key":"k-l","cwd":fx.root}));
    let registry = registry().await;
    let watcher = fx.watcher();
    let young = watcher.adopt_now(&registry,"cx-proof").await;
    assert_eq!(young.map(|r|r.map_err(|e|e.code)),Some(Err("codex_terminal_starting".into())),"recém-aberto: o Rust pede para esperar");
    let old = std::time::SystemTime::now() - Duration::from_secs(600);
    std::fs::File::options().write(true).open(fx.sidecar_path("cx-proof")).unwrap().set_modified(old).unwrap();
    let stuck = watcher.adopt_now(&registry,"cx-proof").await;
    assert_eq!(stuck.map(|r|r.map_err(|e|e.code)),Some(Err("codex_terminal_launch_stuck".into())));
}

fn now_s() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() }

async fn post_owner(server:std::net::SocketAddr,name:&str,route:&str,body:&str) -> (u16,Value) {
    let response = crate::fake::client().post(format!("http://{server}/api/sessions/{name}/{route}"))
        .header("authorization",format!("Bearer {}",crate::fake::OWNER)).header("content-type","application/json")
        .body(body.to_owned()).send().await.unwrap();
    let status = response.status().as_u16();
    (status,serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
}

fn queued(fx:&Fixture,key:&str,text:&str) -> bool {
    fx.queue_state(key).exists() && fx.queue_json(key)["rows"].as_array().unwrap().iter()
        .any(|row|row["text"] == text && row["delivered"] == false && row["pre_transcript"] == true)
}

/// Conversa Codex nova: o pane abriu e o lançador ainda não gravou o sidecar. O texto fica na fila do Rust
/// na hora (sem esperar a TUI nem digitar no pane) e a adoção o entrega pelo app-server.
#[tokio::test]
async fn text_to_a_codex_terminal_still_opening_waits_in_the_rust_queue() {
    let fx = fixture();
    fx.panes_born(&[("cx-new","%7",now_s())]);
    fx.pane_key("k-new");
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    let (python,server) = serve_owner(registry.clone()).await;
    let started = Instant::now();
    let (status,body) = post_owner(server,"cx-new","input",r#"{"text":"oi antes da TUI","steer":true}"#).await;
    assert_eq!((status,body["ok"].clone(),body["delivered"].clone()),(200,json!(true),json!(false)),"{body}");
    assert!(started.elapsed() < Duration::from_secs(3),"responde sem esperar a TUI: {:?}",started.elapsed());
    assert_eq!(python.hits_to("/api/sessions/cx-new/input"),0,"nada vai ao Python");
    assert!(!fx.panes.with_extension("keys").exists(),"nenhuma tecla no pane");
    assert!(queued(&fx,"k-new","oi antes da TUI"),"{}",fx.queue_json("k-new"));
    let app_server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&app_server.endpoint),tui());
    let mut meta = fx.meta("cx-new",&app_server.endpoint,Some(app.id()),&tui);
    meta["key"] = json!("k-new");
    fx.put_sidecar("cx-new",meta);
    watcher.round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-new"));
    let turn = app_server.wait_method(1,"turn/start").await;
    assert!(turn.to_string().contains("oi antes da TUI"),"{turn}");
    finish(&mut [&mut app,&mut tui]);
}

/// Sidecar `launching` (transferência importando): o texto vai à fila da chave do sidecar, no Rust.
#[tokio::test]
async fn text_to_a_launching_codex_terminal_waits_in_the_rust_queue() {
    let fx = fixture();
    fx.panes_born(&[("cx-l","%7",now_s())]);
    fx.put_sidecar("cx-l",json!({"name":"cx-l","provider":"codex","launching":true,"key":"k-l","cwd":fx.root}));
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    let (python,server) = serve_owner(registry).await;
    let (status,body) = post_owner(server,"cx-l","input",r#"{"text":"durante a importação"}"#).await;
    assert_eq!((status,body["delivered"].clone()),(200,json!(false)),"{body}");
    assert_eq!(python.hits_to("/api/sessions/cx-l/input"),0);
    assert!(queued(&fx,"k-l","durante a importação"),"{}",fx.queue_json("k-l"));
}

/// O que não é texto, com a conversa abrindo, pede para esperar; abertura presa é erro com código. Nenhum
/// dos dois vai ao Python, e comando de barra não vira texto de turno.
#[tokio::test]
async fn a_codex_terminal_still_opening_refuses_what_is_not_text_and_reports_a_stuck_launch() {
    let fx = fixture();
    fx.panes_born(&[("cx-new","%7",now_s()),("cx-old","%8",now_s() - 600)]);
    fx.pane_key("k-new");
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    let (python,server) = serve_owner(registry).await;
    for (route,body) in [("interrupt","{}"),("input",r#"{"text":"/compact"}"#)] {
        let (status,reply) = post_owner(server,"cx-new",route,body).await;
        assert_eq!((status,reply["detail"]["code"].as_str()),(409,Some("codex_terminal_starting")),"{route}: {reply}");
        assert_eq!(python.hits_to(&format!("/api/sessions/cx-new/{route}")),0,"{route}");
    }
    assert!(!fx.queue_state("k-new").exists(),"nada entrou na fila");
    let (status,reply) = post_owner(server,"cx-old","input",r#"{"text":"oi"}"#).await;
    assert_eq!((status,reply["detail"]["code"].as_str()),(503,Some("codex_terminal_launch_stuck")),"{reply}");
    assert_eq!(python.hits_to("/api/sessions/cx-old/input"),0);
}

/// A fila da chave guarda a vida de outra conversa (o Rust caiu antes de limpá-la) e não há ator: o texto
/// não entra nela (a adoção a descartaria) e a resposta é um erro com código, sem laço de reenvio.
#[tokio::test]
async fn text_to_an_opening_codex_whose_queue_belongs_to_an_old_life_is_a_coded_error() {
    let fx = fixture();
    fx.panes_born(&[("cx-new","%7",now_s())]);
    fx.pane_key("k-old");
    {
        let path = fx.queue_state("k-old");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lease = hangar_server::runtime::queue::acquire_lease(&path.with_extension("lock")).unwrap();
        let mut store = hangar_server::runtime::queue::Store::open(&path,&fx.queue,
            hangar_server::runtime::queue::State::new("k-old",3,"cx-new",Vec::new())).unwrap();
        store.exec(3,"life",CLOCK,Action::AdvanceLifecycle { generation:3,
            state:json!({"codex_life":{"created":1,"tui_pid":1,"endpoint":"ws://127.0.0.1:1","app_pid":1},"name":"cx-new"}) }).unwrap();
        drop(lease);
    }
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    let (python,server) = serve_owner(registry).await;
    let answered = tokio::time::timeout(Duration::from_secs(10),post_owner(server,"cx-new","input",r#"{"text":"oi"}"#)).await
        .expect("responde sem laço");
    assert_eq!((answered.0,answered.1["detail"]["code"].as_str()),(503,Some("codex_terminal_adopted_meanwhile")),"{}",answered.1);
    assert_eq!(python.hits_to("/api/sessions/cx-new/input"),0);
    assert!(fx.queue_json("k-old")["rows"].as_array().unwrap().is_empty(),"nada gravado na fila da vida antiga");
}

/// Texto que nasce no Python (grupo, broadcast, recibo) entra pela ponte do Claude, porque sem sidecar o
/// Python não sabe que é Codex: o Rust o guarda igual, sem repasse de volta (421).
#[tokio::test]
async fn text_from_python_to_a_codex_terminal_still_opening_is_queued_by_the_bridge() {
    let fx = fixture();
    fx.panes_born(&[("cx-new","%7",now_s())]);
    fx.pane_key("k-new");
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    let (_python,upstream) = crate::fake::spawn_fake().await;
    let state = hangar_server::routes::AppState::new(crate::fake::config(upstream,"127.0.0.1"));
    assert!(state.state.runtime.set(registry).is_ok());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let router = hangar_server::routes::terminal_router(Arc::new(state));
    tokio::spawn(async move { axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    let response = crate::fake::client().post(format!("http://{private}/__hangar_server/claude/cx-new/input"))
        .header("content-type","application/json").header("x-hangar-internal",crate::fake::SECRET)
        .body(r#"{"text":"aviso do grupo","steer":false}"#).send().await.unwrap();
    let status = response.status().as_u16();
    let body:Value = serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null);
    assert_eq!((status,body["delivered"].clone()),(200,json!(false)),"{body}");
    assert!(queued(&fx,"k-new","aviso do grupo"),"{}",fx.queue_json("k-new"));
}

/// Trocar de modo logo depois de criar com terminal (a TUI ainda abrindo a conversa, com ou sem
/// sidecar) é recusa do Rust com código próprio, nunca o Python dizendo que o Rust não está rodando.
#[tokio::test]
async fn switching_mode_while_the_terminal_is_still_opening_asks_to_wait() {
    let fx = fixture();
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    fx.panes(&["cx-new","cx-l"]);
    fx.put_sidecar("cx-l",json!({"name":"cx-l","provider":"codex","launching":true,"key":"k-l","cwd":fx.root}));
    for name in ["cx-new","cx-l"] {
        let error = watcher.switch_mode(&registry,name,false,PermissionRead::Known("Full Access".into())).await
            .unwrap_or_else(||panic!("{name}: a troca do Codex é do Rust")).unwrap_err();
        assert_eq!(error.code,"codex_terminal_starting","{name}: {}",error.message);
    }
    let (status,body) = hangar_server::session_write::relaunch::mode_refusal(&RuntimeError::new("codex_terminal_starting","x"));
    assert_eq!((status.as_u16(),body["detail"]["code"].as_str(),body["detail"]["params"]["code"].as_str()),
        (409,Some("erro_troca_modo"),Some("codex_terminal_starting")));
}

/// Pane de outro agente com o mesmo nome e sem sidecar Codex não é Codex abrindo: a troca segue ao Claude.
#[tokio::test]
async fn switching_mode_of_a_claude_terminal_is_not_the_codex_one() {
    let fx = fixture();
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    std::fs::write(&fx.panes,"cl\t1\t4242\t/w\t%7\t\tclaude\t1700000000\t0\t0\n").unwrap();
    assert!(watcher.switch_mode(&registry,"cl",false,PermissionRead::Known("Full Access".into())).await.is_none());
}

#[tokio::test]
async fn limits_of_a_rust_codex_that_cannot_open_answers_its_code() {
    let fx = fixture();
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    // Sem terminal parado e sem `cwd`: o Rust é o dono e não consegue abrir.
    fx.put_sidecar("cx-h",json!({"name":"cx-h","provider":"codex","headless":true,"key":"k-h","thread_id":"t"}));
    let (python,server) = serve_owner(registry).await;
    let response = crate::fake::client().get(format!("http://{server}/api/sessions/cx-h/limits"))
        .header("authorization",format!("Bearer {}",crate::fake::OWNER)).send().await.unwrap();
    assert_eq!(response.status().as_u16(),503);
    let body:Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_ne!(body["detail"]["code"],"session_transfer_busy","{body}");
    assert!(body["detail"]["code"].is_string(),"{body}");
    assert_eq!(python.hits_to("/api/sessions/cx-h/limits"),0);
}

#[tokio::test]
async fn to_headless_refuses_when_the_conversation_settings_cannot_be_read() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    let mut meta = relaunch_meta(&fx,&server.endpoint,&app,&tui);
    meta["thread_id"] = json!("thread-unreadable");
    fx.put_sidecar("cx-proof",meta);
    fx.panes(&["cx-proof"]);
    let registry = relaunch_registry(&fx).await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let before = fx.sidecar("cx-proof").unwrap();
    let error = switch_until_settled(&watcher,&registry,||PermissionRead::Known("Approve for me".into())).await;
    assert_eq!(error.code,"erro_troca_modo");
    assert!(error.message.contains("configuração"),"{}",error.message);
    untouched(&fx,&registry,&before,&mut app,&mut tui);
    finish(&mut [&mut app,&mut tui]);
}

/// Sidecar do desenho antigo: terminal sem `endpoint` nem `app_pid` (o app-server era filho do backend).
fn legacy_meta(fx:&Fixture) -> Value {
    let rollout = fx.root.join("rollout.jsonl");
    std::fs::write(&rollout,"{}\n").unwrap();
    json!({"name":"cx-old","provider":"codex","thread_id":"thread-1","rollout_path":rollout,"cwd":fx.root,"codex_home":fx.home.join(".codex")})
}

#[tokio::test]
async fn an_old_sidecar_without_endpoint_is_relaunched_by_the_rust_and_a_failure_has_a_code() {
    // Como o `ensure_running` do Python: sem pane, a escrita relança TUI e app-server pelo lançador; aqui o
    // lançador não está no PATH, e a falha sai com o código dele (nunca `codex_terminal_not_ready` calado).
    let fx = fixture();
    fx.put_sidecar("cx-old",legacy_meta(&fx));
    std::fs::write(&fx.panes,"").unwrap();
    let registry = relaunch_registry(&fx).await;
    let watcher = relaunch_watcher(&fx,&relaunch_tmux(&fx));
    registry.bind_watcher(watcher.clone());
    let refused = watcher.adopt_now(&registry,"cx-old").await;
    assert_eq!(refused.map(|r|r.map_err(|e|e.code)),Some(Err("codex_launcher_missing".into())));
    // Três subidas falhas e a escrita para de tentar a cada pedido (o teto do Python), com código próprio.
    for _ in 0..2 { let _ = watcher.adopt_now(&registry,"cx-old").await; }
    let gave_up = watcher.adopt_now(&registry,"cx-old").await;
    assert_eq!(gave_up.map(|r|r.map_err(|e|e.code)),Some(Err("codex_terminal_relaunch_gave_up".into())));
}

/// O relançamento usa o lançador único da criação: o pane roda o `hangar-codex-tui` com a chave, o
/// ambiente da sessão por `-e` (cores, Kimi, provedor) e a política explícita do modo gravado.
#[tokio::test]
async fn the_relaunch_pane_is_the_creation_launcher_with_the_session_environment() {
    let fx = fixture();
    fx.put_sidecar("cx-old",legacy_meta(&fx));
    std::fs::write(&fx.panes,"").unwrap();
    let registry = relaunch_registry(&fx).await;
    let bin = fx.root.join("bin");
    crate::write_executable(&bin.join("hangar-codex-tui"),"#!/bin/sh\nexit 0\n");
    crate::write_executable(&bin.join("codex"),"#!/bin/sh\nexit 0\n");
    let asked = fx.root.join("new-session.args");
    let tmux = fx.root.join("tmux-record");
    crate::write_executable(&tmux,format!("#!/bin/sh\ncase \"$1\" in\n  list-panes) cat '{}';;\n  \
        new-session) for a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'; exit 1;;\n  *) exit 1;;\nesac\n",fx.panes.display(),asked.display()));
    let watcher = relaunch_watcher(&fx,&tmux);
    registry.bind_watcher(watcher.clone());
    let refused = watcher.adopt_now(&registry,"cx-old").await;
    assert_eq!(refused.map(|r|r.map_err(|e|e.code)),Some(Err("terminal_nao_criado".into())),"o tmux falso não cria a sessão");
    let args:Vec<String> = std::fs::read_to_string(&asked).expect("o pane foi pedido ao tmux").lines().map(str::to_owned).collect();
    let key = key_of(&fx.sidecar("cx-old").unwrap());
    for env in ["CP_SESSION_NAME=cx-old","CP_PROVIDER=codex",&format!("CP_SESSION_KEY={key}"),"COLORTERM=truecolor","KIMI_CODE_TUI_FULL_SCREEN=1","HANGAR_JEV=off"] {
        assert!(args.windows(2).any(|w|w[0] == "-e" && w[1] == env),"-e {env}: {args:?}");
    }
    let command = args.last().unwrap();
    assert!(!command.contains("--session-relaunch"),"sem o auxiliar com IPC: {command}");
    let launcher = format!("hangar-codex-tui --cwd {} --codex-home {} --codex-account default --resume thread-1 --approval-policy never --sandbox danger-full-access",
        fx.root.display(),fx.home.join(".codex").display());
    assert!(command.starts_with("exec env ") && command.ends_with(&launcher),"{command}");
}

#[tokio::test]
async fn reload_of_an_old_terminal_sidecar_refuses_without_relaunching_it() {
    // Recarregar só vale sem terminal: perguntar se é do Rust não pode relançar o terminal antigo.
    let fx = fixture();
    fx.put_sidecar("cx-old",legacy_meta(&fx));
    std::fs::write(&fx.panes,"").unwrap();
    let registry = relaunch_registry(&fx).await;
    let watcher = relaunch_watcher(&fx,&relaunch_tmux(&fx));
    registry.bind_watcher(watcher.clone());
    for _ in 0..3 {
        let refused = registry.restart_codex("cx-old").await.expect("Codex do Rust").unwrap_err();
        assert_eq!(refused.code,"erro_recarregar_so_sem_terminal");
    }
    // Nenhuma tentativa de relançar foi contada: a escrita ainda tenta e mostra o motivo real.
    let write = watcher.adopt_now(&registry,"cx-old").await;
    assert_eq!(write.map(|r|r.map_err(|e|e.code)),Some(Err("codex_launcher_missing".into())));
}

#[tokio::test]
async fn only_launch_failures_count_toward_giving_up() {
    // Modo de permissão ilegível no sidecar não é falha do lançador: não leva ao "desisti".
    let fx = fixture();
    let mut meta = legacy_meta(&fx);
    meta["permission_mode"] = json!("Custom (config.toml)");
    fx.put_sidecar("cx-old",meta);
    std::fs::write(&fx.panes,"").unwrap();
    let registry = relaunch_registry(&fx).await;
    let watcher = relaunch_watcher(&fx,&relaunch_tmux(&fx));
    registry.bind_watcher(watcher.clone());
    for _ in 0..4 {
        let refused = watcher.adopt_now(&registry,"cx-old").await;
        assert_ne!(refused.map(|r|r.map_err(|e|e.code)),Some(Err("codex_terminal_relaunch_gave_up".into())));
    }
}

#[tokio::test]
async fn an_old_sidecar_with_its_pane_alive_is_never_relaunched() {
    // Pane vivo: recriar destruiria a tela de quem trabalha nela; fica o código, sem tocar no pane.
    let fx = fixture();
    fx.put_sidecar("cx-old",legacy_meta(&fx));
    std::fs::write(&fx.panes,"cx-old\t1\t4242\t/w\t%7\t\tcodex\t1700000000\t0\t0\n").unwrap();
    let registry = relaunch_registry(&fx).await;
    let watcher = relaunch_watcher(&fx,&relaunch_tmux(&fx));
    registry.bind_watcher(watcher.clone());
    let refused = watcher.adopt_now(&registry,"cx-old").await;
    assert_eq!(refused.map(|r|r.map_err(|e|e.code)),Some(Err("codex_terminal_without_control".into())));
    assert!(std::fs::read_to_string(&fx.panes).unwrap().starts_with("cx-old\t"),"o pane segue");
}

#[tokio::test]
async fn rename_and_mode_switch_close_the_actor_without_killing() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    watcher.round(&registry).await;
    let key = key_of(&fx.sidecar("cx-proof").unwrap());

    // Renomear: o Python move tmux e sidecar; a mesma chave com nome novo fecha uma e abre a outra.
    let mut renamed = fx.sidecar("cx-proof").unwrap();
    renamed["name"] = json!("cx-renamed");
    fx.put_sidecar("cx-renamed",renamed);
    std::fs::remove_file(fx.sidecar_path("cx-proof")).unwrap();
    fx.panes(&["cx-renamed"]);
    watcher.round(&registry).await;
    assert!(!registry.owns_codex_terminal("cx-proof"));
    assert!(registry.owns_codex_terminal("cx-renamed"),"a mesma chave segue com o nome novo");
    assert_eq!(registry.codex_terminals().await.iter().map(|t|t.key.clone()).collect::<Vec<_>>(),vec![key.clone()]);

    // Troca de modo em curso (`app_pid` nulo): o ator sai, nada morre e nada é apagado.
    let mut switching = fx.sidecar("cx-renamed").unwrap();
    switching["app_pid"] = Value::Null;
    fx.put_sidecar("cx-renamed",switching);
    watcher.round(&registry).await;
    assert!(!registry.owns_codex_terminal("cx-renamed"));
    assert!(app.try_wait().unwrap().is_none(),"o app-server segue vivo");
    assert!(fx.sidecar("cx-renamed").is_some(),"o sidecar é do Python durante a troca");
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn waiting_for_the_rollout_does_not_rewrite_the_queue_on_each_attempt() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    fx.watcher().round(&registry).await;
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    let resumes = || server.frames_of(1).iter().filter(|f|f["method"] == "thread/resume").count();
    let wait_resumes = |n:usize| async move {
        tokio::time::timeout(Duration::from_secs(10),async { while resumes() < n { tokio::time::sleep(Duration::from_millis(20)).await; } })
            .await.unwrap_or_else(|_|panic!("menos de {n} tentativas de assinar"));
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    wait_resumes(1).await;
    let before = fx.queue_json(&key)["next_seq"].as_u64().unwrap();
    wait_resumes(3).await;
    // Cada `thread/resume` recusado por `no rollout found` é só "ainda não": nada durável mudou.
    assert_eq!(fx.queue_json(&key)["next_seq"].as_u64().unwrap(),before,"tentativa recusada gravou a fila");
    assert!(registry.owns_codex_terminal("cx-proof"));
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn no_answer_from_the_multiplexer_decides_nothing() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    std::fs::write(fx.panes.with_extension("down"),"").unwrap();
    let registry = registry().await;
    let watcher = fx.watcher();
    watcher.round(&registry).await;
    assert!(!registry.owns_codex_terminal("cx-proof"),"sem lista de panes não adota");
    std::fs::remove_file(fx.panes.with_extension("down")).unwrap();
    watcher.round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-proof"));
    // Pane some com o multiplexador mudo: nunca conta como morte.
    tui.kill().unwrap(); tui.wait().unwrap();
    std::fs::write(fx.panes.with_extension("down"),"").unwrap();
    for _ in 0..3 { watcher.round(&registry).await; }
    assert!(registry.owns_codex_terminal("cx-proof"));
    assert!(app.try_wait().unwrap().is_none());
    finish(&mut [&mut app]);
}

#[tokio::test]
async fn closing_from_the_app_kills_the_pane_first_then_the_whole_life() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    assert_eq!(registry.adopt_now("cx-proof").await.map(|r|r.is_ok()),None,"sem a vigia ligada, nada adota pela escrita");
    registry.bind_watcher(watcher.clone());
    assert_eq!(registry.adopt_now("other").await.map(|r|r.is_ok()),None,"sem sidecar Codex: o caminho de sempre");
    assert_eq!(registry.adopt_now("cx-proof").await.map(|r|r.is_ok()),Some(true),"a escrita adota na hora");
    assert!(registry.owns_codex_terminal("cx-proof"));
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    let closed = registry.close_with_kill(&key,1).await.unwrap();
    assert_eq!(closed,json!({"closed":true,"killed":true}));
    assert!(std::fs::read_to_string(&fx.panes).unwrap().is_empty(),"o kill-session saiu e foi conferido");
    assert!(!registry.owns_codex_terminal("cx-proof"));
    assert!(exited(&mut app));
    assert!(fx.sidecar("cx-proof").is_none());
    assert!(!fx.queue_state(&key).exists());
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn another_tui_with_an_inherited_key_gets_a_fresh_queue() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    // Fila de outra vida de mesmo nome e mesma chave (o `sessions.save` conserva a chave).
    {
        let path = fx.queue_state("old-key");
        let lease = hangar_server::runtime::queue::acquire_lease(&path.with_extension("lock")).unwrap();
        let mut store = hangar_server::runtime::queue::Store::open(&path,&fx.queue,
            hangar_server::runtime::queue::State::new("old-key",3,"cx-proof",Vec::new())).unwrap();
        store.exec(3,"life",CLOCK,Action::AdvanceLifecycle { generation:3,
            state:json!({"codex_life":{"created":1,"tui_pid":1,"endpoint":"ws://127.0.0.1:1","app_pid":1},"name":"cx-proof"}) }).unwrap();
        drop(lease);
    }
    let mut meta = fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui);
    meta["key"] = json!("old-key");
    fx.put_sidecar("cx-proof",meta);
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    fx.watcher().round(&registry).await;
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    assert_ne!(key,"old-key","outra TUI gira a chave");
    assert!(!fx.queue_state("old-key").exists(),"a fila da vida anterior não é entregue nesta");
    assert!(registry.handle(&key,1).await.is_ok());
    finish(&mut [&mut app,&mut tui]);
}

/// Registro com o ambiente do Codex na pasta do teste: conta padrão em `<home>/.codex`, PATH só com `bin/`.
async fn relaunch_registry(fx:&Fixture) -> Arc<RuntimeRegistry> {
    let bin = fx.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let map = [("HOME",fx.home.to_string_lossy().into_owned()),("PATH",bin.to_string_lossy().into_owned())]
        .into_iter().map(|(k,v)|(k.to_owned(),v)).collect();
    crate::runtime_open::registry_with(|registry|registry.with_codex_environment(AccountEnvironment::from_map(map))).await
}

/// Sidecar adotável com o rollout no disco e a conta padrão do ambiente do teste.
fn relaunch_meta(fx:&Fixture,endpoint:&str,app:&Child,tui:&Child) -> Value {
    let rollout = fx.root.join("rollout.jsonl");
    std::fs::write(&rollout,"{}\n").unwrap();
    let mut meta = fx.meta("cx-proof",endpoint,Some(app.id()),tui);
    meta["rollout_path"] = json!(rollout);
    meta["codex_home"] = json!(fx.home.join(".codex"));
    meta
}

/// Troca para sem terminal repetida enquanto o motor recém-adotado ainda não assentou: a recusa de
/// "ociosa" não mexe em nada e pode repetir.
async fn switch_until_settled(watcher:&Watcher,registry:&RuntimeRegistry,read:impl Fn() -> PermissionRead) -> RuntimeError {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match watcher.switch_mode(registry,"cx-proof",false,read()).await.expect("Codex com terminal do Rust") {
            Err(error) if error.message.contains("ociosa") && Instant::now() < deadline => {
                assert!(registry.owns_codex_terminal("cx-proof"),"recusa não solta a sessão");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return error,
            Ok(value) => panic!("a troca não devia passar: {value}"),
        }
    }
}

fn untouched(fx:&Fixture,registry:&RuntimeRegistry,before:&Value,app:&mut Child,tui:&mut Child) {
    assert!(registry.owns_codex_terminal("cx-proof"),"a sessão segue adotada");
    assert!(std::fs::read_to_string(&fx.panes).unwrap().starts_with("cx-proof\t"),"nenhum kill-session");
    assert!(app.try_wait().unwrap().is_none() && tui.try_wait().unwrap().is_none(),"nada morreu");
    assert_eq!(&fx.sidecar("cx-proof").unwrap(),before,"sidecar intacto: nem app_pid nulo nem modo gravado");
}

#[tokio::test]
async fn to_headless_refuses_an_unread_permission_mode_and_kills_nothing() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    fx.put_sidecar("cx-proof",relaunch_meta(&fx,&server.endpoint,&app,&tui));
    fx.panes(&["cx-proof"]);
    let registry = relaunch_registry(&fx).await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let before = fx.sidecar("cx-proof").unwrap();
    // O tmux falso não tem `capture-pane`: o seletor não lê modo nenhum, e a recusa diz isso (não "não suportada").
    let error = switch_until_settled(&watcher,&registry,||PermissionRead::Pane(None)).await;
    assert_eq!(error.code,"erro_troca_modo");
    assert!(error.message.contains("Não consegui ler a permissão atual no terminal"),"{}",error.message);
    assert!(!error.message.contains("não é suportada"),"{}",error.message);
    untouched(&fx,&registry,&before,&mut app,&mut tui);
    assert!(registry.ingress().enter("cx-proof",Duration::ZERO).await.is_ok(),"a porta reabriu");
    // Lido, mas fora dos três modos do sem terminal: a mesma recusa, nunca Full Access.
    let error = switch_until_settled(&watcher,&registry,||PermissionRead::Known("Custom (config.toml)".into())).await;
    assert!(error.message.contains("não é suportada sem terminal"),"{}",error.message);
    untouched(&fx,&registry,&before,&mut app,&mut tui);
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn refused_preconditions_leave_the_terminal_owned_and_untouched() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    let mut meta = relaunch_meta(&fx,&server.endpoint,&app,&tui);
    meta["rollout_path"] = json!(fx.root.join("sem-rollout.jsonl"));
    fx.put_sidecar("cx-proof",meta);
    fx.panes(&["cx-proof"]);
    let registry = relaunch_registry(&fx).await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let known = ||PermissionRead::Known("Full Access".into());

    let before = fx.sidecar("cx-proof").unwrap();
    let error = switch_until_settled(&watcher,&registry,known).await;
    assert!(error.message.contains("Envie a primeira mensagem"),"{}",error.message);
    untouched(&fx,&registry,&before,&mut app,&mut tui);

    // Conta do sidecar diferente da que o catálogo resolve.
    let mut moved = before.clone();
    moved["rollout_path"] = json!(fx.root.join("rollout.jsonl"));
    moved["codex_home"] = json!(fx.root.join("outra-conta"));
    fx.put_sidecar("cx-proof",moved.clone());
    let error = switch_until_settled(&watcher,&registry,known).await;
    assert!(error.message.contains("A conta Codex mudou"),"{}",error.message);
    untouched(&fx,&registry,&moved,&mut app,&mut tui);

    // Transferência de conversa em curso para o nome.
    let mut ready = moved.clone();
    ready["codex_home"] = json!(fx.home.join(".codex"));
    let transfer = "0000000a-0000-0000-0000-000000000001";
    ready["transfer_id"] = json!(transfer);
    fx.put_sidecar("cx-proof",ready.clone());
    let records = fx.home.join(".hangar").join("conversation-transfers");
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(records.join(format!("{transfer}.json")),json!({"name":"cx-proof","phase":"publishing"}).to_string()).unwrap();
    let error = watcher.switch_mode(&registry,"cx-proof",false,known()).await.expect("Codex do Rust").unwrap_err();
    assert_eq!(error.code,"session_transfer_busy");
    untouched(&fx,&registry,&ready,&mut app,&mut tui);
    finish(&mut [&mut app,&mut tui]);
}

/// O terminal só leitura (bwrap) não vai para sem terminal, onde a proteção não existe; nada morre.
#[tokio::test]
async fn a_read_only_terminal_refuses_to_go_headless_and_kills_nothing() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let mut app = crate::runtime_open::with_app_server_argv(std::process::Command::new("sh").args(["-c","sleep 120; true","app-server",&server.endpoint])
        .env("HANGAR_ORQ_READ_ONLY","1").spawn().unwrap(),&server.endpoint);
    let mut tui = tui();
    fx.put_sidecar("cx-proof",relaunch_meta(&fx,&server.endpoint,&app,&tui));
    fx.panes(&["cx-proof"]);
    let registry = relaunch_registry(&fx).await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let before = fx.sidecar("cx-proof").unwrap();
    let error = switch_until_settled(&watcher,&registry,||PermissionRead::Known("Full Access".into())).await;
    assert_eq!(error.code,"erro_troca_modo");
    assert!(error.message.contains("só leitura"),"{}",error.message);
    untouched(&fx,&registry,&before,&mut app,&mut tui);
    assert!(registry.ingress().enter("cx-proof",Duration::ZERO).await.is_ok(),"a porta reabriu");
    finish(&mut [&mut app,&mut tui]);
}

/// Colhe os filhos mortos pelo `kill-session` falso: zumbi segue com `/proc` e pareceria vivo.
fn reap(children:Vec<Child>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut children = children;
        let deadline = Instant::now() + Duration::from_secs(60);
        while !children.is_empty() && Instant::now() < deadline {
            children.retain_mut(|child|child.try_wait().ok().flatten().is_none());
            std::thread::sleep(Duration::from_millis(20));
        }
        for mut child in children { let _ = child.kill(); let _ = child.wait(); }
    })
}

#[tokio::test]
async fn a_failure_after_the_kill_puts_the_terminal_mode_back_and_leaves_no_headless_entry() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (app,tui) = (app_server_process(&server.endpoint),tui());
    let (app_pid,tui_pid) = (app.id(),tui.id());
    // Sidecar atrasado: a TUI já está em "Ask for approval" (o seletor lê isso), o arquivo ainda diz Full Access.
    let mut stale = relaunch_meta(&fx,&server.endpoint,&app,&tui);
    stale["permission_mode"] = json!("Full Access");
    fx.put_sidecar("cx-proof",stale);
    // O pid do pane é o da TUI: nenhum pid alheio entra na espera de saída.
    std::fs::write(&fx.panes,format!("cx-proof\t1\t{tui_pid}\t/w\t%7\t\tcodex\t1700000000\t0\t0\n")).unwrap();
    std::fs::write(fx.panes.with_extension("pids"),format!("{app_pid} {tui_pid}")).unwrap();
    let reaper = reap(vec![app,tui]);
    let registry = relaunch_registry(&fx).await;
    let bin = fx.root.join("bin");
    // Lançador presente (a volta existe antes do kill) e um `codex` que não sobe: o sem terminal falha.
    crate::write_executable(&bin.join("hangar-codex-tui"),"#!/bin/sh\nexit 0\n");
    crate::write_executable(&bin.join("codex"),"#!/bin/sh\nexit 1\n");
    // tmux falso que derruba os processos do pane no kill-session (o SIGHUP de verdade) e não cria sessão.
    let tmux = fx.root.join("tmux-relaunch");
    let p = fx.panes.display();
    crate::write_executable(&tmux,format!("#!/bin/sh\nP='{p}'\ncase \"$1\" in\n  list-panes) cat \"$P\";;\n  \
        has-session) n=\"${{3#=}}\"; grep -q \"^$n\t\" \"$P\";;\n  \
        kill-session) n=\"${{3#=}}\"; for p in $(cat \"$P.pids\"); do pkill -P $p; kill $p; done 2>/dev/null; grep -v \"^$n\t\" \"$P\" > \"$P.new\"; mv \"$P.new\" \"$P\";;\n  \
        display-message) printf 'cx-proof\t%%7\t1700000000\\n';;\n  *) exit 1;;\nesac\n"));
    let watcher = Arc::new(Watcher::new(WatchConfig { home:fx.home.clone(),queue_dir:fx.queue.clone(),
        mux:Mux::with_program(&tmux,Duration::from_secs(5)),mux_program:tmux.clone().into_os_string() }));
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-proof"));

    let error = switch_until_settled(&watcher,&registry,||PermissionRead::Known("Ask for approval".into())).await;
    assert_eq!(error.code,"erro_troca_modo");
    // O pane novo não nasce com este tmux: a volta é tentada e a falha aparece inteira, nunca calada.
    assert!(error.message.contains("e o terminal não voltou"),"{}",error.message);
    let meta = fx.sidecar("cx-proof").expect("o sidecar volta mesmo que o lançador o tenha apagado");
    assert_eq!(meta["headless"],false,"o modo anterior (terminal) volta ao sidecar");
    assert_eq!(meta["permission_mode"],"Ask for approval","a volta relança com o modo lido, nunca com o Full Access velho do sidecar");
    assert!(meta["app_pid"].is_null() && meta["cano"].is_null(),"sem processo de nenhuma das tentativas");
    assert!(registry.writable("cx-proof").await.is_none(),"nenhum sem terminal órfão ficou aberto");
    assert!(registry.ingress().enter("cx-proof",Duration::ZERO).await.is_ok(),"a porta reabriu");
    reaper.join().unwrap();
}

/// tmux falso da troca: `kill-session` tira o pane e manda SIGTERM nos pids de `<panes>.pids` e nos filhos
/// deles (o SIGHUP do grupo do pane de verdade); não cria sessão. Filho que sobra seguraria a troca (N1).
fn relaunch_tmux(fx:&Fixture) -> PathBuf {
    let tmux = fx.root.join("tmux-relaunch");
    let p = fx.panes.display();
    crate::write_executable(&tmux,format!("#!/bin/sh\nP='{p}'\ncase \"$1\" in\n  list-panes) cat \"$P\";;\n  \
        has-session) n=\"${{3#=}}\"; grep -q \"^$n\t\" \"$P\";;\n  \
        kill-session) n=\"${{3#=}}\"; for p in $(cat \"$P.pids\"); do pkill -P $p; kill $p; done 2>/dev/null; grep -v \"^$n\t\" \"$P\" > \"$P.new\"; mv \"$P.new\" \"$P\";;\n  \
        display-message) printf 'cx-proof\t%%7\t1700000000\\n';;\n  *) exit 1;;\nesac\n"));
    tmux
}

fn relaunch_watcher(fx:&Fixture,tmux:&std::path::Path) -> Arc<Watcher> {
    Arc::new(Watcher::new(WatchConfig { home:fx.home.clone(),queue_dir:fx.queue.clone(),
        mux:Mux::with_program(tmux,Duration::from_secs(5)),mux_program:tmux.to_owned().into_os_string() }).with_death_after(DEATH_AFTER))
}

/// App-server falso que ignora o SIGTERM (o `sleep` filho morre, o laço segue).
fn stubborn_app_server(endpoint:&str) -> Child {
    crate::runtime_open::with_app_server_argv(std::process::Command::new("sh").args(["-c","trap '' TERM; while :; do sleep 1; done","app-server",endpoint]).spawn().unwrap(),endpoint)
}

#[tokio::test]
async fn an_app_server_that_outlives_the_kill_leaves_the_session_parked_not_buried() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,tui) = (stubborn_app_server(&server.endpoint),tui());
    let tui_pid = tui.id();
    fx.put_sidecar("cx-proof",relaunch_meta(&fx,&server.endpoint,&app,&tui));
    std::fs::write(&fx.panes,format!("cx-proof\t1\t{tui_pid}\t/w\t%7\t\tcodex\t1700000000\t0\t0\n")).unwrap();
    // O kill-session derruba só a TUI: o app-server sobrevive ao kill e ao SIGTERM da troca.
    std::fs::write(fx.panes.with_extension("pids"),tui_pid.to_string()).unwrap();
    let reaper = reap(vec![tui]);
    let registry = relaunch_registry(&fx).await;
    let bin = fx.root.join("bin");
    crate::write_executable(&bin.join("hangar-codex-tui"),"#!/bin/sh\nexit 0\n");
    crate::write_executable(&bin.join("codex"),"#!/bin/sh\nexit 1\n");
    let watcher = relaunch_watcher(&fx,&relaunch_tmux(&fx));
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let key = key_of(&fx.sidecar("cx-proof").unwrap());

    let error = switch_until_settled(&watcher,&registry,||PermissionRead::Known("Ask for approval".into())).await;
    assert_eq!(error.code,"relaunch_process_alive","processo antigo vivo: 500, e nada novo sobe ao lado");
    assert!(error.message.contains(&app.id().to_string()),"a resposta nomeia o pid vivo: {}",error.message);
    assert!(error.message.contains("feche-a e abra a conversa de novo"),"a resposta diz o que fazer: {}",error.message);
    let parked = fx.sidecar("cx-proof").expect("sidecar no disco");
    // O sidecar do lançador não grava `headless`: ausente é terminal, e a troca não o virou.
    assert_ne!(parked["headless"],true,"continua de terminal");
    assert!(parked["app_pid"].is_null(),"sem `unpark`: a vigia não enterra uma sessão sem pane");
    assert!(registry.writable("cx-proof").await.is_none(),"nenhum sem terminal subiu");
    // Rodadas depois do prazo de morte: nada é apagado.
    watcher.round(&registry).await;
    tokio::time::sleep(DEATH_AFTER + Duration::from_millis(50)).await;
    watcher.round(&registry).await;
    assert!(fx.sidecar("cx-proof").is_some(),"sidecar segue no disco");
    assert!(fx.queue_state(&key).exists(),"fila segue no disco");
    assert!(app.try_wait().unwrap().is_none(),"o app-server teimoso segue vivo (e a resposta disse isso)");
    assert!(registry.ingress().enter("cx-proof",Duration::ZERO).await.is_ok(),"a porta reabriu");
    finish(&mut [&mut app]);
    reaper.join().unwrap();
}

/// `codex app-server --stdio` falso para o sem terminal: retoma a conversa pedida e anota cada método em `log`.
fn fake_headless_codex(bin:&std::path::Path,log:&std::path::Path) {
    let python = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|p|p.join("python3")).find(|p|p.is_file()).expect("python3 no PATH");
    if let Err(error) = std::os::unix::fs::symlink(python,bin.join("python3")) { assert_eq!(error.kind(),std::io::ErrorKind::AlreadyExists); }
    crate::write_executable(&bin.join("codex"),format!(r#"#!/usr/bin/env python3
import json, sys
log = open({log:?}, "a")
thread = "thread-1"
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg or "method" not in msg:
        continue
    method, params = msg["method"], msg.get("params") or {{}}
    log.write(json.dumps({{"method": method, "params": params}}) + "\n"); log.flush()
    if method == "initialize":
        result = {{"userAgent": "hangar/{} (x)"}}
    elif method in ("thread/start", "thread/resume"):
        thread = params.get("threadId") or "thread-new"
        # Sem modelo na resposta: vale o do arquivo da sessão que a troca gravou.
        result = {{"thread": {{"id": thread, "path": "/tmp/rollout.jsonl", "status": {{"type": "idle"}}}}}}
    elif method == "thread/read":
        result = {{"thread": {{"id": thread, "status": {{"type": "idle"}}, "turns": []}}}}
    else:
        result = {{}}
    print(json.dumps({{"id": msg["id"], "result": result}}), flush=True)
"#,hangar_codex::version::CHECKED,log=log.display().to_string()));
}

#[tokio::test]
async fn to_headless_brings_up_the_headless_with_the_read_mode_restores_plan_and_restarts() {
    crate::use_cano_bin();
    let fx = fixture();
    let server = fake_app_server(true).await;
    // Jev ligado só no ambiente do processo vivo, como o terminal o recebe do lançador.
    let app = crate::runtime_open::with_app_server_argv(std::process::Command::new("sh").args(["-c","sleep 120; true","app-server",&server.endpoint])
        .env("HANGAR_JEV","on").spawn().unwrap(),&server.endpoint);
    let tui = tui();
    let (app_pid,tui_pid) = (app.id(),tui.id());
    let mut meta = relaunch_meta(&fx,&server.endpoint,&app,&tui);
    // Sidecar velho em Full Access; o seletor leu "Approve for me". O modo plan volta depois da troca.
    meta["permission_mode"] = json!("Full Access");
    meta["mode"] = json!("plan");
    meta["model"] = json!("gpt-test");
    fx.put_sidecar("cx-proof",meta);
    std::fs::write(&fx.panes,format!("cx-proof\t1\t{tui_pid}\t/w\t%7\t\tcodex\t1700000000\t0\t0\n")).unwrap();
    std::fs::write(fx.panes.with_extension("pids"),format!("{app_pid} {tui_pid}")).unwrap();
    let reaper = reap(vec![app,tui]);
    let registry = relaunch_registry(&fx).await;
    let bin = fx.root.join("bin");
    crate::write_executable(&bin.join("hangar-codex-tui"),"#!/bin/sh\nexit 0\n");
    let log = fx.root.join("codex-methods.jsonl");
    fake_headless_codex(&bin,&log);
    let watcher = relaunch_watcher(&fx,&relaunch_tmux(&fx));
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    let generation = fx.queue_json(&key)["generation"].as_u64().unwrap();

    // Com terminal, reiniciar é a recusa do Python.
    let refused = registry.restart_codex("cx-proof").await.expect("Codex do Rust").unwrap_err();
    assert_eq!(refused.code,"erro_recarregar_so_sem_terminal");

    let deadline = Instant::now() + Duration::from_secs(20);
    let switched = loop {
        match watcher.switch_mode(&registry,"cx-proof",false,PermissionRead::Known("Approve for me".into())).await.expect("Codex do Rust") {
            Err(error) if error.message.contains("ociosa") && Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(50)).await,
            other => break other,
        }
    };
    assert_eq!(switched.unwrap(),json!({"ok":true,"terminal":false}));
    let sidecar = fx.sidecar("cx-proof").unwrap();
    assert_eq!(sidecar["headless"],true);
    assert_eq!(sidecar["permission_mode"],"Approve for me","o modo lido, não o do sidecar velho");
    assert_eq!(sidecar["key"],json!(key),"a mesma chave: a vida segue a fila");
    assert!(sidecar["cano"]["pid"].as_u64().is_some(),"o cano novo foi gravado");
    assert!(sidecar["endpoint"].is_null() && sidecar["app_pid"].is_null(),"nada do terminal antigo");
    assert_eq!(sidecar["jev"],true,"o Jev do processo vivo segue para o sem terminal");
    assert_eq!((sidecar["model"].as_str(),sidecar["effort"].as_str()),(Some("gpt-tui"),Some("high")),"modelo e esforço relidos da conversa, não da vista");
    assert_eq!(fx.queue_json(&key)["generation"].as_u64(),Some(generation + 1),"vida nova na mesma fila, que não foi apagada");
    assert!(registry.owns_codex_headless("cx-proof").await);
    assert!(!registry.owns_codex_terminal("cx-proof"));
    let methods:Vec<Value> = std::fs::read_to_string(&log).unwrap().lines().map(|l|serde_json::from_str(l).unwrap()).collect();
    let resume = methods.iter().find(|m|m["method"] == "thread/resume").expect("retomou a conversa");
    assert_eq!(resume["params"]["threadId"],"thread-1");
    assert!(methods.iter().any(|m|m["method"] == "thread/settings/update" && m["params"]["collaborationMode"]["mode"] == "plan"),"SetMode depois do assentamento");
    let generation_now = generation + 1;
    let handle = registry.handle(&key,generation_now).await.unwrap();
    assert_eq!(handle.view().await.unwrap()["mode"],"plan","a vista confirma o modo restaurado");
    assert!(registry.ingress().enter("cx-proof",Duration::ZERO).await.is_ok(),"a porta reabriu");

    // Sem terminal do Rust: reiniciar é aceito aqui, sem ir ao Python.
    assert_eq!(registry.restart_codex("cx-proof").await.expect("Codex do Rust").unwrap(),json!({"ok":true}));
    registry.close_with_kill(&key,generation_now).await.unwrap();
    reaper.join().unwrap();
}

#[tokio::test]
async fn a_parked_headless_codex_is_opened_here_instead_of_going_to_python() {
    // Depois de reiniciar a máquina (cano morto, fila vazia) o Python recusa o próprio cliente: o Rust abre
    // a sessão na hora, como o `ensure_running` do Python, e atende o Recarregar e os controles.
    crate::use_cano_bin();
    let fx = fixture();
    let registry = relaunch_registry(&fx).await;
    let bin = fx.root.join("bin");
    let log = fx.root.join("codex-methods.jsonl");
    fake_headless_codex(&bin,&log);
    let rollout = fx.root.join("rollout.jsonl");
    std::fs::write(&rollout,"{}\n").unwrap();
    fx.put_sidecar("cx-h",json!({"name":"cx-h","provider":"codex","headless":true,"key":"k-parked","thread_id":"thread-1",
        "rollout_path":rollout,"cwd":fx.root,"codex_home":fx.home.join(".codex"),"permission_mode":"Full Access","model":"gpt-test"}));
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    assert!(!registry.owns_codex_headless("cx-h").await);
    assert_eq!(registry.restart_codex("cx-h").await.expect("Codex sem terminal do Rust").unwrap(),json!({"ok":true}));
    assert!(registry.owns_codex_headless("cx-h").await,"aberto no Rust");
    let generation = fx.queue_json("k-parked")["generation"].as_u64().unwrap();
    registry.close_with_kill("k-parked",generation).await.unwrap();
    // Fechado de novo, a escrita também o abre aqui em vez de repassar.
    assert!(matches!(registry.adopt_now("cx-h").await,Some(Ok(()))));
    assert!(registry.owns_codex_headless("cx-h").await);
    let generation = fx.queue_json("k-parked")["generation"].as_u64().unwrap();
    registry.close_with_kill("k-parked",generation).await.unwrap();
}

#[tokio::test]
async fn relaunch_routes_go_to_python_when_the_session_is_not_a_codex_of_the_rust() {
    let (python, server) = serve_owner(registry().await).await;
    for (route, body) in [("modo-execucao", json!({"terminal": true})), ("recarregar", json!({}))] {
        crate::fake::client().post(format!("http://{server}/api/sessions/claude-x/{route}"))
            .header("authorization", format!("Bearer {}", crate::fake::OWNER)).header("content-type", "application/json")
            .body(body.to_string()).send().await.unwrap();
        assert_eq!(python.hits_to(&format!("/api/sessions/claude-x/{route}")), 1, "{route} sem vigia vai ao Python");
    }
    // Com a vigia ligada: sem sidecar Codex, ou sem terminal que o Rust não abriu, a vez é do Python.
    let fx = fixture();
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    assert!(registry.relaunch_codex("claude-x", true, None).await.is_none());
    assert!(registry.restart_codex("claude-x").await.is_none());
    // Sem terminal parado é do Rust: abrir falha aqui (sem `cwd` para subir) e o erro aparece, sem repasse.
    fx.put_sidecar("cx-h", json!({"name": "cx-h", "provider": "codex", "headless": true, "key": "k-h", "thread_id": "t"}));
    assert!(registry.relaunch_codex("cx-h", true, None).await.expect("do Rust").is_err());
    assert!(registry.restart_codex("cx-h").await.expect("do Rust").is_err());
}

#[tokio::test]
async fn the_hand_over_route_is_gone() {
    let fx = fixture();
    let server_app = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server_app.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server_app.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let (python, upstream) = crate::fake::spawn_fake().await;
    let state = hangar_server::routes::AppState::new(crate::fake::config(upstream, "127.0.0.1"));
    assert!(state.state.runtime.set(registry.clone()).is_ok());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let app_router = hangar_server::routes::terminal_router(Arc::new(state));
    tokio::spawn(async move { axum::serve(listener, app_router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    let response = crate::fake::client().post(format!("http://{private}/__hangar_server/codex/cx-proof/hand-over"))
        .header("x-hangar-internal", crate::fake::SECRET).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 421, "o Rust não solta a sessão para o Python relançar");
    assert!(registry.owns_codex_terminal("cx-proof"), "nada foi solto");
    assert_eq!(python.hits_to("/api/sessions/cx-proof/hand-over"), 0);
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn kill_by_name_closes_a_life_the_rust_has_not_adopted_yet() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    let mut meta = fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui);
    meta["key"] = json!("k-unadopted");
    fx.put_sidecar("cx-proof",meta);
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    assert!(registry.kill_codex_terminal("other").await.is_none(),"sem sidecar Codex: o Python encerra como hoje");
    let killed = registry.kill_codex_terminal("cx-proof").await.unwrap().unwrap();
    assert_eq!(killed,json!({"closed":true,"killed":true}));
    assert!(std::fs::read_to_string(&fx.panes).unwrap().is_empty());
    assert!(exited(&mut app));
    assert!(fx.sidecar("cx-proof").is_none());
    assert_eq!(server.connections.load(std::sync::atomic::Ordering::SeqCst),0,"nada foi adotado só para encerrar");
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn unreadable_sidecar_refuses_the_close_and_never_reaches_python() {
    // Sidecar que existe e não se lê não prova que a sessão não é do Rust: encerrar recusa com código.
    let fx = fixture();
    std::fs::create_dir_all(fx.sidecar_dir()).unwrap();
    std::fs::write(fx.sidecar_path("cx-proof"),b"{ cortado").unwrap();
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    let refused = registry.kill_codex_terminal("cx-proof").await.expect("sidecar presente").unwrap_err();
    assert_eq!(refused.code,"close_sidecar_unreadable");
    assert!(std::fs::read_to_string(&fx.panes).unwrap().starts_with("cx-proof\t"),"nada foi encerrado");
    let (python, server) = serve_owner(registry.clone()).await;
    let (status, body) = delete_session(server, "cx-proof").await;
    assert_eq!(status, 500);
    assert_eq!(body["detail"]["code"], "close_sidecar_unreadable", "{body}");
    assert_eq!(python.hits_to("/api/sessions/cx-proof"), 0, "o Python não encerra no lugar do Rust");
}

/// Servidor com o registro (e a vigia ligada nele), diante de um Python falso.
pub(crate) async fn serve_owner(registry: Arc<hangar_server::runtime::gateway::RuntimeRegistry>) -> (Arc<crate::fake::Fake>, std::net::SocketAddr) {
    let (python, upstream) = crate::fake::spawn_fake().await;
    let state = hangar_server::routes::AppState::new(crate::fake::config(upstream, "127.0.0.1"));
    assert!(state.state.runtime.set(registry).is_ok());
    (python, crate::fake::spawn_state(state).await)
}

async fn delete_session(server: std::net::SocketAddr, name: &str) -> (u16, Value) {
    let response = crate::fake::client().delete(format!("http://{server}/api/sessions/{name}"))
        .header("authorization", format!("Bearer {}", crate::fake::OWNER)).header("x-hangar-rust-closed", "forjado")
        .header("x-hangar-closed-transcript", "%2Fforjado").send().await.unwrap();
    let status = response.status().as_u16();
    (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
}

#[tokio::test]
async fn owner_delete_kills_in_rust_then_forwards_marked_to_python() {
    let fx = fixture();
    let server_app = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server_app.endpoint),tui());
    let mut meta = fx.meta("cx-proof",&server_app.endpoint,Some(app.id()),&tui);
    meta["rollout_path"] = json!("/r/Área de trabalho/rollout-1.jsonl");
    fx.put_sidecar("cx-proof",meta);
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-proof"));
    let (python, server) = serve_owner(registry.clone()).await;
    let (status, _) = delete_session(server, "cx-proof").await;
    assert_eq!(status, 200, "a resposta é a do Python, depois do encerramento");
    assert!(!registry.owns_codex_terminal("cx-proof"));
    assert!(std::fs::read_to_string(&fx.panes).unwrap().is_empty(),"pane encerrado pelo Rust");
    assert!(exited(&mut app));
    assert!(fx.sidecar("cx-proof").is_none());
    assert_eq!(python.hits_to("/api/sessions/cx-proof"), 1, "o mesmo pedido segue ao Python, uma vez");
    let (_, headers) = python.last_hit();
    assert_eq!(headers.get("x-hangar-rust-closed").and_then(|v| v.to_str().ok()), Some(crate::fake::SECRET),
        "a marca é a do Rust, nunca a que veio de fora");
    let transcript = headers.get("x-hangar-closed-transcript").and_then(|v| v.to_str().ok()).unwrap();
    assert_eq!(percent_encoding::percent_decode_str(transcript).decode_utf8().unwrap(), "/r/Área de trabalho/rollout-1.jsonl",
        "a conversa encerrada vai junto, para o nome fechado");
    // Outra sessão (sem sidecar Codex) vai ao Python como chegou, sem marca.
    let _ = delete_session(server, "claude-x").await;
    let (_, headers) = python.last_hit();
    assert!(headers.get("x-hangar-rust-closed").is_none());
    assert!(headers.get("x-hangar-closed-transcript").is_none(), "o forjado sai");
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn owner_delete_without_a_header_secret_warns_the_skipped_python_cleanup() {
    let fx = fixture();
    let server_app = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server_app.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server_app.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let (python, upstream) = crate::fake::spawn_fake().await;
    let mut cfg = crate::fake::config(upstream, "127.0.0.1");
    cfg.internal_secret = "quebra\nlinha".into();
    let state = hangar_server::routes::AppState::new(cfg);
    assert!(state.state.runtime.set(registry.clone()).is_ok());
    let server = crate::fake::spawn_state(state).await;
    let (status, body) = delete_session(server, "cx-proof").await;
    assert_eq!((status, body["warning"]["code"].as_str()), (200, Some("close_python_cleanup_skipped")),
        "encerrou, e a limpeza do Python que não rodou aparece com código");
    assert!(!registry.owns_codex_terminal("cx-proof"));
    assert_eq!(python.hits_to("/api/sessions/cx-proof"), 0);
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn owner_delete_that_fails_in_rust_answers_its_code_and_never_reaches_python() {
    let fx = fixture();
    let server_app = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server_app.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server_app.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    // O multiplexador aceita o kill-session e a sessão segue de pé (o psmux às vezes faz isso).
    let survivor = fx.root.join("tmux-survivor");
    crate::write_executable(&survivor, format!("#!/bin/sh\nP='{}'\ncase \"$1\" in\n  list-panes) cat \"$P\";;\n  has-session) exit 0;;\n  *) exit 0;;\nesac\n",
        fx.panes.display()));
    let watcher = Arc::new(Watcher::new(WatchConfig { home:fx.home.clone(),queue_dir:fx.queue.clone(),
        mux:Mux::with_program(&survivor,Duration::from_secs(5)),mux_program:survivor.clone().into_os_string() }));
    let registry = registry().await;
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let (python, server) = serve_owner(registry.clone()).await;
    let (status, body) = delete_session(server, "cx-proof").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (500, Some("close_kill_survived")));
    assert_eq!(python.hits_to("/api/sessions/cx-proof"), 0, "encerramento falho não repassa");
    assert!(registry.owns_codex_terminal("cx-proof"),"nada foi encerrado");
    assert!(fx.sidecar("cx-proof").is_some());
    assert!(app.try_wait().unwrap().is_none());
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn pane_only_routes_drive_the_codex_pane_with_the_terminal_writer() {
    let fx = fixture();
    fx.panes(&["cx-proof"]);
    let binding = hangar_server::terminal_input::TerminalBinding { name:"cx-proof".into(),pane:"%7".into(),conversation:"thread-1".into(),
        generation:1,created:1_700_000_000,mux_argv:vec![fx.tmux.to_string_lossy().into_owned()],windows:false,clipboard_lock_path:None };
    let driver = hangar_server::runtime::codex_terminal::pane_driver(&binding,Default::default(),None);
    let sent = driver.key("Up",false).await;
    assert_eq!(hangar_server::session_write::control::pane_step_answer(&sent).map_err(|(status,_)|status),Ok(()));
    let refused = driver.key("C-c",false).await;
    assert_eq!(hangar_server::session_write::control::pane_step_answer(&refused).map_err(|(status,_)|status.as_u16()),Err(400),"fora das teclas permitidas");
    assert_eq!(hangar_server::session_write::control::pane_step_answer(&driver.text("ls").await).map_err(|(status,_)|status),Ok(()));
    let keys = std::fs::read_to_string(fx.panes.with_extension("keys")).unwrap();
    assert!(keys.contains("send-keys -t %7 Up") && keys.contains("-l -- ls"),"{keys}");
    // Outro nascimento do pane (sessão recriada com o mesmo nome): nada é escrito.
    let stale = hangar_server::terminal_input::TerminalBinding { created:1,..binding };
    let before = keys.lines().count();
    assert!(hangar_server::session_write::control::pane_step_answer(&hangar_server::runtime::codex_terminal::pane_driver(&stale,Default::default(),None).key("Up",false).await).is_err());
    assert_eq!(std::fs::read_to_string(fx.panes.with_extension("keys")).unwrap().lines().count(),before);
}

#[tokio::test]
async fn implementing_the_plan_is_answered_in_the_rust_through_the_pane_lock() {
    // O Python teclava direto no pane do Rust, fora da trava; a rota agora é do Rust.
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-proof"));
    let menu = "• Plano pronto\n\n  Implement this plan?\n\n› 1. Yes, implement this plan\n  2. No, stay in Plan mode\n\n  Press enter to confirm or esc to go back\n";
    std::fs::write(fx.panes.with_extension("screen"),menu).unwrap();
    std::fs::write(fx.panes.with_extension("after"),"• Implementando o plano\n\n› \n").unwrap();
    let (python, server_addr) = serve_owner(registry.clone()).await;
    let post = || crate::fake::client().post(format!("http://{server_addr}/api/sessions/cx-proof/codex/plan/implement"))
        .header("authorization", format!("Bearer {}", crate::fake::OWNER)).send();
    let response = post().await.unwrap();
    assert_eq!((response.status().as_u16(),response.text().await.unwrap()),(200,json!({"ok":true}).to_string()));
    let keys = std::fs::read_to_string(fx.panes.with_extension("keys")).unwrap_or_default();
    assert!(keys.contains("-l -- \r"),"o Enter da opção 1: {keys:?}");
    // Sem o seletor na tela: 409 com o código do Python, e nada vai ao Python.
    let response = post().await.unwrap();
    assert_eq!(response.status().as_u16(),409);
    assert_eq!(serde_json::from_str::<Value>(&response.text().await.unwrap()).unwrap()["detail"]["code"],"erro_codex_controle");
    assert_eq!(python.hits_to("/api/sessions/cx-proof/codex/plan/implement"),0);
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn a_tui_pid_missing_on_one_side_never_rotates_the_key_nor_wipes_the_queue() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    // O lançador grava o `tui_pid` antes de o sidecar existir: a adoção nasce sem ele.
    let mut meta = fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui);
    meta["tui_pid"] = Value::Null;
    fx.put_sidecar("cx-proof",meta);
    fx.panes(&["cx-proof"]);
    let first = registry().await;
    fx.watcher().round(&first).await;
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    assert!(fx.queue_json(&key)["runtime_state"]["codex_life"]["tui_pid"].is_null());
    first.close(&key,1).await.unwrap();

    // O pid aparece depois; o Rust reinicia e readota: mesma chave, mesma geração, a vida aprende o pid.
    let mut later = fx.sidecar("cx-proof").unwrap();
    later["tui_pid"] = json!(tui.id());
    fx.put_sidecar("cx-proof",later);
    let second = registry().await;
    fx.watcher().round(&second).await;
    assert_eq!(key_of(&fx.sidecar("cx-proof").unwrap()),key,"a chave não gira");
    assert_eq!(fx.queue_json(&key)["generation"],1);
    assert_eq!(fx.queue_json(&key)["runtime_state"]["codex_life"]["tui_pid"],tui.id());
    assert!(second.handle(&key,1).await.is_ok());
    second.close(&key,1).await.unwrap();

    // O `sessions.save` regrava o sidecar sem o pid: a vida gravada o conserva.
    let mut saved = fx.sidecar("cx-proof").unwrap();
    saved["tui_pid"] = Value::Null;
    fx.put_sidecar("cx-proof",saved);
    let third = registry().await;
    fx.watcher().round(&third).await;
    assert_eq!(key_of(&fx.sidecar("cx-proof").unwrap()),key);
    assert_eq!(fx.queue_json(&key)["runtime_state"]["codex_life"]["tui_pid"],tui.id());
    assert!(third.handle(&key,1).await.is_ok());
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn a_new_pane_life_without_tui_pid_on_either_side_gets_a_fresh_queue() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    // Vida anterior de mesmo nome e chave, sem pid da TUI (o `null == null` que no Windows herdava a fila).
    {
        let path = fx.queue_state("old-key");
        let lease = hangar_server::runtime::queue::acquire_lease(&path.with_extension("lock")).unwrap();
        let mut store = hangar_server::runtime::queue::Store::open(&path,&fx.queue,
            hangar_server::runtime::queue::State::new("old-key",2,"cx-proof",Vec::new())).unwrap();
        store.exec(2,"life",CLOCK,Action::AdvanceLifecycle { generation:2,
            state:json!({"codex_life":{"created":1_700_000_000u64,"pane":"%7","tui_pid":null,"endpoint":server.endpoint,"app_pid":app.id()},"name":"cx-proof"}) }).unwrap();
        drop(lease);
    }
    let mut meta = fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui);
    meta["key"] = json!("old-key");
    meta["tui_pid"] = Value::Null;
    fx.put_sidecar("cx-proof",meta);
    fx.panes_born(&[("cx-proof","%8",1_700_000_999)]);
    let registry = registry().await;
    fx.watcher().round(&registry).await;
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    assert_ne!(key,"old-key","outro nascimento do terminal gira a chave");
    assert!(!fx.queue_state("old-key").exists());
    assert!(registry.handle(&key,1).await.is_ok());
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn closing_by_a_name_that_is_no_longer_this_life_kills_nothing() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    let key = key_of(&fx.sidecar("cx-proof").unwrap());
    let original = fx.sidecar("cx-proof").unwrap();

    // Renomeada pelo Python e a rodada ainda não viu: o DELETE com o nome antigo não derruba a sessão viva.
    let mut renamed = original.clone();
    renamed["name"] = json!("cx-renamed");
    fx.put_sidecar("cx-renamed",renamed);
    std::fs::remove_file(fx.sidecar_path("cx-proof")).unwrap();
    fx.panes(&["cx-renamed"]);
    let refused = registry.kill_codex_terminal("cx-proof").await.expect("vida aberta aqui").unwrap_err();
    assert_eq!(refused.code,"close_kill_stale");
    assert!(app.try_wait().unwrap().is_none(),"o app-server da sessão renomeada segue vivo");
    assert!(fx.queue_state(&key).exists(),"a fila dela segue");
    assert!(std::fs::read_to_string(&fx.panes).unwrap().starts_with("cx-renamed\t"));

    // Sidecar desta vida, mas o pane do nome é de outro nascimento (outra sessão com o mesmo nome).
    std::fs::remove_file(fx.sidecar_path("cx-renamed")).unwrap();
    fx.put_sidecar("cx-proof",original);
    fx.panes_born(&[("cx-proof","%9",1_700_000_999)]);
    let refused = registry.kill_codex_terminal("cx-proof").await.expect("vida aberta aqui").unwrap_err();
    assert_eq!(refused.code,"close_kill_stale");
    assert!(app.try_wait().unwrap().is_none());
    assert!(fx.sidecar("cx-proof").is_some() && fx.queue_state(&key).exists());
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn owner_broadcast_answers_the_rust_code_for_a_codex_terminal_it_cannot_adopt() {
    let fx = fixture();
    let server_app = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server_app.endpoint),tui());
    // Sidecar pronto e nenhum pane: a adoção na hora falha com o código do Rust.
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server_app.endpoint,Some(app.id()),&tui));
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    let (python, server) = serve_owner(registry.clone()).await;
    let response = crate::fake::client().post(format!("http://{server}/api/broadcast"))
        .header("authorization", format!("Bearer {}", crate::fake::OWNER)).header("content-type","application/json")
        .body(json!({"names":["cx-proof","claude-x"],"text":"oi"}).to_string()).send().await.unwrap();
    assert_eq!(response.status().as_u16(),200);
    let body:Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(body["results"]["cx-proof"]["error"]["code"],"codex_terminal_absent");
    assert_eq!(body["results"]["cx-proof"]["delivered"],false);
    assert_eq!(python.hits_to("/api/broadcast"),1,"só os outros nomes vão ao Python");
    // O Python falso responde texto: resposta ilegível sai com código, nunca `error: null`.
    assert_eq!(body["results"]["claude-x"]["error"]["code"],"broadcast_reply_unreadable");
    let sent:Value = serde_json::from_slice(&python.last_body()).unwrap();
    assert_eq!(sent["names"],json!(["claude-x"]),"o Codex com terminal do Rust nunca é repassado");
    assert_eq!(python.hits_to("/api/sessions/cx-proof/input"),0);
    finish(&mut [&mut app,&mut tui]);
}

#[tokio::test]
async fn clear_of_a_codex_terminal_of_the_rust_stays_in_the_rust() {
    let fx = fixture();
    let server = fake_app_server(true).await;
    let (mut app,mut tui) = (app_server_process(&server.endpoint),tui());
    fx.put_sidecar("cx-proof",fx.meta("cx-proof",&server.endpoint,Some(app.id()),&tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    fx.watcher().round(&registry).await;
    assert!(registry.is_terminal("cx-proof").await);
    let (python, owner) = serve_owner(registry.clone()).await;
    let response = crate::fake::client().post(format!("http://{owner}/api/sessions/cx-proof/input"))
        .header("authorization", format!("Bearer {}", crate::fake::OWNER)).header("content-type","application/json")
        .body(r#"{"text":"/clear"}"#).send().await.unwrap();
    assert_ne!(response.text().await.unwrap(), "from-python");
    assert_eq!(python.hits_to("/api/sessions/cx-proof/input"), 0, "a fila do Codex com terminal é do Rust: o /clear nunca vai ao Python");
    finish(&mut [&mut app,&mut tui]);
}

/// Sidecar que aponta para uma transferência cujo registro sumiu ou não se lê é erro com código
/// próprio, como o `RuntimeError` do `list_all` do Python; nunca o "ainda não tem app-server" calado.
#[tokio::test]
async fn a_missing_or_corrupt_transfer_record_is_a_visible_error() {
    let fx = fixture();
    let id = "44444444-4444-4444-8444-444444444444";
    fx.put_sidecar("cx-proof",json!({"name":"cx-proof","provider":"codex","key":"k-t","endpoint":"ws://127.0.0.1:1","app_pid":7,
        "thread_id":"t","transfer_id":id}));
    let registry = registry().await;
    let watcher = fx.watcher();
    let code = |r:Option<Result<(),RuntimeError>>|r.map(|r|r.map_err(|e|e.code));
    assert_eq!(code(watcher.adopt_now(&registry,"cx-proof").await),Some(Err("codex_transfer_record_missing".into())));
    let records = fx.home.join(".hangar/conversation-transfers");
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(records.join(format!("{id}.json")),"{torto").unwrap();
    assert_eq!(code(watcher.adopt_now(&registry,"cx-proof").await),Some(Err("codex_transfer_record_unreadable".into())));
    std::fs::write(records.join(format!("{id}.json")),r#"{"phase":"prepared"}"#).unwrap();
    assert_eq!(code(watcher.adopt_now(&registry,"cx-proof").await),Some(Err("codex_terminal_not_ready".into())),"em curso: espera");
}

/// O sidecar da sessão diz a vida dela; sem multiplexador.
struct KeyProbe(std::sync::Mutex<String>);
impl hangar_server::share::life::LifeProbe for KeyProbe {
    fn birth<'a>(&'a self,_:&'a str) -> futures_util::future::BoxFuture<'a,Option<u64>> { Box::pin(async { None }) }
    fn has_session<'a>(&'a self,_:&'a str) -> futures_util::future::BoxFuture<'a,Option<bool>> { Box::pin(async { Some(true) }) }
    fn sidecar(&self,_:&str) -> Option<(String,bool)> { Some((self.0.lock().unwrap().clone(),false)) }
    fn sidecar_exists(&self,_:&str) -> bool { true }
}

/// A troca de modo do Codex passa pela troca de vida, como o `_durante_troca` do Python: o
/// convite da sessão segue a vida nova (também quando a troca falha, que pode ter mudado a vida).
#[tokio::test]
async fn the_codex_mode_switch_carries_the_invite_to_the_new_life() {
    use hangar_server::share::{ShareState,life::{LifeFacts,Lives},pairs::PairStore,store::{Kind,ShareStore}};
    let fx = fixture();
    let registry = registry().await;
    registry.bind_watcher(fx.watcher());
    // Sem terminal parado e sem `cwd`: o Rust tenta abrir e falha, com resposta própria.
    fx.put_sidecar("cx-h",json!({"name":"cx-h","provider":"codex","headless":true,"key":"k-h","thread_id":"t"}));
    // Sem a fotografia do Python a varredura dos convites não revoga nada; só a troca move o convite.
    let probe = Arc::new(KeyProbe(std::sync::Mutex::new("k-old".into())));
    let facts = Arc::new(LifeFacts::default());
    let share = Arc::new(ShareState { store:ShareStore::open(fx.root.join("shares.json"),Arc::new(||100.0)),
        pairs:PairStore::open(fx.root.join("external_pairs.json")),lives:Lives::new(probe.clone(),facts),
        tailscale:Arc::new(hangar_server::share::tunnel::Real),tunnel:Default::default() });
    share.store.create_redeemed("cx-h",Some("k:k-old".into()),Kind::Share).unwrap();
    let (_python, upstream) = crate::fake::spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(crate::fake::config(upstream, "127.0.0.1"));
    state.share = Some(share.clone());
    assert!(state.state.runtime.set(registry).is_ok());
    let server = crate::fake::spawn_state(state).await;
    *probe.0.lock().unwrap() = "k-new".into();
    let response = crate::fake::client().post(format!("http://{server}/api/sessions/cx-h/modo-execucao"))
        .header("authorization", format!("Bearer {}", crate::fake::OWNER)).header("content-type", "application/json")
        .body(json!({"terminal":true}).to_string()).send().await.unwrap();
    let status = response.status().as_u16();
    let body = response.text().await.unwrap();
    assert_eq!(status,409,"a troca do Codex falhou aqui, no Rust: {body}");
    let lives:Vec<Option<String>> = share.store.all().into_iter().filter(|s|s.session=="cx-h").map(|s|s.life).collect();
    assert_eq!(lives,vec![Some("k:k-new".into())],"o convite foi para a vida nova");
}

/// Na subida o Rust abre sozinho o Codex sem terminal que é dele e tem o que fazer (fila
/// pendente ou cano vivo), sem o Python pedir; o parado sem pendência
/// fica fechado e o próximo envio o abre. Nada fica sem dono depois de o Rust voltar.
#[tokio::test]
async fn the_rust_boot_opens_its_codex_headless_with_pending_input() {
    use hangar_server::runtime::queue::{State,Store};
    crate::use_cano_bin();
    let fx = fixture();
    let registry = relaunch_registry(&fx).await;
    let bin = fx.root.join("bin");
    fake_headless_codex(&bin,&fx.root.join("codex-methods.jsonl"));
    let rollout = fx.root.join("rollout.jsonl");
    std::fs::write(&rollout,"{}\n").unwrap();
    for (name,key) in [("cx-h","k-pending"),("cx-idle","k-idle")] {
        fx.put_sidecar(name,json!({"name":name,"provider":"codex","headless":true,"key":key,"thread_id":"thread-1",
            "rollout_path":rollout,"cwd":fx.root,"codex_home":fx.home.join(".codex"),"permission_mode":"Full Access","model":"gpt-test"}));
    }
    std::fs::create_dir_all(fx.queue.join("runtime")).unwrap();
    let mut store = Store::open(&fx.queue_state("k-pending"),&fx.queue,State::new("k-pending",1,"cx-h",vec![])).unwrap();
    store.exec(1,"append:e1",CLOCK,Action::Append { text:"oi".into(),delivered:false,ts:Some(1.0),pre_transcript:false,entry_id:Some("e1".into()) }).unwrap();
    drop(store);
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.register_codex_headless(&registry).await;
    assert!(registry.owns_codex_headless("cx-h").await,"com mensagem esperando: aberto no Rust");
    assert!(!registry.owns_codex_headless("cx-idle").await,"parado sem pendência fica fechado");
    let generation = fx.queue_json("k-pending")["generation"].as_u64().unwrap();
    registry.close_with_kill("k-pending",generation).await.unwrap();
}

#[tokio::test]
async fn clear_of_a_headless_codex_opens_a_new_thread_without_turn_start() {
    crate::use_cano_bin();
    let fx = fixture();
    let registry = relaunch_registry(&fx).await;
    let log = fx.root.join("codex-methods.jsonl");
    fake_headless_codex(&fx.root.join("bin"),&log);
    fx.put_sidecar("cx-h",json!({"name":"cx-h","provider":"codex","headless":true,"key":"k-clear","thread_id":"thread-1",
        "cwd":fx.root,"codex_home":fx.home.join(".codex"),"permission_mode":"Full Access","model":"gpt-test","effort":"high"}));
    registry.bind_watcher(fx.watcher());
    assert!(matches!(registry.adopt_now("cx-h").await,Some(Ok(()))));
    let ready_until = Instant::now() + Duration::from_secs(5);
    while fx.queue_json("k-clear")["runtime_state"]["view"]["ready"] != true {
        assert!(Instant::now() < ready_until,"o Codex terminou de abrir");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (python,server) = serve_owner(registry.clone()).await;
    let generation = fx.queue_json("k-clear")["generation"].as_u64().unwrap();
    let handle = registry.handle("k-clear",generation).await.unwrap();
    let hold = handle.hold_turns().await.unwrap();
    handle.queue("waiting".into(),Action::Append { text:"mensagem esperando".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("waiting".into()) }).await.unwrap();
    for text in ["/clear argumento","/clear"] {
        let refused = crate::fake::client().post(format!("http://{server}/api/sessions/cx-h/input"))
            .header("authorization",format!("Bearer {}",crate::fake::OWNER)).header("content-type","application/json")
            .body(json!({"text":text}).to_string()).send().await.unwrap();
        assert_eq!(refused.status().as_u16(),400);
    }
    assert_eq!(handle.view().await.unwrap()["thread_id"],"thread-1");
    let rows = handle.queue("still-waiting".into(),Action::Load).await.unwrap();
    assert!(rows.as_array().unwrap().iter().any(|row|row["id"] == "waiting" && row["delivered"] == false));
    handle.queue("cancel".into(),Action::Cancel { entry_id:"waiting".into() }).await.unwrap();
    drop(hold);
    let response = crate::fake::client().post(format!("http://{server}/api/sessions/cx-h/input"))
        .header("authorization",format!("Bearer {}",crate::fake::OWNER)).header("content-type","application/json")
        .body(r#"{"text":"/clear"}"#).send().await.unwrap();
    assert_eq!(response.status().as_u16(),200,"{}",response.text().await.unwrap());
    let deadline = Instant::now() + Duration::from_secs(3);
    while fx.sidecar("cx-h").unwrap()["thread_id"] != "thread-new" {
        assert!(Instant::now() < deadline,"a conversa nova foi gravada");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let sidecar = fx.sidecar("cx-h").unwrap();
    assert_eq!(sidecar["key"],"k-clear");
    assert_eq!(sidecar["effort"],"high");
    let methods:Vec<Value> = std::fs::read_to_string(&log).unwrap().lines().map(|line|serde_json::from_str(line).unwrap()).collect();
    assert!(methods.iter().any(|method|method["method"] == "thread/start"));
    assert!(!methods.iter().any(|method|method["method"] == "turn/start"));
    assert_eq!(python.hits_to("/api/sessions/cx-h/input"),0);
    registry.close_with_kill("k-clear",generation).await.unwrap();
}
