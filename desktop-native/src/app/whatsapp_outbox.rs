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
pub(crate) struct Draft {
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
    /// Até quando o cartão conta: o menor entre `expires_at` e o prazo da tool que espera a resposta (do Hangar).
    #[serde(default)]
    answer_by: Option<String>,
    state: String,
}

/// Rascunhos pendentes do evento; `None` = corpo que não se lê (o anterior fica). Rascunho torto ou que não está mais
/// pendente sai da lista: só `pending` aceita resposta. Lê cada rascunho direto do evento, sem copiar o JSON.
pub(super) fn parse(data: &Value) -> Option<Vec<Draft>> {
    let drafts = data.get("drafts")?.as_array()?;
    Some(drafts.iter().filter_map(|d| <Draft as serde::Deserialize>::deserialize(d)
            .map_err(|error| eprintln!("whatsapp: rascunho {} que o app não lê: {error}", d["id"].as_str().unwrap_or("?"))).ok())
        .filter(|d| d.state == "pending" && !d.id.is_empty() && !d.chat_number.is_empty()).collect())
}

// A mesma lista do `whatsappOutbox.ts` do core, conferida pelos casos de `whatsappInvisible.fixtures.json`: o que não
// aparece na tela ou mexe na ordem do texto.
const INVISIBLE: &[(u32, u32)] = &[
    (0x0000, 0x0008), (0x000b, 0x001f), (0x007f, 0x009f), (0x00ad, 0x00ad), (0x034f, 0x034f), (0x061c, 0x061c),
    (0x115f, 0x1160), (0x17b4, 0x17b5), (0x180b, 0x180f), (0x200b, 0x200f), (0x2028, 0x202e), (0x2060, 0x206f),
    (0x2800, 0x2800), (0x3164, 0x3164), (0xfe00, 0xfe0f), (0xfeff, 0xfeff), (0xffa0, 0xffa0), (0xfff0, 0xfffb),
    (0x1bca0, 0x1bca3), (0x1d173, 0x1d17a), (0xe0000, 0xe0fff),
];

/// Emoji para a cola do ZWJ; a mesma classificação do core.
fn emoji(cp: u32) -> bool {
    matches!(cp, 0x1f000..=0x1faff | 0x2600..=0x27bf | 0x2300..=0x23ff | 0x2b00..=0x2bff | 0xfe0f)
}

fn invisible(cp: u32) -> bool { INVISIBLE.iter().any(|&(a, b)| (a..=b).contains(&cp)) }

/// O texto como o cartão o mostra e onde cada marca ficou (em bytes), para o destaque.
pub(crate) type MarkedText = (SharedString, Vec<Range<usize>>);

/// O texto com cada caractere invisível ou de controle trocado pela marca `⟨U+200B⟩`. Quebra de linha e tabulação ficam como
/// estão.
pub(crate) fn mark_invisibles(text: &str) -> MarkedText {
    let (mut out, mut marks) = (String::with_capacity(text.len()), Vec::new());
    let (mut prev, mut prev_marked) = (None::<u32>, false);
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let cp = ch as u32;
        // Fazem parte do emoji e não são marcados: ZWJ só entre dois emojis (👨‍👩‍👧, ❤️‍🔥), seletor depois de símbolo (❤️, ©️).
        let glue = !prev_marked && prev.is_some_and(|p| match cp {
            0x200d => emoji(p) && chars.peek().is_some_and(|&next| emoji(next as u32)),
            0xfe0e | 0xfe0f => p >= 0x2000 || p == 0xa9 || p == 0xae,
            _ => false,
        });
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
    (out.into(), marks)
}

/// Até quando o rascunho aceita resposta: `answer_by`; sem ele legível, `expires_at`. `None`: nenhum se lê, e o cartão conta
/// como expirado.
pub(super) fn answer_deadline(answer_by: Option<&str>, expires_at: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let parse = |at: &str| chrono::DateTime::parse_from_rfc3339(at).ok().map(|t| t.with_timezone(&chrono::Utc));
    answer_by.and_then(|at| parse(at)).or_else(|| parse(expires_at))
}

/// Segundos até `deadline`, nunca negativo.
pub(super) fn seconds_left(deadline: Option<chrono::DateTime<chrono::Utc>>, now: chrono::DateTime<chrono::Utc>) -> i64 {
    deadline.map_or(0, |end| (end - now).num_seconds().max(0))
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

/// O "expira em 9:12" do cartão: o relógio da `WhatsAppOutbox` o redesenha a cada segundo.
pub(super) struct Countdown { deadline: Option<chrono::DateTime<chrono::Utc>> }

impl Render for Countdown {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        countdown_label(&crate::audio::clock(seconds_left(self.deadline, chrono::Utc::now()) as f64))
    }
}

/// O "expira em 9:12" do cabeçalho do cartão.
pub(crate) fn countdown_label(left: &str) -> Div {
    div().flex_shrink_0().font_weight(FontWeight::NORMAL).text_color(theme::muted()).child(tr_shared("whatsapp_draft_expires_in", &[("time", left)]))
}

/// Clique que responde o cartão `id`. Pela referência fraca, como no cofre.
fn answer_on(id: &str, send: bool, cx: &mut Context<WhatsAppOutbox>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static + use<> {
    let (id, outbox) = (id.to_owned(), cx.entity().downgrade());
    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        let _ = outbox.update(cx, |this, cx| this.answer(id.clone(), send, window, cx));
    }
}

/// `text`: o texto do rascunho já marcado, calculado quando o rascunho chega, não a cada desenho.
struct Card { draft: Draft, text: MarkedText, clock: Entity<Countdown>, sending: bool, error: Option<String> }

/// Os cartões de uma máquina. Entidade própria: o diálogo da barra de abas mostra a mesma. `clock`: um relógio para todos os
/// cartões, ligado só enquanto há cartão.
pub(super) struct WhatsAppOutbox { api: Option<Api>, runtime: Arc<Runtime>, machine: String, cards: Vec<Card>, clock: Option<Task<()>> }

impl WhatsAppOutbox {
    /// Rascunho que continua na lista guarda o envio em curso e o erro mostrado.
    fn apply(&mut self, api: Option<Api>, machine: String, list: Vec<Draft>, cx: &mut Context<Self>) {
        (self.api, self.machine) = (api, machine);
        let mut old = std::mem::take(&mut self.cards);
        for draft in list {
            let deadline = answer_deadline(draft.answer_by.as_deref(), &draft.expires_at);
            let card = match old.iter().position(|c| c.draft.id == draft.id) {
                Some(at) => {
                    let mut card = old.swap_remove(at);
                    if card.draft.text != draft.text { card.text = mark_invisibles(&draft.text); }
                    card.clock.update(cx, |clock, _| clock.deadline = deadline);
                    card.draft = draft;
                    card
                }
                None => Card { text: mark_invisibles(&draft.text), clock: cx.new(|_| Countdown { deadline }), draft, sending: false, error: None },
            };
            self.cards.push(card);
        }
        self.tick(cx);
        cx.notify();
    }

    /// Liga o relógio quando há cartão e o desliga quando não há.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.cards.is_empty() { self.clock = None; return; }
        if self.clock.is_some() { return; }
        self.clock = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| if chrome::window_active() {
                    for card in &this.cards { card.clock.update(cx, |_, cx| cx.notify()); }
                });
                if alive.is_err() { break; }
            }
        }));
    }

    pub(super) fn count(&self) -> usize { self.cards.len() }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.cards.clear();
        self.tick(cx);
        cx.notify();
    }

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
                        this.tick(cx);
                        if send { window.push_notification(Notification::success(tr_shared("whatsapp_draft_sending", &[])), cx); }
                    }
                    Err(error) => {
                        let code = error.code.as_deref();
                        let text = refusal_text(code, until.as_deref()).unwrap_or_else(|| Hangar::fetch_failure(&error));
                        // Expirado, respondido em outra tela ou apagado não volta: o cartão sai e o motivo vai no aviso.
                        if ends_card(code) {
                            this.cards.retain(|c| c.draft.id != id);
                            this.tick(cx);
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
        let (id, off) = (card.draft.id.clone(), card.sending);
        let send = request_button("whatsapp-send", &id, tr_shared("whatsapp_draft_send", &[]), off).primary().on_click(answer_on(&id, true, cx));
        let decline = request_button("whatsapp-decline", &id, tr_shared("whatsapp_draft_decline", &[]), off).outline().on_click(answer_on(&id, false, cx));
        draft_card(&card.draft, &card.text, &self.machine, card.clock.clone(), send, decline, card.error.clone()).into_any_element()
    }
}

/// O cartão de um rascunho, só desenho, na casca dos pedidos do cofre: a tela passa o texto marcado, a contagem viva e os
/// botões com clique; a novidade da atualização, a contagem parada e os botões sem clique.
pub(crate) fn draft_card(d: &Draft, text: &MarkedText, machine: &str, clock: impl IntoElement, send: Button, decline: Button, error: Option<String>) -> Stateful<Div> {
    let id = &d.id;
    let heading = request_heading(tr_shared("whatsapp_draft_title", &[]), machine, Some(clock.into_any_element()));
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
    let (shown, marks) = text;
    let mark = HighlightStyle { color: Some(theme::warning()), background_color: Some(theme::warning().opacity(0.14)), ..Default::default() };
    let text = (!d.text.is_empty()).then(|| div().id(SharedString::from(format!("whatsapp-text-{id}"))).max_h(px(132.)).overflow_y_scroll()
        .px(px(8.)).py(px(6.)).rounded(px(5.)).bg(theme::boxed()).font_family(theme::MONO).text_color(theme::text()).whitespace_normal()
        .child(StyledText::new(shown.clone()).with_highlights(marks.iter().map(|range| (range.clone(), mark)))));
    let body = div().flex().flex_col().gap_1().text_xs().text_color(theme::text())
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
        .child(request_pair(send, decline));
    request_card(id, heading, tr_shared("whatsapp_draft_wants", &[("origin", &d.origin)]), "", body, error)
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
            .child(div().whitespace_normal().child(StyledText::new(format!("{} · {}", file.name, api::download::size_label(file.size.unwrap_or(0), crate::i18n::english())))
                .with_highlights([(0..file.name.len(), HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() })])))
            .child(div().font_family(theme::MONO).whitespace_normal().text_color(if outside { theme::warning() } else { theme::muted() })
                .child(file.source.clone())))
}

/// Tira a máquina `key` do mapa; devolve se havia. O diálogo aberto segura a entidade: sem esvaziá-la, ele seguiria mostrando
/// o último cartão.
fn drop_outbox(map: &mut HashMap<String, Entity<WhatsAppOutbox>>, key: &str, cx: &mut App) -> bool {
    let Some(outbox) = map.remove(key) else { return false };
    outbox.update(cx, |outbox, cx| outbox.clear(cx));
    true
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
    /// `None`: corpo que não se lê (os cartões anteriores ficam); senão, se mudou.
    pub(super) fn set_whatsapp_outbox(&mut self, key: &str, data: Option<&Value>, cx: &mut Context<Self>) -> Option<bool> {
        let list = match data {
            None => Vec::new(),
            Some(data) => match parse(data) {
                Some(list) => list,
                None => {
                    crate::log_line("whatsapp: evento whatsapp_outbox ilegível; os cartões anteriores ficam");
                    return None;
                }
            },
        };
        if list.is_empty() { return Some(drop_outbox(&mut self.whatsapp_outbox, key, cx)); }
        let (api, machine) = (self.machine_api(key), self.vault_machine_label(key, cx));
        let runtime = self.runtime.clone();
        let entity = self.whatsapp_outbox.entry(key.to_owned())
            .or_insert_with(|| cx.new(|_| WhatsAppOutbox { api: None, runtime, machine: String::new(), cards: Vec::new(), clock: None }))
            .clone();
        entity.update(cx, |outbox, cx| outbox.apply(api, machine, list, cx));
        // Sem o identificador das máquinas, o cartão não acha a sessão que pediu.
        self.ensure_identities();
        Some(true)
    }

    /// Cartões que passam em `keep`, de todas as máquinas, a ativa primeiro.
    pub(super) fn whatsapp_cards(&self, keep: &dyn Fn(&str) -> bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.machines_active_first(&self.whatsapp_outbox).into_iter()
            .flat_map(|key| self.whatsapp_outbox[key].update(cx, |outbox, cx| outbox.render_where(keep, cx))).collect()
    }

    /// Na barra das abas e no trilho, ao lado do botão do cofre: um botão por máquina que abre os cartões num diálogo.
    pub(super) fn render_whatsapp_chips(&self, rail: bool, cx: &App) -> Vec<AnyElement> {
        self.request_chips(&self.whatsapp_outbox, WhatsAppOutbox::count, rail, "whatsapp", IconName::MessageCircle, "whatsapp_draft_chip",
            &tr_shared("whatsapp_draft_title", &[]), cx)
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
            "created_at": "2026-11-24T14:00:00Z", "expires_at": "2026-11-24T14:10:00Z", "answer_by": "2026-11-24T14:09:30Z", "state": state, "via": "", "answered_at": "",
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
        assert_eq!(parse(&json!([])), None);
    }

    #[test]
    fn null_optional_fields_still_read() {
        let mut d = draft("d1", "pending");
        for key in ["chat_name", "chat_name_source", "recent", "quoted_text", "file", "answer_by"] { d[key] = Value::Null; }
        let list = parse(&json!({"drafts": [d]})).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].file.is_none());
    }

    fn shown(text: &str) -> String { mark_invisibles(text).0.to_string() }

    /// Os mesmos casos do core (`whatsappOutbox.test.ts`).
    #[test]
    fn invisible_and_control_characters_are_marked_like_the_core() {
        let fixtures: Value = serde_json::from_str(include_str!("../../../packages/core/src/whatsappInvisible.fixtures.json")).unwrap();
        let cases = fixtures["cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            assert_eq!(shown(case["input"].as_str().unwrap()), case["shown"].as_str().unwrap(), "{}", case["name"]);
        }
    }

    #[test]
    fn zwj_is_left_unmarked_only_between_emoji() {
        assert_eq!(shown("漢\u{200D}字"), "漢⟨U+200D⟩字");
        assert_eq!(shown("❤\u{FE0F}\u{200D}🔥"), "❤\u{FE0F}\u{200D}🔥");
        assert_eq!(shown("👨\u{200D}a"), "👨⟨U+200D⟩a");
        assert_eq!(shown("a\u{200D}👩"), "a⟨U+200D⟩👩");
        assert_eq!(shown("👨\u{200D}"), "👨⟨U+200D⟩");
    }

    #[::gpui::test]
    fn an_empty_event_empties_the_outbox_an_open_dialog_still_holds(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        let runtime = Arc::new(Runtime::new().unwrap());
        let outbox = cx.new(|_| WhatsAppOutbox { api: None, runtime, machine: String::new(), cards: Vec::new(), clock: None });
        let list = parse(&json!({"drafts": [draft("d1", "pending")]})).unwrap();
        outbox.update(cx, |outbox, cx| outbox.apply(None, "casa".into(), list, cx));
        // O diálogo guarda o seu clone; o mapa perde o dele.
        let mut map = HashMap::from([("casa".to_owned(), outbox.clone())]);
        assert!(cx.update(|cx| drop_outbox(&mut map, "casa", cx)));
        assert!(map.is_empty());
        outbox.update(cx, |outbox, _| {
            assert_eq!(outbox.count(), 0);
            assert!(outbox.clock.is_none());
        });
        assert!(!cx.update(|cx| drop_outbox(&mut map, "casa", cx)));
    }

    #[test]
    fn marks_point_at_their_bytes() {
        let (text, marks) = mark_invisibles("a\u{2060}b");
        let text: &str = &text;
        assert_eq!(marks.len(), 1);
        assert_eq!(&text[marks[0].clone()], "⟨U+2060⟩");
        assert!(text.starts_with('a') && text.ends_with('b'));
    }

    #[test]
    fn countdown_runs_to_answer_by_and_falls_back_to_expires_at() {
        let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&chrono::Utc);
        let now = at("2026-11-24T14:00:48Z");
        let left = |answer_by: Option<&str>, expires_at: &str| seconds_left(answer_deadline(answer_by, expires_at), now);
        // O Hangar manda o prazo da tool (14:09:30), antes das 14:10:00 do serviço.
        assert_eq!(left(Some("2026-11-24T14:09:30Z"), "2026-11-24T14:10:00Z"), 522);
        assert_eq!(crate::audio::clock(522.), "8:42");
        assert_eq!(left(None, "2026-11-24T14:10:00Z"), 552);
        assert_eq!(left(Some("ruim"), "2026-11-24T14:10:00Z"), 552);
        assert_eq!(crate::audio::clock(552.), "9:12");
        assert_eq!(left(None, "2026-11-24T13:00:00Z"), 0);
        assert_eq!(left(None, "ruim"), 0);
        assert_eq!(seconds_left(answer_deadline(Some("2026-11-24T14:09:30Z"), "2026-11-24T14:10:00Z"), at("2026-11-24T14:09:45Z")), 0);
        // O evento traz `answer_by` e o cartão o lê.
        let list = parse(&json!({"drafts": [draft("d1", "pending")]})).unwrap();
        assert_eq!(list[0].answer_by.as_deref(), Some("2026-11-24T14:09:30Z"));
    }

    #[test]
    fn refusals_that_end_the_card() {
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
