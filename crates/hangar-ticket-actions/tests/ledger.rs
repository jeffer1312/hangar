use hangar_ticket_actions::{Ledger, Status};
use serde_json::json;
use std::sync::{Arc, Barrier};

const ID: &str = "1234567890abcdef1234567890abcdef";

#[test]
fn board_action_attempt_is_reserved_once_even_when_two_clients_submit_together() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = Arc::new(Ledger::new(temp.path().join("actions")));
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2).map(|_| {
        let (ledger, barrier) = (ledger.clone(), barrier.clone());
        std::thread::spawn(move || {
            barrier.wait();
            ledger.reserve(ID, &json!({"connector":"help", "ticket_id":"T-1", "stage":"planejamento"}))
        })
    }).collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let reservations: Vec<_> = results.into_iter().map(|r| r.expect("a tentativa precisa ser reservada")).collect();
    assert_eq!(reservations.iter().filter(|r| r.fresh).count(), 1, "somente um cliente recebe a execução");
    assert!(reservations.iter().all(|r| r.attempt.id == ID));
}

#[test]
fn board_action_attempt_survives_a_restart_without_reexecuting_an_uncertain_operation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("actions");
    let request = json!({"connector":"help", "ticket_id":"T-1", "stage":"planejamento", "launch":{"cwd":"repo", "provider":"claude"}});
    let ledger = Ledger::new(path.clone());
    assert!(ledger.reserve(ID, &request).unwrap().fresh);
    assert!(!ledger.reserve(ID, &request).unwrap().fresh);
    let restarted = Ledger::new(path);
    let reserved = restarted.reserve(ID, &request).unwrap();
    assert!(!reserved.fresh, "retomar não concede uma segunda execução");
    assert_eq!(reserved.attempt.status, Status::Uncertain);
    let different = json!({"connector":"help", "ticket_id":"T-1", "stage":"execucao"});
    assert!(matches!(restarted.reserve(ID, &different), Err(error) if error.code == "ticket_action_conflict"));
}

#[test]
fn board_action_receipt_preserves_its_session_and_a_final_result_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("actions");
    let ledger = Ledger::new(path.clone());
    let request = json!({"connector":"help", "ticket_id":"T-1", "stage":"planejamento"});
    ledger.reserve(ID, &request).unwrap();
    ledger.record(ID, Status::Opening, Some("session-1"), None).expect("a identidade precisa ser guardada antes de abrir");
    ledger.record(ID, Status::Sent, None, None).unwrap();
    let result = Ledger::new(path).read(ID).unwrap();
    assert_eq!(result.status, Status::Sent);
    assert_eq!(result.session.as_deref(), Some("session-1"));
    let unchanged = ledger.record(ID, Status::Opening, Some("session-2"), None).unwrap();
    assert_eq!(unchanged.status, Status::Sent, "um resultado definitivo não volta a abrir a operação");
    assert_eq!(unchanged.session.as_deref(), Some("session-1"));
}

#[test]
fn board_action_latest_receipt_belongs_to_its_ticket_and_follows_the_recorded_result() {
    let temp = tempfile::tempdir().unwrap();
    let ledger = Ledger::new(temp.path().join("actions"));
    let request = json!({"connector":"help", "ticket_id":"T-1", "stage":"planejamento"});
    ledger.reserve(ID, &request).unwrap();
    assert_eq!(ledger.latest("help", "T-1").unwrap().unwrap().id, ID);
    let second = "2234567890abcdef1234567890abcdef";
    ledger.reserve(second, &json!({"connector":"help", "ticket_id":"T-2", "stage":"execucao"})).unwrap();
    assert_eq!(ledger.latest("help", "T-2").unwrap().unwrap().id, second);
    assert_eq!(ledger.latest("help", "T-1").unwrap().unwrap().id, ID);
    ledger.record(ID, Status::Queued, Some("session-1"), None).unwrap();
    assert_eq!(ledger.latest("help", "T-1").unwrap().unwrap().status, Status::Queued);
    assert!(ledger.latest("other", "T-1").unwrap().is_none());
}

#[test]
fn board_action_receipt_keeps_the_life_and_conversation_of_an_incomplete_open() {
    let temp=tempfile::tempdir().unwrap();
    let path=temp.path().join("actions");
    let ledger=Ledger::new(path.clone());
    ledger.reserve(ID,&json!({"connector":"help","ticket_id":"T-1","stage":"planejamento"})).unwrap();
    ledger.record(ID,Status::Opening,Some("session-1"),None).unwrap();
    ledger.bind(ID,&hangar_ticket_actions::Session {name:"session-1".into(),key:"life-1".into(),conversation:Some("conversation-1".into())}).unwrap();
    let receipt=Ledger::new(path).read(ID).unwrap();
    assert_eq!(receipt.status,Status::Uncertain);
    assert_eq!(receipt.session_key.as_deref(),Some("life-1"));
    assert_eq!(receipt.conversation.as_deref(),Some("conversation-1"));
}
