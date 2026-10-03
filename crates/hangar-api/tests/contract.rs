use std::{fs, path::{Path, PathBuf}};

use hangar_api::{
    ask::AskQuestion,
    chat::{ChatEvent, ChatKind},
    preview::PreviewEvent,
    state::StateEvent,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn samples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/api_samples")
}

fn samples(prefix: &str) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = fs::read_dir(samples_dir())
        .expect("sem amostras: rode backend/tests/fixtures/contract/gen_api_samples.py")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with(prefix))
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            (name, value)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "nenhuma amostra {prefix}*");
    out
}

// Lê, escreve de volta e compara como JSON: campo a mais ou a menos de um dos lados aparece aqui.
fn round_trip<T: Serialize + DeserializeOwned>(prefix: &str) -> Vec<(String, T)> {
    samples(prefix)
        .into_iter()
        .map(|(name, original)| {
            let typed: T = serde_json::from_value(original.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(serde_json::to_value(&typed).unwrap(), original, "{name}");
            (name, typed)
        })
        .collect()
}

#[test]
fn chat_samples_round_trip() {
    for (name, event) in round_trip::<ChatEvent>("chat_") {
        assert!(!matches!(event.kind, ChatKind::Other(_)), "{name}: tipo novo no models.py sem variante no ChatKind");
    }
}

#[test]
fn state_samples_round_trip() {
    round_trip::<StateEvent>("state_");
}

#[test]
fn preview_samples_round_trip() {
    round_trip::<PreviewEvent>("preview_");
}

#[test]
fn ask_samples_round_trip() {
    round_trip::<AskQuestion>("ask_");
}

#[test]
fn unknown_and_missing_fields_do_not_break() {
    let event: ChatEvent = serde_json::from_value(json!({"kind": "assistant_msg", "id": "a1", "campo_novo": {"x": 1}})).unwrap();
    assert_eq!(event.kind, ChatKind::AssistantMsg);
    assert!(event.text.is_none() && event.tool_input.is_none() && event.offset.is_none());

    let future: ChatEvent = serde_json::from_value(json!({"kind": "tipo_futuro", "id": "f1"})).unwrap();
    assert_eq!(future.kind, "tipo_futuro");
    assert_eq!(serde_json::to_value(&future).unwrap()["kind"], "tipo_futuro");
    assert!(serde_json::from_value::<ChatEvent>(json!({"kind": "user_msg"})).is_err(), "id continua obrigatório");

    let state: StateEvent = serde_json::from_value(json!({"state": "working", "extra": true})).unwrap();
    assert_eq!((state.session.as_str(), state.state.as_str(), state.login, state.shells.len()), ("", "working", false, 0));
    let shells: StateEvent = serde_json::from_value(json!({"shells": [{"pid": 1}]})).unwrap();
    assert_eq!((shells.shells[0].pid, shells.shells[0].cmd.as_str(), shells.shells[0].desde), (1, "", None));

    let preview: PreviewEvent = serde_json::from_value(json!({"text": "oi", "novo": 1})).unwrap();
    assert!(preview.session.is_empty() && !preview.md && !preview.vivo);

    let ask: AskQuestion = serde_json::from_value(json!({"questions": [
        {"header": "h", "question": "q", "options": [{"label": "a"}], "novo": 1}]})).unwrap();
    assert!(!ask.questions[0].multi_select);
    assert_eq!((ask.questions[0].options[0].description.as_str(), ask.questions[0].options[0].preview.as_str()), ("", ""));
}

#[test]
fn none_is_written_as_null_and_offset_never() {
    let event = ChatEvent { kind: ChatKind::UserMsg, id: "u".into(), offset: Some(10), ..Default::default() };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["text"], Value::Null);
    assert!(value.as_object().unwrap().contains_key("text"));
    assert!(!value.as_object().unwrap().contains_key("offset"));
    let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(&keys[..3], ["kind", "id", "text"], "mesma ordem do model_dump_json");
}
