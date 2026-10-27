//! Confirmação da atualização com os avisos e as novidades da versão nova, e "O que chegou para você" depois dela.

use std::{cell::Cell, path::PathBuf, rc::Rc, time::Instant};
use gpui_kit::{assets::IconName, component::{Sizable, WindowExt, button::*, dialog::DialogFooter, radio::Radio, text::TextView}, prelude::FluentBuilder, *};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::{api::{Api, Failure}, app::chrome, i18n::{tr, tr_shared}, theme};

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
struct Warning {
    id: String,
    title: String,
    #[serde(default)]
    text: String,
    /// `"mode"`: o aviso da migração, que pede como a máquina vai rodar o Hangar.
    #[serde(default)]
    choice: Option<String>,
}

#[derive(Deserialize)]
struct Wire { #[serde(default)] warnings: Vec<Warning>, #[serde(default)] news: Vec<News> }

/// O que o modal mostra além do texto de sempre. Servidor antigo (404) = tudo vazio, modal como antes. `label`: o nome da
/// máquina quando o modal junta os avisos de várias ("Atualizar todas").
#[derive(Clone, Default)]
pub(crate) struct Notes { version: Option<String>, warnings: Vec<Warning>, news: Vec<News>, failed: bool, server: String, label: Option<String> }

impl Notes {
    fn sections(&self) -> bool { self.failed || self.version.is_some() || !self.warnings.is_empty() || !self.news.is_empty() }

    /// A identidade (`Api::identity`) da máquina destes avisos.
    pub(crate) fn server(&self) -> &str { &self.server }

    pub(crate) fn labeled(self, label: String) -> Self { Self { label: Some(label), ..self } }

    /// Máquina do lote sem os avisos lidos: fica de fora, com a linha de falha.
    pub(crate) fn unread(server: String, label: String) -> Self { Self { server, failed: true, ..Self::default() }.labeled(label) }

    /// A leitura falhou numa máquina do lote: ela fica de fora, com o motivo no modal.
    pub(crate) fn excluded(&self) -> bool { self.failed && self.label.is_some() }

    fn asks_mode(&self) -> bool { self.warnings.iter().any(|w| w.choice.as_deref() == Some("mode")) }

    /// O que o OK aceita nesta máquina: os avisos mostrados e, se um deles pede, o modo escolhido.
    fn acceptance(&self, developer: bool) -> Acceptance {
        Acceptance { ids: self.warnings.iter().map(|w| w.id.clone()).collect(),
            mode: self.asks_mode().then_some(if developer { "developer" } else { "package" }) }
    }
}

/// Os avisos que a pessoa aceitou numa máquina e a escolha do modo, quando um deles a pede. Vazio = nada a aceitar.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Acceptance { pub(crate) ids: Vec<String>, pub(crate) mode: Option<&'static str> }

/// Corpo da rota dos avisos: `Ok(None)` é o servidor anterior a ela (404), sem avisos.
fn parse_notes(read: Result<Value, Failure>) -> Result<Option<Wire>, ()> {
    match read.map(serde_json::from_value::<Wire>) {
        Ok(Ok(wire)) => Ok(Some(wire)),
        Err(error) if error.status == Some(404) => Ok(None),
        Ok(Err(_)) | Err(_) => Err(()),
    }
}

/// Os avisos de uma máquina do lote, com o nome dela; a versão e as novidades do modal são as do servidor desta máquina.
pub(crate) async fn load_warnings(api: Result<Api, Failure>, server: String, label: String) -> Notes {
    let mut notes = Notes { server, ..Notes::default() }.labeled(label);
    let read = match api { Ok(api) => api.server_read(&["update", "notes"], &[], 20).await, Err(error) => Err(error) };
    match parse_notes(read) {
        Ok(wire) => notes.warnings = wire.map(|w| w.warnings).unwrap_or_default(),
        Err(()) => notes.failed = true,
    }
    notes
}

/// O passo depois do aceite. `reply`: a resposta do `POST /api/update/accept` (nenhuma = nada a aceitar).
#[derive(Debug, PartialEq)]
enum AfterAccept { Follow, Start, Nothing, Failed(String) }

/// O aceite que lançou a migração (ou a troca de modo) é seguido como a atualização; sem lançar, só uma atualização de
/// verdade (`real`) chama o `iniciar`. Servidor sem a rota (404) segue como antes do aceite.
fn after_accept(reply: Option<&Result<Value, Failure>>, real: bool) -> AfterAccept {
    match reply {
        Some(Ok(value)) if value["started"].as_bool() == Some(true) => AfterAccept::Follow,
        Some(Err(error)) if error.status != Some(404) =>
            AfterAccept::Failed(tr("update_accept_failed").replace("{reason}", tr(&error.detail).trim_end_matches('.'))),
        _ if real => AfterAccept::Start,
        _ => AfterAccept::Nothing,
    }
}

/// `Started`: a resposta do pedido que lançou algo (o aceite ou o `iniciar`), tratada como a do `iniciar`.
pub(crate) enum Launch { Started(Result<Value, Failure>), Nothing, AcceptFailed(String) }

/// Aceita os avisos mostrados e, se o aceite não lançou nada, inicia a atualização quando há uma de verdade.
pub(crate) async fn accept_then_start(api: &Api, acceptance: &Acceptance, real: bool) -> Launch {
    let reply = if acceptance.ids.is_empty() { None } else {
        let mut body = serde_json::json!({"ids": acceptance.ids});
        if let Some(mode) = acceptance.mode { body["mode"] = mode.into(); }
        Some(api.server_send(reqwest::Method::POST, &["update", "accept"], Some(body), 30).await)
    };
    match after_accept(reply.as_ref(), real) {
        AfterAccept::Follow => Launch::Started(reply.unwrap_or(Ok(Value::Null))),
        AfterAccept::Start => Launch::Started(api.server_post(&["atualizacao", "iniciar"], 30).await),
        AfterAccept::Nothing => Launch::Nothing,
        AfterAccept::Failed(text) => Launch::AcceptFailed(text),
    }
}

/// A rota dos avisos não busca no remoto: `procurar=1` atualiza o alvo antes. Devolve também o estado lido.
pub(crate) async fn load(api: Api) -> (Option<Value>, Notes) {
    let state = api.server_read(&["atualizacao"], &[("procurar", "1")], 150).await.ok();
    let version = state.as_ref().and_then(|s| {
        let v = &s["versao_legivel"];
        Some(format!("{} → {}", v["backend"].as_str()?, v["remoto"].as_str()?))
    });
    let mut notes = Notes { version, server: api.identity(), ..Notes::default() };
    match parse_notes(api.server_read(&["update", "notes"], &[], 20).await) {
        Ok(Some(wire)) => {
            notes.warnings = wire.warnings;
            notes.news = wire.news.into_iter().filter(|n| matches!(n.platforms.as_str(), "desktop" | "all")).collect();
        }
        // Servidor antigo: modal como antes, sem a linha da versão.
        Ok(None) => notes.version = None,
        Err(()) => notes.failed = true,
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

/// Modal "Atualizar?". `notes`: uma por máquina; a primeira dá a versão e as novidades, e as com `label` aparecem com o
/// nome. `all`: a caixa "Atualizar todas as máquinas (n)". `act` roda ao confirmar, com a lista já guardada e o aceite de
/// cada máquina, na ordem de `notes`.
pub(crate) fn open(window: &mut Window, cx: &mut App, title: String, description: String, notes: Vec<Notes>,
    all: Option<(String, Rc<Cell<bool>>)>, act: impl Fn(Vec<Acceptance>, &mut Window, &mut App) + 'static) {
    let notes = Rc::new(if notes.is_empty() { vec![Notes::default()] } else { notes });
    // A escolha começa no pacote, o recomendado, mesmo em quem já roda do checkout.
    let developer: Rc<Vec<Rc<Cell<bool>>>> = Rc::new(notes.iter().map(|_| Rc::new(Cell::new(false))).collect());
    let understood = Rc::new(Cell::new(false));
    let fixes_open = Rc::new(Cell::new(false));
    let started = Instant::now();
    // Fotografar o modal já marcado e aberto, sem clicar.
    #[cfg(debug_assertions)]
    if std::env::var_os("HANGAR_NATIVE_PREVIEW_CHECKED").is_some() { understood.set(true); fixes_open.set(true); }
    let tall = notes.iter().any(Notes::sections);
    let warned = notes.iter().any(|n| !n.warnings.is_empty());
    let enabled = { let understood = understood.clone(); Rc::new(move || !warned || understood.get()) };
    let (shown, choices) = (notes.clone(), developer.clone());
    let body = move |window: &mut Window, cx: &mut App| {
        let primary = &shown[0];
        let max = window.viewport_size().height * 0.8 - px(110.);
        let failed = shown.iter().filter(|n| n.excluded()).filter_map(|n| n.label.as_deref()).map(|label| div().text_size(px(12.5))
            .text_color(theme::danger()).whitespace_normal().child(tr("update_all_warnings_failed").replace("{server}", label)));
        let groups = shown.iter().enumerate().filter(|(_, n)| !n.warnings.is_empty()).map(|(g, n)| div().flex().flex_col().gap(px(7.))
            .when_some(n.label.as_deref(), |el, label| el.child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text()).whitespace_normal().child(tr("update_all_warnings_from").replace("{server}", label))))
            .children(n.warnings.iter().map(|w| div().flex().flex_col().gap(px(7.))
                .child(div().text_size(px(12.8)).font_weight(FontWeight::SEMIBOLD).text_color(palette::amber_title()).whitespace_normal()
                    .child(format!("{}: {}", tr_shared("update_notes_important", &[]), w.title)))
                // Um parágrafo por vez: a margem de parágrafo do markdown passa do respiro do bloco.
                .children(w.text.split("\n\n").enumerate().map(|(i, paragraph)| div().text_size(px(12.)).line_height(px(18.))
                    .text_color(theme::body_text()).child(TextView::markdown(SharedString::from(format!("update-warning-{g}-{}-{i}", w.id)),
                        paragraph.to_owned()).selectable(true).scrollable(false))))
                .when(w.choice.as_deref() == Some("mode"), |el| el.child(mode_choice(g, choices[g].clone()))))));
        div().id("update-confirm-body").max_h(max).overflow_y_scroll().flex().flex_col().gap(px(12.))
            .child(div().flex().flex_col().gap(px(3.))
                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(title.clone()))
                .when_some(primary.version.clone(), |el, version| el.child(div().font_family(theme::MONO).text_size(px(11.5))
                    .text_color(theme::faint()).child(version))))
            .child(div().text_size(px(12.5)).line_height(px(18.)).text_color(theme::muted()).whitespace_normal().child(description.clone()))
            .when(primary.failed && primary.label.is_none(), |el| el.child(div().text_size(px(12.5)).text_color(theme::danger())
                .whitespace_normal().child(tr_shared("update_notes_failed", &[]))))
            .children(failed)
            .when(warned, |el| el.child(div().flex().flex_col().gap(px(7.)).px(px(12.)).py(px(11.)).rounded(px(10.))
                .border_1().border_color(palette::amber().alpha(0.28)).bg(palette::amber().alpha(0.08))
                .children(groups)
                // Em caixa própria e no tamanho normal: pequena, dentro do bloco âmbar, ela não parecia uma caixa de marcar.
                .child(div().mt(px(4.)).px(px(12.)).py(px(10.)).rounded(px(8.)).border_1().border_color(palette::amber().alpha(0.6))
                    .bg(theme::inset()).text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD)
                    .child(gpui_kit::component::checkbox::Checkbox::new("update-understood").label(tr_shared("update_notes_understood", &[]))
                        .checked(understood.get()).on_click({
                            let value = understood.clone();
                            move |checked: &bool, window, _| { value.set(*checked); window.refresh(); }
                        })))))
            .child(sections(tr_shared("update_notes_news", &[]), &primary.news, true, &fixes_open, started, window, cx))
            .when_some(all.clone(), |el, (label, value)| el.child(div().px(px(10.)).py(px(9.)).rounded(px(9.)).border_1()
                .border_color(palette::edge()).bg(palette::fill(0.04)).text_size(px(13.)).child(check("confirm-check", label, value))))
            .into_any_element()
    };
    // Largo o bastante para o aviso e a escolha não virarem uma coluna estreita; nunca além da janela.
    let (width, height) = if tall { (px(620.).min(window.viewport_size().width * 0.92), window.viewport_size().height * 0.8) } else { (px(360.), px(150.)) };
    chrome::confirm_dialog(window, cx, width, height, None, Rc::new(body), enabled, tr("update_confirm_ok"), None,
        |dialog| dialog.p(px(18.)).rounded(px(14.)), move |window, cx| {
            let primary = &notes[0];
            // Sem novidades, apaga a lista de uma confirmação anterior que não chegou a terminar.
            if primary.news.is_empty() { let _ = take_pending(); } else { save_pending(&primary.server, &primary.news); }
            act(notes.iter().zip(developer.iter()).map(|(n, d)| n.acceptance(d.get())).collect(), window, cx);
            true
        });
}

/// Como a máquina vai rodar o Hangar, embaixo do aviso da migração.
fn mode_choice(group: usize, developer: Rc<Cell<bool>>) -> Div {
    let option = |dev: bool, title: &str, desc: &str| {
        let value = developer.clone();
        Radio::new(SharedString::from(format!("update-choice-{group}-{}", if dev { "developer" } else { "package" }))).small()
            .label(tr(title)).checked(developer.get() == dev)
            .child(div().text_size(px(11.5)).line_height(px(16.)).text_color(theme::muted()).whitespace_normal().child(tr(desc)))
            .on_click(move |_, window, _| { value.set(dev); window.refresh(); })
    };
    div().flex().flex_col().gap(px(8.)).pt(px(2.))
        .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(tr("update_choice_title")))
        .child(option(false, "update_choice_package", "update_choice_package_desc"))
        .child(option(true, "update_choice_developer", "update_choice_developer_desc"))
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
    let notes = Notes { version: Some("0.1.0.8242 → 0.1.0.8311".into()), news: items, server: "preview".into(), failed: false, label: None,
        warnings: vec![Warning { id: "pacote".into(), title: "o Hangar passa a se atualizar por pacote".into(), choice: Some("mode".into()),
            text: "Depois desta atualização ele roda de uma versão pronta em `~/.hangar/current`, sem `git pull`, e volta sozinho para a \
                anterior se a nova não subir.\n\nO seu checkout continua no disco, mas não é mais o que roda. Para editar o código ou abrir \
                PR, ligue **Modo desenvolvedor** nas configurações do servidor.".into() }] };
    let all = Some((tr_shared("update_all_button", &[("n", "4")]), Rc::new(Cell::new(true))));
    open(window, cx, tr("app_update_confirm_server_title"), tr("app_update_confirm_server_desc"), vec![notes], all, |_, _, _| {});
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

    fn refused(status: Option<u16>, detail: &str) -> Failure {
        Failure { status, detail: detail.into(), retry_after: None, uncertain: false, code: None }
    }

    #[test]
    fn notes_read_the_mode_choice_and_accept_every_shown_warning() {
        let body = serde_json::json!({"target": "x", "news": [], "warnings": [
            {"id": "2026-10-24-pacote-por-versao", "title": "t", "text": "x", "choice": "mode", "mode": null},
            {"id": "outro", "title": "t"}]});
        let wire = parse_notes(Ok(body)).unwrap().unwrap();
        let notes = Notes { warnings: wire.warnings, ..Notes::default() };
        assert_eq!(notes.acceptance(false), Acceptance { ids: vec!["2026-10-24-pacote-por-versao".into(), "outro".into()], mode: Some("package") });
        assert_eq!(notes.acceptance(true).mode, Some("developer"));
        let plain = Notes { warnings: parse_notes(Ok(serde_json::json!({"warnings": [{"id": "a", "title": "t"}]}))).unwrap().unwrap().warnings,
            ..Notes::default() };
        assert_eq!(plain.acceptance(true), Acceptance { ids: vec!["a".into()], mode: None }, "sem escolha, sem modo");
        assert_eq!(Notes::default().acceptance(false), Acceptance::default(), "sem avisos, nada a aceitar");
        assert!(parse_notes(Err(refused(Some(404), "Not Found"))).unwrap().is_none(), "servidor antigo: sem avisos");
        assert!(parse_notes(Err(refused(Some(503), "fixture"))).is_err());
        assert!(parse_notes(Err(refused(None, "network_error"))).is_err());
    }

    #[test]
    fn accept_follows_what_it_launched_and_starts_only_a_real_update() {
        let launched = Ok(serde_json::json!({"ok": true, "started": true, "pid": 42}));
        let stored = Ok(serde_json::json!({"ok": true, "started": false}));
        assert_eq!(after_accept(Some(&launched), false), AfterAccept::Follow);
        assert_eq!(after_accept(Some(&launched), true), AfterAccept::Follow, "a migração já roda: não chama o iniciar");
        assert_eq!(after_accept(Some(&stored), true), AfterAccept::Start);
        assert_eq!(after_accept(Some(&stored), false), AfterAccept::Nothing, "só o aviso estava pendente");
        assert_eq!(after_accept(None, true), AfterAccept::Start);
        assert_eq!(after_accept(None, false), AfterAccept::Nothing);
        assert_eq!(after_accept(Some(&Err(refused(Some(404), "Not Found"))), true), AfterAccept::Start, "servidor antigo segue como antes");
        crate::i18n::english_in_this_thread(false);
        assert_eq!(after_accept(Some(&Err(refused(Some(500), "fixture-reason."))), true),
            AfterAccept::Failed("O aceite do aviso não foi gravado: fixture-reason".into()));
        assert!(matches!(after_accept(Some(&Err(refused(None, "delivery_uncertain"))), true), AfterAccept::Failed(_)));
    }
}
