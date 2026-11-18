//! `link::resolve`: a regra da tool `mission` para achar o ticket da sessão, sem gravar nada.
use hangar_mission::link::resolve;
use hangar_mission::model::{Item, Snapshot};
use serde_json::json;

fn item(ticket: &str, extra: serde_json::Value) -> Item {
    let mut value = json!({"connector": "help", "ticket_id": ticket, "key": format!("PM-{ticket}"), "title": "t", "url": "https://help.invalid/"});
    value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

fn snapshot(items: Vec<Item>) -> Snapshot { Snapshot { stages: Vec::new(), items, order: Default::default() } }

fn ticket(found: Option<Item>) -> Option<String> { found.map(|item| item.ticket_id) }

#[test]
fn member_confirmed_by_life_or_group_and_active_preferred() {
    let snap = snapshot(vec![
        item("1", json!({"sessions": ["a"], "session_keys": {"a": "life-a"}, "delivered_at": "2026-11-01T00:00:00Z"})),
        item("2", json!({"sessions": ["a"], "session_keys": {"a": "life-a"}})),
        item("3", json!({"group_ids": ["g1"]})),
    ]);
    assert_eq!(ticket(resolve(&snap, "a", Some("life-a"), None).unwrap()).as_deref(), Some("2"), "o ativo vence o entregue");
    assert_eq!(ticket(resolve(&snap, "a", Some("outra-vida"), None).unwrap()), None, "outra vida com o mesmo nome não herda");
    assert_eq!(ticket(resolve(&snap, "b", Some("life-b"), Some("g1")).unwrap()).as_deref(), Some("3"), "membro do grupo ligado");
    assert_eq!(ticket(resolve(&snap, "b", None, None).unwrap()), None, "fora do vínculo");
}

#[test]
fn delivered_only_falls_back_to_the_last_match() {
    let snap = snapshot(vec![
        item("1", json!({"sessions": ["a"], "delivered_at": "2026-11-01T00:00:00Z"})),
        item("2", json!({"sessions": ["a"], "delivered_at": "2026-11-02T00:00:00Z"})),
    ]);
    assert_eq!(ticket(resolve(&snap, "a", None, None).unwrap()).as_deref(), Some("2"));
}

#[test]
fn two_active_tickets_in_the_group_conflict() {
    let snap = snapshot(vec![item("1", json!({"group_ids": ["g1"]})), item("2", json!({"group_ids": ["g1"]}))]);
    let error = resolve(&snap, "a", None, Some("g1")).unwrap_err();
    assert_eq!(error.code, "ticket_group_conflict");
}
