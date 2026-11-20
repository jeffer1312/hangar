//! Painel da fila do nativo: mesma regra do `packages/core/src/queuePanel.ts` (o nativo não importa o core).
use std::borrow::Cow;

use crate::api::dto::{ChatEvent, ChatEventExt};
use serde_json::Value;

const PREFIX: &str = "queued-";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueueEntry { pub id: String, pub text: String, pub ts: Option<f64> }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueuePanel { pub entries: Vec<QueueEntry>, pub actions: bool }

/// Edição em curso; `saved_draft` é o rascunho que volta ao sair dela.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueEdit { pub entry_id: String, pub original: String, pub saved_draft: String }

/// Corpo do evento `queue`; `entries` que não é lista é quadro inválido.
pub fn parse(data: &Value) -> Option<QueuePanel> {
    let entries = data.get("entries")?.as_array()?.iter().filter_map(|e| Some(QueueEntry {
        id: e.get("id")?.as_str().filter(|id| !id.is_empty())?.to_owned(),
        text: e.get("text")?.as_str()?.to_owned(),
        ts: e.get("ts").and_then(Value::as_f64),
    })).collect();
    Some(QueuePanel { entries, actions: data.get("actions").and_then(Value::as_bool) == Some(true) })
}

/// Ids que esperavam e foram apagados noutro aparelho (`removed` do evento): a bolha `queued-<id>` sai daqui também.
pub fn removed(data: &Value) -> Vec<String> {
    data.get("removed").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
        .filter(|id| !id.is_empty()).map(|id| format!("{PREFIX}{id}")).collect()
}

/// Porta de `waitsInQueue` (core): no Codex e sem terminal (`process`) a fila é do processo, entregue não espera mais.
pub fn waits_in_queue(e: &ChatEvent, process: bool) -> bool {
    e.kind == "user_msg" && e.queued() && e.desistiu != Some(true) && e.queued_consumed != Some(true)
        && !(process && e.queued_delivered == Some(true))
}

/// Bolha que entra no painel derivado: espera e ainda não foi entregue. A "Na conversa" não escrita também: ali
/// ela pode ser cancelada.
fn derived_entry(e: &ChatEvent) -> bool {
    waits_in_queue(e, false) && e.queued_delivered == Some(false) && e.queued_confirmed != Some(true)
}

/// Mandada "Na conversa" e já escrita: bolha no fim da conversa, nunca linha do painel. Antes da escrita (limite,
/// escrita incerta, adiada) ela espera como a da Fila.
pub fn in_conversation(e: &ChatEvent) -> bool { e.queued() && e.queued_conversation == Some(true) && e.queued_delivered == Some(true) }

/// A bolha "Em espera": a mandada "Na conversa" que a CLI ainda não leu.
pub fn conversation_waiting(e: &ChatEvent) -> bool {
    in_conversation(e) && e.kind == "user_msg" && e.queued_confirmed != Some(true) && e.queued_consumed != Some(true) && e.desistiu != Some(true)
}

/// Trecho que identifica a mensagem no rótulo de acessibilidade do "Enviar agora", como o do descarte no Expo.
pub fn label_excerpt(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect()
}

/// Sem o evento (Pi, omp, Kimi, modo `python`): as bolhas da fila que ainda esperam, sem ações.
pub fn derived(events: &[ChatEvent]) -> QueuePanel {
    let entries = events.iter().filter(|e| derived_entry(e))
        .map(|e| QueueEntry { id: e.id[PREFIX.len()..].to_owned(), text: e.text.clone().unwrap_or_default(), ts: e.queued_ts })
        .collect();
    QueuePanel { entries, actions: false }
}

/// O painel do servidor emprestado; sem ele, o derivado das bolhas.
pub fn current<'a>(server: Option<&'a QueuePanel>, events: &[ChatEvent]) -> Cow<'a, QueuePanel> {
    server.map_or_else(|| Cow::Owned(derived(events)), Cow::Borrowed)
}

/// Quantas entradas o painel tem, sem montá-lo.
pub fn len(server: Option<&QueuePanel>, events: &[ChatEvent]) -> usize {
    server.map_or_else(|| events.iter().filter(|e| derived_entry(e)).count(), |panel| panel.entries.len())
}

/// A bolha `queued-<id>` some da conversa enquanto a entrada está no painel.
pub fn hidden(id: &str, panel: &QueuePanel) -> bool {
    id.strip_prefix(PREFIX).is_some_and(|raw| panel.entries.iter().any(|e| e.id == raw))
}

/// Trocar de linha no meio da edição mantém o rascunho de antes da primeira.
pub fn begin_edit(entry: &QueueEntry, draft: &str, current: Option<&QueueEdit>) -> QueueEdit {
    QueueEdit { entry_id: entry.id.clone(), original: entry.text.clone(),
        saved_draft: current.map_or_else(|| draft.to_owned(), |e| e.saved_draft.clone()) }
}

pub fn can_update(text: &str) -> bool { let t = text.trim(); !t.is_empty() && !t.starts_with('/') }

/// `Some(texto do composer)` quando a mensagem saiu da fila durante a edição.
pub fn edit_left(edit: &QueueEdit, panel: &QueuePanel, composer: &str) -> Option<String> {
    if panel.entries.iter().any(|e| e.id == edit.entry_id) { return None; }
    Some(if edit.saved_draft.trim().is_empty() { composer.to_owned() } else { edit.saved_draft.clone() })
}

/// `Some(before_id)` para mover uma posição (`Some(None)` é o fim); `None`, nada a mover.
pub fn move_target(entries: &[QueueEntry], id: &str, up: bool) -> Option<Option<String>> {
    let i = entries.iter().position(|e| e.id == id)?;
    let neighbor = if up { i.checked_sub(1)? } else { i + 1 };
    drop_target(entries, id, &entries.get(neighbor)?.id, !up)
}

/// Soltar `drag` antes (ou depois, com `after`) de `over`; soltar no próprio lugar não move.
pub fn drop_target(entries: &[QueueEntry], drag: &str, over: &str, after: bool) -> Option<Option<String>> {
    let over_i = entries.iter().position(|e| e.id == over)?;
    let from = entries.iter().position(|e| e.id == drag)?;
    let before = if after { entries.get(over_i + 1).map(|e| e.id.clone()) } else { Some(over.to_owned()) };
    let next = entries.get(from + 1).map(|e| e.id.clone());
    (before.as_deref() != Some(drag) && before != next).then_some(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    fn panel(ids: &[&str]) -> QueuePanel {
        QueuePanel { actions: true, entries: ids.iter().map(|id| QueueEntry { id: (*id).into(), text: format!("t {id}"), ts: Some(1.0) }).collect() }
    }
    fn queued(id: &str, delivered: Option<bool>) -> ChatEvent {
        ChatEvent { kind: "user_msg".into(), id: format!("queued-{id}"), text: Some(format!("t {id}")), queued_delivered: delivered, ..Default::default() }
    }

    #[test]
    fn parses_the_server_event_and_derives_without_it() {
        let parsed = parse(&serde_json::json!({"entries":[{"id":"a","text":"x","ts":2.0},{"id":1}],"actions":true})).unwrap();
        assert_eq!(parsed.entries, vec![QueueEntry { id: "a".into(), text: "x".into(), ts: Some(2.0) }]);
        assert!(parsed.actions);
        assert!(!parse(&serde_json::json!({"entries":[],"actions":"yes"})).unwrap().actions);
        assert!(parse(&serde_json::json!({"entries":"x"})).is_none());
        let mut gave_up = queued("c", Some(false));
        gave_up.desistiu = Some(true);
        let derived = derived(&[queued("a", Some(false)), queued("b", Some(true)), gave_up, queued("d", None)]);
        assert_eq!((derived.entries.len(), derived.actions), (1, false));
        assert_eq!(derived.entries[0].id, "a");
        assert_eq!(current(Some(&panel(&["z"])), &[queued("a", Some(false))]).entries[0].id, "z");
        let events = [queued("a", Some(false)), queued("b", Some(true))];
        assert_eq!((len(None, &events), len(Some(&panel(&["y", "z"])), &events)), (1, 2));
    }

    #[test]
    fn written_conversation_rows_wait_in_the_conversation_and_unwritten_ones_in_the_panel() {
        let mut row = queued("c", Some(true));
        row.queued_conversation = Some(true);
        let panel = derived(&[queued("a", Some(false)), row.clone()]);
        assert_eq!(panel.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["a"]);
        assert_eq!(len(None, &[row.clone()]), 0);
        assert!(!hidden("queued-c", &panel));
        assert!(conversation_waiting(&row), "escrita mas ainda não lida pela CLI");
        let ends: [fn(&mut ChatEvent); 3] = [|e| e.queued_confirmed = Some(true), |e| e.queued_consumed = Some(true), |e| e.desistiu = Some(true)];
        for done in ends {
            let mut ended = row.clone();
            done(&mut ended);
            assert!(!conversation_waiting(&ended));
        }
        assert!(!conversation_waiting(&queued("a", Some(false))), "linha comum da fila");
        // Não escrita (limite, escrita incerta, adiada): fica no painel, onde pode ser cancelada.
        let mut unwritten = row.clone();
        unwritten.queued_delivered = Some(false);
        assert!(!conversation_waiting(&unwritten));
        let panel = derived(&[unwritten.clone()]);
        assert_eq!(panel.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["c"]);
        assert!(hidden("queued-c", &panel));
        assert_eq!(len(None, &[unwritten]), 1);
    }

    #[test]
    fn send_now_label_excerpt_is_one_short_line() {
        assert_eq!(label_excerpt("  olha\n\tisso  aqui "), "olha isso aqui");
        assert_eq!(label_excerpt(&"é".repeat(90)).chars().count(), 80);
    }

    #[test]
    fn removed_ids_become_bubble_ids() {
        let data = serde_json::json!({"entries":[],"actions":true,"removed":["a","",3,"b"]});
        assert_eq!(removed(&data), vec!["queued-a".to_owned(), "queued-b".to_owned()]);
        assert!(removed(&serde_json::json!({"entries":[]})).is_empty(), "servidor antigo, sem o campo");
    }

    #[test]
    fn hides_only_bubbles_in_the_panel() {
        assert!(hidden("queued-a", &panel(&["a"])));
        assert!(!hidden("queued-b", &panel(&["a"])));
        assert!(!hidden("a", &panel(&["a"])));
    }

    #[test]
    fn edit_keeps_the_first_draft_and_leaving_follows_the_spec() {
        let first = begin_edit(&panel(&["a"]).entries[0], "rascunho", None);
        assert_eq!((first.entry_id.as_str(), first.original.as_str()), ("a", "t a"));
        let second = begin_edit(&panel(&["b"]).entries[0], "t a mudado", Some(&first));
        assert_eq!(second.saved_draft, "rascunho");
        let edit = QueueEdit { entry_id: "a".into(), original: "x".into(), saved_draft: String::new() };
        assert_eq!(edit_left(&edit, &panel(&["a"]), "novo"), None);
        assert_eq!(edit_left(&edit, &panel(&[]), "novo").as_deref(), Some("novo"));
        let kept = QueueEdit { saved_draft: "antes".into(), ..edit };
        assert_eq!(edit_left(&kept, &panel(&[]), "novo").as_deref(), Some("antes"));
        assert!(!can_update(" ") && !can_update("/clear") && can_update("ok"));
    }

    #[test]
    fn moves_and_drops() {
        let e = panel(&["a", "b", "c"]).entries;
        assert_eq!(move_target(&e, "b", true), Some(Some("a".into())));
        assert_eq!(move_target(&e, "b", false), Some(None));
        assert_eq!(move_target(&e, "a", false), Some(Some("c".into())));
        assert_eq!(move_target(&e, "a", true), None);
        assert_eq!(move_target(&e, "c", false), None);
        assert_eq!(drop_target(&e, "c", "a", false), Some(Some("a".into())));
        assert_eq!(drop_target(&e, "a", "c", true), Some(None));
        assert_eq!(drop_target(&e, "a", "b", false), None);
        assert_eq!(drop_target(&e, "a", "a", true), None);
    }
}
