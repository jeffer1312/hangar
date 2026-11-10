//! Contrato da interface dos mods: o que o Claude Code anuncia, o que os apps pedem e o que a
//! superfície devolve ao ator. Valores medidos com o Claude Code 2.1.289.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// O `requestId` da faixa nos dois modos; nos painéis é o id do painel.
pub const BAND_SITE: &str = "above-prompt";
/// A `key` com que o app de antes da rota `close` fecha o painel `site` pelo `press`.
pub const CLOSE_KEY: &str = "__close__";
/// Um cliente só para todos os aparelhos ligados à sessão (G2). Sem `client_id`, o Claude Code cria
/// um cliente padrão que não sai mais pelo canal (E4).
pub const CLIENT_ID: &str = "hangar";
/// A superfície padrão do protocolo, que os mods já tratam (`Svg` no lugar de `Raster`).
pub const SURFACE: &str = "desktop";
/// Largura para a qual a faixa é pedida; vai no evento como `columns`.
pub const BAND_COLUMNS: u64 = 110;
/// Largura de painel quando o mod não pediu uma.
pub const PANE_COLUMNS: u64 = 60;
/// Teto do nome de mod que os apps e o plugin do Hangar mandam, o mesmo do Python (`PluginPressBody`).
pub const PLUGIN_MAX: usize = 256;
/// Duração do aviso de mod que não traz uma, a mesma do Python (`plugin_bridge`).
pub const TOAST_DEFAULT_MS: u64 = 4000;

/// Sem `isFullscreen`, um viewport com `columns` e `rows` segura todos os painéis (seção Viewport).
pub fn viewport() -> Value {
    json!({"columns": 120, "rows": 40, "isFullscreen": true})
}

/// Item do aviso `ui_panes`. Campos que o Hangar não usa (`close_on_escape`, `hold_toasts`) ficam de fora.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PaneItem {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub plugin: String,
    #[serde(default)]
    pub columns: Option<u64>,
    #[serde(default)]
    pub rows: Option<u64>,
}

impl PaneItem {
    /// O tamanho pedido pelo mod é sugestão (P14): o que ele recebe é o que a superfície manda.
    pub fn columns(&self) -> u64 {
        self.columns.filter(|columns| *columns >= 4).unwrap_or(PANE_COLUMNS)
    }
}

/// Pedido de um app à interface dos mods de uma sessão. Botão e campo vêm com o mod (`plugin`) que os
/// desenhou: a `key` só é única dentro de um mod.
#[derive(Clone, Debug, PartialEq)]
pub enum ModsCall {
    Press { site: String, plugin: String, key: String },
    Close { site: String },
    Show { site: String },
    Input { site: String, plugin: String, key: String, submit: bool, value: String },
}

impl ModsCall {
    pub fn site(&self) -> &str {
        match self {
            Self::Press { site, .. } | Self::Close { site } | Self::Show { site } | Self::Input { site, .. } => site,
        }
    }
}

/// Recusa com código, no formato do `erro()` do Python (`{code, params, msg}`), que o app traduz.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModsError {
    pub code: String,
    pub msg: String,
    pub params: Value,
}

impl ModsError {
    pub fn new(code: &str, msg: &str) -> Self {
        Self { code: code.into(), msg: msg.into(), params: json!({}) }
    }
    pub fn detail(&self) -> Value {
        json!({"code": self.code, "params": self.params, "msg": self.msg})
    }
}

// Os textos de `stale()` e `no_typing()` são os de `messages/pt.json` (fase 1): o app traduz pelo código,
// e a `msg` só aparece em cliente antigo.
pub fn stale() -> ModsError {
    ModsError::new("erro_mod_desenho_vencido", "O mod redesenhou a tela e esse item não está mais nela; tente de novo.")
}
pub fn missing() -> ModsError {
    ModsError::new("erro_mod_botao_inexistente", "Esse item não está mais na tela do mod.")
}
pub fn no_answer() -> ModsError {
    ModsError::new("erro_mod_clique_sem_resposta", "O mod não respondeu a tempo.")
}
/// O pane não respondeu à operação (multiplexador, pane trocado, teclado emprestado ao Python, prazo
/// vencido na caixa do executor). O código vai nos parâmetros, para o log; o app recebe o mesmo
/// `erro_mod_clique_sem_resposta`.
pub fn pane_failed(code: &str) -> ModsError {
    let mut error = no_answer();
    error.params = json!({"causa": code});
    error
}
pub fn pane_missing() -> ModsError {
    ModsError::new("erro_mod_painel_inexistente", "Esse painel não está mais aberto no mod.")
}
pub fn close_refused() -> ModsError {
    ModsError::new("erro_mod_fechar_recusado", "O mod não deixou fechar o painel.")
}
/// `plugin/input` numa sessão que o Rust não atende como superfície (Task 10): com terminal, ou sem
/// terminal fora da superfície (processo parado, antes do `initialize`, Codex, mods desligados). O
/// texto vale para as duas situações; é o de `messages/pt.json`.
pub fn no_typing() -> ModsError {
    ModsError::new("erro_mod_sem_digitacao", "Nesta sessão, o campo do mod só aceita digitação no terminal ou não está ligado ao app.")
}
/// `press`/`close` numa sessão sem a interface dos mods no Rust.
pub fn no_rust() -> ModsError {
    ModsError::new("mods_need_rust_server", "O clique do mod precisa do servidor Rust de pé e da sessão aberta nele.")
}
/// `show` numa sessão sem a interface dos mods no Rust: o app troca a aba sozinho.
pub fn tab_in_app_error() -> ModsError {
    ModsError::new("erro_mod_aba_no_app", "Nesta sessão a troca de aba do mod fica no app.")
}

// Recusas do clique com terminal (fase 3). Os textos são os de `messages/pt.json` (fase 1); o painel que
// fechou no meio do clique usa o `pane_missing` acima.
pub fn dialog_open() -> ModsError {
    ModsError::new("erro_mod_dialogo_aberto", "Há uma pergunta aberta no terminal da sessão; responda a ela antes.")
}
pub fn draft_in_prompt() -> ModsError {
    ModsError::new("erro_mod_rascunho_no_prompt", "Há texto digitado no prompt do terminal; envie ou apague antes de usar este botão pelo app.")
}
pub fn unreachable_pane() -> ModsError {
    ModsError::new("erro_mod_painel_nao_alcancavel", "O terminal da sessão está estreito ou baixo demais para alcançar esse painel; aumente a janela ou use o terminal.")
}
pub fn terminal_in_mode() -> ModsError {
    ModsError::new("erro_mod_terminal_em_modo", "O terminal da sessão está em modo de rolagem; saia dele e tente de novo.")
}
pub fn mouse_off() -> ModsError {
    ModsError::new("erro_mod_mouse_desligado", "O Claude Code da sessão não está em tela cheia, e só nela ele liga o mouse; clique pelo terminal.")
}
pub fn not_found(label: &str) -> ModsError {
    let mut error = ModsError::new("erro_mod_botao_nao_achado", &format!("Não achei “{label}” na tela do terminal."));
    error.params = json!({"rotulo": label});
    error
}
pub fn ambiguous(label: &str) -> ModsError {
    let mut error = ModsError::new("erro_mod_botao_ambiguo", &format!("“{label}” aparece mais de uma vez na tela; clique pelo terminal."));
    error.params = json!({"rotulo": label});
    error
}

/// O que a superfície pede ao ator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SurfaceEffect {
    /// Quadro para o Claude Code fora do diário da fila: `ui_*` não muda a conversa. `until`, no relógio da
    /// superfície: a ação no mod (clique, digitação, fechar) não pode chegar depois dele, quando o app já
    /// ouviu que o pedido falhou; o desenho e as leituras vão sem prazo.
    Write { frame: Value, until: Option<f64> },
    /// O `plugin_ui` inteiro, como os apps recebem.
    Publish { data: Value },
    Toast { plugin: String, text: String, timeout_ms: u64 },
    /// O mod copiou um texto (`ui_copy`), já respondido ao Claude Code.
    Copied { plugin: String, text: String },
    /// Resposta ao pedido `token` de um app.
    Reply { token: u64, result: Result<Value, ModsError> },
}
