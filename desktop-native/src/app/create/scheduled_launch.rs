use std::collections::HashSet;

use serde_json::Value;

pub(super) const FIELDS: [&str; 16] = [
    "cwd",
    "provider",
    "config_dir",
    "codex_account",
    "engine",
    "engine_account",
    "model",
    "effort",
    "service_tier",
    "permission_mode",
    "subagent_model",
    "claude_customizations",
    "jev",
    "omp_profile",
    "read_only",
    "headless",
];

/// O que trocar de provider pode redefinir. `read_only` fica de fora: o editor não tem controle dele e o corpo de
/// abertura nunca o leva, então marcá-lo apagaria a restrição da tarefa.
pub(super) fn provider_fields() -> Vec<&'static str> {
    FIELDS.into_iter().filter(|field| *field != "read_only").collect()
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
        None => current,
    };
    if let Some(body) = result.as_object_mut() {
        body.retain(|key, value| FIELDS.contains(&key.as_str()) && !value.is_null());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{merge, provider_fields};
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
            merge(None, current, &HashSet::new()),
            json!({"cwd":"/workspace/app", "provider":"claude"})
        );
    }
}
