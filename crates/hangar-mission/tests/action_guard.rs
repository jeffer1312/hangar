use hangar_mission::Store;
use hangar_mission::store::{Link, OwnerChange, Report, TicketMetadata};

fn link(name: &str) -> Link {
    Link { key: "T-1".into(), title: "Exemplo".into(), url: "https://example.invalid/T-1".into(), session: name.into() }
}

#[test]
fn board_action_protects_its_ticket_while_allowing_agent_reports_and_other_tickets() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::new(temp.path().join("mission"));
    store.link("help", "T-1", link("session-1")).unwrap();
    let action = store.begin_action("help", "T-1").expect("a primeira ação precisa reservar o ticket");
    assert!(matches!(store.begin_action("help", "T-1"), Err(error) if error.code == "mission_action_busy"));
    assert_eq!(store.link("help", "T-1", link("session-2")).unwrap_err().code, "mission_action_busy");
    assert_eq!(store.owner_change("help", "T-1", OwnerChange::Stage { stage: "revisao".into() }).unwrap_err().code, "mission_action_busy");
    assert_eq!(store.remove("help", "T-1").unwrap_err().code, "mission_action_busy");
    assert_eq!(store.set_stages(store.snapshot().unwrap().stages).unwrap_err().code, "mission_action_busy");
    store.link("help", "T-2", link("other-session")).unwrap();
    let reported = store.report("session-1", Report { stage: Some("execucao".into()), note: None, evidence: Vec::new() }).unwrap();
    assert_eq!(reported.stage.as_deref(), Some("execucao"));
    drop(action);
    assert!(store.link("help", "T-1", link("session-2")).is_ok(), "a reserva precisa sair ao terminar");
}

#[test]
fn board_action_can_link_under_its_reservation_and_keeps_a_newer_agent_stage() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::new(temp.path().join("mission"));
    let action = store.begin_action("help", "T-1").unwrap();
    let (stage, previous) = store.action_stage(&action, "planejamento").expect("a etapa inicial precisa estar disponível");
    assert_eq!(stage.id, "planejamento");
    assert!(previous.is_none());
    let (linked, _) = store.link_action(&action, link("session-1")).unwrap();
    store.report("session-1", Report { stage: Some("execucao".into()), note: None, evidence: Vec::new() }).unwrap();
    let committed = store.commit_action(&action, "planejamento", linked.history.len(), "Ação de planejamento enviada.").unwrap();
    assert_eq!(committed.stage.as_deref(), Some("execucao"), "um relato novo do agente não pode regredir");
    assert!(matches!(store.action_stage(&action, "entrega"), Err(error) if error.code == "mission_gate_pending"));
    let other = Store::new(temp.path().join("other"));
    assert!(other.link_action(&action, link("wrong-store")).is_err(), "a reserva só vale no store que a emitiu");
}

#[test]
fn board_action_without_a_message_tracks_the_stage_without_creating_a_session() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::new(temp.path().join("mission"));
    let action = store.begin_action("help", "T-1").unwrap();
    let item = store.track_action(&action, TicketMetadata { key: "T-1".into(), title: "Exemplo".into(), url: "https://example.invalid/T-1".into() })
        .expect("um ticket sem sessão também precisa ser acompanhado");
    assert!(item.session.is_none());
    let item = store.commit_action(&action, "planejamento", item.history.len(), "Etapa escolhida pelo dono.").unwrap();
    assert_eq!(item.stage.as_deref(), Some("planejamento"));
    assert!(item.session.is_none());
}

#[test]
fn board_action_link_follows_session_rename_without_losing_its_life_or_reservation() {
    let temp=tempfile::tempdir().unwrap();
    let store=Store::new(temp.path().join("mission"));
    let action=store.begin_action("help","T-1").unwrap();
    store.link_bound_action(&action,link("session-1"),"life-1",Some("conversation-1")).unwrap();
    assert!(store.rename_session("session-1","renamed").unwrap());
    let item=store.snapshot().unwrap().items.remove(0);
    assert_eq!(item.session.as_deref(),Some("renamed"));
    assert_eq!(item.session_key.as_deref(),Some("life-1"));
    assert_eq!(item.conversation.as_deref(),Some("conversation-1"));
    assert!(store.begin_action("help","T-1").is_err());
    assert!(store.rename_session("renamed","session-1").unwrap());
    assert_eq!(store.snapshot().unwrap().items[0].session.as_deref(),Some("session-1"));
}

#[test]
fn board_action_manual_move_records_the_owner_choice_after_an_agent_report() {
    let temp=tempfile::tempdir().unwrap();let store=Store::new(temp.path().join("mission"));
    let action=store.begin_action("help","T-1").unwrap();
    store.link_action(&action,link("session-1")).unwrap();
    store.report("session-1",Report {stage:Some("execucao".into()),note:None,evidence:Vec::new()}).unwrap();
    let item=store.move_action(&action,"revisao").unwrap();
    assert_eq!(item.stage.as_deref(),Some("revisao"));
    assert_eq!(item.history.last().unwrap().by,"dono");
    assert!(matches!(store.move_action(&action,"entrega"),Err(error) if error.code=="mission_gate_pending"));
}
