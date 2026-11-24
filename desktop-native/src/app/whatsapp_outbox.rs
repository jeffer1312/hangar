//! Rascunhos de WhatsApp esperando o dono (evento `whatsapp_outbox` da lista da máquina do WhatsApp). O modelo só cria o rascunho;
//! a mensagem sai quando a pessoa toca em Enviar aqui. Um cartão por rascunho, na conversa da sessão que pediu ou flutuando
//! junto com os do cofre; a resposta vai à própria máquina, que repassa ao hangar-whatsapp.
use super::*;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct Recent {
    #[serde(default)]
    from_me: bool,
    #[serde(default)]
    sender: Option<String>,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct DraftFile {
    name: String,
    #[serde(default)]
    size: Option<u64>,
    source: String,
    #[serde(default)]
    outside_cwd: Option<bool>,
}

/// O rascunho do C5, só com o que o cartão mostra.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct Draft {
    id: String,
    origin: String,
    chat_number: String,
    #[serde(default)]
    chat_name: Option<String>,
    #[serde(default)]
    chat_name_source: Option<String>,
    #[serde(default)]
    recent: Option<Vec<Recent>>,
    text: String,
    #[serde(default)]
    quoted_text: Option<String>,
    #[serde(default)]
    file: Option<DraftFile>,
    expires_at: String,
    state: String,
}

#[derive(serde::Deserialize)]
struct Event { drafts: Vec<Value> }

/// Rascunhos pendentes do evento; `None` = corpo que não se lê (o anterior fica). Rascunho torto ou que não está mais
/// pendente sai da lista: só `pending` aceita resposta.
pub(super) fn parse(data: &Value) -> Option<Vec<Draft>> {
    let event = serde_json::from_value::<Event>(data.clone()).ok()?;
    Some(event.drafts.into_iter().filter_map(|d| serde_json::from_value::<Draft>(d).ok())
        .filter(|d| d.state == "pending" && !d.id.is_empty() && !d.chat_number.is_empty()).collect())
}

// A mesma lista do `whatsappOutbox.ts` do core: o que não aparece na tela ou mexe na ordem do texto.
const INVISIBLE: &[(u32, u32)] = &[
    (0x0000, 0x0008), (0x000b, 0x001f), (0x007f, 0x009f), (0x00ad, 0x00ad), (0x034f, 0x034f), (0x061c, 0x061c),
    (0x115f, 0x1160), (0x17b4, 0x17b5), (0x180b, 0x180f), (0x200b, 0x200f), (0x2028, 0x202e), (0x2060, 0x206f),
    (0x2800, 0x2800), (0x3164, 0x3164), (0xfe00, 0xfe0f), (0xfeff, 0xfeff), (0xffa0, 0xffa0), (0xfff0, 0xfffb),
    (0x1bca0, 0x1bca3), (0x1d173, 0x1d17a), (0xe0000, 0xe0fff),
];

/// ZWJ e seletores de emoji fazem parte de emoji (👨‍👩‍👧, ❤️): depois de símbolo, não são marcados.
fn emoji_glue(cp: u32) -> bool { matches!(cp, 0x200d | 0xfe0e | 0xfe0f) }

fn invisible(cp: u32) -> bool { INVISIBLE.iter().any(|&(a, b)| (a..=b).contains(&cp)) }

/// O texto com cada caractere invisível ou de controle trocado pela marca `⟨U+200B⟩`, e onde cada marca ficou (em bytes),
/// para o destaque. Quebra de linha e tabulação ficam como estão.
pub(super) fn mark_invisibles(text: &str) -> (String, Vec<Range<usize>>) {
    let (mut out, mut marks) = (String::with_capacity(text.len()), Vec::new());
    let (mut prev, mut prev_marked) = (None::<u32>, false);
    for ch in text.chars() {
        let cp = ch as u32;
        let glue = emoji_glue(cp) && !prev_marked && prev.is_some_and(|p| p >= 0x2000 || p == 0xa9 || p == 0xae);
        let mark = invisible(cp) && !glue;
        if mark {
            let start = out.len();
            out.push_str(&format!("⟨U+{cp:04X}⟩"));
            marks.push(start..out.len());
        } else {
            out.push(ch);
        }
        (prev, prev_marked) = (Some(cp), mark);
    }
    (out, marks)
}

/// Segundos até expirar, nunca negativo; data ilegível conta como expirada.
pub(super) fn seconds_left(expires_at: &str, now: chrono::DateTime<chrono::Utc>) -> i64 {
    chrono::DateTime::parse_from_rfc3339(expires_at).map_or(0, |at| (at.with_timezone(&chrono::Utc) - now).num_seconds().max(0))
}

/// `9:12`.
pub(super) fn countdown(seconds: i64) -> String { format!("{}:{:02}", seconds / 60, seconds % 60) }

/// Tamanho do anexo como o core: bytes, KB inteiro, MB e GB com uma casa (unidades de 1024).
pub(super) fn file_size(bytes: u64, english: bool) -> String {
    const MB: f64 = 1024. * 1024.;
    let one = |value: f64| { let text = format!("{value:.1}"); if english { text } else { text.replace('.', ",") } };
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{} KB", (b as f64 / 1024.).round() as u64),
        b if (b as f64) < MB * 1024. => format!("{} MB", one(b as f64 / MB)),
        b => format!("{} GB", one(b as f64 / (MB * 1024.))),
    }
}

/// Hora local de uma mensagem (`14:02`); vazio quando a data não se lê.
fn clock(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at).map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default()
}

/// Recusa que tira o cartão: o rascunho não aceita mais resposta.
pub(super) fn ends_card(code: Option<&str>) -> bool {
    matches!(code, Some("whatsapp_expired" | "whatsapp_answered" | "whatsapp_not_found"))
}

/// Frase da recusa pelo código `whatsapp_*`; `locked_until`, o `reachout_locked_until` do status quando lido. `None`: código
/// sem frase própria (quem chama mostra a falha genérica).
pub(super) fn refusal_text(code: Option<&str>, locked_until: Option<&str>) -> Option<String> {
    let key = match code? {
        "whatsapp_reachout_locked" => {
            let Some(at) = locked_until.and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()) else {
                return Some(tr_shared("whatsapp_draft_err_reachout_locked_unknown", &[]));
            };
            let at = at.with_timezone(&chrono::Local);
            let date = at.format(if crate::i18n::english() { "%m/%d" } else { "%d/%m" }).to_string();
            return Some(tr_shared("whatsapp_draft_err_reachout_locked", &[("date", &date), ("time", &at.format("%H:%M").to_string())]));
        }
        "whatsapp_expired" => "whatsapp_draft_err_expired",
        "whatsapp_answered" => "whatsapp_draft_err_answered",
        "whatsapp_not_found" => "whatsapp_draft_err_not_found",
        "whatsapp_rate_limited" => "whatsapp_draft_err_rate_limited",
        "whatsapp_not_paired" => "whatsapp_draft_err_not_paired",
        // Serviço sem envio (api 1): o remédio é o mesmo.
        "whatsapp_outdated" | "whatsapp_send_unsupported" => "whatsapp_draft_err_outdated",
        _ => return None,
    };
    Some(tr_shared(key, &[]))
}

/// O "expira em 9:12" do cartão, redesenhado a cada segundo enquanto o cartão existe.
pub(super) struct Countdown { expires: String }

impl Render for Countdown {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let left = countdown(seconds_left(&self.expires, chrono::Utc::now()));
        div().flex_shrink_0().font_weight(FontWeight::NORMAL).text_color(theme::muted()).child(tr_shared("whatsapp_draft_expires_in", &[("time", &left)]))
    }
}

fn countdown_view(expires: String, cx: &mut App) -> Entity<Countdown> {
    cx.new(|cx| {
        // O cartão some e leva a view junto: o laço para no primeiro `update` que falha.
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if view.update(cx, |_, cx| if chrome::window_active() { cx.notify() }).is_err() { break; }
            }
        }).detach();
        Countdown { expires }
    })
}

/// Clique que responde o cartão `id`. Pela referência fraca, como no cofre.
fn answer_on(id: &str, send: bool, cx: &mut Context<WhatsAppOutbox>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static + use<> {
    let (id, outbox) = (id.to_owned(), cx.entity().downgrade());
    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        let _ = outbox.update(cx, |this, cx| this.answer(id.clone(), send, window, cx));
    }
}

struct Card { draft: Draft, clock: Entity<Countdown>, sending: bool, error: Option<String> }

/// Os cartões de uma máquina. Entidade própria: o diálogo da barra de abas mostra a mesma.
pub(super) struct WhatsAppOutbox { api: Option<Api>, runtime: Arc<Runtime>, machine: String, cards: Vec<Card> }

impl WhatsAppOutbox {
    /// Rascunho que continua na lista guarda o envio em curso e o erro mostrado.
    fn apply(&mut self, api: Option<Api>, machine: String, list: Vec<Draft>, cx: &mut Context<Self>) {
        (self.api, self.machine) = (api, machine);
        let mut old = std::mem::take(&mut self.cards);
        self.cards = list.into_iter().map(|draft| match old.iter().position(|c| c.draft.id == draft.id) {
            Some(at) => {
                let mut card = old.swap_remove(at);
                if card.draft.expires_at != draft.expires_at { card.clock = countdown_view(draft.expires_at.clone(), cx); }
                card.draft = draft;
                card
            }
            None => Card { clock: countdown_view(draft.expires_at.clone(), cx), draft, sending: false, error: None },
        }).collect();
        cx.notify();
    }

    pub(super) fn count(&self) -> usize { self.cards.len() }

    /// Os cartões cujo `servidor::sessao` de origem passa em `keep`.
    pub(super) fn render_where(&self, keep: &dyn Fn(&str) -> bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.cards.iter().filter(|c| keep(&c.draft.origin)).map(|card| self.render_card(card, cx)).collect()
    }

    fn answer(&mut self, id: String, send: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.cards.iter_mut().find(|c| c.draft.id == id).filter(|c| !c.sending) else { return };
        let Some(api) = self.api.clone() else {
            card.error = Some(tr("network_error"));
            cx.notify();
            return;
        };
        (card.sending, card.error) = (true, None);
        let (done, result) = tokio::sync::oneshot::channel();
        let path = id.clone();
        self.runtime.spawn(async move {
            let (action, body) = if send { ("send", Some(json!({"via": "desktop"}))) } else { ("decline", None) };
            let result = api.server_send(reqwest::Method::POST, &["whatsapp", "outbox", &path, action], body, 30).await;
            // A trava diz até quando pelo status do serviço; sem ele, a frase sem data.
            let until = match &result {
                Err(error) if error.code.as_deref() == Some("whatsapp_reachout_locked") => api.server_read(&["whatsapp"], &[], 15).await.ok()
                    .and_then(|state| state["status"]["reachout_locked_until"].as_str().map(str::to_owned)),
                _ => None,
            };
            let _ = done.send((result, until));
        });
        cx.spawn_in(window, async move |this, cx| {
            let (result, until) = result.await.unwrap_or_else(|_| (Err(Failure::local("network_error")), None));
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    // Quem tira o cartão de vez é o evento; aqui ele só sai da frente.
                    Ok(_) => {
                        this.cards.retain(|c| c.draft.id != id);
                        if send { window.push_notification(Notification::success(tr_shared("whatsapp_draft_sending", &[])), cx); }
                    }
                    Err(error) => {
                        let code = error.code.as_deref();
                        let text = refusal_text(code, until.as_deref()).unwrap_or_else(|| Hangar::fetch_failure(&error));
                        // Expirado, respondido em outra tela ou apagado não volta: o cartão sai e o motivo vai no aviso.
                        if ends_card(code) {
                            this.cards.retain(|c| c.draft.id != id);
                            window.push_notification(Notification::warning(text), cx);
                        } else if let Some(card) = this.cards.iter_mut().find(|c| c.draft.id == id) {
                            (card.sending, card.error) = (false, Some(text));
                        }
                    }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn render_card(&self, card: &Card, cx: &mut Context<Self>) -> AnyElement {
        let d = &card.draft;
        let id = d.id.clone();
        let off = card.sending;
        let send = request_button("whatsapp-send", &id, tr_shared("whatsapp_draft_send", &[]), off).primary().on_click(answer_on(&id, true, cx));
        let decline = request_button("whatsapp-decline", &id, tr_shared("whatsapp_draft_decline", &[]), off).outline().on_click(answer_on(&id, false, cx));
        let title = tr_shared("whatsapp_draft_wants", &[("origin", &d.origin)]);
        let heading = div().flex().justify_between().gap_2().font_weight(FontWeight::SEMIBOLD).text_color(theme::warning())
            .child(div().min_w_0().whitespace_normal().child(format!("{} · {}", tr_shared("whatsapp_draft_title", &[]), self.machine)))
            .child(card.clock.clone());
        let source = d.chat_name_source.as_deref().and_then(|s| match s {
            "contact" => Some("whatsapp_draft_source_contact"), "push" => Some("whatsapp_draft_source_push"), "group" => Some("whatsapp_draft_source_group"), _ => None,
        }).map(|key| tr_shared(key, &[]));
        let name = d.chat_name.clone().unwrap_or_default();
        let destination = div().flex().flex_wrap().items_baseline().gap_2()
            .child(div().px(px(7.)).py(px(2.)).rounded(px(5.)).bg(theme::boxed()).font_family(theme::MONO).text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(d.chat_number.clone()))
            .when(!name.is_empty() || source.is_some(), |el| el.child(div().flex().items_center().gap_1().text_color(theme::muted())
                .when(!name.is_empty(), |el| el.child(name.clone()))
                .children(source.map(|s| div().px(px(5.)).rounded(px(4.)).border_1().border_color(theme::border()).child(s)))));
        let recent = d.recent.clone().unwrap_or_default();
        let quoted = d.quoted_text.clone().unwrap_or_default();
        let (shown, marks) = mark_invisibles(&d.text);
        let mark = HighlightStyle { color: Some(theme::warning()), background_color: Some(theme::warning().opacity(0.14)), ..Default::default() };
        let text = (!d.text.is_empty()).then(|| div().id(SharedString::from(format!("whatsapp-text-{id}"))).max_h(px(132.)).overflow_y_scroll()
            .px(px(8.)).py(px(6.)).rounded(px(5.)).bg(theme::boxed()).font_family(theme::MONO).text_color(theme::text()).whitespace_normal()
            .child(StyledText::new(shown).with_highlights(marks.into_iter().map(|range| (range, mark)))));
        div().id(SharedString::from(format!("whatsapp-draft-{id}"))).flex_shrink_0().mx(px(8.)).my(px(4.)).p_2().rounded(px(8.)).bg(theme::inset())
            .flex().flex_col().gap_1().text_xs().text_color(theme::text()).role(Role::Group).aria_label(title.clone())
            .child(heading)
            .child(div().whitespace_normal().child(title))
            .child(destination)
            .when(d.chat_name_source.as_deref() == Some("push"), |el| el.child(warn_line(tr_shared("whatsapp_draft_push_warning", &[]))))
            .when(!recent.is_empty(), |el| el.child(label(tr_shared("whatsapp_draft_recent", &[])))
                .child(div().border_l_2().border_color(theme::border()).pl_2().flex().flex_col().gap(px(2.)).children(recent.iter().map(recent_line))))
            .when(!quoted.is_empty(), |el| el.child(label(tr_shared("whatsapp_draft_quoted", &[])))
                .child(div().border_l_2().border_color(theme::accent()).bg(theme::accent_dim()).px_2().py_1().rounded_r(px(5.))
                    .text_color(theme::muted()).whitespace_normal().child(quoted.clone())))
            .when_some(text, |el, text| el
                .child(label(tr_shared("whatsapp_draft_text", &[("count", &d.text.chars().count().to_string())])))
                .child(text))
            .when_some(d.file.as_ref(), |el, file| el.child(label(tr_shared("whatsapp_draft_attachment", &[]))).child(file_row(file))
                .when(file.outside_cwd.unwrap_or(true), |el| el.child(warn_line(tr_shared("whatsapp_draft_outside_cwd", &[])))))
            .child(request_pair(send, decline))
            .children(card.error.clone().map(|error| div().id(SharedString::from(format!("whatsapp-draft-error-{id}"))).role(Role::Alert)
                .text_color(theme::danger()).whitespace_normal().child(error)))
            .into_any_element()
    }
}

fn label(text: String) -> Div { div().mt(px(3.)).text_color(theme::faint()).child(text) }

fn warn_line(text: String) -> Div { div().text_color(theme::warning()).whitespace_normal().child(text) }

/// Uma das últimas mensagens: quem, a hora e o texto; as da própria conta no tom do texto.
fn recent_line(r: &Recent) -> AnyElement {
    let who = if r.from_me { tr_shared("whatsapp_draft_me", &[]) } else { r.sender.clone().unwrap_or_default() };
    let at = r.at.as_deref().map(clock).unwrap_or_default();
    let head = [who.as_str(), at.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    let line = format!("{head} — {}", r.text.clone().unwrap_or_default());
    let strong = HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() };
    div().whitespace_normal().text_color(if r.from_me { theme::text() } else { theme::muted() })
        .child(StyledText::new(line).with_highlights((!who.is_empty()).then(|| (0..who.len(), strong))))
        .into_any_element()
}

/// O anexo: extensão, nome, tamanho e o caminho real; fora da pasta da sessão, contorno e caminho em laranja.
fn file_row(file: &DraftFile) -> Div {
    let outside = file.outside_cwd.unwrap_or(true);
    let ext = file.name.rsplit_once('.').map(|(_, ext)| ext.to_uppercase()).filter(|ext| !ext.is_empty() && ext.chars().count() <= 4).unwrap_or_default();
    div().flex().items_center().gap_2().px_2().py(px(6.)).rounded(px(5.)).bg(theme::boxed())
        .when(outside, |el| el.border_1().border_color(theme::warning()))
        .child(div().flex_shrink_0().size(px(26.)).rounded(px(5.)).bg(theme::hover()).flex().items_center().justify_center()
            .text_color(theme::muted()).child(ext))
        .child(div().min_w_0().flex().flex_col()
            .child(div().whitespace_normal().child(StyledText::new(format!("{} · {}", file.name, file_size(file.size.unwrap_or(0), crate::i18n::english())))
                .with_highlights([(0..file.name.len(), HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() })])))
            .child(div().font_family(theme::MONO).whitespace_normal().text_color(if outside { theme::warning() } else { theme::muted() })
                .child(file.source.clone())))
}

impl Render for WhatsAppOutbox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.cards.is_empty() { return div().p_2().text_sm().text_color(theme::muted()).child(tr("whatsapp_draft_none")); }
        let cards: Vec<AnyElement> = self.cards.iter().map(|card| self.render_card(card, cx)).collect();
        div().flex().flex_col().children(cards)
    }
}

impl Hangar {
    /// Rascunhos pendentes de uma máquina (evento `whatsapp_outbox`; `None` na queda do stream, que os reenvia ao voltar).
    /// Devolve se mudou.
    pub(super) fn set_whatsapp_outbox(&mut self, key: &str, data: Option<&Value>, cx: &mut Context<Self>) -> bool {
        let list = match data {
            None => Vec::new(),
            Some(data) => match parse(data) {
                Some(list) => list,
                None => {
                    crate::log_line("whatsapp: evento whatsapp_outbox ilegível; os cartões anteriores ficam");
                    return false;
                }
            },
        };
        if list.is_empty() { return self.whatsapp_outbox.remove(key).is_some(); }
        let (api, machine) = (self.machine_api(key), self.vault_machine_label(key, cx));
        let runtime = self.runtime.clone();
        let entity = self.whatsapp_outbox.entry(key.to_owned())
            .or_insert_with(|| cx.new(|_| WhatsAppOutbox { api: None, runtime, machine: String::new(), cards: Vec::new() }))
            .clone();
        entity.update(cx, |outbox, cx| outbox.apply(api, machine, list, cx));
        // Sem o identificador das máquinas, o cartão não acha a sessão que pediu.
        self.ensure_identities();
        true
    }

    /// Cartões que passam em `keep`, de todas as máquinas, a ativa primeiro.
    pub(super) fn whatsapp_cards(&self, keep: &dyn Fn(&str) -> bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let active = self.active_key();
        let mut keys: Vec<&String> = self.whatsapp_outbox.keys().collect();
        keys.sort_by_key(|key| (**key != active, (*key).clone()));
        keys.into_iter().flat_map(|key| self.whatsapp_outbox[key].update(cx, |outbox, cx| outbox.render_where(keep, cx))).collect()
    }

    /// Na barra das abas e no trilho, ao lado do botão do cofre: um botão por máquina que abre os cartões num diálogo.
    pub(super) fn render_whatsapp_chips(&self, rail: bool, cx: &App) -> Vec<AnyElement> {
        let active = self.active_key();
        let mut keys: Vec<&String> = self.whatsapp_outbox.keys().collect();
        keys.sort_by_key(|key| (**key != active, (*key).clone()));
        keys.into_iter().filter_map(|key| {
            let entity = self.whatsapp_outbox.get(key)?.clone();
            let count = entity.read(cx).count();
            if count == 0 { return None; }
            let machine = self.vault_machine_label(key, cx);
            let label = tr("whatsapp_draft_chip").replace("{n}", &count.to_string()).replace("{machine}", &machine);
            let title = format!("{} · {machine}", tr_shared("whatsapp_draft_title", &[]));
            let button = Button::new(SharedString::from(format!("whatsapp-chip-{key}"))).xsmall().outline().flex_shrink_0().icon(IconName::MessageCircle);
            let button = if rail { button.label(count.to_string()).tooltip(label.clone()).accessibility_label(label) } else { button.label(label) };
            Some(button
                .on_click(move |_, window, cx| {
                    let (entity, title) = (entity.clone(), title.clone());
                    window.open_dialog(cx, move |dialog, _, _| popup::dialog(dialog).w(px(420.)).title(title.clone()).child(entity.clone()));
                })
                .into_any_element())
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn draft(id: &str, state: &str) -> Value {
        json!({"id": id, "origin": "casa::web", "chat": "5511987654321@s.whatsapp.net", "chat_kind": "contact", "chat_number": "5511987654321",
            "chat_name": "Carlos", "chat_name_source": "push", "recent": [{"from_me": false, "sender": "Carlos", "at": "2026-11-24T14:02:00Z", "text": "oi"}],
            "text": "segue", "reply_to": "", "quoted_text": "", "file": {"name": "a.zip", "size": 10, "sha256": "x", "source": "/home/a.zip", "outside_cwd": true},
            "created_at": "2026-11-24T14:00:00Z", "expires_at": "2026-11-24T14:10:00Z", "state": state, "via": "", "answered_at": "",
            "message_id": "", "error": "", "wa_code": 0})
    }

    #[test]
    fn outbox_event_keeps_only_pending_drafts_and_rejects_a_broken_body() {
        let list = parse(&json!({"drafts": [draft("d1", "pending"), draft("d2", "sending"), {"id": "d3"}]})).unwrap();
        assert_eq!(list.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["d1"]);
        assert_eq!(list[0].chat_name_source.as_deref(), Some("push"));
        assert_eq!(list[0].file.as_ref().map(|f| (f.name.as_str(), f.outside_cwd)), Some(("a.zip", Some(true))));
        assert_eq!(list[0].recent.as_ref().map(Vec::len), Some(1));
        assert_eq!(parse(&json!({"drafts": []})), Some(Vec::new()));
        assert_eq!(parse(&json!({"drafts": {}})), None);
        assert_eq!(parse(&json!({})), None);
    }

    #[test]
    fn null_optional_fields_still_read() {
        let mut d = draft("d1", "pending");
        for key in ["chat_name", "chat_name_source", "recent", "quoted_text", "file"] { d[key] = Value::Null; }
        let list = parse(&json!({"drafts": [d]})).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].file.is_none());
    }

    fn shown(text: &str) -> String { mark_invisibles(text).0 }

    #[test]
    fn invisible_and_control_characters_are_marked_like_the_core() {
        assert_eq!(shown("nada perdido\u{200B}"), "nada perdido⟨U+200B⟩");
        assert_eq!(shown("a\u{202E}b\u{7}c\u{AD}d"), "a⟨U+202E⟩b⟨U+0007⟩c⟨U+00AD⟩d");
        assert_eq!(shown("x\r\ny"), "x⟨U+000D⟩\ny");
        assert_eq!(shown("a\n\tb"), "a\n\tb");
        assert_eq!(shown("ok\u{E0041}"), "ok⟨U+E0041⟩");
        assert_eq!(shown("👨\u{200D}👩 ❤\u{FE0F} ©\u{FE0F}"), "👨\u{200D}👩 ❤\u{FE0F} ©\u{FE0F}");
        assert_eq!(shown("Enviar\u{200D}"), "Enviar⟨U+200D⟩");
        assert_eq!(shown("\u{200B}\u{200D}"), "⟨U+200B⟩⟨U+200D⟩");
    }

    #[test]
    fn marks_point_at_their_bytes() {
        let (text, marks) = mark_invisibles("a\u{2060}b");
        assert_eq!(marks.len(), 1);
        assert_eq!(&text[marks[0].clone()], "⟨U+2060⟩");
        assert!(text.starts_with('a') && text.ends_with('b'));
    }

    #[test]
    fn countdown_size_and_refusals() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-24T14:00:48Z").unwrap().with_timezone(&chrono::Utc);
        assert_eq!(seconds_left("2026-11-24T14:10:00Z", now), 552);
        assert_eq!(countdown(552), "9:12");
        assert_eq!(seconds_left("2026-11-24T13:00:00Z", now), 0);
        assert_eq!(seconds_left("ruim", now), 0);
        assert_eq!(file_size(500, false), "500 B");
        assert_eq!(file_size(88 * 1024, false), "88 KB");
        assert_eq!(file_size(2_516_582, false), "2,4 MB");
        assert_eq!(file_size(2_516_582, true), "2.4 MB");
        assert!(["whatsapp_expired", "whatsapp_answered", "whatsapp_not_found"].iter().all(|c| ends_card(Some(c))));
        assert!(!ends_card(Some("whatsapp_rate_limited")) && !ends_card(None));
        assert_eq!(refusal_text(Some("whatsapp_qualquer"), None), None);
        assert_eq!(refusal_text(None, None), None);
    }

    /// `tr_shared` devolve a própria chave quando falta a frase: aqui a falta vira erro.
    #[test]
    fn every_shared_phrase_exists_in_both_languages() {
        let keys = ["whatsapp_draft_title", "whatsapp_draft_wants", "whatsapp_draft_expires_in", "whatsapp_draft_source_contact",
            "whatsapp_draft_source_push", "whatsapp_draft_source_group", "whatsapp_draft_push_warning", "whatsapp_draft_recent",
            "whatsapp_draft_me", "whatsapp_draft_quoted", "whatsapp_draft_text", "whatsapp_draft_attachment", "whatsapp_draft_outside_cwd",
            "whatsapp_draft_send", "whatsapp_draft_decline", "whatsapp_draft_sending", "whatsapp_draft_err_expired",
            "whatsapp_draft_err_answered", "whatsapp_draft_err_not_found", "whatsapp_draft_err_reachout_locked",
            "whatsapp_draft_err_reachout_locked_unknown", "whatsapp_draft_err_rate_limited", "whatsapp_draft_err_not_paired",
            "whatsapp_draft_err_outdated"];
        for english in [false, true] {
            crate::i18n::english_in_this_thread(english);
            let missing: Vec<&str> = keys.into_iter().filter(|key| tr_shared(key, &[]) == *key).collect();
            assert!(missing.is_empty(), "frase sem tradução ({english}): {missing:?}");
        }
        crate::i18n::english_in_this_thread(false);
        assert_eq!(refusal_text(Some("whatsapp_outdated"), None), Some(tr_shared("whatsapp_draft_err_outdated", &[])));
        assert_eq!(refusal_text(Some("whatsapp_reachout_locked"), None), Some(tr_shared("whatsapp_draft_err_reachout_locked_unknown", &[])));
        let locked = refusal_text(Some("whatsapp_reachout_locked"), Some("2026-11-25T12:30:00Z")).unwrap();
        assert_ne!(locked, tr_shared("whatsapp_draft_err_reachout_locked_unknown", &[]));
        assert!(!locked.contains("{date}") && !locked.contains("{time}"));
    }
}
