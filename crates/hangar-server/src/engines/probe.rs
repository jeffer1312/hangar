use serde_json::{Value, json};
use crate::{orchestration::truthy, provider_models::ModelError};


pub fn models(data: &Value) -> Result<Vec<Value>, String> {
    let data = data.as_array().ok_or("resposta inesperada do provedor (sem lista 'data')")?;
    Ok(data.iter().filter(|m| m.is_object() && truthy(&m["id"])).map(|m| {
        let vision = m.get("supports_image_in").or_else(|| m["capabilities"].get("vision")).or_else(|| m["capabilities"].get("image_in"));
        json!({"id":m["id"], "context_length": if truthy(&m["context_length"]) { &m["context_length"] } else { &m["context_window"] }, "vision":vision.and_then(Value::as_bool)})
    }).collect())
}

pub async fn fetch(base: &str, key: &str) -> Result<Vec<Value>, ModelError> {
    for (field, value) in [("base_url", base), ("api_key", key)] {
        if value.contains(['\r','\n','\0']) { return Err(ModelError::text(400, format!("{field}: sem quebra de linha nem caractere nulo"))); }
    }
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = match CLIENT.get() {
        Some(client) => client.clone(),
        None => {
            let built = crate::http_client().timeout(std::time::Duration::from_secs(15)).build()
                .map_err(|_| ModelError::text(502, "não foi possível falar com o provedor"))?;
            CLIENT.get_or_init(|| built).clone()
        }
    };
    let response = client.get(format!("{}/v1/models", super::store::normalize_base(base))).bearer_auth(key).header("User-Agent", "hangar/1.0")
        .send().await.map_err(|_| ModelError::text(502, "não foi possível falar com o provedor"))?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|_| ModelError::text(502, "não foi possível falar com o provedor"))?;
    let text = String::from_utf8_lossy(&bytes);
    let short = text.chars().take(200).collect::<String>();
    if !status.is_success() { return Err(ModelError::text(502, format!("{} {short}", status.as_u16()))); }
    let value: Value = serde_json::from_str(&text).map_err(|_| ModelError::text(502, format!("resposta não-JSON do provedor: {short}")))?;
    if !value.is_object() { return Err(ModelError::text(502, format!("resposta em formato inesperado do provedor: {short}"))); }
    models(&value["data"]).map_err(|e| ModelError::text(502, e))
}
