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

/// Onde ficam as sessões: na barra lateral ou numa faixa de abas no topo, que tira a barra lateral.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Navigation { Sidebar, Tabs }

/// Automático segue a preferência do sistema; Desktop pinta com a paleta do papel de parede.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode { Auto, Light, Dark, Desktop }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Palette { Classic, Neutral }

/// No tema Desktop, o texto vem do papel de parede ou fica com as cores do app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopText { Desktop, App }

/// O que fica atrás das caixas: Imagem é um arquivo deste computador; Desktop é o que está atrás da janela.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Background { Plain, Texture, Light, Image, Desktop }

/// No fundo Desktop: a janela deixa ver a área de trabalho, ou desenha dentro dela a foto do papel de parede.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wallpaper { Window, Glass }

/// O que segura o texto da conversa sobre o fundo; Automática liga o Texto só com imagem ou desktop atrás.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading { Auto, None, Text, Sheet }

/// Como a chamada de ferramenta aparece na conversa: linha com nome e resumo, ou verbo e chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolLook { Classic, Chips }

/// Que chamadas feitas no meio do raciocínio ficam dentro do bloco do pensamento.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingTools { None, Search, All }

/// Idioma da interface: Sistema segue `HANGAR_NATIVE_LANG`/`LANG`; os outros vencem as variáveis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language { System, Pt, En }

/// Moeda dos custos. Real sem cotação lida mostra dólar: número convertido por taxa que não temos seria inventado.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency { Usd, Brl }

/// Amostra escolhida: índice numa lista fixa ou cor livre, gravada como "#rrggbb".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Swatch { Preset(usize), Custom(Hex) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hex(pub u32);

impl Serialize for Hex {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> { s.serialize_str(&format!("#{:06x}", self.0)) }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.strip_prefix('#').filter(|h| h.len() == 6).and_then(|h| u32::from_str_radix(h, 16).ok())
            .map(Hex).ok_or_else(|| serde::de::Error::custom("cor fora do formato #rrggbb"))
    }
}

/// Destaque e tinta de um modo (escuro ou claro), como no web.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeColors {
    /// `Preset(0)` é o destaque da própria paleta; os demais indexam as amostras do modo.
    pub accent: Swatch,
    /// `Preset(0)` é sem tinta; os demais indexam as tintas do modo.
    pub tint: Swatch,
    /// Força da tinta, 5–100: quanto a cor escolhida entra no fundo.
    pub tint_strength: u16,
}

const MODE_COLORS: ModeColors = ModeColors { accent: Swatch::Preset(0), tint: Swatch::Preset(0), tint_strength: 40 };

impl Default for ModeColors {
    fn default() -> Self { MODE_COLORS }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub panels: Panels,
    pub theme: ThemeMode,
    pub palette: Palette,
    pub desktop_text: DesktopText,
    /// Cores do modo escuro. Achatadas: o arquivo da versão anterior guardava `accent`/`tint` no topo.
    #[serde(flatten)]
    pub dark: ModeColors,
    pub light: ModeColors,
    /// Quanto do que está atrás da janela aparece na caixa solta, 0–100.
    pub transparency: u16,
    /// Opacidade das caixas na caixa solta, 0–100.
    pub solidity: u16,
    pub background: Background,
    pub wallpaper: Wallpaper,
    pub reading: Reading,
    /// Opacidade da folha atrás da conversa, 0–100.
    pub sheet_solidity: u16,
    /// Quanto o texto vai para o branco (escuro) ou o preto (claro) no modo Texto, 0–100.
    pub text_contrast: u16,
    pub font: Font,
    /// Tamanho, entrelinha e largura da coluna da conversa, em % do padrão (50–150).
    /// `u16` para que um valor fora da escala no arquivo seja limitado, não recusado na leitura.
    pub text_size: u16,
    pub line_height: u16,
    pub column: u16,
    pub sidebar_height: SidebarHeight,
    pub navigation: Navigation,
    /// Caixa do "Ver ao vivo": distância da borda direita e da de baixo da janela, em px lógicos.
    pub live_corner: [f32; 2],
    pub tool_look: ToolLook,
    /// Lista de tarefas do agente (TaskCreate/TaskUpdate) como um bloco de progresso na conversa.
    pub task_list: bool,
    pub thinking_tools: ThinkingTools,
    /// Botão Gráfico sobre as tabelas numéricas das respostas.
    pub table_chart: bool,
    /// Geral: também deste computador, no mesmo arquivo; o "Voltar ao padrão" da Aparência não mexe nelas.
    pub language: Language,
    pub currency: Currency,
    /// Contas e modelos em uma linha por conta, sem barras: escolha deste aparelho, como no web.
    pub accounts_compact: bool,
}

const DEFAULT: Appearance = Appearance { panels: Panels::Attached, theme: ThemeMode::Dark, palette: Palette::Classic,
    desktop_text: DesktopText::Desktop, dark: MODE_COLORS, light: MODE_COLORS, transparency: 40, solidity: 70,
    background: Background::Plain, wallpaper: Wallpaper::Window, reading: Reading::Auto, sheet_solidity: 60, text_contrast: 30,
    font: Font::System, text_size: 100, line_height: 100, column: 100, sidebar_height: SidebarHeight::Full,
    navigation: Navigation::Sidebar, live_corner: [16., 16.],
    tool_look: ToolLook::Classic, task_list: false, thinking_tools: ThinkingTools::Search, table_chart: false,
    language: Language::System, currency: Currency::Usd, accounts_compact: false };

impl Default for Appearance {
    fn default() -> Self { DEFAULT }
}

impl Appearance {
    /// "Voltar ao padrão" do web: não mexe em tema, fonte, fundo, painéis nem no jeito da conversa.
    pub fn reset_keeping_choices(self) -> Self {
        Self { panels: self.panels, font: self.font, theme: self.theme, palette: self.palette, desktop_text: self.desktop_text,
            background: self.background, wallpaper: self.wallpaper, tool_look: self.tool_look, task_list: self.task_list,
            thinking_tools: self.thinking_tools, table_chart: self.table_chart, navigation: self.navigation, live_corner: self.live_corner,
            language: self.language, currency: self.currency, accounts_compact: self.accounts_compact, ..Self::default() }
    }

    /// Imagem ou área de trabalho atrás do texto: é o que a Leitura Automática resolve.
    pub fn busy_background(&self) -> bool { matches!(self.background, Background::Image | Background::Desktop) }

    /// Leitura em vigor: a Automática vira Texto só sobre fundo ocupado.
    pub fn effective_reading(&self) -> Reading {
        match self.reading {
            Reading::Auto if self.busy_background() => Reading::Text,
            Reading::Auto => Reading::None,
            other => other,
        }
    }

    pub fn colors(&self, dark: bool) -> &ModeColors { if dark { &self.dark } else { &self.light } }
    pub fn colors_mut(&mut self, dark: bool) -> &mut ModeColors { if dark { &mut self.dark } else { &mut self.light } }

    // Arquivo editado à mão ou de outra versão não pode levar valor para fora da escala.
    fn clamped(mut self) -> Self {
        self.transparency = self.transparency.min(100);
        self.solidity = self.solidity.min(100);
        self.sheet_solidity = self.sheet_solidity.min(100);
        self.text_contrast = self.text_contrast.min(100);
        for colors in [&mut self.dark, &mut self.light] { colors.tint_strength = colors.tint_strength.clamp(5, 100); }
        for v in [&mut self.text_size, &mut self.line_height, &mut self.column] { *v = (*v).clamp(50, 150); }
        // O limite de cima depende da janela e é aplicado ao desenhar; aqui só o que nunca vale.
        for v in &mut self.live_corner { *v = if v.is_finite() { v.max(0.) } else { 16. }; }
        self
    }
}

static CURRENT: RwLock<Appearance> = RwLock::new(DEFAULT);

pub fn get() -> Appearance { *CURRENT.read().unwrap_or_else(|e| e.into_inner()) }

pub fn set(value: Appearance) { *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = value.clamped(); }

fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("hangar-native"))
}

fn path() -> Option<PathBuf> { Some(dir()?.join("appearance.json")) }

/// Cópia da imagem de fundo escolhida: o original pode sumir ou mudar depois.
pub fn image_path() -> Option<PathBuf> { Some(dir()?.join("background-image")) }

/// A raiz escolhida por último em Nova sessão, como o `cp:last-root` do web; falha de disco só faz esquecer. Bloqueantes.
pub fn last_root() -> Option<String> { std::fs::read_to_string(dir()?.join("last-root")).ok().map(|s| s.trim().to_owned()) }

pub fn remember_root(path: &str) {
    if let Some(dir) = dir() { let _ = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("last-root"), path)); }
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

/// Bloqueante: chamar fora da thread da janela. Grava o valor atual lido dentro da trava, não o do clique:
/// gravações que terminam fora de ordem deixam no arquivo o que está na tela.
pub fn save() -> std::io::Result<()> {
    static WRITING: Mutex<()> = Mutex::new(());
    let _guard = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let value = get();
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
        assert_eq!(parsed.dark.tint_strength, 40);
        assert_eq!((parsed.theme, parsed.palette), (ThemeMode::Dark, Palette::Classic));
    }

    #[test]
    fn tint_strength_stays_in_range() {
        let parsed: Appearance = serde_json::from_str(r#"{"tint_strength":0,"light":{"tint_strength":900}}"#).unwrap();
        let parsed = parsed.clamped();
        assert_eq!((parsed.dark.tint_strength, parsed.light.tint_strength), (5, 100));
    }

    #[test]
    fn previous_file_keeps_its_dark_colors_and_custom_colors_round_trip() {
        let old: Appearance = serde_json::from_str(r#"{"accent":3,"tint":1,"tint_strength":80}"#).unwrap();
        assert_eq!(old.dark, ModeColors { accent: Swatch::Preset(3), tint: Swatch::Preset(1), tint_strength: 80 });
        assert_eq!(old.light, ModeColors::default());
        let custom = Appearance { light: ModeColors { accent: Swatch::Custom(Hex(0x12ab9f)), ..ModeColors::default() }, ..Appearance::default() };
        let text = serde_json::to_string(&custom).unwrap();
        assert!(text.contains("\"#12ab9f\""));
        assert_eq!(serde_json::from_str::<Appearance>(&text).unwrap(), custom);
    }

    #[test]
    fn reset_keeps_theme_panels_and_font() {
        let custom = Appearance { panels: Panels::Floating, font: Font::Mono, theme: ThemeMode::Light, palette: Palette::Neutral,
            dark: ModeColors { accent: Swatch::Preset(3), ..ModeColors::default() }, column: 140, ..Appearance::default() };
        let reset = custom.reset_keeping_choices();
        assert_eq!((reset.panels, reset.font, reset.theme, reset.palette), (Panels::Floating, Font::Mono, ThemeMode::Light, Palette::Neutral));
        assert_eq!((reset.dark.accent, reset.column), (Swatch::Preset(0), 100));
    }

    #[test]
    fn reset_keeps_background_and_resets_reading() {
        let custom = Appearance { background: Background::Image, wallpaper: Wallpaper::Glass, reading: Reading::Sheet, text_contrast: 90, ..Appearance::default() };
        let reset = custom.reset_keeping_choices();
        assert_eq!((reset.background, reset.wallpaper, reset.reading, reset.text_contrast), (Background::Image, Wallpaper::Glass, Reading::Auto, 30));
    }

    #[test]
    fn conversation_choices_default_like_the_web_and_survive_reset() {
        let a = Appearance::default();
        assert_eq!((a.tool_look, a.task_list, a.thinking_tools, a.table_chart), (ToolLook::Classic, false, ThinkingTools::Search, false));
        let custom = Appearance { tool_look: ToolLook::Chips, task_list: true, thinking_tools: ThinkingTools::All, table_chart: true, ..a };
        let reset = custom.reset_keeping_choices();
        assert_eq!((reset.tool_look, reset.task_list, reset.thinking_tools, reset.table_chart), (ToolLook::Chips, true, ThinkingTools::All, true));
        let parsed: Appearance = serde_json::from_str(r#"{"tool_look":"chips","thinking_tools":"none"}"#).unwrap();
        assert_eq!((parsed.tool_look, parsed.thinking_tools), (ToolLook::Chips, ThinkingTools::None));
    }

    #[test]
    fn navigation_and_live_corner_persist_and_survive_reset() {
        assert_eq!((Appearance::default().navigation, Appearance::default().live_corner), (Navigation::Sidebar, [16., 16.]));
        let parsed: Appearance = serde_json::from_str(r#"{"navigation":"tabs","live_corner":[300.5,-4]}"#).unwrap();
        let parsed = parsed.clamped();
        assert_eq!((parsed.navigation, parsed.live_corner), (Navigation::Tabs, [300.5, 0.]));
        let reset = parsed.reset_keeping_choices();
        assert_eq!((reset.navigation, reset.live_corner), (Navigation::Tabs, [300.5, 0.]));
    }

    #[test]
    fn general_choices_persist_and_survive_reset() {
        assert_eq!((Appearance::default().language, Appearance::default().currency), (Language::System, Currency::Usd));
        let parsed: Appearance = serde_json::from_str(r#"{"language":"en","currency":"brl"}"#).unwrap();
        assert_eq!((parsed.language, parsed.currency), (Language::En, Currency::Brl));
        let reset = parsed.reset_keeping_choices();
        assert_eq!((reset.language, reset.currency), (Language::En, Currency::Brl));
    }

    #[test]
    fn automatic_reading_turns_text_only_over_busy_background() {
        let mut a = Appearance::default();
        assert_eq!(a.effective_reading(), Reading::None);
        for (background, expected) in [(Background::Texture, Reading::None), (Background::Image, Reading::Text), (Background::Desktop, Reading::Text)] {
            a.background = background;
            assert_eq!(a.effective_reading(), expected);
        }
        a.reading = Reading::Sheet;
        assert_eq!(a.effective_reading(), Reading::Sheet);
    }
}
