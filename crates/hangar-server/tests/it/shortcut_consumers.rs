//! Contrato HTTP dos consumidores dos terminais auxiliares.
use super::{Execution, isolated, value};
use crate::fake::{OWNER, client};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn auxiliary_consumers_preserve_ownership_and_close_without_python() {
    if isolated("project_shortcuts::consumers_tests::auxiliary_consumers_preserve_ownership_and_close_without_python") { return; }
    let execution=Execution::new().await;
    let created=value(execution.run("main","shortcut-shell",json!({"command":"hold","label":"Auxiliar","ask":false})).await,202).await;
    let id=created["terminal"]["id"].as_str().unwrap();
    let base=format!("http://{}",execution.addr);
    let listed=value(client().get(format!("{base}/api/sessions/fixture-main/shortcut-terminals")).bearer_auth(OWNER).send().await.unwrap(),200).await;
    assert_eq!(listed["terminals"][0]["id"],id);
    assert_eq!(listed["terminals"][0]["label"],"Auxiliar");
    assert_eq!(listed["terminals"][0]["alive"],true);
    assert!(listed["terminals"][0]["question"].is_null());
    assert!(listed["terminals"][0].get("owner").is_none());
    let other=value(client().get(format!("{base}/api/sessions/fixture-sub/shortcut-terminals")).bearer_auth(OWNER).send().await.unwrap(),200).await;
    assert_eq!(other["terminals"],json!([]));
    let denied=value(execution.run("sub",&format!("shortcut-terminals/{id}/close"),json!({})).await,404).await;
    assert_eq!(denied["detail"]["code"],"erro_shortcut_terminal_inexistente");
    let invalid=value(execution.run("main",&format!("shortcut-terminals/{id}/answer"),json!({"text":"a\nb"})).await,400).await;
    assert_eq!(invalid["detail"]["code"],"erro_shortcut_resposta_invalida");
    value(execution.run("main",&format!("shortcut-terminals/{id}/close"),json!({})).await,200).await;
    let closed=value(execution.run("main",&format!("shortcut-terminals/{id}/close"),json!({})).await,404).await;
    assert_eq!(closed["detail"]["code"],"erro_shortcut_terminal_inexistente");
    assert_eq!(execution.hits.load(Ordering::Relaxed),0);
}

#[tokio::test]
async fn hangar_consumers_answer_focus_and_keep_global_terminal_outside_session() {
    if isolated("project_shortcuts::consumers_tests::hangar_consumers_answer_focus_and_keep_global_terminal_outside_session") { return; }
    let execution=Execution::new().await;
    let mux=execution.scene.home.join("execute-mux");
    let script=std::fs::read_to_string(&mux).unwrap();
    let capture=execution.scene.home.join("typed");
    crate::write_executable(&mux,script.replace("case \"$1\" in",&format!("case \"$1\" in\nsend-keys) printf '%s\\n' \"$6\" >> '{}'; exit 0 ;;",capture.display())));
    let created=value(execution.run("main","shortcut-shell",json!({"command":"hold","label":"Global","runs_in":"hangar","key":"global:test","ask":false})).await,202).await;
    let id=created["terminal"]["id"].as_str().unwrap();
    let base=format!("http://{}/api/hangar-terminals",execution.addr);
    let listed=value(client().get(&base).bearer_auth(OWNER).send().await.unwrap(),200).await;
    assert_eq!(listed["terminals"][0]["id"],id);
    assert_eq!(listed["terminals"][0]["owner"],"");
    assert_eq!(listed["terminals"][0]["key"],"global:test");
    value(client().post(format!("{base}/{id}/answer")).bearer_auth(OWNER).json(&json!({"text":"--valor;"})).send().await.unwrap(),200).await;
    assert_eq!(std::fs::read_to_string(&capture).unwrap(),"--valor\\;\n\r\n");
    std::fs::write(&capture,"").unwrap();
    value(client().post(format!("{base}/{id}/answer")).bearer_auth(OWNER).json(&json!({})).send().await.unwrap(),200).await;
    assert_eq!(std::fs::read_to_string(&capture).unwrap(),"\r\n");
    let focused=value(client().post(format!("{base}/{id}/focus")).bearer_auth(OWNER).send().await.unwrap(),200).await;
    assert_eq!(focused["focused"],false);
    let script=std::fs::read_to_string(&mux).unwrap();
    crate::write_executable(&mux,script.replace("send-keys) printf", "send-keys) exit 1 ;;\nignored) printf"));
    let failed=value(client().post(format!("{base}/{id}/answer")).bearer_auth(OWNER).json(&json!({"text":"sim"})).send().await.unwrap(),500).await;
    assert_eq!(failed["detail"]["code"],"erro_shortcut_resposta");
    value(client().post(format!("{base}/{id}/close")).bearer_auth(OWNER).send().await.unwrap(),200).await;
    let gone=value(client().get(&base).bearer_auth(OWNER).send().await.unwrap(),200).await;
    assert_eq!(gone["terminals"],json!([]));
    assert_eq!(execution.hits.load(Ordering::Relaxed),0);
}
