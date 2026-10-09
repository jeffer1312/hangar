// Sem isto o Windows abre um console preto junto da janela. O build de desenvolvimento fica com ele, para ver o stderr.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod api;
mod app;
mod appearance;
mod audio;
mod browser;
mod cards;
mod chat;
mod composer;
mod conversation;
mod delivery;
mod editdiff;
mod fileicons;
mod i18n;
mod interaction;
mod media;
mod effects;
mod electron;
mod mend;
mod motion;
mod plugin_ui;
mod single_instance;
mod term_view;
mod status;
mod tables;
mod ws;
mod theme;
mod tray;
mod ui_map;
mod update;
mod voice;
#[cfg(test)]
#[path = "../vendor/gpui-pre-0.3.7/src/elements/list_tail.rs"]
mod list_tail_tests;
#[cfg(test)]
mod a11y_snapshot_tests {
    use gpui_kit::accesskit;
    include!("../vendor/gpui-pre-0.3.7/src/window/a11y/snapshot.rs");
}
use gpui_kit::{component::{Root, Theme, ThemeMode}, *};
use std::{borrow::Cow, sync::Arc};

gpui_kit::assets::icon_assets!(ExtraIcons, [ArrowUp, GitBranch, RotateCcwClock, Paperclip, Plug, SquareSlash,
    Activity, Contrast, Droplet, Image, Keyboard, Layers, List, Mic, Monitor, RefreshCw, Server, SlidersHorizontal, Type, Users,
    SquarePen, FilePlus, Wrench, Circle, CircleDashed, ChartColumn, Table, ListChecks, Download, Clock, Languages, Banknote,
    Zap, Rocket, MessageCircle, Key, Pencil, GripVertical, AudioLines, Volume2, Hash, LogOut, Smartphone, FolderTree, ChevronsDownUp, FileCode,
    RotateCcw, CornerDownRight, MessageSquare, Sparkles, CircleAlert, CircleCheck, TriangleAlert, Link, Wifi,
    CircleStop, Upload, Workflow, CloudDownload, ArrowDownToLine, ArrowUpFromLine, GitFork, Trash, MessageCircleQuestionMark]);

pub const HANGAR_MARK: &str = "brand/hangar-mark.svg";
pub const GROUP_GLYPH: &str = "brand/group-glyph.svg";
pub const NO_TERMINAL: &str = "signals/no-terminal.svg";

struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == HANGAR_MARK { return Ok(Some(Cow::Borrowed(include_bytes!("../assets/hangar-mark.svg")))); }
        if path == GROUP_GLYPH { return Ok(Some(Cow::Borrowed(include_bytes!("../assets/group-glyph.svg")))); }
        if path == NO_TERMINAL { return Ok(Some(Cow::Borrowed(include_bytes!("../assets/signals/no-terminal.svg")))); }
        match path {
            "providers/claude.svg" => return Ok(Some(Cow::Borrowed(include_bytes!("../assets/providers/claude.svg")))),
            "providers/codex.svg" => return Ok(Some(Cow::Borrowed(include_bytes!("../assets/providers/codex.svg")))),
            "providers/kimi.svg" => return Ok(Some(Cow::Borrowed(include_bytes!("../assets/providers/kimi.svg")))),
            _ => {}
        }
        if let Some(bytes) = fileicons::load(path) { return Ok(Some(Cow::Borrowed(bytes))); }
        if let Some(bytes) = ExtraIcons.load(path)? { return Ok(Some(bytes)); }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

const FONTS: [&[u8]; 6] = [
    include_bytes!("../assets/fonts/Geist-Regular.ttf"), include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"), include_bytes!("../assets/fonts/Geist-Bold.ttf"),
    include_bytes!("../assets/fonts/Geist-Italic.ttf"),
    include_bytes!("../assets/fonts/jetbrains-mono/JetBrainsMono-Regular.ttf"),
];

/// Só para medir: HANGAR_NATIVE_WINDOW=LxA abre a janela nesse tamanho lógico. Sem a variável, 1180×800.
fn window_size() -> Size<Pixels> {
    let parsed = std::env::var("HANGAR_NATIVE_WINDOW").ok().and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some((w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?))
    }).filter(|(w, h)| *w >= 640. && *h >= 480.);
    let (w, h) = parsed.unwrap_or((1180., 800.));
    size(px(w), px(h))
}

/// Pasta de logs do Hangar, a mesma do backend e do shell Electron (`log_paths.base()`).
pub(crate) fn log_dir() -> std::path::PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(|| home_dir().join("AppData/Local"));
        root.join("hangar/logs/privado")
    } else {
        home_dir().join(".hangar/logs/privado")
    }
}

/// Falha sem pânico também precisa de rastro: aberto pelo lançador, o stderr vai pro nada.
pub fn log_line(text: &str) {
    use std::io::Write;
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("native.log")) {
        let _ = writeln!(file, "[{when}] {text} (v{})", env!("CARGO_PKG_VERSION"));
    }
}

fn home_dir() -> std::path::PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(std::path::PathBuf::from).unwrap_or_default()
}

/// Aberto pelo lançador, o stderr vai pro nada: sem isto um pânico fecha a janela sem deixar rastro.
fn log_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let dir = log_dir();
        let _ = std::fs::create_dir_all(&dir);
        let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let thread = std::thread::current().name().unwrap_or("?").to_owned();
        let line = format!("[{when}] pânico na thread {thread} (v{}): {info}", env!("CARGO_PKG_VERSION"));
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("native.log")) {
            let _ = writeln!(file, "{line}");
        }
        // Só o pânico da thread principal fecha a janela; o de uma tarefa do tokio fica no log e o app segue.
        if thread == "main" {
            let _ = std::fs::write(dir.join(CRASH_FILE), &line);
            // Daemon de notificação travado não pode segurar a janela morta aberta.
            let (done, wait) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = notify_rust::Notification::new().appname("Hangar").summary(&i18n::tr("crash_title")).body(&i18n::tr("crash_notify")).show();
                let _ = done.send(());
            });
            let _ = wait.recv_timeout(std::time::Duration::from_secs(2));
        }
        default(info);
    }));
}

const CRASH_FILE: &str = "native-crash.txt";

/// Erro que fechou a execução anterior, lido uma vez: quem abre depois não vê o mesmo aviso.
fn take_crash() -> Option<String> {
    let path = log_dir().join(CRASH_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(text.trim().to_owned()).filter(|text| !text.is_empty())
}

/// O script chama o atalho do assistente (`HANGAR_ASKPASS`) com o motivo e, quando o `sudo -S` recusou a senha anterior,
/// `--retry`; o atalho repassa como `--askpass <motivo> [--retry]`.
fn askpass_prompt(mut args: impl Iterator<Item = String>) -> Option<(String, bool)> {
    let _program = args.next();
    if args.next()? != "--askpass" { return None; }
    let reason = args.next().unwrap_or_default();
    Some((reason, args.next().as_deref() == Some("--retry")))
}

fn main() {
    // Antes da instância única: senão esta execução viraria um repasse para a janela e sairia sem imprimir a senha.
    if let Some((reason, retry)) = askpass_prompt(std::env::args()) { std::process::exit(single_instance::askpass_client(&reason, retry)); }
    log_panics();
    let (url_tx, links) = match single_instance::claim(single_instance::invite_arg(std::env::args())) {
        single_instance::Claim::Forwarded => {
            eprintln!("Hangar já está aberto; o pedido foi entregue à janela existente.");
            return;
        }
        single_instance::Claim::Primary(tx, rx) => (tx, rx),
    };
    let crash = take_crash();
    let runtime = Arc::new(tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("async runtime"));
    // Lida antes da primeira janela: o tema já nasce na escolha salva. Falha de leitura abre no padrão e aparece na tela.
    let appearance_error = match appearance::load() { Ok(value) => { appearance::set(value); None } Err(e) => Some(e) };
    i18n::set_language(appearance::get().language);
    let app = gpui_kit::application().with_assets(AppAssets);
    // macOS entrega o link por aqui; Linux e Windows, pela linha de comando (acima).
    app.on_open_urls(move |urls| for url in urls.into_iter().filter(|u| u.starts_with("hangar://")) { let _ = url_tx.try_send(url); });
    app.run(move |cx| {
        gpui_kit::init(cx);
        // Só para provar: HANGAR_NATIVE_REDUCE_MOTION=1 liga o movimento reduzido onde o sistema não informa.
        if std::env::var_os("HANGAR_NATIVE_REDUCE_MOTION").is_some() { cx.set_reduce_motion(true); }
        Theme::change(ThemeMode::Dark, None, cx);
        if let Err(error) = cx.text_system().add_fonts(FONTS.iter().map(|bytes| Cow::Borrowed(*bytes)).collect()) {
            eprintln!("fonte embutida recusada: {error}");
        }
        theme::sync_kit(None, cx);
        update::start(runtime.clone(), cx);
        cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, window_size(), cx))),
            app_id: Some("com.hangar.native".into()),
            // No Windows a barra do app faz as vezes da do sistema, com os botões dela (`topbar::window_buttons`).
            titlebar: Some(TitlebarOptions { title: Some("Hangar".into()), appears_transparent: cfg!(target_os = "windows"), ..Default::default() }),
            // A raiz pinta o fundo escolhido; o Vidro troca para Blurred no Windows e no macOS (`refresh_backdrop`).
            window_background: WindowBackgroundAppearance::Transparent,
            // Resposta chegando com o foco no outro monitor anda no ritmo da tela, não a 30 quadros.
            inactive_frame_interval: None,
            ..Default::default()
        }, |window, cx| {
            ui_map::install(window);
            let view = cx.new(|cx| app::Hangar::new(runtime.clone(), appearance_error.clone(), crash.clone(), links.clone(), window, cx));
            cx.new(|cx| Root::new(view, window, cx).bg(rgba(0x00000000)))
        }).expect("open native window");
        update::report_alive();
        cx.on_window_closed(|cx, _| { if cx.windows().is_empty() { cx.quit(); } }).detach();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob da gpui_kit traz um `test` que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn askpass_mode_takes_the_reason_and_the_retry_marker() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        assert_eq!(askpass_prompt(args(&["hangar-native", "--askpass", "instalar o tmux"])), Some(("instalar o tmux".into(), false)));
        assert_eq!(askpass_prompt(args(&["hangar-native", "--askpass", "instalar o tmux", "--retry"])), Some(("instalar o tmux".into(), true)));
        assert_eq!(askpass_prompt(args(&["hangar-native", "--askpass"])), Some((String::new(), false)));
        assert_eq!(askpass_prompt(args(&["hangar-native", "hangar://convite/h:8443/AB"])), None);
    }

    fn kebab(name: &str) -> String {
        let mut out = String::new();
        let mut prev: Option<char> = None;
        for c in name.chars() {
            let boundary = prev.is_some_and(|p| (c.is_ascii_uppercase() && (p.is_ascii_lowercase() || p.is_ascii_digit()))
                || (c.is_ascii_digit() && p.is_ascii_alphabetic()));
            if boundary { out.push('-'); }
            out.push(c.to_ascii_lowercase());
            prev = Some(c);
        }
        out
    }

    // Ícone fora do catálogo embutido não dá erro: o botão só fica em branco.
    #[test]
    fn every_icon_used_in_the_source_is_embedded() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        let mut missing = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() { stack.push(path); continue; }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") { continue; }
                let text = std::fs::read_to_string(&path).unwrap();
                for piece in text.split("IconName::").skip(1) {
                    let name: String = piece.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                    if !name.starts_with(|c: char| c.is_ascii_uppercase()) { continue; }
                    let asset = format!("icons/{}.svg", kebab(&name));
                    if !matches!(AppAssets.load(&asset), Ok(Some(_))) { missing.push(name); }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "ícones sem SVG embutido (acrescentar em ExtraIcons): {missing:?}");
    }
}
