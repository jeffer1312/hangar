//! Leitura em voz alta, narração e os consumidores do cliente externo de texto.
pub mod config;
pub mod http;
mod body;
pub(crate) mod history;
pub mod tts;
pub mod text_client;
pub mod narration;
pub mod thought;
pub mod translations;
pub mod suggestion;

#[derive(Default)]
pub struct Service {
    pub tts: tts::Service,
    pub thought: thought::Service,
    pub translations: translations::Service,
}

pub async fn configuration(state: &crate::routes::AppState) -> Result<config::SpeechConfig, SpeechError> {
    let home = state.accounts.env.home.clone();
    let claude = state.accounts.env.claude_base.clone();
    let backend = crate::install_root::root(&home).map(|root| root.join("backend"));
    tokio::task::spawn_blocking(move || config::SpeechConfig::load(&home, &claude, backend.as_deref()))
        .await.map_err(|_| SpeechError::new(503, "Não foi possível carregar a configuração de voz."))?
}

#[derive(Debug, Clone)]
pub struct SpeechError {
    pub status: u16,
    pub detail: String,
}

impl SpeechError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self { status, detail: detail.into() }
    }
}

impl std::fmt::Display for SpeechError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status, self.detail)
    }
}

impl std::error::Error for SpeechError {}
