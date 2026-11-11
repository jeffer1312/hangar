//! Confirmação da atualização com os avisos e as novidades da versão nova, e "O que chegou para você" depois dela.

use std::{cell::Cell, path::Path, rc::Rc, time::Instant};
use gpui_kit::{assets::IconName, component::{Sizable, WindowExt, button::*, dialog::DialogFooter, radio::Radio, text::TextView}, prelude::FluentBuilder, *};
use serde::Deserialize;
use serde_json::Value;
use crate::{api::{Api, Failure}, app::chrome, i18n::{tr, tr_shared}, theme};

#[derive(Clone, Debug, Deserialize, PartialEq)]
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
    /// A peça da tela onde a novidade mora (`tour::anchors`). Servidor anterior ao campo não o manda.
    #[serde(default)]
    anchor: Option<String>,
}

/// "Mostrar no app": aponta a peça da novidade com o balão do tour, já com o modal fechado.
type ShowInApp = Rc<dyn Fn(crate::tour::Note, &mut Window, &mut App)>;

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
struct Wire { #[serde(default)] warnings: Vec<Warning>, #[serde(default)] news: Vec<News>, #[serde(default)] incomplete: bool }

/// O que o modal mostra além do texto de sempre. Servidor antigo (404) = tudo vazio, modal como antes. `label`: o nome da
/// máquina quando o modal junta os avisos de várias ("Atualizar todas").
#[derive(Clone, Default)]
/// `incomplete`: o servidor não leu os avisos da versão nova (só o da migração, que é local); aceita-se o que veio, e
/// nada de `iniciar` por cima de avisos que ninguém viu.
pub(crate) struct Notes { version: Option<String>, warnings: Vec<Warning>, news: Vec<News>, failed: bool, incomplete: bool, server: String,
    label: Option<String> }

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
            mode: self.asks_mode().then_some(if developer { "developer" } else { "package" }), incomplete: self.incomplete }
    }
}

/// Os avisos que a pessoa aceitou numa máquina e a escolha do modo, quando um deles a pede. Vazio = nada a aceitar.
/// `incomplete`: as notas vieram sem os avisos da versão nova; o aceite não é seguido de `iniciar`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Acceptance { pub(crate) ids: Vec<String>, pub(crate) mode: Option<&'static str>, pub(crate) incomplete: bool }

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
    let read = match api {
        Ok(api) => {
            // Busca o alvo antes, como na desta máquina: sem isso os avisos podem ser os da versão anterior. Falhou, segue
            // com o alvo que ela tem; a releitura antes de iniciar (`unseen_warnings`) barra o que mudou.
            if let Err(error) = api.server_read(&["atualizacao"], &[("procurar", "1")], 120).await {
                eprintln!("update warnings: {} target search failed: {}", notes.server, tr(&error.detail));
            }
            api.server_read(&["update", "notes"], &[], 20).await
        }
        Err(error) => Err(error),
    };
    match parse_notes(read) {
        Ok(wire) => if let Some(wire) = wire { notes.warnings = wire.warnings; notes.incomplete = wire.incomplete; },
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

/// Aviso do alvo de agora que o modal não mostrou (versão publicada depois dele): recusa, nada é aceito nem iniciado.
async fn unseen_warnings(api: &Api, acceptance: &Acceptance) -> Option<String> {
    match parse_notes(api.server_read(&["update", "notes"], &[], 20).await) {
        Ok(Some(wire)) if wire.warnings.iter().any(|w| !acceptance.ids.contains(&w.id)) => Some(tr("update_new_warnings")),
        // O modal leu os avisos da versão nova e agora não deu: não há como saber se ela trouxe outro.
        Ok(Some(wire)) if wire.incomplete && !acceptance.incomplete => Some(tr_shared("update_notes_failed", &[])),
        Ok(_) => None,
        Err(()) => Some(tr_shared("update_notes_failed", &[])),
    }
}

/// `Started`: a resposta do pedido que lançou algo (o aceite ou o `iniciar`), tratada como a do `iniciar`.
pub(crate) enum Launch { Started(Result<Value, Failure>), Nothing, AcceptFailed(String) }

/// Aceita os avisos mostrados e, se o aceite não lançou nada, inicia a atualização quando há uma de verdade.
pub(crate) async fn accept_then_start(api: &Api, acceptance: &Acceptance, real: bool) -> Launch {
    if let Some(text) = unseen_warnings(api, acceptance).await { return Launch::AcceptFailed(text); }
    let reply =if acceptance.ids.is_empty() { None } else {
        let mut body = serde_json::json!({"ids": acceptance.ids});
        if let Some(mode) = acceptance.mode { body["mode"] = mode.into(); }
        Some(api.server_send(reqwest::Method::POST, &["update", "accept"], Some(body), 30).await)
    };
    match after_accept(reply.as_ref(), real) {
        AfterAccept::Follow => Launch::Started(reply.unwrap_or(Ok(Value::Null))),
        // Os avisos da versão nova não foram lidos: iniciar por cima deles seria aceitá-los sem mostrar.
        AfterAccept::Start if acceptance.incomplete => Launch::AcceptFailed(tr("update_accept_incomplete")),
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
            notes.incomplete = wire.incomplete;
            notes.news = wire.news.into_iter().filter(for_desktop).collect();
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
            .child(sections(tr_shared("update_notes_news", &[]), &primary.news, None, &fixes_open, None, started, window, cx))
            .when_some(all.clone(), |el, (label, value)| el.child(div().px(px(10.)).py(px(9.)).rounded(px(9.)).border_1()
                .border_color(palette::edge()).bg(palette::fill(0.04)).text_size(px(13.)).child(check("confirm-check", label, value))))
            .into_any_element()
    };
    // Largo o bastante para o aviso e a escolha não virarem uma coluna estreita; nunca além da janela.
    let (width, height) = if tall { (px(620.).min(window.viewport_size().width * 0.92), window.viewport_size().height * 0.8) } else { (px(360.), px(150.)) };
    chrome::confirm_dialog(window, cx, width, height, None, Rc::new(body), enabled, tr("update_confirm_ok"), None,
        |dialog| dialog.p(px(18.)).rounded(px(14.)), move |window, cx| {
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
/// `stage`: largura da animação no cartão em pé, com o "Experimente"; sem ela, cartão deitado com a animação à esquerda
/// (o "Atualizar?"). `show`: o "Mostrar no app" dos cartões com âncora; só depois de atualizar, quando a peça já existe.
fn sections(label: String, news: &[News], stage: Option<Pixels>, fixes_open: &Rc<Cell<bool>>, show: Option<&ShowInApp>, started: Instant,
    window: &mut Window, cx: &mut App) -> Div {
    let (fixes, cards): (Vec<&News>, Vec<&News>) = news.iter().partition(|n| n.kind == "fix");
    let open = fixes_open.get();
    let toggle = fixes_open.clone();
    div().flex().flex_col().gap(px(12.))
        .when(!cards.is_empty(), |el| el.child(div().mt(px(2.)).text_size(px(10.5)).text_color(theme::faint()).child(label.to_uppercase())))
        .children(cards.into_iter().map(|item| card(item, stage, show, started, window, cx)))
        .when(!fixes.is_empty(), |el| el.child(div().flex().flex_col().gap(px(6.))
            .child(div().flex().child(Button::new("update-fixes").ghost().xsmall().ml(px(-6.)).text_size(px(12.)).text_color(theme::muted())
                .icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                .label(tr_shared("update_notes_fixes", &[("n", &fixes.len().to_string())]))
                .on_click(move |_, window, _| { toggle.set(!toggle.get()); window.refresh(); })))
            .when(open, |el| el.child(div().pl(px(16.)).flex().flex_col().text_size(px(12.)).line_height(px(18.6)).text_color(theme::body_text())
                .children(fixes.into_iter().map(|item| div().flex().gap(px(6.)).child("•")
                    .child(div().flex_1().min_w_0().whitespace_normal().child(item.title.clone()))))))))
}

fn card(item: &News, stage: Option<Pixels>, show: Option<&ShowInApp>, started: Instant, window: &mut Window, cx: &mut App) -> Div {
    let animation = item.animation.as_deref().and_then(crate::news::animation);
    let point = show.zip(item.in_app()).map(|(show, note)| {
        let show = show.clone();
        div().flex().child(Button::new(SharedString::from(format!("update-news-show-{}", item.id))).outline().xsmall()
            .icon(IconName::Sparkles).label(tr("news_show_in_app"))
            .on_click(move |_, window, cx| { window.close_dialog(cx); show(note.clone(), window, cx); }))
    });
    let (badge_hue, badge_text) = palette::badge();
    let kind = match item.kind.as_str() { "new" => Some("update_news_kind_new"), "improvement" => Some("update_news_kind_improvement"), _ => None };
    let heading = div().flex().flex_wrap().items_center().gap_x(px(5.)).gap_y(px(2.))
        .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(item.title.clone()))
        .when_some(kind, |el, key| el.child(div().px(px(6.)).py(px(1.)).rounded_full().bg(badge_hue.alpha(0.18))
            .text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(badge_text).child(tr_shared(key, &[]))));
    let text = (!item.text.is_empty()).then(|| div().text_size(px(11.8)).line_height(px(16.5)).text_color(theme::muted())
        .child(TextView::markdown(SharedString::from(format!("update-news-{}", item.id)), item.text.clone()).selectable(true).scrollable(false)));
    let base = div().rounded(px(10.)).border_1().border_color(palette::edge()).bg(palette::fill(0.035));
    let Some(stage) = stage else {
        return base.flex().items_center().gap(px(12.)).p(px(10.))
            .when_some(animation, |el, draw| el.child(div().flex_shrink_0().w(px(160.))
                .child(stage_slot(&item.id, px(160.), draw, started, window, cx))))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.)).child(heading).children(text));
    };
    base.flex().flex_col().gap(px(10.)).p(px(12.))
        .when_some(animation, |el, draw| el.child(stage_slot(&item.id, stage, draw, started, window, cx)))
        .child(div().flex().flex_col().gap(px(3.)).child(heading).children(text))
        .when_some(item.try_it.clone(), |el, text| el.child(div().text_size(px(11.5)).text_color(badge_text).whitespace_normal()
            .child(tr_shared("update_notes_try_it", &[("text", &text)]))))
        .children(point)
}

impl News {
    /// O balão do "Mostrar no app": só com âncora que o app desenha; o texto é o "Experimente", senão o do cartão.
    fn in_app(&self) -> Option<crate::tour::Note> {
        let anchor = self.anchor.as_deref().filter(|anchor| crate::tour::anchors::ALL.contains(anchor))?;
        Some(crate::tour::Note { id: self.id.clone(), anchor: anchor.to_owned(), title: self.title.clone(),
            body: self.try_it.clone().unwrap_or_else(|| self.text.clone()) })
    }
}

/// O lugar da animação no cartão, medido a cada quadro para o seguinte: a largura real (o Dialog encolhe em janela
/// pequena) substitui `guess`, e fora da área visível do modal a animação não é montada — fica só a altura dela, sem
/// marcar redesenho da janela.
fn stage_slot(id: &str, guess: Pixels, draw: crate::news::NewsAnimation, started: Instant, window: &mut Window, cx: &mut App) -> Div {
    let measured = window.use_keyed_state(SharedString::from(format!("news-stage-{id}")), cx,
        |_, _| Rc::new(Cell::new(None::<(bool, Size<Pixels>)>))).read(cx).clone();
    let slot = div().relative().w_full();
    let slot = match measured.get() {
        Some((false, size)) => slot.h(size.height),
        last => slot.child(draw(started, last.map_or(guess, |(_, size)| size.width), window, cx)),
    };
    slot.child(canvas(move |bounds, window, _| {
        let visible = window.content_mask().bounds.intersects(&bounds);        // Voltou à vista ou mudou de largura: o quadro seguinte desenha com o que acabou de ser medido.
        if measured.get().is_none_or(|(was, size)| (visible && !was) || size.width != bounds.size.width) {
            window.request_animation_frame();
        }
        measured.set(Some((visible, bounds.size)));
    // Sem `top_0`/`left_0` o absoluto fica depois da animação, e a medida sai deslocada pela altura dela.
    }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
}

/// "O que chegou para você": uma vez, com o que o servidor diz que chegou.
fn open_whats_new(items: Vec<News>, show: Option<ShowInApp>, window: &mut Window, cx: &mut App) {
    let fixes_open = Rc::new(Cell::new(false));
    let started = Instant::now();
    window.open_dialog(cx, move |dialog, window, cx| {
        let max = window.viewport_size().height * 0.8 - px(110.);
        let top = (window.viewport_size().height * 0.1).max(px(24.));
        let width = px(780.).min(window.viewport_size().width * 0.92);
        // Largura do primeiro quadro da animação (modal menos borda e respiros dele e do cartão); depois vale a medida.
        let stage = width - px(64.);
        crate::app::popup::dialog(dialog).w(width).p(px(18.)).rounded(px(14.)).close_button(false).margin_top(top)
            .child(div().id("whats-new-body").max_h(max).overflow_y_scroll()
                .child(sections(tr_shared("update_whats_new_title", &[]), &items, Some(stage), &fixes_open, show.as_ref(), started, window, cx)))
            .footer(DialogFooter::new().child(chrome::light_button("whats-new-close", true, cx).label(tr("close"))
                .on_click(|_, window, cx| window.close_dialog(cx))))
    });
}

fn for_desktop(news: &News) -> bool { matches!(news.platforms.as_str(), "desktop" | "all") }

/// A resposta de `GET /api/update/arrived`, já só com as novidades do desktop. `Seen`: nada a abrir, só marcar visto.
#[derive(Debug, PartialEq)]
pub(super) enum Arrived { Show { news: Vec<News>, through: u64 }, Seen(u64), Already, Absent, Failed(String) }

/// `last`: o `through` que este processo já tratou; resposta até ele veio de outro gatilho e não abre de novo.
/// 404/405 = servidor sem a rota: não há o que mostrar.
pub(super) fn arrived(read: Result<Value, Failure>, last: Option<u64>) -> Arrived {
    #[derive(Deserialize)]
    struct Wire { #[serde(default)] news: Vec<News>, through: u64 }
    match read.map(serde_json::from_value::<Wire>) {
        Ok(Ok(wire)) if last.is_some_and(|last| wire.through <= last) => Arrived::Already,
        Ok(Ok(Wire { mut news, through })) => {
            news.retain(|item| for_desktop(item) && !legacy_shown(&item.id));
            if news.is_empty() { Arrived::Seen(through) } else { Arrived::Show { news, through } }
        }
        Ok(Err(error)) => Arrived::Failed(error.to_string()),
        Err(error) if matches!(error.status, Some(404 | 405)) => Arrived::Absent,
        Err(error) => Arrived::Failed(error.detail),
    }
}

/// O `device` que o servidor aceita: casar com `hangar_update_news::valid_device` e com a regex do mobile.
fn valid_device(id: &str) -> bool { (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') }

/// Um id por instalação do app, igual para todos os servidores: criado na primeira vez, lido depois.
fn device_id_in(dir: &Path) -> std::io::Result<String> {
    let path = dir.join("device-id");
    let replace = match std::fs::read_to_string(&path) {
        Ok(text) if valid_device(text.trim()) => return Ok(text.trim().to_owned()),
        Ok(_) => true,
        Err(_) => false,
    };
    let mut raw = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut raw).map_err(|_| std::io::Error::other("sem número aleatório"))?;
    // UUID versão 4.
    raw[6] = raw[6] & 0x0f | 0x40;
    raw[8] = raw[8] & 0x3f | 0x80;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let id = format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]);
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(&tmp)?;
    // O `mode` só vale na criação: um tmp que sobrou levaria o modo antigo ao arquivo no `rename`.
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; file.set_permissions(std::fs::Permissions::from_mode(0o600))?; }
    std::io::Write::write_all(&mut file, id.as_bytes())?;
    drop(file);
    // Sem sobrescrever: duas instâncias abrindo juntas ficam com o id de quem gravou primeiro. Ilegível, ou sistema de
    // arquivos sem hard link, troca pelo `rename`.
    let linked = if replace { Err(std::io::ErrorKind::Unsupported.into()) } else { std::fs::hard_link(&tmp, &path) };
    match linked {
        Ok(()) => { let _ = std::fs::remove_file(&tmp); Ok(id) }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&tmp);
            let text = std::fs::read_to_string(&path)?;
            if valid_device(text.trim()) { Ok(text.trim().to_owned()) } else { Err(std::io::Error::other("device-id ilegível")) }
        }
        Err(_) => { std::fs::rename(&tmp, &path)?; Ok(id) }
    }
}

/// Lido do disco uma vez por processo; falha não fica guardada e tenta de novo no próximo pedido.
pub(super) fn device_id() -> Option<String> {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(id) = ID.get() { return Some(id.clone()); }
    let id = crate::appearance::dir().ok_or_else(|| std::io::Error::other("sem pasta de configuração")).and_then(|dir| device_id_in(&dir))
        .map_err(|error| crate::log_line(&format!("update news: id do aparelho: {error}"))).ok()?;
    Some(ID.get_or_init(|| id).clone())
}

/// Abre "O que chegou para você" na janela ativa (ou na primeira), com o "Mostrar no app" do app; `false` sem janela.
pub(super) fn show_news(items: Vec<News>, cx: &mut App) -> bool {
    let Some(handle) = cx.active_window().or_else(|| cx.windows().first().copied()) else { return false };
    let show = show_in_app(cx);
    handle.update(cx, |_, window, cx| open_whats_new(items, show, window, cx)).is_ok()
}

// Chamado também do `Updater`, que não tem a entidade do app no contexto: ela vem do global.
fn show_in_app(cx: &App) -> Option<ShowInApp> {
    crate::app::Hangar::weak(cx).map(|hangar| -> ShowInApp {
        Rc::new(move |note: crate::tour::Note, _: &mut Window, cx: &mut App| { let _ = hangar.update(cx, |this, cx| this.show_news(note, cx)); })
    })
}

/// Ids que a lista antiga já mostrou nesta abertura: a primeira leitura do servidor não os repete.
static LEGACY_SHOWN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn legacy_shown(id: &str) -> bool { LEGACY_SHOWN.lock().is_ok_and(|ids| ids.iter().any(|shown| shown == id)) }

/// A lista que o "Atualizar?" de um app anterior guardou em `novidades-pendentes.json`: abre uma vez se o app foi
/// trocado desde então, e o arquivo some. O servidor guarda o que chegou desde esta versão.
pub(crate) fn show_legacy_pending(window: &mut Window, cx: &mut App) {
    #[derive(Deserialize)]
    struct Pending { from_native: String, items: Vec<News> }
    let Some(path) = crate::appearance::dir().map(|dir| dir.join("novidades-pendentes.json")) else { return };
    let Ok(bytes) = std::fs::read(&path) else { return };
    let _ = std::fs::remove_file(&path);
    let Ok(pending) = serde_json::from_slice::<Pending>(&bytes) else { return };
    if pending.from_native == super::CURRENT || pending.items.is_empty() { return; }
    if let Ok(mut ids) = LEGACY_SHOWN.lock() { ids.extend(pending.items.iter().map(|item| item.id.clone())); }
    let show = show_in_app(cx);
    open_whats_new(pending.items, show, window, cx);
}

/// `HANGAR_NATIVE_PREVIEW=update-confirm` e `=whats-new` (build de debug): os dois modais com uma amostra fixa.
#[cfg(debug_assertions)]
pub(crate) fn open_preview(whats_new: bool, window: &mut Window, cx: &mut App) {
    let news = |id: &str, title: &str, kind: &str, text: &str, try_it: Option<&str>, animation: Option<&str>| News { id: id.into(),
        title: title.into(), kind: kind.into(), text: text.into(), platforms: "desktop".into(), try_it: try_it.map(Into::into),
        animation: animation.map(Into::into), anchor: None };
    let items = vec![
        news("historico", "Histórico do que você digitou", "new",
            "A seta para cima no compositor traz de volta o que você já mandou nesta conversa, do mais recente para o mais antigo.",
            Some("↑ em qualquer conversa"), Some("historico-compositor")),
        News { anchor: Some(crate::tour::anchors::COMPOSER.into()), ..news("tour", "Os recursos escondidos, mostrados na tela", "new",
            "Balões em cima das peças de verdade mostram o que estava escondido.", Some("Configurações → Sobre → Ver o tour de novo."),
            Some("tour-nativo")) },
        news("colar-windows", "Texto colado com várias linhas no Windows chega ao Claude", "fix", "", None, None),
        news("windows-passo", "O backend no Windows não cai mais ao subir com passo pendente", "fix", "", None, None),
        news("worktrees", "A limpeza de worktrees reconhece as já mescladas", "fix", "", None, None),
    ];
    if whats_new {
        // O "Mostrar no app" aparece para a foto; aqui ele só fecha o modal.
        let close_only: ShowInApp = Rc::new(|_: crate::tour::Note, _: &mut Window, _: &mut App| {});
        // Um cartão por animação: a rolagem e o redesenho são provados com o modal cheio, como numa versão de verdade.
        let more = ["codex-atividade", "etiquetas-maquina-conta", "sugestao-de-nome", "cofre-de-senhas", "relatorio-automatico", "ocultar-da-lista", "tour-nativo"]
            .map(|name| news(name, name, "new", "Texto de amostra da novidade, com duas linhas para o cartão ter a altura de um real.",
                Some("abra e teste"), Some(name)));
        return open_whats_new(items.into_iter().chain(more).collect(), Some(close_only), window, cx);
    }
    let notes = Notes { version: Some("0.1.0.8242 → 0.1.0.8311".into()), news: items, server: "preview".into(), failed: false,
        incomplete: false, label: None,
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
    fn arrived_keeps_desktop_news_marks_an_empty_list_and_treats_an_old_server_as_absent() {
        let item = |id: &str, platforms: &str| serde_json::json!({"id": id, "title": "t", "kind": "new", "platforms": platforms});
        let read = Ok(serde_json::json!({"news": [item("a", "all"), item("b", "mobile"), item("c", "desktop")], "through": 7}));
        let Arrived::Show { news, through } = arrived(read, None) else { panic!("com novidade do desktop, abre") };
        assert_eq!((news.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), through), (vec!["a", "c"], 7));
        let only_mobile = Ok(serde_json::json!({"news": [item("b", "mobile")], "through": 8}));
        assert_eq!(arrived(only_mobile, None), Arrived::Seen(8), "lista vazia depois do filtro: marca visto, não abre");
        assert_eq!(arrived(Ok(serde_json::json!({"news": [], "through": 3})), None), Arrived::Seen(3), "aparelho novo");
        let again = Ok(serde_json::json!({"news": [item("a", "all")], "through": 7}));
        assert_eq!(arrived(again, Some(7)), Arrived::Already, "o mesmo `through` não abre duas vezes no processo");
        assert_eq!(arrived(Err(refused(Some(404), "Not Found")), None), Arrived::Absent);
        assert_eq!(arrived(Err(refused(Some(405), "Method Not Allowed")), None), Arrived::Absent);
        assert!(matches!(arrived(Err(refused(Some(503), "update_arrived_unavailable")), None), Arrived::Failed(_)));
        assert!(matches!(arrived(Ok(serde_json::json!({"news": []})), None), Arrived::Failed(_)), "sem `through` não marca nada");
    }

    #[test]
    fn device_id_is_created_once_and_reused() {
        let dir = std::env::temp_dir().join(format!("hangar-native-device-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = device_id_in(&dir).unwrap();
        assert!(valid_device(&first) && first.len() == 36, "UUID: {first}");
        assert_eq!(device_id_in(&dir).unwrap(), first);
        std::fs::write(dir.join("device-id"), "não serve").unwrap();
        let replaced = device_id_in(&dir).unwrap();
        assert!(replaced != first && valid_device(&replaced), "id ilegível é trocado por outro");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn news_without_anchor_reads_and_only_known_anchors_point() {
        let old: News = serde_json::from_value(serde_json::json!({"id": "a", "title": "t", "kind": "new", "text": "x"})).unwrap();
        assert_eq!(old.anchor, None);
        assert!(old.in_app().is_none(), "sem âncora, sem Mostrar no app");

        let item = |anchor: &str, try_it: Option<&str>| serde_json::from_value::<News>(serde_json::json!({"id": "b", "title": "t",
            "kind": "new", "text": "corpo", "try_it": try_it, "anchor": anchor})).unwrap();
        let note = item("composer", Some("aperte ↑")).in_app().unwrap();
        assert_eq!((note.id.as_str(), note.anchor.as_str(), note.body.as_str()), ("b", "composer", "aperte ↑"));
        assert_eq!(item("composer", None).in_app().unwrap().body, "corpo", "sem Experimente, o texto do cartão");
        assert!(item("peca-que-nao-existe", None).in_app().is_none());
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
        assert_eq!(notes.acceptance(false), Acceptance { ids: vec!["2026-10-24-pacote-por-versao".into(), "outro".into()], mode: Some("package"),
            incomplete: false });
        assert_eq!(notes.acceptance(true).mode, Some("developer"));
        let plain = Notes { warnings: parse_notes(Ok(serde_json::json!({"warnings": [{"id": "a", "title": "t"}]}))).unwrap().unwrap().warnings,
            ..Notes::default() };
        assert_eq!(plain.acceptance(true), Acceptance { ids: vec!["a".into()], mode: None, incomplete: false }, "sem escolha, sem modo");
        // Sem o índice o servidor manda só o aviso local, marcado: o aceite sai, o `iniciar` não.
        let partial = parse_notes(Ok(serde_json::json!({"warnings": [{"id": "a", "title": "t"}], "incomplete": true}))).unwrap().unwrap();
        let partial = Notes { warnings: partial.warnings, incomplete: partial.incomplete, ..Notes::default() };
        assert!(partial.acceptance(false).incomplete);
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
