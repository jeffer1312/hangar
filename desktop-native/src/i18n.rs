use std::{collections::HashMap, sync::OnceLock};

pub fn tr(key: &str) -> String {
    static MESSAGES: OnceLock<HashMap<String, serde_json::Value>> = OnceLock::new();
    let messages = MESSAGES.get_or_init(|| {
        let locale = std::env::var("HANGAR_NATIVE_LANG").or_else(|_| std::env::var("LANG")).unwrap_or_default();
        let source = if locale.starts_with("en") { include_str!("../../messages/en.json") } else { include_str!("../../messages/pt.json") };
        serde_json::from_str(source).expect("valid bundled translations")
    });
    messages.get(&format!("native_{key}")).and_then(|v| v.as_str()).unwrap_or(key).to_owned()
}
