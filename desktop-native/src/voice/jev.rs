//! O Jev na voz: decide em ~0,5 s, entre opções que o app monta, o que uma fala pede (trocar de sessão, ação de tela,
//! mandar para a sessão, conversa). Só certeza alta age; o resto segue pelo organizador como antes.
use serde_json::{Value, json};
use std::{path::Path, sync::Mutex, time::Duration};

const TYPESAFE_URL: &str = "https://api.typesafe.ai/v1/systemone";
const TYPESAFE_MODEL: &str = "jev-1.13.0";
const OPENROUTER_URL: &str = "https://openrouter.ai/api/alpha/decisions";
const OPENROUTER_MODEL: &str = "typesafe/jev-1.13-20260917";
const TIMEOUT: Duration = Duration::from_secs(3);
/// Certeza mínima para o app agir sozinho ou mudar a trava de envio.
pub const SURE: f64 = 0.9;
/// Troca e ação de tela fazem o que a pessoa pediu e se desfazem com outra fala; a trava `switch_asked` ainda confere que a
/// fala pediu a troca. Com 0,9 o Jev deixava passar demais ("vai pro voz" veio com destino 0,89 e 0,63).
pub const SWITCH_INTENT: f64 = 0.7;
pub const SESSION_SURE: f64 = 0.65;
pub const ACTION_SURE: f64 = 0.7;

#[derive(Clone, Debug, PartialEq)]
pub struct Config { pub key: String, pub url: String, pub model: String }

fn text(value: &Value) -> Option<String> { value.as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned) }

/// Endereço e modelo do Jev para esta chave; a mesma regra do servidor (`runtime_config.destino_jev`). OpenRouter quando
/// o endereço é dele ou, sem endereço, quando a chave é dele (`sk-or-`): modelo vazio vira o padrão de lá e
/// `typesafe/jev-latest` ganha o `~` do apelido. Na TypeSafe, vazio fica vazio (cada cliente tem o seu padrão).
pub fn destination(key: &str, endpoint: Option<&str>, model: Option<&str>) -> (Option<String>, Option<String>) {
    let (endpoint, model) = (endpoint.map(str::trim).filter(|v| !v.is_empty()), model.map(str::trim).filter(|v| !v.is_empty()));
    if endpoint.is_some_and(|e| e.contains("openrouter.ai")) || (endpoint.is_none() && key.trim().starts_with("sk-or-")) {
        let mut model = model.unwrap_or(OPENROUTER_MODEL).to_owned();
        if model.starts_with("typesafe/") && model.ends_with("-latest") { model.insert(0, '~'); }
        return (Some(endpoint.unwrap_or(OPENROUTER_URL).to_owned()), Some(model));
    }
    (endpoint.map(str::to_owned), model.map(str::to_owned))
}

/// O que a tela do Jev grava no servidor (`runtime-config.json`), senão o ambiente.
pub fn config_from(runtime: &Value, settings_env: &Value, env_key: Option<String>) -> Option<Config> {
    let key = text(&runtime["jev_api_key"]).or(env_key).or_else(|| text(&settings_env["TYPESAFE_API_KEY"]))?;
    let (url, model) = destination(&key, text(&runtime["jev_endpoint"]).as_deref(), text(&runtime["jev_model"]).as_deref());
    Some(Config { key, url: url.unwrap_or_else(|| TYPESAFE_URL.to_owned()), model: model.unwrap_or_else(|| TYPESAFE_MODEL.to_owned()) })
}

/// Bloqueia (lê arquivos). Pasta de config do servidor desta máquina: a mesma regra do backend.
pub fn load(home: &Path) -> Option<Config> {
    let read = |path: &Path| std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).unwrap_or(Value::Null);
    let base = std::env::var_os("CLAUDE_CONFIG_DIR").map_or_else(|| home.join(".claude"), Into::into);
    let runtime = read(&base.join("runtime-config.json"));
    let settings = read(&home.join(".claude").join("settings.json"));
    config_from(&runtime, &settings["env"], std::env::var("TYPESAFE_API_KEY").ok().filter(|v| !v.is_empty()))
}

/// Modelo que o endereço recusou ("does not exist"): troca pelo padrão uma vez e lembra pelo resto do app aberto.
static FALLBACK: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq)]
pub struct Pick { pub choice: String, pub p: f64 }

/// Respostas por pergunta. Erro volta como texto curto para o diário (nunca a chave).
pub async fn ask(config: &Config, state: &str, questions: &Value) -> Result<std::collections::HashMap<String, Pick>, String> {
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().map_err(|_| "cliente http".to_owned())?;
    let mut model = FALLBACK.lock().ok().and_then(|f| f.clone()).unwrap_or_else(|| config.model.clone());
    for attempt in 0..2 {
        let body = json!({"model": model, "state": state, "questions": questions});
        let response = client.post(&config.url).bearer_auth(&config.key).json(&body).send().await.map_err(|e| if e.is_timeout() { "timeout".to_owned() } else { "rede".to_owned() })?;
        let status = response.status();
        let value: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() { return Ok(picks(&value["answers"])); }
        let missing = status.as_u16() == 400 && value["error"]["message"].as_str().is_some_and(|m| m.contains("does not exist"));
        let fallback = if config.url.contains("openrouter.ai") { OPENROUTER_MODEL } else { TYPESAFE_MODEL };
        if attempt == 0 && missing && model != fallback {
            crate::voice::log("jev model refused, using default");
            model = fallback.to_owned();
            if let Ok(mut f) = FALLBACK.lock() { *f = Some(model.clone()); }
            continue;
        }
        return Err(format!("http {}", status.as_u16()));
    }
    Err("modelo recusado".into())
}

fn picks(answers: &Value) -> std::collections::HashMap<String, Pick> {
    answers.as_object().into_iter().flatten().filter_map(|(name, answer)| {
        let choice = answer["choice"].as_str()?.to_owned();
        let p = answer["probabilities"][&choice].as_f64().unwrap_or(0.0);
        Some((name.clone(), Pick { choice, p }))
    }).collect()
}

/// O que a fala pede, com as opções que o app conhece agora.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent { Switch, Screen, Send, Talk, Hold, Noise }

impl Intent {
    fn from(choice: &str) -> Option<Self> {
        Some(match choice { "switch" => Self::Switch, "screen" => Self::Screen, "send" => Self::Send, "talk" => Self::Talk,
            "hold" => Self::Hold, "none" => Self::Noise, _ => return None })
    }
}

/// Decisão sobre uma fala: a intenção com a certeza dela e, quando cabe, a sessão e a ação escolhidas (índices nas listas dadas).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Decision { pub intent: Option<(Intent, f64)>, pub session: Option<(usize, f64)>, pub action: Option<(usize, f64)> }

fn criteria(items: &[String], prefix: char) -> Value {
    let mut map: serde_json::Map<String, Value> = items.iter().enumerate().map(|(i, label)| (format!("{prefix}{}", i + 1), json!(label))).collect();
    map.insert("none".into(), json!("nenhuma ou não dá para saber"));
    Value::Object(map)
}

/// Rótulo de uma sessão nas opções: o nome em destaque, a máquina depois, e a marca da que já está na tela.
pub fn session_label(name: &str, machine: &str, on_screen: bool) -> String {
    format!("sessão «{name}» (máquina {machine}){}", if on_screen { " — é a que já está na tela" } else { "" })
}

/// Perguntas e estado de uma fala. `sessions` vem de `session_label`; `actions` são "Abrir o terminal: …".
/// `recent`: as últimas falas da conversa (o "isso" depois de "qual das duas?" só se entende com elas).
pub fn questions(utterance: &str, recent: &str, on_screen: Option<&str>, sessions: &[String], actions: &[String]) -> (String, Value) {
    let screen = on_screen.map(|name| format!("Sessão na tela agora: {name}. ")).unwrap_or_default();
    let recent: String = recent.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
    let recent = if recent.trim().is_empty() { String::new() } else { format!("Conversa recente:\n{}\n", recent.trim()) };
    let state = format!("{screen}{recent}Fala do usuário ao assistente de voz do Hangar (app que controla sessões de trabalho Claude/Codex em várias máquinas): \"{utterance}\"");
    let questions = json!({
        "intent": {"type": "choice", "instructions": "O que o usuário quer com esta fala? 'vai pra', 'volta pra', 'abre a', 'troca pra' seguidos de um nome de sessão é trocar de sessão.", "criteria": {
            "switch": "trocar a sessão aberta na tela para outra sessão",
            "screen": "abrir, fechar ou mostrar algo na tela do Hangar (configurações, terminal, painel, custos)",
            "send": "mandar um trabalho para a sessão executar",
            "talk": "conversar, perguntar, analisar ou planejar junto com o assistente",
            "hold": "pedir para esperar ou não mandar ainda",
            "none": "fala incompleta, hesitação ou ruído"}},
        "session": {"type": "choice", "instructions": "Se ele quer trocar de sessão, para qual destas? Ele costuma dizer só um pedaço do nome \
            ('voz' é voz-entendimento, 'api' é pm18920-api) e a transcrição pode separar ou errar letras ('P-Workstation' é PWorkStation, \
            'seção' é sessão). A máquina não precisa ser dita: a sessão pode estar em qualquer uma.", "criteria": criteria(sessions, 's')},
        "action": {"type": "choice", "instructions": "Se ele quer uma ação na tela do Hangar, qual destas?", "criteria": criteria(actions, 'a')},
    });
    (state, questions)
}

pub fn decision(picks: &std::collections::HashMap<String, Pick>) -> Decision {
    let index = |name: &str, prefix: char| picks.get(name).and_then(|p| p.choice.strip_prefix(prefix)?.parse::<usize>().ok().filter(|n| *n >= 1).map(|n| (n - 1, p.p)));
    Decision {
        intent: picks.get("intent").and_then(|p| Intent::from(&p.choice).map(|i| (i, p.p))),
        session: index("session", 's'),
        action: index("action", 'a'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn config_follows_the_settings_screen_and_openrouter_defaults() {
        let or = json!({"jev_api_key": "sk-or-x", "jev_endpoint": "https://openrouter.ai/api/alpha/decisions", "jev_model": "typesafe/jev-latest"});
        assert_eq!(config_from(&or, &Value::Null, None).unwrap().model, "~typesafe/jev-latest", "sem o til o OpenRouter recusa");
        assert_eq!(destination("apik-x", None, None), (None, None), "TypeSafe: cada cliente usa o seu padrão");
        assert_eq!(destination("sk-or-x", None, Some("~typesafe/jev-latest")).1.as_deref(), Some("~typesafe/jev-latest"));
        let bare = config_from(&json!({"jev_api_key": "sk-or-x"}), &Value::Null, None).unwrap();
        assert_eq!((bare.url.as_str(), bare.model.as_str()), (OPENROUTER_URL, OPENROUTER_MODEL), "chave do OpenRouter sem endereço");
        let ts = config_from(&Value::Null, &json!({"TYPESAFE_API_KEY": "apik-1"}), None).unwrap();
        assert_eq!((ts.url.as_str(), ts.model.as_str()), (TYPESAFE_URL, TYPESAFE_MODEL));
        assert!(config_from(&Value::Null, &Value::Null, None).is_none());
        let endpoint_only = config_from(&json!({"jev_api_key": "k", "jev_endpoint": "https://openrouter.ai/api/alpha/decisions"}), &Value::Null, None).unwrap();
        assert_eq!(endpoint_only.model, OPENROUTER_MODEL, "endereço do OpenRouter sem modelo não manda o nome da TypeSafe");
    }

    #[test]
    fn decision_reads_choices_and_indexes() {
        let picks: std::collections::HashMap<String, Pick> = [("intent", "switch", 0.96), ("session", "s2", 0.99), ("action", "none", 0.8)]
            .into_iter().map(|(n, c, p)| (n.to_owned(), Pick { choice: c.into(), p })).collect();
        assert_eq!(decision(&picks), Decision { intent: Some((Intent::Switch, 0.96)), session: Some((1, 0.99)), action: None });
        let (with_recent, _) = questions("isso", "assistant: qual das duas?", None, &[], &[]);
        assert!(with_recent.contains("Conversa recente:\nassistant: qual das duas?\n"));
        let (state, q) = questions("volta pra voz", "", Some("PWorkStation"), &["a".into(), "b".into()], &[]);
        assert_eq!(q["session"]["criteria"]["s2"], json!("b"));
        assert!(q["action"]["criteria"].get("none").is_some(), "sempre há a saída 'nenhuma'");
        assert!(state.starts_with("Sessão na tela agora: PWorkStation. ") && state.ends_with("\"volta pra voz\""));
        assert_eq!(session_label("PWorkStation", "delphi-02", false), "sessão «PWorkStation» (máquina delphi-02)");
        assert!(session_label("voz", "Notebook", true).ends_with("já está na tela"));
    }
}
