//! Aparência escolhida neste computador: vale só aqui, fica num arquivo ao lado da conexão.
//! O tema lê daqui a cada desenho; quem muda chama `set` e grava fora da thread da janela.
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::{Mutex, RwLock}};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panels { Attached, Floating }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Font { System, Mono }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarHeight { Full, Content }

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub panels: Panels,
    /// Índice em `theme::ACCENTS`.
    pub accent: usize,
    /// Índice em `theme::TINTS`; 0 é sem tinta.
    pub tint: usize,
    /// Força da tinta, 5–100: quanto a cor escolhida entra no fundo.
    pub tint_strength: u16,
    /// Quanto do que está atrás da janela aparece na caixa solta, 0–100.
    pub transparency: u16,
    /// Opacidade das caixas na caixa solta, 0–100.
    pub solidity: u16,
    pub font: Font,
    /// Tamanho, entrelinha e largura da coluna da conversa, em % do padrão (50–150).
    /// `u16` para que um valor fora da escala no arquivo seja limitado, não recusado na leitura.
    pub text_size: u16,
    pub line_height: u16,
    pub column: u16,
    pub sidebar_height: SidebarHeight,
}

const DEFAULT: Appearance = Appearance { panels: Panels::Attached, accent: 0, tint: 0, tint_strength: 40, transparency: 40, solidity: 70,
    font: Font::System, text_size: 100, line_height: 100, column: 100, sidebar_height: SidebarHeight::Full };

impl Default for Appearance {
    fn default() -> Self { DEFAULT }
}

impl Appearance {
    /// "Voltar ao padrão" do web: não mexe em tema, fonte, fundo nem painéis.
    pub fn reset_keeping_choices(self) -> Self {
        Self { panels: self.panels, font: self.font, ..Self::default() }
    }

    // Arquivo editado à mão ou de outra versão não pode levar valor para fora da escala.
    fn clamped(mut self) -> Self {
        self.transparency = self.transparency.min(100);
        self.solidity = self.solidity.min(100);
        self.tint_strength = self.tint_strength.clamp(5, 100);
        for v in [&mut self.text_size, &mut self.line_height, &mut self.column] { *v = (*v).clamp(50, 150); }
        self
    }
}

static CURRENT: RwLock<Appearance> = RwLock::new(DEFAULT);

pub fn get() -> Appearance { *CURRENT.read().unwrap_or_else(|e| e.into_inner()) }

pub fn set(value: Appearance) { *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = value.clamped(); }

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("hangar-native").join("appearance.json"))
}

/// Sem arquivo, ou arquivo ilegível, abre no padrão; o motivo da falha de leitura volta para ser mostrado.
pub fn load() -> Result<Appearance, String> {
    let Some(path) = path() else { return Ok(Appearance::default()) };
    match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Appearance::default()),
        Err(e) => Err(e.to_string()),
        Ok(bytes) => serde_json::from_slice::<Appearance>(&bytes).map(Appearance::clamped).map_err(|e| e.to_string()),
    }
}

/// Bloqueante: chamar fora da thread da janela. Gravações seguidas não se atropelam no arquivo temporário.
pub fn save(value: Appearance) -> std::io::Result<()> {
    static WRITING: Mutex<()> = Mutex::new(());
    let _guard = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = path().ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?;
    let dir = path.parent().ok_or_else(|| std::io::Error::other("caminho sem pasta"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults_and_values_stay_in_range() {
        let parsed: Appearance = serde_json::from_str(r#"{"panels":"floating","text_size":400}"#).unwrap();
        let parsed = parsed.clamped();
        assert_eq!(parsed.panels, Panels::Floating);
        assert_eq!(parsed.text_size, 150);
        assert_eq!(parsed.solidity, 70);
        assert_eq!(parsed.tint_strength, 40);
    }

    #[test]
    fn tint_strength_stays_in_range() {
        let low: Appearance = serde_json::from_str(r#"{"tint_strength":0}"#).unwrap();
        let high: Appearance = serde_json::from_str(r#"{"tint_strength":900}"#).unwrap();
        assert_eq!((low.clamped().tint_strength, high.clamped().tint_strength), (5, 100));
    }

    #[test]
    fn reset_keeps_panels_and_font() {
        let custom = Appearance { panels: Panels::Floating, font: Font::Mono, accent: 3, column: 140, ..Appearance::default() };
        let reset = custom.reset_keeping_choices();
        assert_eq!((reset.panels, reset.font, reset.accent, reset.column), (Panels::Floating, Font::Mono, 0, 100));
    }
}
