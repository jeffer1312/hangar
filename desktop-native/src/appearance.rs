//! Aparência escolhida neste computador: vale só aqui, fica num arquivo ao lado da conexão.
//! O tema lê daqui a cada desenho; quem muda chama `set` e grava fora da thread da janela.
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf, sync::{Mutex, RwLock}};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panels { Attached, Floating }

/// `System` e `Mono` são as duas escolhas antigas, que o arquivo continua guardando assim; `Named` é uma fonte instalada.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Font { System, Mono, Named(FontName) }

impl Font {
    pub fn family(self) -> &'static str {
        match self { Self::System => crate::theme::SANS, Self::Mono => crate::theme::MONO, Self::Named(name) => name.0 }
    }

    /// A fonte de uma das escolhas antigas volta a ser ela: o "Estilo compacto" compara por igualdade.
    pub fn from_family(family: &str) -> Self {
        [Self::System, Self::Mono].into_iter().find(|f| f.family() == family).unwrap_or(Self::Named(FontName::intern(family)))
    }
}

/// `System` é a monoespaçada que o kit ou o sistema resolvem; `Named` é uma fonte instalada.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeFont { JetBrainsMono, System, Named(FontName) }

impl CodeFont {
    pub fn from_family(family: &str) -> Self {
        if family == crate::theme::CODE_MONO { Self::JetBrainsMono } else { Self::Named(FontName::intern(family)) }
    }
}

/// Nome de família de fonte. `&'static` para a Aparência continuar `Copy`: o tema a lê a cada desenho.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontName(pub &'static str);

impl FontName {
    // ponytail: cada nome distinto vaza uma vez; o teto é a lista de fontes instaladas.
    pub fn intern(name: &str) -> Self {
        static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
        let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = names.iter().find(|known| **known == name) { return Self(known); }
        let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
        names.push(leaked);
        Self(leaked)
    }
}

impl Serialize for FontName {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> { s.serialize_str(self.0) }
}

impl<'de> Deserialize<'de> for FontName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> { Ok(Self::intern(&String::deserialize(d)?)) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarHeight { Full, Content }

/// Onde ficam as sessões e como as linhas da barra lateral são apresentadas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Navigation { Tabs, BottomTabs, Conversations, #[serde(other)] Sidebar }

impl Navigation {
    pub fn sidebar_width(self) -> f32 {
        match self { Self::Sidebar => 284., Self::Tabs | Self::BottomTabs => 0., Self::Conversations => 256. }
    }

    /// As sessões numa faixa de abas, em cima ou embaixo da janela, sem barra lateral.
    pub fn tabs(self) -> bool { matches!(self, Self::Tabs | Self::BottomTabs) }
}

/// Limites do arrasto da borda da barra, os mesmos do web.
pub const SIDEBAR_MIN: f32 = 200.;
pub const SIDEBAR_MAX: f32 = 520.;

/// Como a barra lateral agrupa as sessões (`cp_group_by` do web; "Servidor" não se aplica a um servidor só). `Machine` é o
/// do app do celular: quem trabalha sobe para "Trabalhando" no topo e o resto fica no bloco da máquina dele.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarGroup { None, Project, Machine }

/// Aba do painel da direita, lembrada entre aberturas (`ctxPanel.aba` do web).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideTab { Files, Activity, Git, Browser, #[serde(other)] Context }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundScope { Chat, Everywhere }

/// No fundo Desktop: a janela deixa ver a área de trabalho, ou desenha dentro dela a foto do papel de parede.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wallpaper { Window, Glass }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceMaterial { Glass, Opaque }

/// O que segura o texto da conversa sobre o fundo; Automática liga o Texto só com imagem ou desktop atrás.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading { Auto, None, Text, Sheet }

/// Como a chamada de ferramenta aparece na conversa: linha com nome e resumo, verbo e chip, árvore com o
/// raciocínio dentro do grupo, ou o desenho do terminal do Claude Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolLook { Classic, Chips, Tree, Terminal }

/// Que chamadas feitas no meio do raciocínio ficam dentro do bloco do pensamento.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingTools { None, Search, All }

/// Cor da moldura do card em que o agente pergunta: a de destaque escolhida ou o âmbar de aviso.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskHighlight { Accent, Amber }

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
    pub surface_material: SurfaceMaterial,
    /// Opacidade das caixas na caixa solta, 0–100.
    pub solidity: u16,
    pub background: Background,
    pub background_effect: crate::effects::BackgroundEffect,
    pub background_scope: BackgroundScope,
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
    /// As abas reúnem as máquinas acompanhadas; ocultá-las não desliga as listas nem os avisos.
    pub show_remote_tabs: bool,
    /// Largura arrastada da barra cheia (`cp_sidebar_w` do web), uma só para Barra lateral e Conversas; `None` usa a do modo.
    pub sidebar_width: Option<f32>,
    pub sidebar_compact: bool,
    /// Abas com pasta, branch e a marca do terminal sob o nome, numa faixa mais alta.
    pub tabs_full: bool,
    /// Com o mouse parado numa aba, a conversa dela e o compositor abrem por cima.
    pub tab_preview: bool,
    /// Depois dos grupos, as abas que estão trabalhando vêm antes das outras.
    pub tabs_working_first: bool,
    /// Depois dos grupos e das que trabalham, as abas paradas vêm da resposta mais recente para a mais antiga.
    pub tabs_recent_first: bool,
    /// Caixa do "Ver ao vivo": distância da borda direita e da de baixo da janela, em px lógicos.
    pub live_corner: [f32; 2],
    /// Larguras arrastadas do painel lateral (abas comuns e Navegador; `None` no Navegador segue a fração da janela) e
    /// altura do terminal do rodapé. O que a janela de agora não comporta é cortado ao desenhar, sem mexer no guardado.
    pub side_width: f32,
    pub side_browser_width: Option<f32>,
    pub terminal_height: f32,
    pub tool_look: ToolLook,
    /// Lista de tarefas do agente (TaskCreate/TaskUpdate) como um bloco de progresso na conversa.
    pub task_list: bool,
    pub thinking_tools: ThinkingTools,
    /// Na pele Terminal, o pensamento terminado chega numa linha e abre no clique.
    pub thinking_collapsed: bool,
    /// Botão Gráfico sobre as tabelas numéricas das respostas.
    pub table_chart: bool,
    pub ask_highlight: AskHighlight,
    /// Geral: também deste computador, no mesmo arquivo; o "Voltar ao padrão" da Aparência não mexe nelas.
    pub language: Language,
    pub currency: Currency,
    pub hands_free: bool,
    /// Dispensa os avisos antes de enviar comandos destrutivos ou interromper a resposta.
    pub skip_chat_confirmations: bool,
    /// Com o agente trabalhando, a mensagem enviada sai como Orientar em vez de esperar no painel "Na fila".
    pub steer_while_working: bool,
    #[serde(default)]
    pub jev_delivery: bool,
    /// Contas e modelos em uma linha por conta, sem barras: escolha deste aparelho, como no web.
    pub accounts_compact: bool,
    pub sidebar_group: SidebarGroup,
    pub side_tab: SideTab,
    pub terminal_font: CodeFont,
    /// Tamanho em pixels, independente do texto e do código da conversa.
    pub terminal_size: u16,
    pub code_font: CodeFont,
    /// Meio pixel por unidade, para permitir 12,5 px sem arredondar o controle.
    pub code_size: u16,
    /// Geral: fechar a janela esconde o app na bandeja em vez de encerrar.
    pub keep_in_tray: bool,
    /// Geral: o navegador embutido preenche sozinho as senhas salvas no Chrome. Nasce desligado: é senha indo para
    /// página sem a pessoa pedir.
    pub chrome_autofill: bool,
    /// Geral, só Linux: o navegador embutido usa o Chrome completo do sistema em vez do `chrome-headless-shell`.
    pub full_chrome: bool,
}

const DEFAULT: Appearance = Appearance { panels: Panels::Attached, theme: ThemeMode::Dark, palette: Palette::Classic,
    desktop_text: DesktopText::Desktop, dark: MODE_COLORS, light: MODE_COLORS, transparency: 40, surface_material: SurfaceMaterial::Glass, solidity: 70,
    background: Background::Plain, background_effect: crate::effects::BackgroundEffect::None, background_scope: BackgroundScope::Everywhere, wallpaper: Wallpaper::Window, reading: Reading::Auto, sheet_solidity: 60, text_contrast: 30,
    font: Font::System, text_size: 100, line_height: 100, column: 100, sidebar_height: SidebarHeight::Full,
    navigation: Navigation::Sidebar, show_remote_tabs: true, sidebar_width: None, sidebar_compact: false, tabs_full: false, tab_preview: false, tabs_working_first: false, tabs_recent_first: false, live_corner: [16., 16.],
    side_width: 300., side_browser_width: None, terminal_height: 260.,
    tool_look: ToolLook::Classic, task_list: false, thinking_tools: ThinkingTools::Search, thinking_collapsed: false, table_chart: false, ask_highlight: AskHighlight::Accent,
    language: Language::System, currency: Currency::Usd, hands_free: false, skip_chat_confirmations: false, steer_while_working: false, jev_delivery: false, accounts_compact: false, sidebar_group: SidebarGroup::None, side_tab: SideTab::Context,
    terminal_font: CodeFont::JetBrainsMono, terminal_size: 12, code_font: CodeFont::JetBrainsMono, code_size: 25, keep_in_tray: false, chrome_autofill: false, full_chrome: false };

impl Default for Appearance {
    fn default() -> Self { DEFAULT }
}

impl Appearance {
    /// Atalho de valores: ajustes posteriores continuam independentes.
    pub fn compact_style(self) -> Self {
        Self { font: Font::System, text_size: 100, line_height: 108, column: 94,
            code_font: CodeFont::JetBrainsMono, code_size: 25, tool_look: ToolLook::Tree,
            navigation: Navigation::Conversations, sidebar_compact: true, ..self }
    }

    /// "Voltar ao padrão" do web: não mexe em tema, fonte, fundo, painéis nem no jeito da conversa.
    pub fn reset_keeping_choices(self) -> Self {
        Self { panels: self.panels, font: self.font, theme: self.theme, palette: self.palette, desktop_text: self.desktop_text,
            surface_material: self.surface_material,
            background: self.background, background_effect: self.background_effect, background_scope: self.background_scope, wallpaper: self.wallpaper, tool_look: self.tool_look, task_list: self.task_list,
            thinking_tools: self.thinking_tools, table_chart: self.table_chart, navigation: self.navigation, show_remote_tabs: self.show_remote_tabs, sidebar_width: self.sidebar_width, sidebar_compact: self.sidebar_compact, live_corner: self.live_corner,
            side_width: self.side_width, side_browser_width: self.side_browser_width, terminal_height: self.terminal_height,
            language: self.language, currency: self.currency, hands_free: self.hands_free, skip_chat_confirmations: self.skip_chat_confirmations,
            steer_while_working: self.steer_while_working, jev_delivery: self.jev_delivery, accounts_compact: self.accounts_compact, sidebar_group: self.sidebar_group, side_tab: self.side_tab,
            code_font: self.code_font, terminal_font: self.terminal_font, keep_in_tray: self.keep_in_tray, chrome_autofill: self.chrome_autofill, full_chrome: self.full_chrome,
            ..Self::default() }
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

    /// Largura da barra cheia em vigor: com abas não há barra; valor torto no arquivo volta para a escala.
    pub fn full_sidebar_width(&self) -> f32 {
        match (self.navigation, self.sidebar_width) {
            (navigation, _) if navigation.tabs() => 0.,
            (_, Some(width)) if width.is_finite() => width.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
            (navigation, _) => navigation.sidebar_width(),
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
        self.code_size = self.code_size.clamp(16, 48);
        self.terminal_size = self.terminal_size.clamp(8, 24);
        // O limite de cima depende da janela e é aplicado ao desenhar; aqui só o que nunca vale.
        for v in &mut self.live_corner { *v = if v.is_finite() { v.max(0.) } else { 16. }; }
        if !self.side_width.is_finite() { self.side_width = DEFAULT.side_width; }
        self.side_browser_width = self.side_browser_width.filter(|v| v.is_finite());
        self.terminal_height = if self.terminal_height.is_finite() { self.terminal_height.clamp(120., 800.) } else { DEFAULT.terminal_height };
        self
    }
}

static CURRENT: RwLock<Appearance> = RwLock::new(DEFAULT);
static IMAGE_NAME: RwLock<Option<String>> = RwLock::new(None);

pub fn get() -> Appearance { *CURRENT.read().unwrap_or_else(|e| e.into_inner()) }

pub fn set(value: Appearance) { *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = value.clamped(); }

/// Pasta das configurações deste app. No Windows não há `HOME`: sem o `APPDATA`, nada era gravado.
pub(crate) fn dir() -> Option<PathBuf> {
    let base = if cfg!(windows) { std::env::var_os("APPDATA").map(PathBuf::from) } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }?;
    Some(base.join("hangar-native"))
}

fn path() -> Option<PathBuf> { Some(dir()?.join("appearance.json")) }

/// O que já foi visto do tour e das dicas (`tour::state`).
pub(crate) fn tour_path() -> Option<PathBuf> { Some(dir()?.join("tour.json")) }

/// Bloqueante. Arquivo ausente é `None`; ilegível ou inválido volta com o motivo, para quem precisa não gravar por cima.
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<Option<T>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("arquivo ilegível em {}: {e}", path.display())),
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("arquivo inválido em {}: {e}", path.display()))
}

/// Arquivo ausente é a primeira abertura; ilegível vai ao log em vez de voltar ao padrão sem explicação.
pub(crate) fn load_json<T: serde::de::DeserializeOwned + Default>(path: Option<PathBuf>) -> T {
    let Some(path) = path else { return T::default() };
    read_json(&path).unwrap_or_else(|e| { eprintln!("{e}"); None }).unwrap_or_default()
}

/// Bloqueante. Só a geração mais nova de cada arquivo grava, trocando o arquivo inteiro (tmp + rename), como `save`.
pub(crate) fn save_json<T: serde::Serialize>(path: Option<PathBuf>, generation: u64, written: &std::sync::Mutex<u64>, saved: &T) -> std::io::Result<()> {
    let mut written = written.lock().unwrap_or_else(|e| e.into_inner());
    if generation <= *written { return Ok(()); }
    let path = path.ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?;
    std::fs::create_dir_all(path.parent().expect("arquivo dentro de hangar-native"))?;
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec(saved).map_err(std::io::Error::other)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    // Só do usuário: o `tour.json` guarda os ids dos celulares.
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(&tmp)?;
    // O `mode` só vale na criação: um tmp que sobrou levaria o modo antigo ao arquivo no `rename`.
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; file.set_permissions(std::fs::Permissions::from_mode(0o600))?; }
    std::io::Write::write_all(&mut file, &bytes)?;
    drop(file);
    std::fs::rename(&tmp, &path)?;
    *written = generation;
    Ok(())
}

/// Cópia da imagem de fundo escolhida: o original pode sumir ou mudar depois.
pub fn image_path() -> Option<PathBuf> { Some(dir()?.join("background-image")) }

pub fn image_name() -> Option<String> {
    IMAGE_NAME.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn read_image_name() -> Option<String> {
    std::fs::read_to_string(dir()?.join("background-name")).ok().filter(|name| !name.is_empty())
}

/// Bloqueante; o nome fica separado porque Appearance é Copy.
pub fn save_image_name(name: Option<&str>) -> std::io::Result<()> {
    let dir = dir().ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("background-name");
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, name.unwrap_or_default())?;
    std::fs::rename(tmp, path)?;
    *IMAGE_NAME.write().unwrap_or_else(|e| e.into_inner()) = name.filter(|n| !n.is_empty()).map(str::to_owned);
    Ok(())
}

/// Aviso de "continua na bandeja": `true` só na primeira vez que a janela é escondida neste computador. Bloqueante.
pub fn take_tray_notice() -> bool {
    let Some(file) = dir().map(|dir| dir.join("tray-notice")) else { return false };
    !file.exists() && std::fs::write(file, "1").is_ok()
}

/// A raiz escolhida por último em Nova sessão, como o `cp:last-root` do web; falha de disco só faz esquecer. Bloqueantes.
pub fn last_root() -> Option<String> { std::fs::read_to_string(dir()?.join("last-root")).ok().map(|s| s.trim().to_owned()) }

pub fn remember_root(path: &str) {
    if let Some(dir) = dir() { let _ = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("last-root"), path)); }
}

/// Ordem escolhida na lista de Contas, no formato do `cp_accounts_sort` do web; ilegível volta à ordem padrão. Bloqueantes.
pub fn accounts_sort() -> Option<(String, i8)> {
    #[derive(Deserialize)]
    struct Saved { key: String, dir: i8 }
    let saved: Saved = serde_json::from_slice(&std::fs::read(dir()?.join("accounts-sort.json")).ok()?).ok()?;
    matches!(saved.dir, 1 | -1).then_some((saved.key, saved.dir))
}

pub fn remember_accounts_sort(sort: Option<&(String, i8)>) {
    let Some(dir) = dir() else { return };
    let file = dir.join("accounts-sort.json");
    let result = match sort {
        Some((key, dir_)) => std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(&file, serde_json::json!({"key": key, "dir": dir_}).to_string())),
        None => std::fs::remove_file(&file).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }),
    };
    // Diagnóstico, não aviso: a ordem vale nesta abertura e o log do app guarda por que não ficou para a próxima.
    if let Err(error) = result { crate::log_line(&format!("ordem das contas não gravou: {error}")); }
}

pub fn search_all_folders() -> Result<bool, String> {
    let file = dir().ok_or_else(|| "sem pasta de configuração".to_owned())?.join("create-search-all.json");
    read_search_all(&file)
}

fn read_search_all(file: &std::path::Path) -> Result<bool, String> {
    match std::fs::read(file) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", file.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!("{}: {error}", file.display())),
    }
}

pub fn set_search_all_folders(value: bool) -> Result<(), String> {
    let dir = dir().ok_or_else(|| "sem pasta de configuração".to_owned())?;
    write_search_all(&dir.join("create-search-all.json"), value)
}

fn write_search_all(file: &std::path::Path, value: bool) -> Result<(), String> {
    let dir = file.parent().ok_or_else(|| "sem pasta de configuração".to_owned())?;
    let tmp = file.with_extension("json.tmp");
    std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&tmp, if value { "true\n" } else { "false\n" }))
        .and_then(|_| std::fs::rename(&tmp, &file)).map_err(|error| format!("{}: {error}", file.display()))
}

/// Último modelo e esforço escolhidos na criação, pela chave servidor:provider:conta/motor (`cp_last_model` do web).
pub fn last_model(key: &str) -> (String, String) {
    let saved: HashMap<String, (String, String)> = dir().and_then(|d| std::fs::read(d.join("last-models.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    saved.get(key).cloned().unwrap_or_default()
}

/// Modelo, esforço e permissão marcados como padrão de um harness, valendo para todas as contas dele. Bloqueante.
pub fn harness_default(key: &str) -> Option<(String, String, String)> {
    let saved: HashMap<String, (String, String, String)> = dir().and_then(|d| std::fs::read(d.join("harness-defaults.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    saved.get(key).cloned()
}

/// O prefixo completo impede importar o padrão de outro servidor com endereço parecido.
pub fn legacy_harness_defaults(server: &str) -> Vec<(String, (String, String, String))> {
    let saved: HashMap<String, (String, String, String)> = dir().and_then(|d| std::fs::read(d.join("harness-defaults.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let prefix = format!("{server}:");
    saved.into_iter().filter_map(|(key, value)| key.strip_prefix(&prefix).map(|scope| (scope.to_owned(), value))).collect()
}

/// Escolha em "Padrão" apaga a lembrança, como o `removeItem` do web.
pub fn remember_model(key: &str, model: &str, effort: &str) {
    let Some(dir) = dir() else { return };
    let file = dir.join("last-models.json");
    let mut saved: HashMap<String, (String, String)> = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if model.is_empty() && effort.is_empty() { saved.remove(key); } else { saved.insert(key.to_owned(), (model.to_owned(), effort.to_owned())); }
    // Falha de disco só faz esquecer a escolha; fica no log para dar para saber por quê.
    if let Ok(bytes) = serde_json::to_vec(&saved) && let Err(error) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(file, bytes)) {
        eprintln!("memória do modelo não gravou: {error}");
    }
}

/// Sem arquivo, ou arquivo ilegível, abre no padrão; o motivo da falha de leitura volta para ser mostrado.
pub fn load() -> Result<Appearance, String> {
    *IMAGE_NAME.write().unwrap_or_else(|e| e.into_inner()) = read_image_name();
    let Some(path) = path() else { return Ok(Appearance::default()) };
    *SEEN.lock().unwrap_or_else(|e| e.into_inner()) = stamp(&path);
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
    std::fs::rename(&tmp, &path)?;
    *SEEN.lock().unwrap_or_else(|e| e.into_inner()) = stamp(&path);
    Ok(())
}

/// Última versão do arquivo que este app leu ou gravou. A configuração compartilhada grava nele com o app aberto.
static SEEN: Mutex<Option<std::time::SystemTime>> = Mutex::new(None);

fn stamp(path: &std::path::Path) -> Option<std::time::SystemTime> { std::fs::metadata(path).and_then(|m| m.modified()).ok() }

/// Bloqueante. `true` quando o arquivo mudou por fora desde a última leitura ou gravação deste app.
pub fn changed_on_disk() -> bool {
    let Some(path) = path() else { return false };
    let now = stamp(&path);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if now.is_none() || *seen == now { return false; }
    *seen = now;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_tabs_default_on_and_hidden_choice_survives_reload_and_reset() {
        let old: Appearance = serde_json::from_str(r#"{"navigation":"tabs"}"#).unwrap();
        assert!(old.show_remote_tabs);
        let hidden = Appearance { show_remote_tabs: false, ..old };
        let loaded: Appearance = serde_json::from_slice(&serde_json::to_vec(&hidden).unwrap()).unwrap();
        assert!(!loaded.show_remote_tabs);
        assert!(!loaded.reset_keeping_choices().show_remote_tabs);
    }
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn search_all_defaults_on_and_persists_without_creating_a_session() {
        let dir = std::env::temp_dir().join(format!("hangar-search-all-{}", std::process::id()));
        let file = dir.join("preference.json");
        assert!(!dir.exists());
        assert_eq!(read_search_all(&file), Ok(true));
        write_search_all(&file, false).unwrap();
        assert_eq!(read_search_all(&file), Ok(false));
        write_search_all(&file, true).unwrap();
        assert_eq!(read_search_all(&file), Ok(true));
        std::fs::write(&file, "invalid").unwrap();
        assert!(read_search_all(&file).is_err());
        std::fs::create_dir(file.with_extension("json.tmp")).unwrap();
        assert!(write_search_all(&file, false).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "invalid");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_from_before_the_tray_option_opens_with_it_off() {
        let old: Appearance = serde_json::from_str("{}").unwrap();
        assert!(!old.keep_in_tray);
        let on: Appearance = serde_json::from_str(r#"{"keep_in_tray": true}"#).unwrap();
        assert!(on.keep_in_tray);
        // É de Geral: o "Voltar ao padrão" da Aparência não desliga.
        assert!(on.reset_keeping_choices().keep_in_tray);
    }

    #[test]
    fn chrome_autofill_starts_off_and_survives_the_appearance_reset() {
        assert!(!serde_json::from_str::<Appearance>("{}").unwrap().chrome_autofill);
        let on: Appearance = serde_json::from_str(r#"{"chrome_autofill": true}"#).unwrap();
        assert!(on.reset_keeping_choices().chrome_autofill);
        let full: Appearance = serde_json::from_str(r#"{"full_chrome": true}"#).unwrap();
        assert!(full.reset_keeping_choices().full_chrome && !serde_json::from_str::<Appearance>("{}").unwrap().full_chrome);
    }

    #[test]
    fn compact_style_preserves_other_choices_and_keeps_adjustments_after_reload() {
        let before = Appearance { theme: ThemeMode::Light, background: Background::Texture,
            font: Font::Mono, text_size: 120, line_height: 120, column: 120,
            code_font: CodeFont::System, code_size: 30, ..Appearance::default() };
        let compact = before.compact_style();
        assert_eq!((compact.font, compact.text_size, compact.line_height, compact.column), (Font::System, 100, 108, 94));
        assert_eq!((compact.code_font, compact.code_size, compact.tool_look), (CodeFont::JetBrainsMono, 25, ToolLook::Tree));
        assert_eq!((compact.navigation, compact.sidebar_compact), (Navigation::Conversations, true));
        assert_eq!((compact.theme, compact.background), (before.theme, before.background));
        let adjusted = Appearance { text_size: 105, ..compact };
        let loaded: Appearance = serde_json::from_str(&serde_json::to_string(&adjusted).unwrap()).unwrap();
        assert_eq!(loaded, adjusted);
        assert_ne!(loaded, loaded.compact_style());
        assert_eq!(loaded.compact_style(), compact);
    }

    #[test]
    fn grouping_by_machine_is_saved_off_by_default_and_kept_by_reset() {
        assert_eq!(Appearance::default().sidebar_group, SidebarGroup::None);
        let loaded: Appearance = serde_json::from_value(serde_json::json!({"sidebar_group": "machine"})).unwrap();
        assert_eq!(loaded.sidebar_group, SidebarGroup::Machine);
        assert_eq!(serde_json::to_value(loaded).unwrap()["sidebar_group"], "machine");
        assert_eq!(loaded.reset_keeping_choices().sidebar_group, SidebarGroup::Machine);
    }

    #[test]
    fn sidebar_navigation_keeps_old_files_and_round_trips_both_densities() {
        for (value, expected) in [("sidebar", Navigation::Sidebar), ("tabs", Navigation::Tabs), ("bottom_tabs", Navigation::BottomTabs), ("unknown", Navigation::Sidebar)] {
            let old: Appearance = serde_json::from_value(serde_json::json!({"navigation": value})).unwrap();
            assert_eq!((old.navigation, old.sidebar_compact), (expected, false));
        }
        for compact in [false, true] {
            let saved = Appearance { navigation: Navigation::Conversations, sidebar_compact: compact, ..Appearance::default() };
            let loaded: Appearance = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
            assert_eq!(loaded, saved);
            assert_eq!((loaded.reset_keeping_choices().navigation, loaded.reset_keeping_choices().sidebar_compact), (Navigation::Conversations, compact));
        }
    }

    #[test]
    fn missing_fields_take_defaults_and_values_stay_in_range() {
        let parsed: Appearance = serde_json::from_str(r#"{"panels":"floating","text_size":400}"#).unwrap();
        let parsed = parsed.clamped();
        assert_eq!(parsed.panels, Panels::Floating);
        assert_eq!(parsed.text_size, 150);
        assert_eq!(parsed.solidity, 70);
        assert_eq!(parsed.surface_material, SurfaceMaterial::Glass);
        assert_eq!(parsed.dark.tint_strength, 40);
        assert_eq!((parsed.theme, parsed.palette), (ThemeMode::Dark, Palette::Classic));
        assert_eq!((parsed.code_font, parsed.code_size), (CodeFont::JetBrainsMono, 25));
    }

    #[test]
    fn terminal_typography_is_independent_and_survives_reload() {
        let old: Appearance = serde_json::from_str("{}").unwrap();
        assert_eq!((old.terminal_font, old.terminal_size), (CodeFont::JetBrainsMono, 12));
        let saved = Appearance { terminal_font: CodeFont::System, terminal_size: 18, ..old };
        let loaded: Appearance = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(loaded, saved);
        assert_eq!((loaded.font, loaded.text_size, loaded.code_font, loaded.code_size),
            (old.font, old.text_size, old.code_font, old.code_size));
        assert_eq!((loaded.compact_style().terminal_font, loaded.compact_style().terminal_size), (CodeFont::System, 18));
        assert_eq!((Appearance { terminal_size: 0, ..saved }.clamped().terminal_size,
            Appearance { terminal_size: 900, ..saved }.clamped().terminal_size), (8, 24));
        let reset = saved.reset_keeping_choices();
        assert_eq!((reset.terminal_font, reset.terminal_size), (CodeFont::System, 12));
    }

    #[test]
    fn installed_fonts_round_trip_and_old_choices_still_read() {
        let old: Appearance = serde_json::from_str(r#"{"font":"mono","code_font":"system","terminal_font":"jet_brains_mono"}"#).unwrap();
        assert_eq!((old.font, old.code_font, old.terminal_font), (Font::Mono, CodeFont::System, CodeFont::JetBrainsMono));
        let saved = Appearance { font: Font::from_family("Fira Sans"), code_font: CodeFont::from_family("Fira Code"),
            terminal_font: CodeFont::from_family(crate::theme::CODE_MONO), ..old };
        let loaded: Appearance = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(loaded, saved);
        assert_eq!((loaded.font.family(), loaded.terminal_font), ("Fira Sans", CodeFont::JetBrainsMono));
        assert_eq!(Font::from_family(crate::theme::SANS), Font::System);
    }

    #[test]
    fn code_font_round_trips_and_size_is_bounded_independently() {
        let saved = Appearance { code_font: CodeFont::System, code_size: 31, ..Appearance::default() };
        let loaded: Appearance = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!((loaded.code_font, loaded.code_size, loaded.text_size), (CodeFont::System, 31, 100));
        assert_eq!((Appearance { code_size: 0, ..saved }.clamped().code_size,
            Appearance { code_size: 900, ..saved }.clamped().code_size), (16, 48));
        let reset = saved.reset_keeping_choices();
        assert_eq!((reset.code_font, reset.code_size), (CodeFont::System, 25));
    }

    #[test]
    fn background_scope_defaults_and_persists_independently_of_panels() {
        for panels in [Panels::Attached, Panels::Floating] {
            let old: Appearance = serde_json::from_value(serde_json::json!({ "panels": panels })).unwrap();
            assert_eq!(old.background_scope, BackgroundScope::Everywhere);
            for scope in [BackgroundScope::Chat, BackgroundScope::Everywhere] {
                let saved = Appearance { background_scope: scope, ..old };
                let loaded: Appearance = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
                assert_eq!((loaded.background_scope, loaded.panels), (scope, panels));
                assert_eq!(loaded.reset_keeping_choices().background_scope, scope);
            }
        }
    }

    #[test]
    fn surface_material_round_trips_and_survives_appearance_reset() {
        let saved = Appearance { surface_material: SurfaceMaterial::Opaque, ..Appearance::default() };
        let loaded: Appearance = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(loaded.surface_material, SurfaceMaterial::Opaque);
        assert_eq!(loaded.reset_keeping_choices().surface_material, SurfaceMaterial::Opaque);
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
    fn dragged_sidebar_width_stays_in_scale_and_survives_reset() {
        let width = |navigation, sidebar_width| Appearance { navigation, sidebar_width, ..Appearance::default() }.full_sidebar_width();
        assert_eq!(width(Navigation::Sidebar, None), 284.);
        assert_eq!(width(Navigation::Conversations, None), 256.);
        assert_eq!(width(Navigation::Conversations, Some(330.)), 330.);
        assert_eq!((width(Navigation::Sidebar, Some(90.)), width(Navigation::Sidebar, Some(900.))), (SIDEBAR_MIN, SIDEBAR_MAX));
        assert_eq!(width(Navigation::Sidebar, Some(f32::NAN)), 284.);
        assert_eq!((width(Navigation::Tabs, Some(330.)), width(Navigation::BottomTabs, Some(330.))), (0., 0.));
        let custom = Appearance { sidebar_width: Some(330.), ..Appearance::default() };
        assert_eq!(custom.reset_keeping_choices().sidebar_width, Some(330.));
        // Arquivo de antes do campo continua abrindo.
        let old: Appearance = serde_json::from_str(r#"{"navigation":"conversations"}"#).unwrap();
        assert_eq!(old.sidebar_width, None);
        assert_eq!((old.side_width, old.side_browser_width, old.terminal_height), (300., None, 260.));
        let panels = Appearance { side_width: 410., side_browser_width: Some(700.), terminal_height: 500., ..Appearance::default() };
        let kept = panels.reset_keeping_choices();
        assert_eq!((kept.side_width, kept.side_browser_width, kept.terminal_height), (410., Some(700.), 500.));
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
        let terminal: Appearance = serde_json::from_str(r#"{"tool_look":"terminal"}"#).unwrap();
        assert_eq!(terminal.tool_look, ToolLook::Terminal);
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
