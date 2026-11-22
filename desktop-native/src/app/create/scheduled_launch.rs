use std::collections::HashSet;

use serde_json::Value;

pub(super) const FIELDS: [&str; 18] = [
    "cwd",
    "provider",
    "config_dir",
    "codex_account",
    "engine",
    "engine_account",
    "engine_binding",
    "model",
    "effort",
    "service_tier",
    "permission_mode",
    "subagent_model",
    "claude_customizations",
    "jev",
    "code_mode",
    "omp_profile",
    "read_only",
    "headless",
];

/// O que trocar de provider pode redefinir. `read_only` fica de fora: o editor não tem controle dele e o corpo de
/// abertura nunca o leva, então marcá-lo apagaria a restrição da tarefa.
pub(super) fn provider_fields() -> Vec<&'static str> {
    FIELDS.into_iter().filter(|field| *field != "read_only").collect()
}

/// Trocar de provedor na tarefa nova é escolher só o provedor: a conta e o modelo que a tela preenche continuam do
/// servidor. Editando, a troca apaga as opções do provedor anterior.
pub(super) fn switch_provider(changed: &mut HashSet<&'static str>, editing: bool) {
    if editing { changed.extend(provider_fields()); return; }
    // A escolha feita no provedor anterior não vale no novo; pasta, Jev e modos valem em todos. O Fast segue na tela
    // depois da troca, e a abertura o deixa vazio onde ele não vale.
    changed.retain(|key| matches!(*key, "cwd" | "jev" | "headless" | "read_only" | "service_tier"));
    changed.insert("provider");
}

pub(super) fn merge(
    original: Option<&Value>,
    current: Value,
    changed: &HashSet<&'static str>,
) -> Value {
    let mut result = match original.and_then(Value::as_object) {
        Some(original) => {
            let mut result = original.clone();
            for key in changed {
                match current.get(*key).filter(|value| !value.is_null()) {
                    Some(value) => {
                        result.insert((*key).to_owned(), value.clone());
                    }
                    None => {
                        result.remove(*key);
                    }
                }
            }
            Value::Object(result)
        }
        // Tarefa nova: só a pasta e o que a pessoa escolheu; opção de um provedor leva o provedor junto.
        None => {
            let provider_bound = changed.iter().any(|key| !matches!(*key, "cwd" | "provider" | "jev" | "headless" | "read_only"));
            let keep = |key: &str| key == "cwd" || changed.contains(key) || (key == "provider" && provider_bound);
            Value::Object(current.as_object().into_iter().flatten().filter(|(key, _)| keep(key)).map(|(k, v)| (k.clone(), v.clone())).collect())
        }
    };
    if let Some(body) = result.as_object_mut() {
        body.retain(|key, value| FIELDS.contains(&key.as_str()) && !value.is_null());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{merge, provider_fields, switch_provider};
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn changing_only_folder_preserves_every_other_public_option() {
        let original = json!({"cwd":"/workspace/old", "provider":"omp", "config_dir":"/accounts/claude", "codex_account":"chatgpt",
            "engine":"proxy", "engine_account":"team", "model":"sonnet", "effort":"high", "service_tier":"priority", "permission_mode":"bypassPermissions",
            "subagent_model":"small", "claude_customizations":{"plugins":{"review":false},"skills":{"guide":true},"blocked_skills":["blocked"]},
            "jev":false, "omp_profile":"review-profile", "read_only":true, "headless":false});
        let current = json!({"name":"forbidden", "cwd":"/workspace/new", "provider":"omp", "model":"server-default", "effort":null});
        let actual = merge(Some(&original), current, &HashSet::from(["cwd"]));
        assert_eq!(
            actual,
            json!({"cwd":"/workspace/new", "provider":"omp", "config_dir":"/accounts/claude", "codex_account":"chatgpt",
            "engine":"proxy", "engine_account":"team", "model":"sonnet", "effort":"high", "service_tier":"priority", "permission_mode":"bypassPermissions",
            "subagent_model":"small", "claude_customizations":{"plugins":{"review":false},"skills":{"guide":true},"blocked_skills":["blocked"]},
            "jev":false, "omp_profile":"review-profile", "read_only":true, "headless":false})
        );
    }

    #[test]
    fn an_explicit_clear_removes_only_the_selected_option() {
        let original = json!({"cwd":"/workspace/app", "provider":"omp", "omp_profile":"old-profile", "jev":true, "read_only":true});
        let current = json!({"cwd":"/workspace/app", "provider":"omp", "jev":false});
        assert_eq!(
            merge(
                Some(&original),
                current,
                &HashSet::from(["omp_profile", "jev"])
            ),
            json!({"cwd":"/workspace/app", "provider":"omp", "jev":false, "read_only":true})
        );
    }

    #[test]
    fn changing_provider_keeps_read_only() {
        let original = json!({"cwd":"/workspace/app", "provider":"claude", "model":"sonnet", "read_only":true});
        let current = json!({"cwd":"/workspace/app", "provider":"pi"});
        let changed: HashSet<&'static str> = provider_fields().into_iter().collect();
        assert_eq!(
            merge(Some(&original), current, &changed),
            json!({"cwd":"/workspace/app", "provider":"pi", "read_only":true})
        );
    }

    #[test]
    fn new_templates_keep_only_public_non_null_choices() {
        let current = json!({"cwd":"/workspace/app", "provider":"claude", "model":null, "name":"internal", "remember_provider":true,
            "creator":"internal", "initial_prompt":"internal", "prewarm":"internal", "link":true, "branch":"internal", "new_branch":true, "base":"main"});
        assert_eq!(
            merge(None, current, &HashSet::from(["provider"])),
            json!({"cwd":"/workspace/app", "provider":"claude"})
        );
    }

    /// Tarefa nova guarda só o que a pessoa escolheu: o padrão que a tela mostrou é decidido pelo servidor a cada rodada.
    #[test]
    fn a_new_template_does_not_freeze_what_the_screen_showed() {
        let current = json!({"cwd":"/workspace/app", "provider":"codex", "model":"gpt", "effort":"high", "headless":true, "jev":false});
        assert_eq!(merge(None, current.clone(), &HashSet::new()), json!({"cwd":"/workspace/app"}));
        // Modelo só existe no provedor mostrado: escolher um fixa o provedor junto.
        assert_eq!(merge(None, current, &HashSet::from(["model"])), json!({"cwd":"/workspace/app", "provider":"codex", "model":"gpt"}));
    }

    /// Na tarefa nova, o que a tela preencheu no provedor novo e o que a pessoa escolheu no anterior não viram escolha.
    #[test]
    fn switching_provider_on_a_new_template_keeps_only_the_provider() {
        let current = json!({"cwd":"/workspace/app", "provider":"codex", "codex_account":"default-account", "model":"gpt", "jev":true, "service_tier":"priority"});
        let mut changed: HashSet<&'static str> = HashSet::from(["model", "effort", "jev", "service_tier"]);
        switch_provider(&mut changed, false);
        assert_eq!(merge(None, current, &changed), json!({"cwd":"/workspace/app", "provider":"codex", "jev":true, "service_tier":"priority"}));
        let mut editing = HashSet::new();
        switch_provider(&mut editing, true);
        assert_eq!(editing, provider_fields().into_iter().collect());
    }
}
