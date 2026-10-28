//! Mensagens entre o aparelho dono da chamada e o controlador da voz no servidor.
use super::organizer::ModeModels;
use serde_json::Value;

/// Sessão na tela do aparelho. `server`: id do peer, ou vazio para este servidor.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Screen { pub server: String, pub name: String }

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// `actions`: catálogo de ações de tela do nativo (`{"id","label","description"}`), que o Jev oferece.
    Hello { client: String, screen: Option<Screen>, caps: Vec<String>, #[serde(default)] actions: Vec<Value> },
    Offer { sdp: String },
    Live,
    Level { #[serde(deserialize_with = "crate::json_envelope::float32")] input: f32 },
    Screen { screen: Option<Screen> },
    ToolResult { call: u64, ok: bool, text: String },
    Mode { mode: String },
    Stop,
    Ping,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Answer { sdp: String },
    Tool { call: u64, name: String, args: Value },
    State { state: Value },
    Taken,
    Error { code: String, detail: Option<String> },
    Pong,
    Closed,
}

/// O que chega ao controlador: o aparelho dono e os avisos do hub.
#[derive(Debug)]
pub enum ToController {
    Device(ClientMsg),
    /// O aparelho dono caiu.
    Detached,
    /// Outro aparelho assumiu: as ferramentas de tela pedidas ao anterior não terão resposta.
    OwnerChanged,
    Models(ModeModels),
}

#[cfg(test)]
mod tests {
    use super::ClientMsg;

    #[test]
    fn level_with_a_fraction_is_read_through_the_tag_buffer() {
        let msg = serde_json::from_str::<ClientMsg>(r#"{"type":"level","input":0.4}"#).unwrap();
        assert!(matches!(msg, ClientMsg::Level { input } if (input - 0.4).abs() < 1e-6));
    }
}
