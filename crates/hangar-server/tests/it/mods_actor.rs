//! A interface dos mods no ator do runtime: pedido de app e escrita fora do diário.
use crate::mods_support;

use hangar_server::mods::model::ModsCall;
use hangar_server::mods::state::Mods;
use hangar_server::runtime::{actor::*,cano,protocol::*,queue::*};
use mods_support::{budget,claude_target,registry,vitrine_cano,wait_request,wait_ui};
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};

/// Cano que só confirma as escritas.
async fn quiet_cano() -> (String,tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut raw = String::new(); reader.read_line(&mut raw).await.unwrap();
        let snapshot = mods_support::cano_snapshot_json();
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        loop {
            raw.clear();
            if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { break; }
            let envelope:serde_json::Value = serde_json::from_str(&raw).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            reader.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
        }
    });
    (format!("tcp:{address}"),task)
}

#[tokio::test]
async fn mods_call_before_attach_answers_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (escuta,server) = quiet_cano().await;
    let target = claude_target(dir.path(),escuta,false);
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    let connection = cano::connect(&target.binding).await.unwrap();
    let engine = RuntimeEngine::new("claude",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap()
        .with_mods(Mods::default());
    let handle = RuntimeActor::spawn(target,QueueActor::start(store,lease),connection,engine);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2),handle.mods(ModsCall::Press { site: "above-prompt".into(), plugin: "vitrine".into(), key: "k".into() },budget())).await.unwrap();
    assert_eq!(result.unwrap_err().code,"erro_mod_botao_inexistente");
    handle.stop().await.unwrap();
    let after = handle.mods(ModsCall::Show { site:"p".into() },budget()).await.unwrap_err();
    assert_eq!(after.code,"erro_mod_clique_sem_resposta","ator parado responde com código");
    server.abort();
}

/// Fila, trava e conexão de um ator montado à mão, sem o registro.
async fn actor_parts(target:&RuntimeTarget) -> (QueueActor,cano::CanoConnection) {
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    (QueueActor::start(store,lease),cano::connect(&target.binding).await.unwrap())
}

#[tokio::test]
async fn surface_session_publishes_the_band_and_answers_a_press() {
    let dir = tempfile::tempdir().unwrap();
    let (escuta,_,server) = vitrine_cano(&[]).await;
    let mods = Mods::default();
    let registry = registry(&mods);
    registry.open(claude_target(dir.path(),escuta,true)).await.unwrap();
    assert!(mods.owns("session"));
    let band = wait_ui(&mods,|ui|ui["above"].to_string().contains("superfície desktop")).await;
    assert_eq!(band["source"],"surface");
    let link = mods.link("session").unwrap().link;
    let pressed = link.call(ModsCall::Press { site: "above-prompt".into(), plugin: "vitrine".into(), key: "abrir-vitrine-botoes".into() },budget()).await.unwrap();
    assert_eq!(pressed["element"],"abrir-vitrine-botoes");
    wait_ui(&mods,|ui|ui["panes"][0]["id"] == "vitrine-botoes").await;
    let journal = std::fs::read_to_string(dir.path().join("key.queue-state.json")).unwrap();
    assert!(!journal.contains("ui_attach") && !journal.contains("ui_render") && !journal.contains("ui_press"),"pedido da superfície não entra no diário");
    registry.close("key",1).await.unwrap();
    assert!(!mods.owns("session"),"fechar esquece a sessão (S9)");
    server.abort();
}

#[tokio::test]
async fn closing_the_session_answers_pending_calls() {
    let dir = tempfile::tempdir().unwrap();
    let (escuta,seen,server) = vitrine_cano(&["ui_press"]).await;
    let mods = Mods::default();
    let registry = registry(&mods);
    registry.open(claude_target(dir.path(),escuta,true)).await.unwrap();
    wait_ui(&mods,|ui|!ui["above"].is_null()).await;
    let link = mods.link("session").unwrap().link;
    let pending = tokio::spawn(async move { link.call(ModsCall::Press { site: "above-prompt".into(), plugin: "vitrine".into(), key: "abrir-vitrine-botoes".into() },budget()).await });
    // O clique tem que estar em aberto no ator, já no fio, quando a sessão fecha.
    wait_request(&seen,"ui_press").await;
    registry.close("key",1).await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2),pending).await.unwrap().unwrap();
    assert_eq!(result.unwrap_err().code,"erro_mod_clique_sem_resposta");
    server.abort();
}

#[tokio::test]
async fn reopening_attaches_again_with_a_new_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let (escuta,ids,server) = vitrine_cano(&[]).await;
    let mods = Mods::default();
    let registry = registry(&mods);
    let target = claude_target(dir.path(),escuta,true);
    registry.open(target.clone()).await.unwrap();
    wait_ui(&mods,|ui|!ui["above"].is_null()).await;
    registry.close("key",1).await.unwrap();
    registry.open(target).await.unwrap();
    wait_ui(&mods,|ui|!ui["above"].is_null()).await;
    let prefixes:std::collections::BTreeSet<String> = ids.lock().unwrap().iter().map(|(_,id)|id.rsplit_once(':').unwrap().0.to_owned()).collect();
    assert_eq!(prefixes.len(),2,"cada vida do ator tem prefixo próprio: {prefixes:?}");
    registry.close("key",1).await.unwrap();
    server.abort();
}

/// Com o runtime multithread, a tarefa do ator rodaria em outra thread durante o `before` se já
/// existisse: o `ui_attach` chegaria ao cano e a primeira faixa iria para um `Mods` sem a sessão.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_is_registered_before_the_actor_takes_its_first_step() {
    let dir = tempfile::tempdir().unwrap();
    let (escuta,seen,server) = vitrine_cano(&[]).await;
    let target = claude_target(dir.path(),escuta,true);
    let (queue,connection) = actor_parts(&target).await;
    let mods = Mods::default();
    let engine = RuntimeEngine::new("claude",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap()
        .with_mods(mods.clone());
    let mut quiet_before = None;
    let handle = RuntimeActor::spawn_with(target,queue,connection,engine,|handle| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        quiet_before = Some(seen.lock().unwrap().is_empty());
        mods.attach("session",1,Arc::new(handle.clone()));
    });
    assert_eq!(quiet_before,Some(true),"o ator não pode dar o primeiro passo antes do registro");
    wait_ui(&mods,|ui|ui["above"].to_string().contains("superfície desktop")).await;
    handle.stop().await.unwrap();
    server.abort();
}

/// Cano cujo retrato traz um pedido pendente com id inválido: o `hydrate` falha e o ator termina com erro.
async fn broken_cano() -> (String,tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut raw = String::new(); reader.read_line(&mut raw).await.unwrap();
        let mut snapshot = mods_support::cano_snapshot_json();
        snapshot["pendentes"] = json!([json!({"type":"control_request","request_id":[1]}).to_string()]);
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        while reader.read_line(&mut raw).await.unwrap_or(0) > 0 { raw.clear(); }
    });
    (format!("tcp:{address}"),task)
}

#[tokio::test]
async fn an_actor_that_dies_with_an_error_clears_the_band_and_keeps_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let (escuta,server) = broken_cano().await;
    let target = claude_target(dir.path(),escuta,true);
    let (queue,connection) = actor_parts(&target).await;
    let mods = Mods::default();
    let engine = RuntimeEngine::new("claude",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap()
        .with_mods(mods.clone());
    let handle = RuntimeActor::spawn_with(target,queue,connection,engine,|handle| {
        mods.attach("session",1,Arc::new(handle.clone()));
        // A faixa de uma vida anterior, com botão: é ela que não pode ficar nos apps.
        mods.publish_ui("session",1,json!({"above":{"type":"Button","key":"k"},"panes":[],"shown_id":null,"columns":110,"source":"surface"}));
    });
    let ui = wait_ui(&mods,|ui|ui["above"].is_null()).await;
    assert_eq!(ui["panes"],json!([]));
    assert!(mods.owns("session"),"a posse só sai no close");
    assert!(handle.stop().await.is_err(),"o ator terminou com erro");
    server.abort();
}
