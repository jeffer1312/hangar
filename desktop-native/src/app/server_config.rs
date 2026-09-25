//! Configuração do servidor pelo app: as páginas Notificações e Anexos (e, depois, Avançado) editam UM rascunho, e o
//! Salvar do rodapé grava tudo o que foi mexido nelas num só `POST /api/config`. Porta de `serverConfig.svelte.ts`,
//! `ServerSettings.svelte` e `LinhaConfig.svelte`. As horas silenciosas têm leitura e Salvar próprios (`PushQuiet.svelte`),
//! fora do rascunho: são gravadas no servidor e silenciam o push que chega no celular.
use super::*;
use super::device::Remote;
use super::settings::{settings_box, Page};
use gpui_kit::component::switch::Switch;
use serde_json::Map;

#[derive(Clone, Copy, PartialEq)]
enum Kind { Toggle, Number(&'static str) }

struct Field { key: &'static str, label: &'static str, help: &'static str, icon: IconName, kind: Kind, page: Page }

/// Na ordem do `CAMPOS` do web, filtrada por página.
const FIELDS: [Field; 5] = [
    Field { key: "upload_retention_days", label: "server_keep_attachments", help: "server_keep_attachments_help", icon: IconName::Paperclip,
        kind: Kind::Number("server_days"), page: Page::Attachments },
    Field { key: "notify_finished", label: "server_notify_finished", help: "server_notify_finished_help", icon: IconName::CircleCheck,
        kind: Kind::Toggle, page: Page::Notifications },
    Field { key: "finish_min_seconds", label: "server_short_turn", help: "server_short_turn_help", icon: IconName::Clock,
        kind: Kind::Number("server_seconds"), page: Page::Notifications },
    Field { key: "notify_dead", label: "server_notify_dead", help: "server_notify_dead_help", icon: IconName::CircleX,
        kind: Kind::Toggle, page: Page::Notifications },
    Field { key: "stall_seconds", label: "server_stall", help: "server_stall_help", icon: IconName::TriangleAlert,
        kind: Kind::Number("server_seconds"), page: Page::Notifications },
];

/// Páginas que leem e gravam o rascunho do servidor.
pub(super) fn is_server_page(page: Page) -> bool { matches!(page, Page::Notifications | Page::Attachments) }

#[derive(Default)]
pub(in crate::app) struct ServerConfig {
    load: Remote<()>,
    /// `campos` da última leitura ou gravação: valor, se é segredo definido e de onde veio (`origem`).
    fields: Map<String, Value>,
    /// O que foi mexido e ainda não gravado, das páginas todas. Morre só na troca de servidor.
    draft: Map<String, Value>,
    saving: bool,
    save_seq: u64,
    /// Número do salvar cujo "salvo" está no rodapé: some 2,5 s depois se nenhum outro o trocou.
    saved: Option<u64>,
    save_error: Option<String>,
    inputs: Vec<(&'static str, Entity<InputState>)>,
    quiet: Quiet,
    /// Servidor e chave do rascunho: trocar qualquer um dos dois é outro dono, e o rascunho não passa para ele.
    owner: String,
    _subscriptions: Vec<Subscription>,
}

/// Horas silenciosas: dois campos HH:MM e um Salvar próprio. Vazio num dos dois desliga a janela.
#[derive(Default)]
struct Quiet {
    load: Remote<()>,
    inputs: Option<[Entity<InputState>; 2]>,
    /// O que o servidor tem: com os campos iguais a isso não há o que salvar, e reler não apaga edição.
    loaded: [String; 2],
    /// Os campos mostram o que o servidor tem (última leitura ou gravação boa). Sem isso não se edita nem salva:
    /// campo vazio de uma leitura que falhou viraria `null` e desligaria a janela que o servidor tem.
    confirmed: bool,
    saving: bool,
    seq: u64,
    /// Resultado do último gesto (texto, é erro).
    note: Option<(String, bool)>,
    why: bool,
}

impl Quiet {
    fn editable(&self) -> bool { self.confirmed && !self.load.loading && !self.saving }
}

pub(super) enum ServerConfigReply {
    Loaded(u64, Result<Value, Failure>),
    /// Número do pedido e o corpo enviado.
    Saved(u64, Map<String, Value>, Result<Value, Failure>),
    QuietLoaded(u64, Result<Value, Failure>),
    QuietSaved(u64, [String; 2], Result<Value, Failure>),
}

impl ServerConfig {
    /// Nova conexão. Ao mesmo servidor com a mesma chave (o "Reconectar" depois de uma queda) o que foi digitado fica;
    /// o que estava em voo morre com a conexão antiga, cuja resposta o filtro de conexão já descarta.
    pub(super) fn reconnected(&mut self, owner: String) {
        if self.owner != owner {
            *self = Self { owner, ..Self::default() };
            return;
        }
        (self.load, self.saving, self.saved, self.save_error) = (Remote::default(), false, None, None);
        self.fields.clear();
        (self.quiet.load, self.quiet.saving, self.quiet.note) = (Remote::default(), false, None);
    }

    /// Valor que a tela mostra: o do rascunho, senão o do servidor.
    fn current(&self, key: &str) -> Value {
        self.draft.get(key).cloned().or_else(|| self.fields.get(key).and_then(|f| f.get("valor")).cloned()).unwrap_or(Value::Null)
    }

    fn edited_in_app(&self, key: &str) -> bool { self.fields.get(key).and_then(|f| f.get("origem")).and_then(Value::as_str) == Some("app") }

    /// Resposta da gravação: sai do rascunho só a chave cujo valor ainda é o enviado — o que foi mexido durante o
    /// salvar continua lá e vai no próximo.
    fn settle(&mut self, sent: &Map<String, Value>) {
        for (key, value) in sent {
            if self.draft.get(key) == Some(value) { self.draft.remove(key); }
        }
    }
}

fn text_of(value: &Value) -> String {
    match value { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() }
}

impl Hangar {
    fn server_config_send_later(&self) -> impl Fn(ServerConfigReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::ServerConfig(reply) }).await; })
        }
    }

    /// Página aberta: relê do servidor, mantendo o rascunho (é um só para as páginas). Com um salvar em voo não relê:
    /// a resposta dele traz os campos, e uma leitura mais velha que ela desfaria o gravado.
    pub(super) fn server_config_opened(&mut self, page: Page, cx: &mut Context<Self>) {
        if !self.server_config.saving { self.load_server_config(cx); }
        if page == Page::Notifications && !self.server_config.quiet.saving && !self.quiet_dirty(cx) { self.load_quiet(cx); }
    }

    fn load_server_config(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let s = &mut self.server_config;
        let seq = s.load.start();
        // Como o web: os campos da leitura anterior saem, para uma falha agora aparecer como falha e não como a lista velha.
        s.fields.clear();
        (s.save_error, s.saved) = (None, None);
        let done = self.server_config_send_later();
        self.runtime.spawn(async move { done(ServerConfigReply::Loaded(seq, api.config().await)).await });
        cx.notify();
    }

    fn save_server_config(&mut self, cx: &mut Context<Self>) {
        let s = &mut self.server_config;
        if s.saving || s.draft.is_empty() { return; }
        let Some(api) = self.api.clone() else { return };
        let sent = s.draft.clone();
        s.save_seq += 1;
        (s.saving, s.save_error, s.saved) = (true, None, None);
        let (seq, done) = (s.save_seq, self.server_config_send_later());
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::POST, &["config"], Some(Value::Object(sent.clone())), 8).await;
            done(ServerConfigReply::Saved(seq, sent, result)).await
        });
        cx.notify();
    }

    fn quiet_dirty(&self, cx: &App) -> bool {
        let q = &self.server_config.quiet;
        q.inputs.as_ref().is_some_and(|[start, end]| [start, end].iter().zip(&q.loaded).any(|(i, l)| i.read(cx).value() != l.as_str()))
    }

    fn load_quiet(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let q = &mut self.server_config.quiet;
        if q.load.loading { return; }
        let seq = q.load.start();
        q.note = None;
        let done = self.server_config_send_later();
        self.runtime.spawn(async move { done(ServerConfigReply::QuietLoaded(seq, api.server_read(&["push", "settings"], &[], 15).await)).await });
        cx.notify();
    }

    fn save_quiet(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let Some([start, end]) = self.server_config.quiet.inputs.clone() else { return };
        let q = &mut self.server_config.quiet;
        if !q.editable() { return; }
        let window = [start.read(cx).value().to_string(), end.read(cx).value().to_string()];
        q.seq += 1;
        (q.saving, q.note) = (true, None);
        let (seq, done) = (q.seq, self.server_config_send_later());
        let or_null = |t: &str| if t.is_empty() { Value::Null } else { Value::String(t.to_owned()) };
        let body = json!({"start": or_null(&window[0]), "end": or_null(&window[1])});
        self.runtime.spawn(async move {
            done(ServerConfigReply::QuietSaved(seq, window, api.server_send(reqwest::Method::POST, &["push", "quiet-hours"], Some(body), 15).await)).await
        });
        cx.notify();
    }

    pub(super) fn receive_server_config(&mut self, reply: ServerConfigReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            ServerConfigReply::Loaded(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).and_then(|config| match config.get("campos") {
                    Some(Value::Object(campos)) => Ok(campos.clone()),
                    _ => Err(tr("invalid_response")),
                });
                let campos = parsed.as_ref().ok().cloned();
                if !self.server_config.load.finish(seq, parsed.map(|_| ())) { return; }
                if let Some(campos) = campos { self.server_config.fields = campos; self.fill_config_inputs(window, cx); }
            }
            ServerConfigReply::Saved(seq, sent, result) => {
                let s = &mut self.server_config;
                if seq != s.save_seq { return; }
                s.saving = false;
                // Sem `campos` não dá para confirmar o que valeu: é falha, e o rascunho fica.
                let result = result.map_err(|e| Self::failure(&e)).and_then(|r| match r.get("campos") {
                    Some(Value::Object(campos)) => Ok(campos.clone()),
                    _ => Err(tr("invalid_response")),
                });
                match result {
                    Ok(campos) => {
                        s.fields = campos;
                        s.settle(&sent);
                        s.saved = Some(seq);
                        self.fill_config_inputs(window, cx);
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(2500)).await;
                            let _ = this.update(cx, |this, cx| if this.server_config.saved == Some(seq) { this.server_config.saved = None; cx.notify(); });
                        }).detach();
                    }
                    // Erro de validação do backend chega como veio ("stall_seconds: …"); o rascunho fica intacto.
                    Err(error) => s.save_error = Some(error),
                }
            }
            ServerConfigReply::QuietLoaded(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).map(|r| {
                    let part = |k: &str| r.pointer(&format!("/quiet_hours/{k}")).and_then(Value::as_str).unwrap_or("").to_owned();
                    [part("start"), part("end")]
                });
                let loaded = parsed.as_ref().ok().cloned();
                let failed = parsed.as_ref().err().cloned();
                if !self.server_config.quiet.load.finish(seq, parsed.map(|_| ())) { return; }
                self.ensure_quiet_inputs(window, cx);
                let q = &mut self.server_config.quiet;
                if let (Some(loaded), Some(inputs)) = (loaded, q.inputs.clone()) {
                    for (input, value) in inputs.iter().zip(&loaded) { input.update(cx, |state, cx| state.set_value(value.clone(), window, cx)); }
                    q.loaded = loaded;
                }
                // Falhou: o que está nos campos (vazio ou a leitura anterior) fica à vista, mas não vale como o do servidor.
                q.confirmed = failed.is_none();
                q.note = failed.map(|f| (f, true));
            }
            ServerConfigReply::QuietSaved(seq, window_sent, result) => {
                let q = &mut self.server_config.quiet;
                if seq != q.seq { return; }
                q.saving = false;
                q.note = Some(match result {
                    Ok(_) => {
                        let text = if window_sent.iter().all(|t| !t.is_empty()) {
                            tr("server_quiet_on").replace("{start}", &window_sent[0]).replace("{end}", &window_sent[1])
                        } else { tr("server_quiet_off") };
                        q.loaded = window_sent;
                        (text, false)
                    }
                    // "horario invalido (use HH:MM)" e afins chegam como vieram; o que foi digitado fica nos campos.
                    Err(error) => (Self::failure(&error), true),
                });
            }
        }
        cx.notify();
    }

    /// Campos numéricos com o valor atual. Criados na primeira leitura; nas seguintes só o que não está no rascunho muda.
    fn fill_config_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_config.inputs.is_empty() {
            for field in FIELDS.iter().filter(|f| matches!(f.kind, Kind::Number(_))) {
                let key = field.key;
                // Como o `type="number" min="0"` do web: só dígitos entram.
                let input = cx.new(|cx| InputState::new(window, cx).validate(|text, _| text.chars().all(|c| c.is_ascii_digit())));
                let sub = cx.subscribe_in(&input, window, move |this: &mut Hangar, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.server_config.draft.insert(key.to_owned(), Value::String(input.read(cx).value().to_string()));
                        cx.notify();
                    }
                });
                self.server_config._subscriptions.push(sub);
                self.server_config.inputs.push((key, input));
            }
        }
        let s = &self.server_config;
        for (key, input) in &s.inputs {
            if s.draft.contains_key(*key) { continue; }
            let value = text_of(&s.current(key));
            input.update(cx, |state, cx| if state.value() != value.as_str() { state.set_value(value, window, cx) });
        }
    }

    fn ensure_quiet_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_config.quiet.inputs.is_some() { return; }
        // O kit não tem campo de hora: a máscara só deixa entrar dígitos no formato do `<input type="time">`.
        let make = |window: &mut Window, cx: &mut Context<Self>| {
            let input = cx.new(|cx| InputState::new(window, cx).mask_pattern("99:99"));
            let sub = cx.subscribe_in(&input, window, |_: &mut Hangar, _, event: &InputEvent, _, cx| if matches!(event, InputEvent::Change) { cx.notify() });
            (input, sub)
        };
        let (start, a) = make(window, cx);
        let (end, b) = make(window, cx);
        self.server_config._subscriptions.extend([a, b]);
        self.server_config.quiet.inputs = Some([start, end]);
    }

    fn set_config_toggle(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>) {
        self.server_config.draft.insert(key.to_owned(), Value::Bool(on));
        cx.notify();
    }

    pub(super) fn render_server_page(&mut self, page: Page, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.server_config;
        let top = div().text_xl().font_weight(FontWeight::SEMIBOLD).child(page.title());
        let body = if self.api.is_none() {
            div().mt_4().text_sm().text_color(theme::muted()).child(tr("settings_offline"))
        } else if s.load.loading || (s.fields.is_empty() && s.load.value.is_none()) {
            div().mt_4().text_sm().text_color(theme::muted()).child(tr("server_loading"))
        } else if let (true, Some(Err(error))) = (s.fields.is_empty(), &s.load.value) {
            div().mt_4().flex().flex_col().items_start().gap(px(10.))
                .child(div().text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone()))
                .child(Button::new("server-config-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_server_config(cx))))
        } else {
            let rows = FIELDS.iter().filter(|f| f.page == page).map(|f| self.config_row(f, cx)).collect::<Vec<_>>();
            div().mt(px(24.)).child(settings_box().children(rows))
                .when_some(s.save_error.clone(), |el, error| el.child(div().mt_3().text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
        };
        div().flex().flex_col().child(top).child(body)
            .when(page == Page::Notifications && self.api.is_some(), |el| el.child(self.render_quiet(cx)))
            .into_any_element()
    }

    /// Chip de onde a linha grava, e "editado" quando o valor veio do app e não do `.env`.
    fn config_badges(&self, key: &str) -> Div {
        let chip = |text: String, color: Hsla, bg: Hsla| div().px(px(6.)).rounded_full().bg(bg).text_size(px(10.5))
            .font_weight(FontWeight::BOLD).text_color(color).child(text);
        div().flex().items_center().gap(px(6.))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()))
            .when(self.server_config.edited_in_app(key), |el| el.child(chip(tr("server_edited"), theme::accent_text(), theme::accent_dim())))
    }

    /// A linha de cada campo, no desenho do `row` das outras páginas, com as etiquetas ao lado do título.
    fn config_row(&self, field: &Field, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        let key = field.key;
        let control = match field.kind {
            Kind::Toggle => Switch::new(SharedString::from(format!("server-{key}"))).checked(s.current(key) == Value::Bool(true))
                .accessibility_label(tr(field.label))
                .on_click(cx.listener(move |this, on: &bool, _, cx| this.set_config_toggle(key, *on, cx))).into_any_element(),
            Kind::Number(suffix) => {
                let input = s.inputs.iter().find(|(k, _)| *k == key).map(|(_, i)| i.clone());
                div().flex().items_center().gap(px(8.))
                    .children(input.map(|i| div().w(px(96.)).child(Input::new(&i).small().aria_label(tr(field.label)))))
                    .child(div().text_size(px(13.)).text_color(theme::muted()).child(tr(suffix)))
                    .into_any_element()
            }
        };
        let head = div().flex_1().min_w_0().flex().items_center().gap(px(14.))
            .child(div().size(px(36.)).flex_shrink_0().rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
                .flex().items_center().justify_center().child(chrome::small_icon(field.icon, 16., theme::muted())))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().flex().flex_wrap().items_center().gap(px(8.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(tr(field.label))).child(self.config_badges(key)))
                .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr(field.help))));
        let row = div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_center().gap(px(14.)).px_4().py(px(14.))
            .child(head).child(div().flex_shrink_0().child(control));
        self.mark(row, field.label)
    }

    fn render_quiet(&self, cx: &mut Context<Self>) -> Div {
        let q = &self.server_config.quiet;
        let busy = !q.editable();
        let head = div().flex().flex_wrap().items_center().gap(px(8.))
            .child(chrome::small_icon(IconName::Moon, 16., theme::muted()))
            .child(div().font_weight(FontWeight::MEDIUM).child(tr("server_quiet")))
            .child(div().px(px(6.)).rounded_full().bg(theme::raised()).text_size(px(10.5)).font_weight(FontWeight::BOLD)
                .text_color(theme::muted()).child(tr("server_scope")));
        let why = div().flex().items_center().gap(px(6.)).text_size(px(13.))
            .child(div().text_color(theme::muted()).child(tr("server_quiet_verdict")))
            .child(Button::new("server-quiet-why").ghost().xsmall().label(tr("accounts_engine_why"))
                .icon(if q.why { IconName::ChevronUp } else { IconName::ChevronDown })
                .on_click(cx.listener(|this, _, _, cx| { this.server_config.quiet.why = !this.server_config.quiet.why; cx.notify(); })));
        let fields = match &q.inputs {
            None => div().text_sm().text_color(theme::muted()).child(tr("server_loading")),
            Some([start, end]) => div().flex().items_center().gap(px(10.))
                .child(div().w(px(88.)).child(Input::new(start).small().disabled(busy).aria_label(tr("server_quiet_start"))))
                .child(div().text_sm().text_color(theme::muted()).child(tr("server_quiet_and")))
                .child(div().w(px(88.)).child(Input::new(end).small().disabled(busy).aria_label(tr("server_quiet_end"))))
                .child(Button::new("server-quiet-save").outline().small().label(tr("server_save")).loading(q.saving).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.save_quiet(cx)))),
        };
        let note = q.note.clone().map(|(text, error)| div().text_sm().whitespace_normal().text_color(if error { theme::danger() } else { theme::success() }).child(text));
        let retry = (!q.confirmed && !q.load.loading && q.load.value.as_ref().is_some_and(Result::is_err)).then(|| div().flex()
            .child(Button::new("server-quiet-retry").outline().small().label(tr("server_retry")).on_click(cx.listener(|this, _, _, cx| this.load_quiet(cx)))));
        let body = div().px_4().py(px(14.)).flex().flex_col().gap(px(10.))
            .child(head).child(why)
            .when(q.why, |el| el.child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr("server_quiet_why"))))
            .child(fields).children(note).children(retry);
        div().mt(px(28.)).child(settings_box().child(self.mark(body, "server_quiet")))
    }

    /// Rodapé do Salvar, fora da rolagem: só existe com o que salvar, salvando ou com o "salvo" na tela, e grava o que foi
    /// mexido em todas as páginas do servidor — com um rascunho só, é o único significado honesto do botão.
    pub(super) fn server_config_footer(&self, page: Page, cx: &mut Context<Self>) -> Option<Div> {
        let s = &self.server_config;
        let dirty = !s.draft.is_empty();
        if !is_server_page(page) || s.load.loading || s.fields.is_empty() || !(dirty || s.saving || s.saved.is_some()) { return None; }
        // O botão fica na mesma coluna de 720px do conteúdo, não na borda da janela.
        let column = div().w(px(720.)).max_w_full().px_4().flex().items_center().justify_end().gap(px(12.))
            .when(s.saved.is_some(), |el| el.child(div().text_size(px(12.5)).text_color(theme::success()).child(tr("server_saved"))))
            .when(dirty || s.saving, |el| el.child(Button::new("server-config-save").primary().small()
                .label(tr(if s.saving { "server_saving" } else { "server_save" })).loading(s.saving).disabled(s.saving)
                .on_click(cx.listener(|this, _, _, cx| this.save_server_config(cx)))));
        Some(div().h(px(56.)).flex_shrink_0().border_t_1().border_color(theme::border()).bg(theme::chrome())
            .flex().items_center().justify_center().child(column))
    }
}

#[cfg(test)]
mod tests {
    use super::{Quiet, ServerConfig, text_of};
    use serde_json::{Value, json};

    #[test]
    fn reconnecting_keeps_the_draft_only_for_the_same_owner() {
        let mut s = ServerConfig::default();
        s.reconnected("http://a/\nt".into());
        s.draft.insert("notify_dead".into(), json!(false));
        (s.quiet.confirmed, s.saving) = (true, true);
        s.reconnected("http://a/\nt".into());
        assert!(s.draft.contains_key("notify_dead") && s.quiet.confirmed && !s.saving, "Reconectar ao mesmo: fica, e o voo antigo morre");
        s.reconnected("http://b/\nt".into());
        assert!(s.draft.is_empty() && !s.quiet.confirmed, "outro servidor: nada passa");
        s.draft.insert("notify_dead".into(), json!(false));
        s.reconnected("http://b/\noutra".into());
        assert!(s.draft.is_empty(), "outra chave também é outro dono");
    }

    #[test]
    fn quiet_hours_only_save_what_a_read_confirmed() {
        let mut q = Quiet::default();
        assert!(!q.editable(), "nunca lido");
        let seq = q.load.start();
        assert!(!q.editable(), "lendo");
        q.load.finish(seq, Err("falhou".into()));
        assert!(!q.editable(), "leitura falhou: vazio não pode virar null no servidor");
        q.confirmed = true;
        assert!(q.editable());
        q.saving = true;
        assert!(!q.editable(), "salvando");
    }

    #[test]
    fn save_keeps_what_changed_during_the_request() {
        let mut s = ServerConfig::default();
        s.draft.insert("stall_seconds".into(), json!("600"));
        s.draft.insert("notify_dead".into(), json!(false));
        let sent = s.draft.clone();
        // Durante o salvar a pessoa muda um dos campos de novo.
        s.draft.insert("stall_seconds".into(), json!("700"));
        s.settle(&sent);
        assert_eq!(s.draft.get("stall_seconds"), Some(&json!("700")));
        assert!(!s.draft.contains_key("notify_dead"));
    }

    #[test]
    fn current_prefers_the_draft_and_reads_the_origin() {
        let mut s = ServerConfig::default();
        s.fields.insert("stall_seconds".into(), json!({"valor": 900, "origem": "app"}));
        s.fields.insert("notify_dead".into(), json!({"valor": true, "origem": "env"}));
        assert_eq!(text_of(&s.current("stall_seconds")), "900");
        s.draft.insert("stall_seconds".into(), json!("30"));
        assert_eq!(text_of(&s.current("stall_seconds")), "30");
        assert!(s.edited_in_app("stall_seconds") && !s.edited_in_app("notify_dead"));
        assert_eq!(s.current("missing"), Value::Null);
    }
}
