mod api;
mod app;
mod appearance;
mod chat;
mod composer;
mod conversation;
mod delivery;
mod i18n;
mod interaction;
mod media;
mod mend;
mod status;
mod tables;
mod theme;
use gpui_kit::{component::{Root, Theme, ThemeMode}, *};
use std::{borrow::Cow, sync::Arc};

gpui_kit::assets::icon_assets!(ExtraIcons, [ArrowUp, GitBranch, RotateCcwClock, Paperclip, Plug, SquareSlash,
    Activity, Contrast, Droplet, Image, Keyboard, Layers, List, Mic, Monitor, RefreshCw, Server, SlidersHorizontal, Type, Users,
    SquarePen, FilePlus, Wrench, Circle, CircleDashed, ChartColumn, Table, ListChecks, Download, Clock, Languages, Banknote]);

pub const HANGAR_MARK: &str = "brand/hangar-mark.svg";

struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == HANGAR_MARK { return Ok(Some(Cow::Borrowed(include_bytes!("../assets/hangar-mark.svg")))); }
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

const FONTS: [&[u8]; 5] = [
    include_bytes!("../assets/fonts/Geist-Regular.ttf"), include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"), include_bytes!("../assets/fonts/Geist-Bold.ttf"),
    include_bytes!("../assets/fonts/Geist-Italic.ttf"),
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

fn main() {
    let runtime = Arc::new(tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("async runtime"));
    // Lida antes da primeira janela: o tema já nasce na escolha salva. Falha de leitura abre no padrão e aparece na tela.
    let appearance_error = match appearance::load() { Ok(value) => { appearance::set(value); None } Err(e) => Some(e) };
    i18n::set_language(appearance::get().language);
    gpui_kit::application().with_assets(AppAssets).run(move |cx| {
        gpui_kit::init(cx);
        // Só para provar: HANGAR_NATIVE_REDUCE_MOTION=1 liga o movimento reduzido onde o sistema não informa.
        if std::env::var_os("HANGAR_NATIVE_REDUCE_MOTION").is_some() { cx.set_reduce_motion(true); }
        Theme::change(ThemeMode::Dark, None, cx);
        if let Err(error) = cx.text_system().add_fonts(FONTS.iter().map(|bytes| Cow::Borrowed(*bytes)).collect()) {
            eprintln!("fonte embutida recusada: {error}");
        }
        theme::sync_kit(None, cx);
        cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, window_size(), cx))),
            app_id: Some("com.hangar.native".into()),
            titlebar: Some(TitlebarOptions { title: Some("Hangar Native — Experimental".into()), ..Default::default() }),
            window_background: if cfg!(target_os = "linux") { WindowBackgroundAppearance::Transparent } else { WindowBackgroundAppearance::Opaque },
            ..Default::default()
        }, |window, cx| {
            let view = cx.new(|cx| app::Hangar::new(runtime.clone(), appearance_error.clone(), window, cx));
            cx.new(|cx| {
                let root = Root::new(view, window, cx);
                if cfg!(target_os = "linux") { root.bg(rgba(0x00000000)) } else { root }
            })
        }).expect("open native window");
        cx.on_window_closed(|cx, _| { if cx.windows().is_empty() { cx.quit(); } }).detach();
        cx.activate(true);
    });
}
