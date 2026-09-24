mod api;
mod app;
mod chat;
mod composer;
mod conversation;
mod delivery;
mod i18n;
mod interaction;
mod media;
mod status;
mod theme;
use gpui_kit::{component::{Root, Theme, ThemeMode}, *};
use std::{borrow::Cow, sync::Arc};

gpui_kit::assets::icon_assets!(ExtraIcons, [ArrowUp, GitBranch, RotateCcwClock, Paperclip, Plug, SquareSlash]);

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

fn main() {
    let runtime = Arc::new(tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("async runtime"));
    gpui_kit::application().with_assets(AppAssets).run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1180.0), px(800.0)), cx))),
            app_id: Some("com.hangar.native".into()),
            titlebar: Some(TitlebarOptions { title: Some("Hangar Native — Experimental".into()), ..Default::default() }),
            window_background: if cfg!(target_os = "linux") { WindowBackgroundAppearance::Transparent } else { WindowBackgroundAppearance::Opaque },
            ..Default::default()
        }, |window, cx| {
            let view = cx.new(|cx| app::Hangar::new(runtime.clone(), window, cx));
            cx.new(|cx| {
                let root = Root::new(view, window, cx);
                if cfg!(target_os = "linux") { root.bg(rgba(0x00000000)) } else { root }
            })
        }).expect("open native window");
        cx.on_window_closed(|cx, _| { if cx.windows().is_empty() { cx.quit(); } }).detach();
        cx.activate(true);
    });
}
