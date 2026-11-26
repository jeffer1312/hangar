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
    /// `text`, `image`, `video`, `audio`, `document`, `sticker` ou `other`; ausente, é texto.
    #[serde(default)]
    kind: Option<String>,
    /// Nome do arquivo de um documento, de terceiro como o texto.
    #[serde(default)]
    file_name: Option<String>,
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
    /// `group` ou `contact`; ausente, conversa individual.
    #[serde(default)]
    chat_kind: Option<String>,
    chat_number: String,
    #[serde(default)]
    chat_name: Option<String>,
    #[serde(default)]
    chat_name_source: Option<String>,
    /// Foto da conversa em JPEG, base64; sem ela, o cartão mostra a inicial.
    #[serde(default)]
    chat_avatar: Option<String>,
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

impl Draft {
    /// Quem fala quando a mensagem não diz, e o nome da foto: o nome do contato, ou o número sem ele.
    fn contact(&self) -> &str { self.chat_name.as_deref().filter(|name| !name.is_empty()).unwrap_or(&self.chat_number) }

    /// Quem falou numa mensagem dele, a mesma regra do Expo (`recentSpeaker` do core): no grupo, só o remetente (vazio fica
    /// só a hora); na conversa individual, o nome da conversa, senão o remetente, senão o número.
    fn speaker<'a>(&'a self, r: &'a Recent) -> &'a str {
        let sender = r.sender.as_deref().filter(|s| !s.is_empty());
        if self.chat_kind.as_deref() == Some("group") { return sender.unwrap_or_default(); }
        self.chat_name.as_deref().filter(|name| !name.is_empty()).or(sender).unwrap_or(&self.chat_number)
    }

    /// Os balões das últimas mensagens, prontos para desenhar.
    pub(crate) fn bubbles(&self) -> Vec<RecentBubble> {
        self.recent.iter().flatten().map(|r| {
            let (tag, text) = recent_body(r);
            let at = r.at.as_deref().map(clock).unwrap_or_default();
            let who = if r.from_me { "" } else { self.speaker(r) };
            let meta = [who, at.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            RecentBubble { from_me: r.from_me, tag, text, meta }
        }).collect()
    }
}

/// Uma das últimas mensagens já resolvida: a chave da etiqueta do tipo, o texto marcado (`None`: a palavra do tipo) e a linha
/// de baixo (quem, nas dele, e a hora). As palavras do tipo saem no desenho, para acompanhar a troca de idioma.
pub(crate) struct RecentBubble { from_me: bool, tag: Option<&'static str>, text: Option<MarkedText>, meta: String }

/// Rascunhos pendentes do evento; `None` = corpo que não se lê (o anterior fica). Rascunho torto ou que não está mais
/// pendente sai da lista: só `pending` aceita resposta. Lê cada rascunho direto do evento, sem copiar o JSON.
pub(super) fn parse(data: &Value) -> Option<Vec<Draft>> {
    let drafts = data.get("drafts")?.as_array()?;
    Some(drafts.iter().filter_map(|d| <Draft as serde::Deserialize>::deserialize(d)
            .map_err(|error| eprintln!("whatsapp: rascunho {} que o app não lê: {error}", d["id"].as_str().unwrap_or("?"))).ok())
        .filter(|d| d.state == "pending" && !d.id.is_empty() && !d.chat_number.is_empty()).collect())
}

/// Se a máquina aceita o texto editado (`edit` do evento). Ausente: hangar-server antigo, sem "Editar".
pub(super) fn can_edit(data: &Value) -> bool { data["edit"].as_bool().unwrap_or(false) }

/// Teto do texto do WhatsApp, em pontos de código, como o serviço conta.
const TEXT_MAX: usize = 4096;

/// `\r\n` e `\r` viram `\n`, como o serviço faz antes de comparar e de enviar.
pub(super) fn normalize_newlines(text: &str) -> String { text.replace("\r\n", "\n").replace('\r', "\n") }

/// O texto que vai pela rota da edição: `None` quando o campo é o do rascunho, e o Enviar normal basta. Os dois já
/// normalizados (`normalize_newlines`).
pub(super) fn edited_text(original: &str, field: String) -> Option<String> { (field != original).then_some(field) }

/// Chave do motivo pelo qual o texto não pode sair; as mesmas regras da criação do rascunho no serviço. `text` já
/// normalizado e `count` seus pontos de código, contados uma vez por quem chama.
pub(super) fn text_problem(text: &str, count: usize, has_file: bool) -> Option<&'static str> {
    if count > TEXT_MAX { Some("whatsapp_draft_err_text_long") }
    else if text.trim().is_empty() && !has_file { Some("whatsapp_draft_err_text_empty") }
    else { None }
}

/// O 202 do envio editado traz o texto que o serviço aprovou: só vale o mesmo que o cartão mandou.
/// Os dois lados normalizados: com texto igual ao do rascunho, o serviço devolve o texto cru dele, que pode ter `\r\n`.
pub(super) fn sent_text_confirmed(reply: &Value, sent: &str) -> bool {
    reply["sent_text"].as_str().is_some_and(|text| normalize_newlines(text) == normalize_newlines(sent))
}

/// Lado da foto decodificada: o dobro do círculo de 40, para tela densa.
const AVATAR_PIXELS: u32 = 80;
const AVATAR: f32 = 40.;

/// A foto da conversa pronta para desenhar; `None` sem foto ou com bytes que não são JPEG legível, e o cartão mostra a
/// inicial. Só JPEG: o decodificador da casa também desenharia SVG, e a foto vem de terceiro.
pub(crate) fn avatar_image(data: Option<&str>) -> Option<Arc<RenderImage>> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data?.trim()).ok()?;
    if !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) { return None; }
    media::decode(&bytes, AVATAR_PIXELS, AVATAR_PIXELS, None)
}

/// A letra do círculo sem foto: a primeira visível do nome, maiúscula; sem nome, o primeiro dígito do número.
pub(super) fn initial(name: Option<&str>, number: &str) -> String {
    name.and_then(|name| name.chars().find(|c| !c.is_whitespace() && !invisible(*c as u32))).map(|c| c.to_uppercase().collect::<String>())
        .or_else(|| number.chars().find(char::is_ascii_digit).map(String::from)).unwrap_or_default()
}

/// Chave da palavra do tipo de uma mensagem recente; `None` em texto, `other` e tipo desconhecido, que mostram só o texto.
fn kind_key(kind: Option<&str>) -> Option<&'static str> {
    Some(match kind? {
        "image" => "whatsapp_draft_kind_image",
        "video" => "whatsapp_draft_kind_video",
        "audio" => "whatsapp_draft_kind_audio",
        "sticker" => "whatsapp_draft_kind_sticker",
        "document" => "whatsapp_draft_kind_document",
        _ => return None,
    })
}

/// O que o balão mostra: a chave da etiqueta do tipo e o texto já marcado. O texto é a legenda; em documento sem legenda, o
/// nome do arquivo; sem nenhum, `None`, e o balão mostra a palavra do tipo.
pub(super) fn recent_body(r: &Recent) -> (Option<&'static str>, Option<MarkedText>) {
    let text = r.text.as_deref().unwrap_or_default();
    let Some(key) = kind_key(r.kind.as_deref()) else { return (None, Some(mark_invisibles(text))) };
    let file = r.file_name.as_deref().filter(|_| r.kind.as_deref() == Some("document")).unwrap_or_default();
    let shown = if !text.trim().is_empty() { Some(mark_invisibles(text)) }
        else if !file.trim().is_empty() { Some(mark_invisibles(file)) }
        else { None };
    (Some(key), shown)
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
/// Quantos caracteres do texto viram marca: no campo da edição eles não aparecem, e o aviso diz que vão junto.
pub(super) fn invisible_count(text: &str) -> usize { mark_invisibles(text).1.len() }

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
        // Aprovado depois de parear outro número: o serviço confere o contato de novo.
        "whatsapp_new_contact" => "whatsapp_draft_err_new_contact",
        // Serviço sem envio (api 1): o remédio é o mesmo.
        "whatsapp_outdated" | "whatsapp_send_unsupported" => "whatsapp_draft_err_outdated",
        // Hangar novo com serviço sem a rota da edição: o rascunho continua esperando.
        "whatsapp_edit_unsupported" => "whatsapp_draft_err_edit_unsupported",
        // A vez do envio está com a troca de versão ou outra operação; o rascunho continua esperando.
        "whatsapp_updating" => return Some(tr("whatsapp_send_updating")),
        "whatsapp_busy" => return Some(tr("whatsapp_update_busy")),
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

/// Clique do "Editar" (`open`) ou do "Desfazer edição" do cartão `id`.
fn edit_on(id: &str, open: bool, cx: &mut Context<WhatsAppOutbox>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static + use<> {
    let (id, outbox) = (id.to_owned(), cx.entity().downgrade());
    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        let _ = outbox.update(cx, |this, cx| if open { this.start_edit(&id, window, cx) } else { this.undo_edit(&id, cx) });
    }
}

/// O campo da edição, vivo só enquanto o cartão edita. `original`: o texto do rascunho já normalizado, para comparar com o campo.
struct Editor { input: Entity<TextareaState>, original: String, _changes: Subscription }

/// `text`: o texto do rascunho já marcado, `bubbles` as últimas mensagens e `avatar` a foto decodificada, calculados quando
/// o rascunho chega, não a cada desenho.
struct Card { draft: Draft, text: MarkedText, bubbles: Vec<RecentBubble>, avatar: Option<Arc<RenderImage>>, clock: Entity<Countdown>,
    sending: bool, error: Option<String>, editor: Option<Editor> }

/// A foto do cartão que saiu deixa a memória de texturas: cada rascunho traz a sua.
fn forget(card: Card, cx: &mut App) {
    if let Some(image) = card.avatar { cx.drop_image(image, None); }
}

/// Os cartões de uma máquina. Entidade própria: o diálogo da barra de abas mostra a mesma. `clock`: um relógio para todos os
/// cartões, ligado só enquanto há cartão. `edit`: a máquina aceita o texto editado.
pub(super) struct WhatsAppOutbox { api: Option<Api>, runtime: Arc<Runtime>, machine: String, cards: Vec<Card>, clock: Option<Task<()>>, edit: bool }

impl WhatsAppOutbox {
    /// Rascunho que continua na lista guarda o envio em curso, o erro mostrado e a edição aberta.
    fn apply(&mut self, api: Option<Api>, machine: String, list: Vec<Draft>, edit: bool, cx: &mut Context<Self>) {
        (self.api, self.machine, self.edit) = (api, machine, edit);
        let mut old = std::mem::take(&mut self.cards);
        for draft in list {
            let deadline = answer_deadline(draft.answer_by.as_deref(), &draft.expires_at);
            let card = match old.iter().position(|c| c.draft.id == draft.id) {
                Some(at) => {
                    let mut card = old.swap_remove(at);
                    if card.draft.text != draft.text {
                        card.text = mark_invisibles(&draft.text);
                        if let Some(editor) = &mut card.editor { editor.original = normalize_newlines(&draft.text); }
                    }
                    if (&card.draft.recent, &card.draft.chat_kind, &card.draft.chat_name, &card.draft.chat_number)
                        != (&draft.recent, &draft.chat_kind, &draft.chat_name, &draft.chat_number) {
                        card.bubbles = draft.bubbles();
                    }
                    if card.draft.chat_avatar != draft.chat_avatar
                        && let Some(stale) = std::mem::replace(&mut card.avatar, avatar_image(draft.chat_avatar.as_deref())) {
                        cx.drop_image(stale, None);
                    }
                    card.clock.update(cx, |clock, _| clock.deadline = deadline);
                    card.draft = draft;
                    card
                }
                None => Card { text: mark_invisibles(&draft.text), bubbles: draft.bubbles(), avatar: avatar_image(draft.chat_avatar.as_deref()),
                    clock: cx.new(|_| Countdown { deadline }), draft, sending: false, error: None, editor: None },
            };
            self.cards.push(card);
        }
        for card in old { forget(card, cx); }
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
        for card in std::mem::take(&mut self.cards) { forget(card, cx); }
        self.tick(cx);
        cx.notify();
    }

    /// Tira o cartão `id` da frente; quem o tira de vez é o evento.
    fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(at) = self.cards.iter().position(|c| c.draft.id == id) { forget(self.cards.remove(at), cx); }
        self.tick(cx);
    }

    /// Os cartões cujo `servidor::sessao` de origem passa em `keep`.
    pub(super) fn render_where(&self, keep: &dyn Fn(&str) -> bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.cards.iter().filter(|c| keep(&c.draft.origin)).map(|card| self.render_card(card, cx)).collect()
    }

    /// Abre o texto do rascunho num campo do próprio cartão. Destino e anexo continuam os do rascunho.
    fn start_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.cards.iter_mut().find(|c| c.draft.id == id).filter(|c| !c.sending && c.editor.is_none()) else { return };
        let (text, original) = (card.draft.text.clone(), normalize_newlines(&card.draft.text));
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(EDIT_ROWS, EDIT_MAX_ROWS).default_value(text));
        let key = id.to_owned();
        let changes = cx.subscribe_in(&input, window, move |this: &mut Self, _, event: &InputEvent, _, cx| if matches!(event, InputEvent::Change) {
            // O erro da última tentativa era do texto de antes.
            if let Some(card) = this.cards.iter_mut().find(|c| c.draft.id == key) { card.error = None; }
            cx.notify();
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        card.editor = Some(Editor { input, original, _changes: changes });
        cx.notify();
    }

    /// Fecha o campo e volta ao texto que a sessão escreveu.
    fn undo_edit(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(card) = self.cards.iter_mut().find(|c| c.draft.id == id).filter(|c| !c.sending) else { return };
        (card.editor, card.error) = (None, None);
        cx.notify();
    }

    fn answer(&mut self, id: String, send: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.cards.iter_mut().find(|c| c.draft.id == id).filter(|c| !c.sending) else { return };
        // Só o texto mudado vai pela rota da edição: igual ao do rascunho, sai pelo Enviar normal.
        let edited = card.editor.as_ref().filter(|_| send)
            .and_then(|editor| edited_text(&editor.original, normalize_newlines(&editor.input.read(cx).value())));
        if edited.as_deref().is_some_and(|text| text_problem(text, text.chars().count(), card.draft.file.is_some()).is_some()) { return; }
        let Some(api) = self.api.clone() else {
            card.error = Some(tr("network_error"));
            cx.notify();
            return;
        };
        (card.sending, card.error) = (true, None);
        let (done, result) = tokio::sync::oneshot::channel();
        let (path, sent) = (id.clone(), edited.clone());
        self.runtime.spawn(async move {
            let (action, body) = match edited {
                Some(text) => ("send-edited", Some(json!({"via": "desktop", "text": text}))),
                None if send => ("send", Some(json!({"via": "desktop"}))),
                None => ("decline", None),
            };
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
                    // O serviço aprovou outro texto: nunca dizer que saiu o editado. O cartão fica com o aviso e sem
                    // botão, e o aviso vai também à notificação, porque o evento tira o cartão logo depois.
                    Ok(reply) if sent.as_deref().is_some_and(|text| !sent_text_confirmed(&reply, text)) => {
                        let warning = tr_shared("whatsapp_draft_err_sent_text_mismatch", &[]);
                        if let Some(card) = this.cards.iter_mut().find(|c| c.draft.id == id) { card.error = Some(warning.clone()); }
                        window.push_notification(Notification::warning(warning), cx);
                    }
                    // Quem tira o cartão de vez é o evento; aqui ele só sai da frente.
                    Ok(_) => {
                        this.remove(&id, cx);
                        if send { window.push_notification(Notification::success(tr_shared("whatsapp_draft_sending", &[])), cx); }
                    }
                    Err(error) => {
                        let code = error.code.as_deref();
                        let text = refusal_text(code, until.as_deref()).unwrap_or_else(|| Hangar::fetch_failure(&error));
                        // Expirado, respondido em outra tela ou apagado não volta: o cartão sai e o motivo vai no aviso.
                        if ends_card(code) {
                            this.remove(&id, cx);
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
        let (block, edited, problem) = match &card.editor {
            Some(editor) => {
                let field = normalize_newlines(&editor.input.read(cx).value());
                let count = field.chars().count();
                let problem = text_problem(&field, count, card.draft.file.is_some());
                let undo = edit_button("edit-undo", &id, tr_shared("whatsapp_draft_edit_undo", &[]), off).on_click(edit_on(&id, false, cx));
                let input = Textarea::new(&editor.input).aria_label(tr_shared("whatsapp_draft_text_editing", &[("count", &count.to_string())])).disabled(off);
                let hidden = invisible_count(&field);
                (TextBlock::Editing { count, hidden, field: input.into_any_element(), undo }, edited_text(&editor.original, field).is_some(), problem)
            }
            None => {
                let edit = self.edit.then(|| edit_button("edit", &id, tr_shared("whatsapp_draft_edit", &[]), off).on_click(edit_on(&id, true, cx)));
                (TextBlock::Exact { text: &card.text, edit }, false, None)
            }
        };
        let label = tr_shared(if edited { "whatsapp_draft_send_edited" } else { "whatsapp_draft_send" }, &[]);
        let send = request_button("whatsapp-send", &id, label, off || problem.is_some()).primary().on_click(answer_on(&id, true, cx));
        let decline = request_button("whatsapp-decline", &id, tr_shared("whatsapp_draft_decline", &[]), off).outline().on_click(answer_on(&id, false, cx));
        let error = problem.map(|key| tr_shared(key, &[])).or_else(|| card.error.clone());
        draft_card(&card.draft, block, &card.bubbles, card.avatar.clone(), &self.machine, card.clock.clone(), send, decline, error).into_any_element()
    }
}

/// Linhas do campo da edição: cresce de ~120 px até rolar.
const EDIT_ROWS: usize = 7;
const EDIT_MAX_ROWS: usize = 16;

/// O bloco do texto do cartão: o texto exato, com o "Editar" quando a máquina aceita edição, ou o campo da edição com o
/// "Desfazer edição" e a contagem do campo.
pub(crate) enum TextBlock<'a> {
    Exact { text: &'a MarkedText, edit: Option<Button> },
    /// `hidden`: invisíveis no campo, que o editor mostra crus.
    Editing { count: usize, hidden: usize, field: AnyElement, undo: Button },
}

/// O "Editar" e o "Desfazer edição": link à direita do rótulo do texto.
pub(crate) fn edit_button(suffix: &str, id: &str, label: String, off: bool) -> Button {
    Button::new(SharedString::from(format!("whatsapp-{suffix}-{id}"))).link().xsmall().label(label).disabled(off)
}

/// O cartão de um rascunho, só desenho, na casca dos pedidos do cofre: a tela passa o bloco do texto, os balões
/// (`Draft::bubbles`), a foto, a contagem viva e os botões com clique; a novidade da atualização, a contagem parada e os botões
/// sem clique.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draft_card(d: &Draft, block: TextBlock, bubbles: &[RecentBubble], avatar: Option<Arc<RenderImage>>, machine: &str,
    clock: impl IntoElement, send: Button, decline: Button, error: Option<String>) -> Stateful<Div> {
    let id = &d.id;
    let heading = request_heading(tr_shared("whatsapp_draft_title", &[]), machine, Some(clock.into_any_element()));
    let source = d.chat_name_source.as_deref().and_then(|s| match s {
        "contact" => Some("whatsapp_draft_source_contact"), "push" => Some("whatsapp_draft_source_push"), "group" => Some("whatsapp_draft_source_group"), _ => None,
    }).map(|key| tr_shared(key, &[]));
    let name = d.chat_name.clone().unwrap_or_default();
    let circle = || div().id(SharedString::from(format!("whatsapp-avatar-{id}"))).flex_shrink_0().size(px(AVATAR)).rounded_full();
    let photo = match avatar {
        Some(image) => circle().border_1().border_color(theme::border_strong()).role(Role::Image)
            .aria_label(tr_shared("whatsapp_draft_avatar", &[("name", d.contact())]))
            .child(img(image).size_full().rounded_full().object_fit(ObjectFit::Cover)),
        None => circle().bg(theme::accent_dim()).flex().items_center().justify_center().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::accent_text()).child(initial(d.chat_name.as_deref(), &d.chat_number)),
    };
    let destination = div().flex().items_center().gap_2().child(photo)
        .child(div().min_w_0().flex().flex_col().items_start().gap(px(3.))
            .child(div().px(px(7.)).py(px(2.)).rounded(px(5.)).bg(theme::boxed()).font_family(theme::MONO).text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(d.chat_number.clone()))
            .when(!name.is_empty() || source.is_some(), |el| el.child(div().flex().flex_wrap().items_center().gap_1().text_color(theme::muted())
                .when(!name.is_empty(), |el| el.child(name.clone()))
                .children(source.map(|s| div().px(px(5.)).rounded(px(4.)).border_1().border_color(theme::border()).child(s))))));
    let quoted = d.quoted_text.clone().unwrap_or_default();
    let mark = HighlightStyle { color: Some(theme::warning()), background_color: Some(theme::warning().opacity(0.14)), ..Default::default() };
    let text = match block {
        TextBlock::Exact { text: (shown, marks), edit } => (!d.text.is_empty() || edit.is_some()).then(|| div().flex().flex_col().gap_1()
            .child(label_row(tr_shared("whatsapp_draft_text", &[("count", &d.text.chars().count().to_string())]), edit))
            .when(!d.text.is_empty(), |el| el.child(text_box().id(SharedString::from(format!("whatsapp-text-{id}"))).max_h(px(132.)).overflow_y_scroll()
                .whitespace_normal().child(StyledText::new(shown.clone()).with_highlights(marks.iter().map(|range| (range.clone(), mark))))))),
        TextBlock::Editing { count, hidden, field, undo } => Some(div().flex().flex_col().gap_1()
            .child(label_row(tr_shared("whatsapp_draft_text_editing", &[("count", &count.to_string())]), Some(undo)))
            .child(text_box().min_h(px(EDIT_MIN_HEIGHT)).border_1().border_color(theme::warning()).child(field))
            .when(hidden > 0, |el| el.child(warn_line(tr_shared("whatsapp_draft_edit_invisibles", &[("count", &hidden.to_string())]))))),
    };
    let body = div().flex().flex_col().gap_1().text_xs().text_color(theme::text())
        .child(destination)
        .when(d.chat_name_source.as_deref() == Some("push"), |el| el.child(warn_line(tr_shared("whatsapp_draft_push_warning", &[]))))
        .when(!bubbles.is_empty(), |el| el.child(label(tr_shared("whatsapp_draft_recent", &[])))
            .child(div().flex().flex_col().gap_1().p(px(6.)).rounded(px(6.)).bg(theme::boxed().opacity(0.4))
                .children(bubbles.iter().map(|b| bubble(b, mark)))))
        .when(!quoted.is_empty(), |el| el.child(label(tr_shared("whatsapp_draft_quoted", &[])))
            .child(div().border_l_2().border_color(theme::accent()).bg(theme::accent_dim()).px_2().py_1().rounded_r(px(5.))
                .text_color(theme::muted()).whitespace_normal().child(quoted.clone())))
        .children(text)
        .when_some(d.file.as_ref(), |el, file| el.child(label(tr_shared("whatsapp_draft_attachment", &[]))).child(file_row(file))
            .when(file.outside_cwd.unwrap_or(true), |el| el.child(warn_line(tr_shared("whatsapp_draft_outside_cwd", &[])))))
        .child(request_pair(send, decline));
    request_card(id, heading, tr_shared("whatsapp_draft_wants", &[("origin", &d.origin)]), "", body, error)
}

/// Altura mínima do campo da edição, a do desenho aprovado.
const EDIT_MIN_HEIGHT: f32 = 120.;

/// A caixa do texto exato e do campo da edição: o mesmo fundo e a mesma letra nos dois.
fn text_box() -> Div {
    div().px(px(8.)).py(px(6.)).rounded(px(5.)).bg(theme::boxed()).font_family(theme::MONO).text_color(theme::text())
}

fn label(text: String) -> Div { div().mt(px(3.)).text_color(theme::faint()).child(text) }

/// O rótulo do texto com o link da edição à direita.
fn label_row(text: String, link: Option<Button>) -> Div {
    div().mt(px(3.)).flex().items_center().justify_between().gap_2()
        .child(div().min_w_0().text_color(theme::faint()).child(text))
        .children(link)
}

fn warn_line(text: String) -> Div { div().text_color(theme::warning()).whitespace_normal().child(text) }

/// Uma das últimas mensagens como balão: as minhas à direita, as dele à esquerda; embaixo, a hora (e quem, nas dele). Mídia
/// ganha a etiqueta do tipo antes do texto.
fn bubble(b: &RecentBubble, mark: HighlightStyle) -> AnyElement {
    let word = b.tag.map(|key| tr_shared(key, &[]));
    let (shown, marks) = match &b.text {
        Some((shown, marks)) => (shown.clone(), marks.as_slice()),
        None => (word.clone().unwrap_or_default().into(), &[][..]),
    };
    let text = div().min_w_0().whitespace_normal().child(StyledText::new(shown).with_highlights(marks.iter().map(|range| (range.clone(), mark))));
    let content = match word {
        Some(word) => div().flex().items_start().gap(px(5.)).text_color(theme::muted())
            .child(div().flex_shrink_0().px(px(4.)).rounded(px(4.)).bg(theme::hover()).text_size(px(10.)).child(word.to_uppercase()))
            .child(text),
        None => text,
    };
    div().max_w(relative(0.82)).px(px(8.)).py(px(4.)).rounded(px(8.))
        .map(|el| if b.from_me { el.self_end().rounded_br(px(2.)).bg(theme::success().opacity(0.16)).text_color(theme::text()) }
            else { el.self_start().rounded_bl(px(2.)).bg(theme::boxed()).text_color(theme::body_text()) })
        .child(content)
        .when(!b.meta.is_empty(), |el| el.child(div().mt(px(1.)).text_size(px(10.)).text_color(theme::faint()).child(b.meta.clone())))
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
pub(super) fn drop_outbox(map: &mut HashMap<String, Entity<WhatsAppOutbox>>, key: &str, cx: &mut App) -> bool {
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
        let edit = data.is_some_and(can_edit);
        let (api, machine) = (self.machine_api(key), self.vault_machine_label(key, cx));
        let runtime = self.runtime.clone();
        let entity = self.whatsapp_outbox.entry(key.to_owned())
            .or_insert_with(|| cx.new(|_| WhatsAppOutbox { api: None, runtime, machine: String::new(), cards: Vec::new(), clock: None, edit: false }))
            .clone();
        entity.update(cx, |outbox, cx| outbox.apply(api, machine, list, edit, cx));
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
        let outbox = cx.new(|_| WhatsAppOutbox { api: None, runtime, machine: String::new(), cards: Vec::new(), clock: None, edit: false });
        let list = parse(&json!({"drafts": [draft("d1", "pending")]})).unwrap();
        outbox.update(cx, |outbox, cx| outbox.apply(None, "casa".into(), list, true, cx));
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
    fn the_event_reads_edit_and_the_new_fields_and_an_old_draft_still_reads() {
        assert!(!can_edit(&json!({"drafts": []})), "hangar-server antigo não manda edit");
        assert!(can_edit(&json!({"drafts": [], "edit": true})));
        assert!(!can_edit(&json!({"drafts": [], "edit": false})));
        assert!(!can_edit(&json!({"drafts": [], "edit": "true"})));
        let mut d = draft("d1", "pending");
        d["chat_avatar"] = json!("/9j/AAAA");
        d["edited"] = json!(false);
        d["sent_text"] = json!("");
        d["recent"] = json!([{"from_me": false, "sender": "Carlos", "at": "2026-11-24T14:02:00Z", "text": "", "kind": "document", "file_name": "nota.pdf"}]);
        let list = parse(&json!({"drafts": [d], "edit": true})).unwrap();
        assert_eq!(list[0].chat_avatar.as_deref(), Some("/9j/AAAA"));
        let recent = list[0].recent.clone().unwrap();
        assert_eq!((recent[0].kind.as_deref(), recent[0].file_name.as_deref()), (Some("document"), Some("nota.pdf")));
        // Rascunho sem foto e sem tipo: a recente é texto.
        let old = parse(&json!({"drafts": [draft("d2", "pending")]})).unwrap();
        assert_eq!(old[0].chat_avatar, None);
        let (tag, shown) = recent_body(&old[0].recent.clone().unwrap()[0]);
        assert_eq!((tag, shown.map(|(shown, _)| shown.to_string())), (None, Some("oi".to_owned())));
        let mut no_photo = draft("d3", "pending");
        no_photo["chat_avatar"] = Value::Null;
        assert_eq!(parse(&json!({"drafts": [no_photo]})).unwrap()[0].chat_avatar, None);
    }

    #[test]
    fn an_unreadable_avatar_falls_back_to_the_initial() {
        use base64::Engine as _;
        assert!(avatar_image(None).is_none());
        assert!(avatar_image(Some("")).is_none());
        assert!(avatar_image(Some("isto não é base64!")).is_none());
        let encoded = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        assert!(avatar_image(Some(&encoded(b"not a jpeg at all"))).is_none());
        assert!(avatar_image(Some(&encoded(br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"/>"#))).is_none(), "só JPEG");
        assert!(avatar_image(Some(&encoded(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]))).is_none(), "JPEG cortado");
        assert_eq!(initial(Some("carlos"), "5511987654321"), "C");
        assert_eq!(initial(Some("  \u{200B}ana"), "5511987654321"), "A");
        assert_eq!(initial(Some("élida"), "5511987654321"), "É");
        assert_eq!(initial(None, "5511987654321"), "5");
        assert_eq!(initial(Some(""), "+55 11"), "5");
        assert_eq!(initial(None, ""), "");
    }

    fn recent(value: Value) -> Recent { serde_json::from_value(value).unwrap() }

    /// O balão como aparece no idioma da hora: a etiqueta traduzida em maiúsculas e o texto, ou a palavra do tipo.
    fn body(value: Value) -> (Option<String>, String) {
        let (tag, shown) = recent_body(&recent(value));
        let word = tag.map(|key| tr_shared(key, &[]));
        let shown = shown.map(|(shown, _)| shown.to_string()).or_else(|| word.clone()).unwrap_or_default();
        (word.map(|word| word.to_uppercase()), shown)
    }

    #[test]
    fn a_media_message_shows_its_kind_and_the_caption_the_file_name_or_the_word() {
        crate::i18n::english_in_this_thread(false);
        let tagged = |tag: &str, text: &str| (Some(tag.to_owned()), text.to_owned());
        assert_eq!(body(json!({"kind": "image", "text": "na praia"})), tagged("FOTO", "na praia"));
        assert_eq!(body(json!({"kind": "image", "text": ""})), tagged("FOTO", "foto"));
        assert_eq!(body(json!({"kind": "video"})), tagged("VÍDEO", "vídeo"));
        assert_eq!(body(json!({"kind": "audio", "text": ""})), tagged("ÁUDIO", "áudio"));
        assert_eq!(body(json!({"kind": "sticker", "text": ""})), tagged("FIGURINHA", "figurinha"));
        assert_eq!(body(json!({"kind": "document", "text": "", "file_name": "nota.pdf"})), tagged("DOCUMENTO", "nota.pdf"));
        assert_eq!(body(json!({"kind": "document", "text": "segue", "file_name": "nota.pdf"})), tagged("DOCUMENTO", "segue"));
        assert_eq!(body(json!({"kind": "document", "text": "", "file_name": ""})), tagged("DOCUMENTO", "documento"));
        // Nome de arquivo e legenda são de terceiro: os invisíveis aparecem marcados.
        assert_eq!(body(json!({"kind": "document", "text": "", "file_name": "a\u{202E}fdp.exe"})), tagged("DOCUMENTO", "a⟨U+202E⟩fdp.exe"));
        assert_eq!(body(json!({"kind": "image", "text": "x\u{200B}y"})), tagged("FOTO", "x⟨U+200B⟩y"));
        // Texto, outro e tipo desconhecido: só o texto, e o nome de arquivo fica fora.
        assert_eq!(body(json!({"kind": "text", "text": "oi", "file_name": "a.pdf"})), (None, "oi".to_owned()));
        assert_eq!(body(json!({"kind": "other", "text": "localização"})), (None, "localização".to_owned()));
        assert_eq!(body(json!({"kind": "poll", "text": "enquete"})), (None, "enquete".to_owned()));
        assert_eq!(body(json!({"text": "\u{2060}"})), (None, "⟨U+2060⟩".to_owned()));
        crate::i18n::english_in_this_thread(true);
        assert_eq!(body(json!({"kind": "image", "text": ""})), tagged("PHOTO", "photo"));
        crate::i18n::english_in_this_thread(false);
    }

    #[test]
    fn the_send_button_follows_the_field_after_normalizing_line_breaks() {
        // O cartão normaliza os dois lados uma vez (o original ao abrir o campo, o campo a cada desenho) e compara.
        let edited_text = |original: &str, field: &str| edited_text(&normalize_newlines(original), normalize_newlines(field));
        // Igual depois de normalizar: Enviar e a rota normal.
        assert_eq!(edited_text("a\r\nb", "a\nb"), None);
        assert_eq!(edited_text("a\nb", "a\r\nb"), None);
        assert_eq!(edited_text("a\rb", "a\nb"), None);
        assert_eq!(edited_text("oi", "oi"), None);
        // Diferente: Enviar texto editado, com o campo normalizado.
        assert_eq!(edited_text("oi", "olá"), Some("olá".to_owned()));
        assert_eq!(edited_text("oi", "oi\r\ntudo bem?\r"), Some("oi\ntudo bem?\n".to_owned()));
        assert_eq!(edited_text("oi", ""), Some(String::new()));
        assert_eq!(normalize_newlines("a\r\r\nb"), "a\n\nb");
    }

    #[test]
    fn the_edited_text_follows_the_service_rules() {
        let text_problem = |text: &str, has_file: bool| text_problem(text, text.chars().count(), has_file);
        assert_eq!(text_problem(&"x".repeat(4096), false), None);
        assert_eq!(text_problem(&"x".repeat(4097), false), Some("whatsapp_draft_err_text_long"));
        // Pontos de código, não bytes nem unidades UTF-16.
        assert_eq!(text_problem(&"é".repeat(4096), false), None);
        assert_eq!(text_problem(&"😀".repeat(4096), false), None);
        assert_eq!(text_problem(&"😀".repeat(4097), true), Some("whatsapp_draft_err_text_long"));
        assert_eq!(text_problem("", false), Some("whatsapp_draft_err_text_empty"));
        assert_eq!(text_problem("  \n\t", false), Some("whatsapp_draft_err_text_empty"));
        assert_eq!(text_problem("", true), None);
        assert_eq!(text_problem("oi", false), None);
    }

    #[test]
    fn bubbles_say_who_spoke_by_the_kind_of_chat() {
        let mut d = draft("d1", "pending");
        d["recent"] = json!([
            {"from_me": false, "sender": "Ana", "text": "oi"},
            {"from_me": false, "sender": "", "text": "tudo?"},
            {"from_me": true, "sender": "eu", "kind": "image", "text": ""},
        ]);
        let bubbles = |d: &Value| parse(&json!({"drafts": [d]})).unwrap()[0].bubbles();
        let metas = |d: &Value| bubbles(d).iter().map(|b| b.meta.clone()).collect::<Vec<_>>();
        // Conversa individual: o nome da conversa, senão o remetente, senão o número.
        let got = bubbles(&d);
        assert_eq!(got.iter().map(|b| (b.from_me, b.meta.as_str(), b.tag)).collect::<Vec<_>>(),
            [(false, "Carlos", None), (false, "Carlos", None), (true, "", Some("whatsapp_draft_kind_image"))]);
        assert_eq!(got[1].text.as_ref().map(|(shown, _)| shown.to_string()), Some("tudo?".to_owned()));
        assert!(got[2].text.is_none(), "foto sem legenda mostra a palavra do tipo, traduzida no desenho");
        d["chat_name"] = json!("");
        assert_eq!(metas(&d), ["Ana", "5511987654321", ""]);
        d["chat_kind"] = Value::Null;
        assert_eq!(metas(&d), ["Ana", "5511987654321", ""], "sem chat_kind, conversa individual");
        // Grupo: só o remetente; sem ele, a linha fica só com a hora.
        d["chat_kind"] = json!("group");
        d["chat_name"] = json!("Família");
        d["recent"][1]["at"] = json!("2026-11-24T14:02:00Z");
        let clock = clock("2026-11-24T14:02:00Z");
        assert_eq!(metas(&d), ["Ana".to_owned(), clock, String::new()]);
        d["recent"] = Value::Null;
        assert!(bubbles(&d).is_empty());
    }

    #[test]
    fn the_accepted_reply_must_carry_the_text_that_was_sent() {
        assert!(sent_text_confirmed(&json!({"id": "d1", "state": "sending", "sent_text": "olá\nmundo"}), "olá\nmundo"));
        assert!(sent_text_confirmed(&json!({"sent_text": "olá\r\nmundo"}), "olá\nmundo"), "o texto cru do rascunho volta com \\r\\n");
        assert!(!sent_text_confirmed(&json!({"sent_text": "oi"}), "olá"));
        assert!(!sent_text_confirmed(&json!({"id": "d1"}), "olá"));
        assert!(!sent_text_confirmed(&json!({"sent_text": null}), "olá"));
        assert!(!sent_text_confirmed(&Value::Null, ""));
    }

    #[test]
    fn edit_unsupported_has_its_own_phrase_and_keeps_the_card() {
        crate::i18n::english_in_this_thread(false);
        assert_eq!(refusal_text(Some("whatsapp_edit_unsupported"), None), Some(tr_shared("whatsapp_draft_err_edit_unsupported", &[])));
        assert!(!ends_card(Some("whatsapp_edit_unsupported")));
    }

    /// A troca de versão segura a vez do envio: o cartão diz por que, no idioma do app, não a falha genérica.
    #[test]
    fn a_held_turn_has_its_own_phrase_and_keeps_the_card() {
        for english in [false, true] {
            crate::i18n::english_in_this_thread(english);
            assert_eq!(refusal_text(Some("whatsapp_updating"), None), Some(tr("whatsapp_send_updating")));
            assert_eq!(refusal_text(Some("whatsapp_busy"), None), Some(tr("whatsapp_update_busy")));
            for key in ["whatsapp_send_updating", "whatsapp_update_busy"] { assert_ne!(tr(key), key, "{key} sem tradução ({english})"); }
        }
        crate::i18n::english_in_this_thread(false);
        assert!(!ends_card(Some("whatsapp_updating")) && !ends_card(Some("whatsapp_busy")), "o rascunho continua esperando");
    }

    #[test]
    fn refusals_that_end_the_card() {
        assert!(["whatsapp_expired", "whatsapp_answered", "whatsapp_not_found"].iter().all(|c| ends_card(Some(c))));
        assert!(!ends_card(Some("whatsapp_rate_limited")) && !ends_card(None));
        assert_eq!(refusal_text(Some("whatsapp_qualquer"), None), None);
        assert_eq!(refusal_text(None, None), None);
    }

    #[test]
    fn the_edit_field_counts_the_invisible_characters_it_hides() {
        assert_eq!(invisible_count("oi\u{200B} tudo\u{202E}bem"), 2);
        assert_eq!(invisible_count("linha 1\nlinha 2\tfim"), 0, "quebra de linha e tabulação aparecem");
        assert_eq!(invisible_count("❤️ 👨\u{200D}👩\u{200D}👧"), 0, "emoji não conta");
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
            "whatsapp_draft_err_new_contact",
            "whatsapp_draft_err_outdated", "whatsapp_draft_kind_image", "whatsapp_draft_kind_video", "whatsapp_draft_kind_audio",
            "whatsapp_draft_kind_sticker", "whatsapp_draft_kind_document", "whatsapp_draft_edit", "whatsapp_draft_edit_undo",
            "whatsapp_draft_text_editing", "whatsapp_draft_send_edited", "whatsapp_draft_err_edit_unsupported", "whatsapp_draft_err_text_empty",
            "whatsapp_draft_err_text_long", "whatsapp_draft_err_sent_text_mismatch", "whatsapp_draft_avatar", "whatsapp_draft_edit_invisibles"];
        for english in [false, true] {
            crate::i18n::english_in_this_thread(english);
            let missing: Vec<&str> = keys.into_iter().filter(|key| tr_shared(key, &[]) == *key).collect();
            assert!(missing.is_empty(), "frase sem tradução ({english}): {missing:?}");
        }
        crate::i18n::english_in_this_thread(false);
        assert_eq!(refusal_text(Some("whatsapp_outdated"), None), Some(tr_shared("whatsapp_draft_err_outdated", &[])));
        assert_eq!(refusal_text(Some("whatsapp_reachout_locked"), None), Some(tr_shared("whatsapp_draft_err_reachout_locked_unknown", &[])));
        assert_eq!(refusal_text(Some("whatsapp_new_contact"), None), Some(tr_shared("whatsapp_draft_err_new_contact", &[])));
        assert!(!ends_card(Some("whatsapp_new_contact")), "o rascunho fica pendente até expirar");
        let locked = refusal_text(Some("whatsapp_reachout_locked"), Some("2026-11-25T12:30:00Z")).unwrap();
        assert_ne!(locked, tr_shared("whatsapp_draft_err_reachout_locked_unknown", &[]));
        assert!(!locked.contains("{date}") && !locked.contains("{time}"));
    }
}
