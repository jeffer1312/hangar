//! Configurações do app Electron (`shell/`): o web guarda aparência e servidores no localStorage do Chromium, um LevelDB
//! dentro do perfil do Electron. Lê uma cópia do banco: o Electron aberto segura a trava dele.
use crate::appearance::*;
use base64::Engine;
use serde_json::Value;
use std::{collections::HashMap, path::{Path, PathBuf}};

pub enum Failure { Missing, Read(String) }

pub struct Imported {
    pub appearance: Appearance,
    /// Bytes da imagem de fundo, só quando o fundo do web é Imagem.
    pub image: Option<Vec<u8>>,
    /// Endereço e token do servidor ativo do web.
    pub server: Option<(String, String)>,
    /// Todas as máquinas da lista do web (`cp_servers`), a ativa inclusive.
    pub servers: Vec<crate::app::ServerEntry>,
}

/// `Electron` é o shell aberto com `electron main.cjs` (sem package.json, fica o nome padrão); `hangar-shell`, com
/// `electron .`; `Hangar`, o empacotado (productName).
fn profiles() -> Vec<PathBuf> {
    let base = if cfg!(windows) { std::env::var_os("APPDATA").map(PathBuf::from) }
        else if cfg!(target_os = "macos") { std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support")) }
        else {
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
                .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        };
    base.map(|base| ["Electron", "hangar-shell", "Hangar"].iter().map(|name| base.join(name)).collect()).unwrap_or_default()
}

fn store(profile: &Path) -> PathBuf { profile.join("Local Storage").join("leveldb") }

/// Perfis do shell com localStorage, o gravado por último primeiro. O `settings.json` é do shell (URL e janela): separa
/// o perfil dele do de outro app Electron aberto do mesmo jeito. Bloqueante, só olha o disco.
fn found() -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = profiles().into_iter().filter_map(|profile| {
        if !(store(&profile).join("CURRENT").is_file() && profile.join("settings.json").is_file()) { return None; }
        let newest = std::fs::read_dir(store(&profile)).ok()?.filter_map(|e| e.ok()?.metadata().ok()?.modified().ok()).max()?;
        Some((newest, profile))
    }).collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, profile)| profile).collect()
}

pub fn exists() -> bool { !found().is_empty() }

/// Bloqueante. A imagem ainda não foi gravada: quem aplica chama `save_image`.
pub fn load(base: Appearance) -> Result<Imported, Failure> {
    let mut failure = Failure::Missing;
    for profile in found() {
        let mut origins = match read_store(&store(&profile)) { Ok(origins) => origins, Err(e) => { failure = Failure::Read(e); continue } };
        // A origem que o shell abriu por último vence; sem ela, a que tem mais preferências.
        let preferred = std::fs::read(profile.join("settings.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| url::Url::parse(v.get("url")?.as_str()?).ok()).map(|u| u.origin().ascii_serialization());
        let Some(origin) = preferred.filter(|o| origins.contains_key(o))
            .or_else(|| origins.iter().max_by_key(|(_, keys)| keys.len()).map(|(o, _)| o.clone())) else { continue };
        let local = origins.remove(&origin).unwrap_or_default();
        let mut imported = map(&local, &origin, base);
        prefer_loopback(&mut imported);
        return Ok(imported);
    }
    Err(failure)
}

/// Onde o app conecta sem configuração: o backend desta máquina.
const HERE: &str = "http://127.0.0.1:8765";

/// O Electron pode ter aberto o servidor desta máquina pela tailnet. Esse entra pelo loopback: pelo endereço de fora o
/// app dá a volta na rede e trata a própria máquina como remota. Mesma máquina é o backend local responder com o mesmo
/// identificador; identificador vazio não prova nada. Bloqueante, como o `load`.
fn prefer_loopback(imported: &mut Imported) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    let mut moved: Vec<(String, String)> = Vec::new();
    for entry in &mut imported.servers {
        if runtime.block_on(same_machine(&entry.address, &entry.token)) {
            moved.push((std::mem::replace(&mut entry.address, HERE.into()), entry.token.clone()));
        }
    }
    if let Some((address, token)) = imported.server.as_mut()
        && moved.iter().any(|(a, t)| a == address && t == token) {
        *address = HERE.into();
    }
}

async fn same_machine(address: &str, token: &str) -> bool {
    let Some(here) = identifier(HERE, token).await else { return false };
    identifier(address, token).await == Some(here)
}

/// Falha vai ao stderr: sem ela, o servidor desta máquina fica pelo endereço de fora e ninguém sabe por quê.
async fn identifier(address: &str, token: &str) -> Option<String> {
    let api = crate::api::Api::new(address, token).map_err(|e| eprintln!("importar do Electron: {address} inválido: {}", e.detail)).ok()?;
    let value = api.server_read(&["peers", "identificador"], &[], 5).await
        .map_err(|e| eprintln!("importar do Electron: identificador de {address} não veio: {}", e.detail)).ok()?;
    value.get("identificador")?.as_str().filter(|id| !id.is_empty()).map(str::to_owned)
}

/// Grava a imagem trazida como a do fundo Imagem, pelo mesmo caminho de validação da escolha de arquivo. Bloqueante.
pub fn save_image(imported: Imported) -> Result<Imported, Failure> {
    let Some(bytes) = &imported.image else { return Ok(imported) };
    let fail = |reason: &str| Failure::Read(crate::i18n::tr(reason));
    let dest = image_path().ok_or_else(|| fail("backdrop_not_saved"))?;
    let tmp = std::env::temp_dir().join(format!("hangar-electron-bg-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| Failure::Read(e.to_string()))?;
    let adopted = crate::media::adopt_backdrop(&tmp, &dest);
    let _ = std::fs::remove_file(&tmp);
    adopted.map_err(fail)?;
    save_image_name(Some("Electron")).map_err(|e| Failure::Read(e.to_string()))?;
    Ok(imported)
}

/// Chaves `cp_*` e `PARAGLIDE_LOCALE` de cada origem. A cópia fica numa pasta só do usuário: o banco guarda o token.
fn read_store(dir: &Path) -> Result<HashMap<String, HashMap<String, String>>, String> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let copy = std::env::temp_dir().join(format!("hangar-electron-{}-{stamp}", std::process::id()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)] { use std::os::unix::fs::DirBuilderExt; builder.mode(0o700); }
    builder.create(&copy).map_err(|e| e.to_string())?;
    let result = (|| {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_file() && entry.file_name() != "LOCK" {
                std::fs::copy(entry.path(), copy.join(entry.file_name())).map_err(|e| e.to_string())?;
            }
        }
        let mut options = rusty_leveldb::Options::default();
        options.create_if_missing = false;
        let mut db = rusty_leveldb::DB::open(&copy, options).map_err(|e| e.to_string())?;
        let mut iter = db.new_iter().map_err(|e| e.to_string())?;
        let mut origins: HashMap<String, HashMap<String, String>> = HashMap::new();
        while let Some((key, value)) = rusty_leveldb::LdbIterator::next(&mut iter) {
            let Some((origin, name)) = split_key(&key) else { continue };
            if !(name.starts_with("cp_") || name == "PARAGLIDE_LOCALE") { continue; }
            if let Some(value) = value.split_first().and_then(|(kind, bytes)| decode(*kind, bytes)) {
                origins.entry(origin).or_default().insert(name, value);
            }
        }
        Ok(origins)
    })();
    let _ = std::fs::remove_dir_all(&copy);
    result
}

/// Chave do localStorage no LevelDB do Chromium: `_<origem>\0<codificação><nome>`.
fn split_key(key: &[u8]) -> Option<(String, String)> {
    let rest = key.strip_prefix(b"_")?;
    let cut = rest.iter().position(|b| *b == 0)?;
    let (kind, name) = rest[cut + 1..].split_first()?;
    Some((String::from_utf8(rest[..cut].to_vec()).ok()?, decode(*kind, name)?))
}

/// Texto do Chromium: 1 é Latin-1, um byte por caractere; 0 é UTF-16 little-endian.
fn decode(kind: u8, bytes: &[u8]) -> Option<String> {
    match kind {
        1 => Some(bytes.iter().map(|b| *b as char).collect()),
        0 if bytes.len() % 2 == 0 => String::from_utf16(&bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()).ok(),
        _ => None,
    }
}

/// Cada chave com par no nativo, no valor em vigor no web: chave ausente vale o padrão do web, não o daqui.
fn map(local: &HashMap<String, String>, origin: &str, base: Appearance) -> Imported {
    let get = |key: &str| local.get(key).map(String::as_str);
    let on = |key: &str, value: &str| get(key) == Some(value);
    // `lerNumero`/`lerEscala` do web: fora da faixa vale o padrão.
    let number = |key: &str, range: std::ops::RangeInclusive<f64>, default: u16| get(key).and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| range.contains(v)).map_or(default, |v| v.round() as u16);
    let palette = match get("cp_palette") { Some("neutro") => Palette::Neutral, _ => Palette::Classic };
    let image = on("cp_bg", "image").then(|| get("cp_bg_image")).flatten()
        .and_then(|url| url.split_once(";base64,")).and_then(|(_, data)| base64::engine::general_purpose::STANDARD.decode(data.trim()).ok());
    let appearance = Appearance {
        theme: match get("cp_theme") { Some("light") => ThemeMode::Light, Some("dark") => ThemeMode::Dark, Some("desktop") => ThemeMode::Desktop, _ => ThemeMode::Auto },
        palette,
        desktop_text: if on("cp_texto_desktop", "app") { DesktopText::App } else { DesktopText::Desktop },
        panels: match get("cp_panels") {
            Some("edge") => Panels::Attached,
            Some("card") => Panels::Floating,
            _ => if palette == Palette::Neutral { Panels::Attached } else { Panels::Floating },
        },
        dark: colors(get("cp_cor_dark"), base.dark),
        light: colors(get("cp_cor_light"), base.light),
        background: match get("cp_bg") {
            Some("texture") => Background::Texture,
            Some("aurora") => Background::Light,
            Some("image") if image.is_some() => Background::Image,
            Some("desktop") => Background::Desktop,
            _ => Background::Plain,
        },
        wallpaper: if on("cp_desktop_glass", "1") { Wallpaper::Glass } else { Wallpaper::Window },
        // Mesma conta do véu nos dois: 100 é a imagem crua.
        transparency: number("cp_bg_scrim", 0.0..=100.0, 11),
        // "Solidez das caixas" do web; a tinta dos painéis anda com ela nos dois (`theme::panel_alpha`).
        solidity: number("cp_surface_solid", 0.0..=100.0, 12),
        reading: match get("cp_read") { Some("glass") => Reading::None, Some("text") => Reading::Text, Some("solid") => Reading::Sheet, _ => Reading::Auto },
        sheet_solidity: number("cp_read_alpha", 0.0..=100.0, 92),
        text_contrast: number("cp_text_boost", 0.0..=100.0, 10),
        font: if on("cp_font", "mono") { Font::Mono } else { Font::System },
        text_size: number("cp_text_size", 50.0..=150.0, 100),
        line_height: number("cp_text_lh", 50.0..=150.0, 100),
        column: number("cp_text_width", 50.0..=150.0, 100),
        tool_look: if on("cp_tool_look", "chips") { ToolLook::Chips } else { ToolLook::Classic },
        task_list: on("cp_task_rows", "on"),
        thinking_tools: match get("cp_pensamento_tools") { Some("nada") => ThinkingTools::None, Some("tudo") => ThinkingTools::All, _ => ThinkingTools::Search },
        table_chart: on("cp_table_chart", "on"),
        sidebar_group: if on("cp_group_by", "project") { SidebarGroup::Project } else { SidebarGroup::None },
        sidebar_height: if on("cp_sidebar_height", "content") { SidebarHeight::Content } else { SidebarHeight::Full },
        sidebar_compact: on("cp_density", "compact"),
        accounts_compact: on("cp_contas_compacta", "1"),
        language: match get("PARAGLIDE_LOCALE") { Some("pt") => Language::Pt, Some("en") => Language::En, _ => Language::System },
        currency: if get("cp_moeda").or(get("cp_costs_currency")) == Some("BRL") { Currency::Brl } else { Currency::Usd },
        hands_free: on("cp_ditado_maos_livres", "1"),
        ..base
    };
    Imported { appearance, image, server: server(get("cp_servers"), get("cp_active"), origin), servers: servers(get("cp_servers"), origin) }
}

/// `cp_cor_*` guarda só o desvio: `{destaque, tinta, forca}`, com a força em 0–100 de um teto de 45% de mistura.
fn colors(raw: Option<&str>, base: ModeColors) -> ModeColors {
    let saved = raw.and_then(|raw| serde_json::from_str::<Value>(raw).ok()).unwrap_or(Value::Null);
    let swatch = |key: &str| saved.get(key).and_then(Value::as_str).and_then(|h| h.strip_prefix('#')).filter(|h| h.len() == 6)
        .and_then(|h| u32::from_str_radix(h, 16).ok()).map_or(Swatch::Preset(0), |hex| Swatch::Custom(Hex(hex)));
    let tint_strength = saved.get("forca").and_then(Value::as_f64).map_or(base.tint_strength, |f| ((f.clamp(0., 100.) * 0.45).round() as u16).max(5));
    ModeColors { accent: swatch("destaque"), tint: swatch("tinta"), tint_strength }
}

/// O ativo da lista do web, ou o primeiro ligado. `baseUrl` vazio é o servidor da própria origem.
fn server(list: Option<&str>, active: Option<&str>, origin: &str) -> Option<(String, String)> {
    let list: Vec<Value> = serde_json::from_str(list?).ok()?;
    let usable = |s: &&Value| s.get("token").and_then(Value::as_str).is_some_and(|t| !t.is_empty());
    let pick = list.iter().filter(usable).find(|s| active.is_some() && s.get("id").and_then(Value::as_str) == active)
        .or_else(|| list.iter().filter(usable).find(|s| s.get("disabled").and_then(Value::as_bool) != Some(true)))?;
    let address = pick.get("baseUrl").and_then(Value::as_str).filter(|a| !a.is_empty()).unwrap_or(origin);
    Some((address.trim_end_matches('/').to_owned(), pick.get("token")?.as_str()?.to_owned()))
}

/// A lista inteira, com o endereço vazio resolvido para a origem. Sem token não dá para entrar: fica de fora.
fn servers(list: Option<&str>, origin: &str) -> Vec<crate::app::ServerEntry> {
    let list: Vec<Value> = list.and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
    list.iter().filter_map(|s| {
        let token = s.get("token")?.as_str().filter(|t| !t.is_empty())?.to_owned();
        let address = s.get("baseUrl").and_then(Value::as_str).filter(|a| !a.is_empty()).unwrap_or(origin).trim_end_matches('/').to_owned();
        let text = |key: &str| s.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned);
        Some(crate::app::ServerEntry { id: text("id").unwrap_or_else(crate::app::new_server_id), label: text("label").unwrap_or_default(),
            address, token, disabled: s.get("disabled").and_then(Value::as_bool) == Some(true),
            // O web marca o servidor de convite; sem a marca ele chamaria rotas do servidor inteiro e levaria 403.
            invite: s.get("invite").and_then(Value::as_bool) == Some(true), lan: None, connect: None, connect_refused: false, ephemeral: false })
    }).collect()
}

/// Grupos da Aparência que mudaram, pelas chaves de tradução dos títulos, para o resumo depois de importar.
pub fn changed(before: &Appearance, after: &Appearance, image: bool) -> Vec<&'static str> {
    let (b, a) = (before, after);
    [
        ("settings_theme", (b.theme, b.palette, b.desktop_text) != (a.theme, a.palette, a.desktop_text)),
        ("settings_panels", b.panels != a.panels),
        ("settings_color", (b.dark, b.light) != (a.dark, a.light)),
        ("settings_background_group", image || (b.background, b.wallpaper, b.transparency, b.solidity) != (a.background, a.wallpaper, a.transparency, a.solidity)),
        ("settings_reading_group", (b.reading, b.sheet_solidity, b.text_contrast) != (a.reading, a.sheet_solidity, a.text_contrast)),
        ("settings_text_group", (b.font, b.text_size, b.line_height, b.column) != (a.font, a.text_size, a.line_height, a.column)),
        ("settings_conversation_group", (b.tool_look, b.task_list, b.thinking_tools, b.table_chart) != (a.tool_look, a.task_list, a.thinking_tools, a.table_chart)),
        ("settings_sidebar_group", (b.sidebar_group, b.sidebar_height, b.sidebar_compact) != (a.sidebar_group, a.sidebar_height, a.sidebar_compact)),
        ("settings_page_general", (b.language, b.currency, b.hands_free, b.accounts_compact) != (a.language, a.currency, a.hands_free, a.accounts_compact)),
    ].into_iter().filter(|(_, changed)| *changed).map(|(key, _)| key).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn reads_chromium_keys_and_maps_web_preferences() {
        let key = b"_http://127.0.0.1:8765\x00\x01cp_theme";
        assert_eq!(split_key(key), Some(("http://127.0.0.1:8765".into(), "cp_theme".into())));
        assert_eq!(decode(0, &[b'o', 0, b'k', 0]).as_deref(), Some("ok"));
        assert_eq!(decode(1, b"desktop").as_deref(), Some("desktop"));

        let local: HashMap<String, String> = [
            ("cp_theme", "desktop"), ("cp_bg", "desktop"), ("cp_desktop_glass", "1"), ("cp_bg_scrim", "30"), ("cp_surface_solid", "24"),
            ("cp_cor_dark", r##"{"destaque":"#ff8800","tinta":null,"forca":100}"##), ("cp_text_size", "120"), ("cp_text_lh", "999"),
            ("cp_read", "solid"), ("cp_pensamento_tools", "tudo"), ("PARAGLIDE_LOCALE", "en"),
            ("cp_active", "b"), ("cp_servers", r#"[{"id":"a","baseUrl":"http://x:1","token":"t1"},{"id":"b","baseUrl":"","token":"t2"}]"#),
        ].into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        let base = Appearance { text_size: 70, font: Font::Mono, ..Appearance::default() };
        let got = map(&local, "http://127.0.0.1:8765", base);
        let a = got.appearance;
        assert_eq!((a.theme, a.background, a.wallpaper, a.transparency, a.solidity), (ThemeMode::Desktop, Background::Desktop, Wallpaper::Glass, 30, 24));
        assert_eq!((a.palette, a.panels), (Palette::Classic, Panels::Floating));
        assert_eq!((a.dark.accent, a.dark.tint, a.dark.tint_strength), (Swatch::Custom(Hex(0xff8800)), Swatch::Preset(0), 45));
        // Fora da faixa e ausente valem o padrão do web, não o que estava aqui.
        assert_eq!((a.text_size, a.line_height, a.font), (120, 100, Font::System));
        assert_eq!((a.reading, a.sheet_solidity, a.thinking_tools, a.language), (Reading::Sheet, 92, ThinkingTools::All, Language::En));
        assert_eq!(got.server, Some(("http://127.0.0.1:8765".into(), "t2".into())));
        let addresses: Vec<&str> = got.servers.iter().map(|s| s.address.as_str()).collect();
        assert_eq!(addresses, ["http://x:1", "http://127.0.0.1:8765"]);
        assert!(got.image.is_none());
        assert!(changed(&base, &a, false).contains(&"settings_theme"));
    }
}
