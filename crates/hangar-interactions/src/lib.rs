//! Identidade efêmera das interações; as pendências continuam no motor ou plugin dono.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use http::StatusCode;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteractionBinding {
    pub key: String,
    pub conversation: String,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InteractionKind {
    Question,
    Approval,
    Menu,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InteractionTarget {
    pub kind: InteractionKind,
    /// Identidade nativa completa, com thread e tipo do ID; em menus, a vida da abertura.
    pub request_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionError {
    Missing,
    Changed,
    Unavailable,
}

impl InteractionError {
    pub fn from_code(code: &str) -> Option<Self> {
        [Self::Missing, Self::Changed, Self::Unavailable].into_iter().find(|error| error.code() == code)
    }

    pub fn status(self) -> StatusCode {
        match self {
            Self::Missing | Self::Changed => StatusCode::CONFLICT,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Missing => "erro_interacao_sem_identidade",
            Self::Changed => "erro_interacao_desatualizada",
            Self::Unavailable => "erro_interacao_indisponivel",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Missing => "Atualize o app ou recarregue a página antes de responder.",
            Self::Changed => "Essa pergunta mudou ou já foi encerrada. Confira a pergunta atual.",
            Self::Unavailable => "Não foi possível conferir a pergunta. Nenhuma resposta foi enviada.",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedInteraction {
    pub binding: InteractionBinding,
    pub target: InteractionTarget,
    pub interaction_id: String,
    pub semantic: Value,
    pub source_revision: Option<String>,
}

pub fn error_code(error: &InteractionError) -> &'static str { error.code() }

/// O tipo do ID faz parte da identidade: zero e texto "0" são pedidos distintos.
pub fn native_target(kind: InteractionKind, conversation: &str, request_id: &Value) -> InteractionTarget {
    InteractionTarget { kind, request_key: serde_json::json!([conversation, request_id]).to_string() }
}

/// O prefixo nativo pertence ao ID: `ask:x` e `perm:x` não são a mesma pergunta.
pub fn plugin_target(kind: InteractionKind, id: &str) -> InteractionTarget {
    InteractionTarget { kind, request_key: format!("plugin:{id}") }
}

pub fn plugin_semantic(pending: &Value) -> Value {
    serde_json::json!({"questions":pending["questions"], "tool":pending["tool"], "resumo":pending["resumo"]})
}

impl ExpectedInteraction {
    pub fn native_id(&self, supplied: Option<&Value>) -> Result<Value, InteractionError> {
        let key: Value = serde_json::from_str(&self.target.request_key).map_err(|_| InteractionError::Changed)?;
        let parts = key.as_array().filter(|parts| parts.len() == 2 && parts[0].as_str().is_some_and(|thread| !thread.is_empty())).ok_or(InteractionError::Changed)?;
        let id = &parts[1];
        if !(id.is_string() || id.is_i64() || id.is_u64()) || supplied.filter(|value| !value.is_null()).is_some_and(|value| value != id) {
            return Err(InteractionError::Changed);
        }
        Ok(id.clone())
    }

    pub fn check(&self, target: &InteractionTarget, semantic: &Value, revision: Option<&str>) -> Result<(), InteractionError> {
        if self.target != *target || self.semantic != *semantic || self.source_revision.as_deref() != revision {
            return Err(InteractionError::Changed);
        }
        Ok(())
    }
}

struct Entry {
    binding: InteractionBinding,
    target: InteractionTarget,
    semantic: Value,
    id: String,
    source_revision: Option<String>,
}

#[derive(Default)]
struct StoreState {
    bindings: HashMap<String, InteractionBinding>,
    entries: HashMap<(String, InteractionTarget), Entry>,
}

#[derive(Default)]
pub struct InteractionStore {
    state: Mutex<StoreState>,
}

impl InteractionStore {
    /// Só o dono da vida ativa o vínculo; um poll antigo nunca toma a vida nova.
    pub fn bind(&self, binding: &InteractionBinding) -> Result<(), InteractionError> {
        if binding.key.is_empty() || binding.conversation.is_empty() { return Err(InteractionError::Unavailable); }
        let mut state = self.state.lock().map_err(|_| InteractionError::Unavailable)?;
        if state.bindings.get(&binding.key) != Some(binding) {
            state.entries.retain(|(key, _), _| key != &binding.key);
            state.bindings.insert(binding.key.clone(), binding.clone());
        }
        Ok(())
    }

    /// Só retratos completos: `None` encerra este alvo, não representa falha de leitura.
    pub fn observe(
        &self,
        binding: &InteractionBinding,
        target: &InteractionTarget,
        semantic: Option<Value>,
    ) -> Result<Option<String>, InteractionError> {
        self.observe_revision(binding, target, semantic, None)
    }

    pub fn observe_revision(
        &self,
        binding: &InteractionBinding,
        target: &InteractionTarget,
        semantic: Option<Value>,
        source_revision: Option<String>,
    ) -> Result<Option<String>, InteractionError> {
        let mut state = self.state.lock().map_err(|_| InteractionError::Unavailable)?;
        match state.bindings.get(&binding.key) {
            Some(current) if current == binding => {},
            Some(_) => return Err(InteractionError::Changed),
            None => return Err(InteractionError::Unavailable),
        }
        let entries = &mut state.entries;
        let key = (binding.key.clone(), target.clone());
        let Some(semantic) = semantic else {
            entries.remove(&key);
            return Ok(None);
        };
        if let Some(entry) = entries.get(&key).filter(|entry| entry.semantic == semantic && entry.source_revision == source_revision) {
            return Ok(Some(entry.id.clone()));
        }
        let mut bytes = [0u8; 16];
        SystemRandom::new().fill(&mut bytes).map_err(|_| InteractionError::Unavailable)?;
        let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        entries.insert(key, Entry {
            binding: binding.clone(), target: target.clone(), semantic, id: id.clone(), source_revision,
        });
        Ok(Some(id))
    }

    /// A identidade vigente de um alvo, sem observar nem trocar nada.
    pub fn current(&self, binding: &InteractionBinding, target: &InteractionTarget) -> Result<Option<ExpectedInteraction>, InteractionError> {
        let state = self.state.lock().map_err(|_| InteractionError::Unavailable)?;
        match state.bindings.get(&binding.key) {
            Some(current) if current == binding => {},
            Some(_) => return Err(InteractionError::Changed),
            None => return Err(InteractionError::Unavailable),
        }
        Ok(state.entries.get(&(binding.key.clone(), target.clone())).map(|entry| ExpectedInteraction {
            binding: entry.binding.clone(), target: entry.target.clone(), interaction_id: entry.id.clone(),
            semantic: entry.semantic.clone(), source_revision: entry.source_revision.clone(),
        }))
    }

    pub fn validate(
        &self,
        binding: &InteractionBinding,
        supplied: Option<&str>,
    ) -> Result<InteractionTarget, InteractionError> {
        self.expected(binding, supplied).map(|expected| expected.target)
    }

    pub fn expected(
        &self,
        binding: &InteractionBinding,
        supplied: Option<&str>,
    ) -> Result<ExpectedInteraction, InteractionError> {
        let supplied = supplied.filter(|id| !id.is_empty()).ok_or(InteractionError::Missing)?;
        let state = self.state.lock().map_err(|_| InteractionError::Unavailable)?;
        if state.bindings.get(&binding.key) != Some(binding) { return Err(InteractionError::Changed); }
        state.entries.values()
            .find(|entry| entry.binding == *binding && entry.id == supplied)
            .map(|entry| ExpectedInteraction {
                binding: entry.binding.clone(), target: entry.target.clone(), interaction_id: entry.id.clone(),
                semantic: entry.semantic.clone(), source_revision: entry.source_revision.clone(),
            })
            .ok_or(InteractionError::Changed)
    }

    /// Depois do Esc de A, sua retirada é esperada; outra interação impede digitar no composer.
    pub fn check_after_close(&self, expected: &ExpectedInteraction) -> Result<(), InteractionError> {
        let state = self.state.lock().map_err(|_| InteractionError::Unavailable)?;
        if state.bindings.get(&expected.binding.key) != Some(&expected.binding) { return Err(InteractionError::Changed); }
        if state.entries.values().any(|entry| entry.binding.key == expected.binding.key
            && (entry.binding != expected.binding || entry.id != expected.interaction_id || entry.target != expected.target
                || entry.semantic != expected.semantic || entry.source_revision != expected.source_revision)) {
            return Err(InteractionError::Changed);
        }
        Ok(())
    }

    pub fn forget_binding(&self, binding: &InteractionBinding) -> Result<(), InteractionError> {
        let mut state = self.state.lock().map_err(|_| InteractionError::Unavailable)?;
        state.entries.retain(|_, entry| entry.binding != *binding);
        if state.bindings.get(&binding.key) == Some(binding) { state.bindings.remove(&binding.key); }
        Ok(())
    }

    pub fn forget(&self, key: &str) -> Result<(), InteractionError> {
        let mut state = self.state.lock().map_err(|_| InteractionError::Unavailable)?;
        state.entries.retain(|(session_key, _), _| session_key != key);
        state.bindings.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding() -> InteractionBinding {
        InteractionBinding { key: "key-a".into(), conversation: "sid".into(), generation: 1 }
    }

    fn active_store() -> InteractionStore {
        let store = InteractionStore::default();
        store.bind(&binding()).unwrap();
        store
    }

    fn target(kind: InteractionKind, request_key: &str) -> InteractionTarget {
        InteractionTarget { kind, request_key: request_key.into() }
    }

    #[test]
    fn observation_without_an_authoritative_binding_does_not_activate_one() {
        let store = InteractionStore::default();
        let form = target(InteractionKind::Question, "question");
        for semantic in [None, Some(json!({}))] {
            assert_eq!(store.observe(&binding(), &form, semantic), Err(InteractionError::Unavailable));
        }
        store.bind(&binding()).unwrap();
        assert!(store.observe(&binding(), &form, Some(json!({}))).unwrap().is_some());
    }

    #[test]
    fn conversation_cleanup_of_a_keeps_tokens_from_b() {
        let store = active_store();
        let a = binding();
        let b = InteractionBinding { generation: 2, ..a.clone() };
        let form = target(InteractionKind::Question, "question");
        store.observe(&a, &form, Some(json!({}))).unwrap();
        store.bind(&b).unwrap();
        let id = store.observe(&b, &form, Some(json!({}))).unwrap().unwrap();
        store.forget_binding(&a).unwrap();
        assert_eq!(store.observe(&a, &form, None), Err(InteractionError::Changed));
        assert_eq!(store.observe(&a, &form, Some(json!({}))), Err(InteractionError::Changed));
        store.bind(&b).unwrap();
        assert_eq!(store.validate(&b, Some(&id)), Ok(form));
    }

    #[test]
    fn chat_may_continue_after_a_closes_but_not_over_b() {
        let store = active_store();
        let binding = binding();
        let form = target(InteractionKind::Question, "a");
        let id = store.observe(&binding, &form, Some(json!({}))).unwrap().unwrap();
        let expected = store.expected(&binding, Some(&id)).unwrap();
        store.observe(&binding, &form, None).unwrap();
        assert_eq!(store.check_after_close(&expected), Ok(()));
        store.observe(&binding, &target(InteractionKind::Approval, "b"), Some(json!({}))).unwrap();
        assert_eq!(store.check_after_close(&expected), Err(InteractionError::Changed));
    }

    #[test]
    fn conversation_expected_keeps_revision_and_exact_native_zero() {
        let store = active_store();
        let binding = binding();
        let target = native_target(InteractionKind::Question, &binding.conversation, &json!(0));
        let id = store.observe_revision(&binding, &target, Some(json!({"questions":[]})), Some("a".into())).unwrap().unwrap();
        let expected = store.expected(&binding, Some(&id)).unwrap();
        assert_eq!(expected.native_id(None), Ok(json!(0)));
        assert_eq!(expected.native_id(Some(&json!("0"))), Err(InteractionError::Changed));
        store.observe_revision(&binding, &target, Some(expected.semantic.clone()), Some("b".into())).unwrap();
        assert_eq!(expected.source_revision.as_deref(), Some("a"));
        assert_eq!(store.expected(&binding, Some(&id)), Err(InteractionError::Changed));
    }

    #[test]
    fn conversation_identity_keeps_question_and_approval_independent() {
        let store = active_store();
        let a = binding();
        let approval = target(InteractionKind::Approval, "approval-1");
        let form = target(InteractionKind::Question, "question-1");
        let prompt = json!({"question":"Permitir?","options":["Sim","Não"]});
        let first = store.observe(&a, &approval, Some(prompt.clone())).unwrap().unwrap();
        let form_id = store.observe(&a, &form, Some(json!({"question":"Qual pasta?"}))).unwrap().unwrap();
        assert_eq!(store.observe(&a, &approval, Some(prompt.clone())).unwrap(), Some(first.clone()));
        assert_eq!(store.validate(&a, Some(&form_id)), Ok(form.clone()));
        assert_eq!(store.validate(&a, Some(&first)), Ok(approval.clone()));
        assert_eq!(store.validate(&a, None), Err(InteractionError::Missing));
        assert_eq!(store.validate(&a, Some("")), Err(InteractionError::Missing));
        assert_eq!(store.validate(&a, Some("unknown")), Err(InteractionError::Changed));
        assert_eq!(store.observe(&a, &approval, None), Ok(None));
        assert_eq!(store.validate(&a, Some(&first)), Err(InteractionError::Changed));
        assert_eq!(store.validate(&a, Some(&form_id)), Ok(form.clone()));
        let second = store.observe(&a, &approval, Some(prompt.clone())).unwrap().unwrap();
        assert_ne!(first, second);
        assert_eq!(store.validate(&a, Some(&first)), Err(InteractionError::Changed));
        let b = InteractionBinding { generation: 2, ..a.clone() };
        store.bind(&b).unwrap();
        store.observe(&b, &approval, Some(prompt)).unwrap();
        assert_eq!(store.validate(&a, Some(&second)), Err(InteractionError::Changed));
        assert_eq!(store.validate(&a, Some(&form_id)), Err(InteractionError::Changed));
        assert_eq!(store.validate(&b, Some(&form_id)), Err(InteractionError::Changed));
    }

    #[test]
    fn conversation_identity_rotates_for_conversation_even_with_identical_content() {
        let store = active_store();
        let a = binding();
        let b = InteractionBinding { conversation: "sid-b".into(), ..a.clone() };
        let approval = target(InteractionKind::Approval, "approval-1");
        let form = target(InteractionKind::Question, "question-1");
        let prompt = json!({"question":"Permitir?"});
        let old = store.observe(&a, &approval, Some(prompt.clone())).unwrap().unwrap();
        let old_form = store.observe(&a, &form, Some(prompt.clone())).unwrap().unwrap();
        store.bind(&b).unwrap();
        let current = store.observe(&b, &approval, Some(prompt.clone())).unwrap().unwrap();
        assert_ne!(old, current);
        for id in [&old, &old_form] {
            assert_eq!(store.validate(&a, Some(id)), Err(InteractionError::Changed));
            assert_eq!(store.validate(&b, Some(id)), Err(InteractionError::Changed));
        }
        assert_eq!(store.validate(&b, Some(&current)), Ok(approval.clone()));
        store.bind(&a).unwrap();
        let returned = store.observe(&a, &approval, Some(prompt)).unwrap().unwrap();
        assert_ne!(old, returned);
        assert_eq!(store.validate(&b, Some(&current)), Err(InteractionError::Changed));
    }

    #[test]
    fn conversation_identity_closing_in_new_life_invalidates_all_previous_targets() {
        let store = active_store();
        let old = binding();
        let current = InteractionBinding { generation: 2, ..old.clone() };
        let form = target(InteractionKind::Question, "question-1");
        let approval = target(InteractionKind::Approval, "approval-1");
        let id = store.observe(&old, &form, Some(json!({}))).unwrap().unwrap();
        store.bind(&current).unwrap();
        assert_eq!(store.observe(&current, &approval, None), Ok(None));
        assert_eq!(store.validate(&old, Some(&id)), Err(InteractionError::Changed));
    }

    #[test]
    fn conversation_identity_uses_session_key_not_reused_display_name() {
        let store = active_store();
        let a = binding();
        let b = InteractionBinding { key: "key-b".into(), ..a.clone() };
        let form = target(InteractionKind::Question, "question-1");
        let prompt = json!({"question":"Qual pasta?"});
        let first = store.observe(&a, &form, Some(prompt.clone())).unwrap().unwrap();
        assert_eq!(store.validate(&b, Some(&first)), Err(InteractionError::Changed));
        store.bind(&b).unwrap();
        let second = store.observe(&b, &form, Some(prompt)).unwrap().unwrap();
        assert_ne!(first, second);
        assert_eq!(store.validate(&a, Some(&second)), Err(InteractionError::Changed));
        assert_eq!(store.validate(&a, Some(&first)), Ok(form.clone()));
        store.forget(&a.key).unwrap();
        assert_eq!(store.validate(&a, Some(&first)), Err(InteractionError::Changed));
        assert_eq!(store.validate(&b, Some(&second)), Ok(form));
    }

    #[test]
    fn conversation_identity_semantic_change_only_rotates_the_affected_target() {
        let store = active_store();
        let a = binding();
        let form = target(InteractionKind::Question, "question-1");
        let approval = target(InteractionKind::Approval, "approval-1");
        let first = store.observe(&a, &form, Some(json!({"question":"Qual pasta?","options":["A","B"]}))).unwrap().unwrap();
        let other = store.observe(&a, &approval, Some(json!({"question":"Permitir?"}))).unwrap().unwrap();
        let second = store.observe(&a, &form, Some(json!({"question":"Qual pasta?","options":["B","A"]}))).unwrap().unwrap();
        assert_ne!(first, second);
        assert_eq!(store.validate(&a, Some(&first)), Err(InteractionError::Changed));
        assert_eq!(store.validate(&a, Some(&second)), Ok(form));
        assert_eq!(store.validate(&a, Some(&other)), Ok(approval));
    }

    #[test]
    fn conversation_identity_preserves_native_type_thread_kind_and_menu_opening() {
        let store = active_store();
        let a = binding();
        let targets = [
            target(InteractionKind::Question, r#"["thread-a",1]"#),
            target(InteractionKind::Question, r#"["thread-a","1"]"#),
            target(InteractionKind::Question, r#"["thread-b",1]"#),
            target(InteractionKind::Approval, r#"["thread-a",1]"#),
            target(InteractionKind::Menu, "opening-1"),
            target(InteractionKind::Menu, "opening-2"),
        ];
        let prompt = json!({"question":"Continuar?","options":["Sim","Não"]});
        let ids: Vec<_> = targets.iter().map(|target| {
            store.observe(&a, target, Some(prompt.clone())).unwrap().unwrap()
        }).collect();
        for (target, id) in targets.iter().zip(&ids) {
            assert_eq!(id.len(), 32);
            assert!(id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            assert_eq!(store.validate(&a, Some(id)), Ok(target.clone()));
        }
        assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), targets.len());
        store.forget(&a.key).unwrap();
        for id in &ids {
            assert_eq!(store.validate(&a, Some(id)), Err(InteractionError::Changed));
        }
    }

    #[test]
    fn conversation_identity_new_store_never_accepts_previous_tokens() {
        let a = binding();
        let form = target(InteractionKind::Question, "question-1");
        let prompt = json!({"question":"Qual pasta?"});
        let first = active_store().observe(&a, &form, Some(prompt.clone())).unwrap().unwrap();
        let restarted = active_store();
        let second = restarted.observe(&a, &form, Some(prompt)).unwrap().unwrap();
        assert_ne!(first, second);
        assert_eq!(restarted.validate(&a, Some(&first)), Err(InteractionError::Changed));
        assert_eq!(restarted.validate(&a, Some(&second)), Ok(form));
    }

    #[test]
    fn conversation_identity_unavailable_lock_does_not_mean_closed() {
        let store = active_store();
        let a = binding();
        let form = target(InteractionKind::Question, "question-1");
        let prompt = json!({"question":"Qual pasta?"});
        let id = store.observe(&a, &form, Some(prompt.clone())).unwrap().unwrap();
        let result = std::panic::catch_unwind(|| {
            let _guard = store.state.lock().unwrap();
            panic!("interromper escrita de teste");
        });
        assert!(result.is_err());
        assert_eq!(store.observe(&a, &form, Some(prompt)), Err(InteractionError::Unavailable));
        assert_eq!(store.observe(&a, &form, None), Err(InteractionError::Unavailable));
        assert_eq!(store.validate(&a, Some(&id)), Err(InteractionError::Unavailable));
        assert_eq!(store.forget(&a.key), Err(InteractionError::Unavailable));
        store.state.clear_poison();
        assert_eq!(store.validate(&a, Some(&id)), Ok(form));
    }

    #[test]
    fn conversation_identity_dtos_read_old_payloads_and_keep_independent_ids() {
        use hangar_api::{ask::AskQuestion, session::SessionRow, state::StateEvent};

        fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(legacy: Value) {
            let old: T = serde_json::from_value(legacy.clone()).unwrap();
            let old_json = serde_json::to_value(old).unwrap();
            assert!(old_json.get("interaction_id").is_none());
            let mut current = legacy;
            current["interaction_id"] = json!("selector-id");
            let dto: T = serde_json::from_value(current).unwrap();
            let mut encoded = serde_json::to_value(dto).unwrap();
            assert_eq!(encoded.as_object_mut().unwrap().remove("interaction_id"), Some(json!("selector-id")));
            assert_eq!(encoded, old_json);
        }

        round_trip::<StateEvent>(json!({"state":"awaiting_input","question":"Permitir?","options":["Sim","Não"]}));
        round_trip::<AskQuestion>(json!({"questions":[{"header":"Pasta","question":"Qual pasta?","options":[]}]}));
        round_trip::<SessionRow>(json!({"name":"same-name","question":"Permitir?","options":["Sim","Não"]}));
        let state: StateEvent = serde_json::from_value(json!({
            "interaction_id":"approval-id",
            "codex_question":{"interaction_id":"form-id","questions":[]}
        })).unwrap();
        assert_eq!(state.interaction_id.as_deref(), Some("approval-id"));
        let form: AskQuestion = serde_json::from_value(json!(state.codex_question.unwrap())).unwrap();
        assert_eq!(form.interaction_id.as_deref(), Some("form-id"));
    }
}
