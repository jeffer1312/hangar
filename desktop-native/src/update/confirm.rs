//! Confirmação da atualização com os avisos e as novidades da versão nova, e "O que chegou para você" depois dela.

use std::{cell::Cell, path::PathBuf, rc::Rc, time::Instant};
use gpui_kit::{assets::IconName, component::{Sizable, WindowExt, button::*, dialog::DialogFooter, text::TextView}, prelude::FluentBuilder, *};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::{api::Api, app::chrome, i18n::{tr, tr_shared}, theme};

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct News {
    id: String,
    title: String,
    kind: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    platforms: String,
    #[serde(default)]
    try_it: Option<String>,
    #[serde(default)]
    animation: Option<String>,
}

#[derive(Clone, Deserialize)]
struct Warning { id: String, title: String, #[serde(default)] text: String }

#[derive(Deserialize)]
struct Wire { #[serde(default)] warnings: Vec<Warning>, #[serde(default)] news: Vec<News> }

/// O que o modal mostra além do texto de sempre. Servidor antigo (404) = tudo vazio, modal como antes.
#[derive(Clone, Default)]
pub(crate) struct Notes { version: Option<String>, warnings: Vec<Warning>, news: Vec<News>, failed: bool, server: String }

impl Notes {
    fn sections(&self) -> bool { self.failed || self.version.is_some() || !self.warnings.is_empty() || !self.news.is_empty() }
}

/// A rota dos avisos não busca no remoto: `procurar=1` atualiza o alvo antes. Devolve também o estado lido.
pub(crate) async fn load(api: Api) -> (Option<Value>, Notes) {
    let state = api.server_read(&["atualizacao"], &[("procurar", "1")], 150).await.ok();
    let version = state.as_ref().and_then(|s| {
        let v = &s["versao_legivel"];
        Some(format!("{} → {}", v["backend"].as_str()?, v["remoto"].as_str()?))
    });
    let mut notes = Notes { version, server: api.identity(), ..Notes::default() };
    match api.server_read(&["update", "notes"], &[], 20).await.map(serde_json::from_value::<Wire>) {
        Ok(Ok(wire)) => {
            notes.warnings = wire.warnings;
            notes.news = wire.news.into_iter().filter(|n| matches!(n.platforms.as_str(), "desktop" | "all")).collect();
        }
        // Servidor antigo: modal como antes, sem a linha da versão.
        Err(error) if error.status == Some(404) => notes.version = None,
        Ok(Err(_)) | Err(_) => notes.failed = true,
    }
    (state, notes)
}

/// Lê os avisos e as novidades fora da janela e devolve a `loaded` já nela, com o estado lido; quem chama limpa o
/// "carregando" e abre o modal ali.
pub(crate) fn load_then_open<T: 'static>(api: Api, runtime: &tokio::runtime::Runtime, window: &mut Window, cx: &mut Context<T>,
    loaded: impl FnOnce(&mut T, Option<Value>, Notes, &mut Window, &mut Context<T>) + 'static) {
    let task = runtime.spawn(load(api));
    let handle = window.window_handle();
    cx.spawn(async move |this, cx| {
        // A leitura que morreu no meio não pode virar "sem avisos": abre com a linha de falha.
        let (state, notes) = task.await.unwrap_or_else(|_| (None, Notes { failed: true, ..Notes::default() }));
        let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx| loaded(this, state, notes, window, cx)); });
    }).detach();
}

/// Modal "Atualizar?". `all`: a caixa "Atualizar todas as máquinas (n)". `act` roda ao confirmar, com a lista já guardada.
pub(crate) fn open(window: &mut Window, cx: &mut App, title: String, description: String, notes: Notes,
    all: Option<(String, Rc<Cell<bool>>)>, act: impl Fn(&mut Window, &mut App) + 'static) {
    let notes = Rc::new(notes);
    let understood = Rc::new(Cell::new(false));
    let fixes_open = Rc::new(Cell::new(false));
    let started = Instant::now();
    // Fotografar o modal já marcado e aberto, sem clicar.
    #[cfg(debug_assertions)]
    if std::env::var_os("HANGAR_NATIVE_PREVIEW_CHECKED").is_some() { understood.set(true); fixes_open.set(true); }
    let tall = notes.sections();
    let enabled = { let (understood, blocked) = (understood.clone(), !notes.warnings.is_empty()); Rc::new(move || !blocked || understood.get()) };
    let shown = notes.clone();
    let body = move |window: &mut Window, cx: &mut App| {
        let notes = &*shown;
        let max = window.viewport_size().height * 0.8 - px(110.);
        div().id("update-confirm-body").max_h(max).overflow_y_scroll().flex().flex_col().gap(px(12.))
            .child(div().flex().flex_col().gap(px(3.))
                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(title.clone()))
                .when_some(notes.version.clone(), |el, version| el.child(div().font_family(theme::MONO).text_size(px(11.5))
                    .text_color(theme::faint()).child(version))))
            .child(div().text_size(px(12.5)).line_height(px(18.)).text_color(theme::muted()).whitespace_normal().child(description.clone()))
            .when(notes.failed, |el| el.child(div().text_size(px(12.5)).text_color(theme::danger()).whitespace_normal()
                .child(tr_shared("update_notes_failed", &[]))))
            .when(!notes.warnings.is_empty(), |el| el.child(div().flex().flex_col().gap(px(7.)).px(px(12.)).py(px(11.)).rounded(px(10.))
                .border_1().border_color(palette::amber().alpha(0.28)).bg(palette::amber().alpha(0.08))
                .children(notes.warnings.iter().map(|w| div().flex().flex_col().gap(px(7.))
                    .child(div().text_size(px(12.8)).font_weight(FontWeight::SEMIBOLD).text_color(palette::amber_title()).whitespace_normal()
                        .child(format!("{}: {}", tr_shared("update_notes_important", &[]), w.title)))
                    // Um parágrafo por vez: a margem de parágrafo do markdown passa do respiro do bloco.
                    .children(w.text.split("\n\n").enumerate().map(|(i, paragraph)| div().text_size(px(12.)).line_height(px(18.))
                        .text_color(theme::body_text()).child(TextView::markdown(SharedString::from(format!("update-warning-{}-{i}", w.id)),
                            paragraph.to_owned()).selectable(true).scrollable(false))))))
                .child(div().pt(px(2.)).text_size(px(13.)).child(check("update-understood", tr_shared("update_notes_understood", &[]), understood.clone())))))
            .child(sections(tr_shared("update_notes_news", &[]), &notes.news, true, &fixes_open, started, window, cx))
            .when_some(all.clone(), |el, (label, value)| el.child(div().px(px(10.)).py(px(9.)).rounded(px(9.)).border_1()
                .border_color(palette::edge()).bg(palette::fill(0.04)).text_size(px(13.)).child(check("confirm-check", label, value))))
            .into_any_element()
    };
    let (width, height) = if tall { (px(440.), window.viewport_size().height * 0.8) } else { (px(360.), px(150.)) };
    chrome::confirm_dialog(window, cx, width, height, None, Rc::new(body), enabled, tr("update_confirm_ok"), None,
        |dialog| dialog.p(px(18.)).rounded(px(14.)), move |window, cx| {
            // Sem novidades, apaga a lista de uma confirmação anterior que não chegou a terminar.
            if notes.news.is_empty() { let _ = take_pending(); } else { save_pending(&notes.server, &notes.news); }
            act(window, cx);
            true
        });
}

/// Cores dos dois modais que o tema não tem (bloco âmbar e selo violeta), e as do tema que eles usam com outra opacidade.
mod palette {
    use gpui_kit::*;
    use crate::theme;

    pub fn amber() -> Hsla { if theme::is_dark() { rgb(0xe8aa3c).into() } else { theme::warning() } }
    pub fn amber_title() -> Hsla { if theme::is_dark() { rgb(0xf0c46a).into() } else { theme::warning_text() } }
    pub fn badge() -> (Hsla, Hsla) {
        if theme::is_dark() { (rgb(0xb9a6f0).into(), rgb(0xc9baf5).into()) } else { (theme::accent(), theme::accent_text()) }
    }
    /// A linha do tema na opacidade das superfícies do mock: cartões e a linha "todas as máquinas".
    pub fn fill(alpha: f32) -> Hsla { theme::border().alpha(alpha) }
    pub fn edge() -> Hsla { theme::border().alpha(0.06) }
}

/// Caixa de marcar sem caixa própria: o bloco em volta já a destaca.
fn check(id: &'static str, label: String, value: Rc<Cell<bool>>) -> impl IntoElement {
    gpui_kit::component::checkbox::Checkbox::new(id).small().label(label).checked(value.get())
        .on_click(move |checked: &bool, window, _| { value.set(*checked); window.refresh(); })
}

/// Rótulo em caixa alta, os cartões (novo e melhoria) e as correções recolhidas: o mesmo bloco nos dois modais.
/// `compact`: cartão deitado, animação à esquerda (o "Atualizar?"); sem ele, em pé com o "Experimente".
fn sections(label: String, news: &[News], compact: bool, fixes_open: &Rc<Cell<bool>>, started: Instant, window: &mut Window, cx: &mut App) -> Div {
    let (fixes, cards): (Vec<&News>, Vec<&News>) = news.iter().partition(|n| n.kind == "fix");
    let open = fixes_open.get();
    let toggle = fixes_open.clone();
    div().flex().flex_col().gap(px(12.))
        .when(!cards.is_empty(), |el| el.child(div().mt(px(2.)).text_size(px(10.5)).text_color(theme::faint()).child(label.to_uppercase())))
        .children(cards.into_iter().map(|item| card(item, compact, started, window, cx)))
        .when(!fixes.is_empty(), |el| el.child(div().flex().flex_col().gap(px(6.))
            .child(div().flex().child(Button::new("update-fixes").ghost().xsmall().ml(px(-6.)).text_size(px(12.)).text_color(theme::muted())
                .icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                .label(tr_shared("update_notes_fixes", &[("n", &fixes.len().to_string())]))
                .on_click(move |_, window, _| { toggle.set(!toggle.get()); window.refresh(); })))
            .when(open, |el| el.child(div().pl(px(16.)).flex().flex_col().text_size(px(12.)).line_height(px(18.6)).text_color(theme::body_text())
                .children(fixes.into_iter().map(|item| div().flex().gap(px(6.)).child("•")
                    .child(div().flex_1().min_w_0().whitespace_normal().child(item.title.clone()))))))))
}

fn card(item: &News, compact: bool, started: Instant, window: &mut Window, cx: &mut App) -> Div {
    let animation = item.animation.as_deref().and_then(crate::news::animation);
    let (badge_hue, badge_text) = palette::badge();
    let kind = match item.kind.as_str() { "new" => Some("update_news_kind_new"), "improvement" => Some("update_news_kind_improvement"), _ => None };
    let heading = div().flex().flex_wrap().items_center().gap_x(px(5.)).gap_y(px(2.))
        .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(item.title.clone()))
        .when_some(kind, |el, key| el.child(div().px(px(6.)).py(px(1.)).rounded_full().bg(badge_hue.alpha(0.18))
            .text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(badge_text).child(tr_shared(key, &[]))));
    let text = (!item.text.is_empty()).then(|| div().text_size(px(11.8)).line_height(px(16.5)).text_color(theme::muted())
        .child(TextView::markdown(SharedString::from(format!("update-news-{}", item.id)), item.text.clone()).selectable(true).scrollable(false)));
    let base = div().rounded(px(10.)).border_1().border_color(palette::edge()).bg(palette::fill(0.035));
    if compact {
        return base.flex().items_center().gap(px(12.)).p(px(10.))
            .when_some(animation, |el, draw| el.child(div().flex_shrink_0().w(px(160.)).child(draw(started, px(160.), window, cx))))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.)).child(heading).children(text));
    }
    base.flex().flex_col().gap(px(10.)).p(px(12.))
        .when_some(animation, |el, draw| el.child(draw(started, px(500.), window, cx)))
        .child(div().flex().flex_col().gap(px(3.)).child(heading).children(text))
        .when_some(item.try_it.clone(), |el, text| el.child(div().text_size(px(11.5)).text_color(badge_text).whitespace_normal()
            .child(tr_shared("update_notes_try_it", &[("text", &text)]))))
}

/// "O que chegou para você": uma vez, com o que o "Atualizar?" guardou.
fn open_whats_new(items: Vec<News>, window: &mut Window, cx: &mut App) {
    let fixes_open = Rc::new(Cell::new(false));
    let started = Instant::now();
    window.open_dialog(cx, move |dialog, window, cx| {
        let max = window.viewport_size().height * 0.8 - px(110.);
        let top = (window.viewport_size().height * 0.1).max(px(24.));
        crate::app::popup::dialog(dialog).w(px(560.)).p(px(18.)).rounded(px(14.)).close_button(false).margin_top(top)
            .child(div().id("whats-new-body").max_h(max).overflow_y_scroll()
                .child(sections(tr_shared("update_whats_new_title", &[]), &items, false, &fixes_open, started, window, cx)))
            .footer(DialogFooter::new().child(chrome::light_button("whats-new-close", true, cx).label(tr("close"))
                .on_click(|_, window, cx| window.close_dialog(cx))))
    });
}

#[derive(Deserialize, Serialize)]
struct Pending { from_native: String, server: String, items: Vec<News> }

fn pending_path() -> Option<PathBuf> { Some(crate::appearance::dir()?.join("novidades-pendentes.json")) }

fn save_pending(server: &str, items: &[News]) {
    let result = (|| {
        let path = pending_path().ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?;
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        let tmp = path.with_extension("json.tmp");
        let pending = Pending { from_native: super::CURRENT.to_owned(), server: server.to_owned(), items: items.to_vec() };
        std::fs::write(&tmp, serde_json::to_vec(&pending).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    })();
    // Sem a lista guardada, só a tela de depois não aparece.
    if let Err(error) = result { crate::log_line(&format!("novidades pendentes não gravaram: {error}")); }
}

/// Lê e apaga: a tela aparece uma vez só, mesmo que feche sem ver.
fn take_pending() -> Option<Pending> {
    let path = pending_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice(&bytes).ok()
}

/// Na abertura: com o mesmo binário de quando foi guardada, a atualização não chegou a trocar o app.
fn shows_at_startup(pending: &Pending, current: &str) -> bool { pending.from_native != current && !pending.items.is_empty() }

/// Mostra o que o "Atualizar?" guardou. `require_new_binary` na abertura do app; sem ele, o fluxo terminou neste
/// processo sem trocar o binário.
pub(crate) fn show_pending(require_new_binary: bool, window: &mut Window, cx: &mut App) {
    let shows = |p: &Pending| if require_new_binary { shows_at_startup(p, super::CURRENT) } else { !p.items.is_empty() };
    if let Some(pending) = take_pending().filter(shows) { open_whats_new(pending.items, window, cx); }
}

/// `HANGAR_NATIVE_PREVIEW=update-confirm` e `=whats-new` (build de debug): os dois modais com uma amostra fixa.
#[cfg(debug_assertions)]
pub(crate) fn open_preview(whats_new: bool, window: &mut Window, cx: &mut App) {
    let news = |id: &str, title: &str, kind: &str, text: &str, try_it: Option<&str>, animation: Option<&str>| News { id: id.into(),
        title: title.into(), kind: kind.into(), text: text.into(), platforms: "desktop".into(), try_it: try_it.map(Into::into),
        animation: animation.map(Into::into) };
    let items = vec![
        news("historico", "Histórico do que você digitou", "new",
            "A seta para cima no compositor traz de volta o que você já mandou nesta conversa, do mais recente para o mais antigo.",
            Some("↑ em qualquer conversa"), Some("historico-compositor")),
        news("colar-windows", "Texto colado com várias linhas no Windows chega ao Claude", "fix", "", None, None),
        news("windows-passo", "O backend no Windows não cai mais ao subir com passo pendente", "fix", "", None, None),
        news("worktrees", "A limpeza de worktrees reconhece as já mescladas", "fix", "", None, None),
    ];
    if whats_new { return open_whats_new(items, window, cx); }
    let notes = Notes { version: Some("0.1.0.8242 → 0.1.0.8311".into()), news: items, server: "preview".into(), failed: false,
        warnings: vec![Warning { id: "pacote".into(), title: "o Hangar passa a se atualizar por pacote".into(),
            text: "Depois desta atualização ele roda de uma versão pronta em `~/.hangar/current`, sem `git pull`, e volta sozinho para a \
                anterior se a nova não subir.\n\nO seu checkout continua no disco, mas não é mais o que roda. Para editar o código ou abrir \
                PR, ligue **Modo desenvolvedor** nas configurações do servidor.".into() }] };
    let all = Some((tr_shared("update_all_button", &[("n", "4")]), Rc::new(Cell::new(true))));
    open(window, cx, tr("app_update_confirm_server_title"), tr("app_update_confirm_server_desc"), notes, all, |_, _| {});
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn startup_shows_only_after_the_binary_changed() {
        let pending = |from: &str, items: usize| Pending { from_native: from.into(), server: "s".into(), items: (0..items).map(|i| News {
            id: i.to_string(), title: "t".into(), kind: "new".into(), text: String::new(), platforms: "all".into(), try_it: None, animation: None }).collect() };
        assert!(shows_at_startup(&pending("1", 1), "2"));
        assert!(!shows_at_startup(&pending("2", 1), "2"));
        assert!(!shows_at_startup(&pending("1", 0), "2"));
    }
}
