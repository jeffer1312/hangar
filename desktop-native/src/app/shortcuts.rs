//! Atalhos: a fileira de ações do painel da sessão, deste servidor (`runtime_config.shortcuts`, JSON numa string; vazio =
//! os cinco nativos). Porta de `packages/core/src/shortcuts.ts` e do editor `ShortcutsSettings.svelte`: a página edita a
//! lista inteira e grava de uma vez (`POST /api/config`); o painel lateral lê a mesma resolução, e o que for salvo ou lido
//! aqui aparece lá na hora.
use super::*;
use super::device::Remote;
use super::settings::{segments, settings_box};
use serde_json::Map;

/// Os botões nativos de hoje, na ordem de hoje. Só "anexos" roda no painel nativo; os outros são preservados na ordem.
pub(super) const NATIVES: [&str; 5] = ["terminal", "modo", "navegador", "anexos", "rodar"];

/// Glifos curados do web (`ShortcutIcon.svelte`, `GLYPHS`), com o par no Lucide do kit (mesma família de traço).
const GLYPHS: [(&str, IconName); 12] = [("bolt", IconName::Zap), ("play", IconName::Play), ("rocket", IconName::Rocket),
    ("gear", IconName::Settings), ("git", IconName::GitBranch), ("chat", IconName::MessageCircle), ("star", IconName::Star),
    ("folder", IconName::Folder), ("key", IconName::Key), ("terminal", IconName::SquareTerminal), ("globe", IconName::Globe),
    ("robot", IconName::Bot)];

const LABEL_MAX: usize = 24;
const EMOJI_MAX: usize = 4;

/// Um atalho já validado, guardado como o objeto que veio: gravar de volta não perde campo que esta versão não conhece.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Item(Map<String, Value>);

impl Item {
    fn text(&self, key: &str) -> &str { self.0.get(key).and_then(Value::as_str).unwrap_or("") }
    pub(super) fn id(&self) -> &str { self.text("id") }
    pub(super) fn kind(&self) -> &str { self.text("type") }
    pub(super) fn action(&self) -> &str { self.text("action") }
    pub(super) fn label(&self) -> &str { self.text("label") }
    /// Texto do `send_text` ou comando do `shell`.
    pub(super) fn content(&self) -> &str { self.text(if self.kind() == "shell" { "command" } else { "text" }) }
    pub(super) fn icon(&self) -> Option<&str> { self.0.get("icon").and_then(Value::as_str) }
    pub(super) fn confirm(&self) -> bool { self.0.get("confirm") == Some(&Value::Bool(true)) }
    /// Ausente = envia direto; desligado pré-preenche o campo de mensagem.
    pub(super) fn sends_direct(&self) -> bool { self.0.get("send_direct") != Some(&Value::Bool(false)) }

    fn native(action: &str) -> Self {
        Item(Map::from_iter([("id".into(), json!(action)), ("type".into(), json!("internal")), ("action".into(), json!(action))]))
    }

    /// `isValid` do web: opcional com tipo errado derrubaria quem lê, então o item inteiro sai.
    fn valid(o: &Map<String, Value>) -> bool {
        let filled = |key: &str| o.get(key).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty());
        let optional = |key: &str, ok: fn(&Value) -> bool| o.get(key).is_none_or(ok);
        if !filled("id") || !optional("icon", Value::is_string) || !optional("confirm", Value::is_boolean)
            || !optional("send_direct", Value::is_boolean) { return false; }
        match o.get("type").and_then(Value::as_str) {
            Some("internal") => o.get("action").and_then(Value::as_str).is_some_and(|a| NATIVES.contains(&a)),
            Some("send_text") => filled("label") && filled("text"),
            Some("shell") => filled("label") && filled("command"),
            _ => false,
        }
    }
}

pub(super) fn defaults() -> Vec<Item> { NATIVES.map(Item::native).to_vec() }

/// `resolveShortcuts`: nunca falha. Vazio, JSON quebrado ou forma estranha voltam aos nativos; item inválido sai sozinho;
/// id repetido fica o primeiro.
pub(super) fn resolve(raw: &str) -> Vec<Item> {
    if raw.trim().is_empty() { return defaults(); }
    let Ok(Value::Array(data)) = serde_json::from_str::<Value>(raw) else { return defaults() };
    let mut seen = HashSet::new();
    data.into_iter().filter_map(|v| match v { Value::Object(o) if Item::valid(&o) => Some(Item(o)), _ => None })
        .filter(|item| seen.insert(item.id().to_owned())).collect()
}

fn serialize(items: &[Item]) -> String {
    Value::Array(items.iter().map(|item| Value::Object(item.0.clone())).collect()).to_string()
}

/// Ícone da config ("emoji:🚀" | "glifo:bolt" | ausente): glifo desconhecido cai no raio, como no web.
pub(super) enum Glyph { Icon(IconName), Emoji(String) }

pub(super) fn parse_icon(icon: Option<&str>) -> Glyph {
    if let Some(e) = icon.and_then(|i| i.strip_prefix("emoji:")).map(str::trim).filter(|e| !e.is_empty()) { return Glyph::Emoji(e.to_owned()); }
    let name = icon.and_then(|i| i.strip_prefix("glifo:")).unwrap_or("bolt");
    Glyph::Icon(GLYPHS.iter().find(|(g, _)| *g == name).map_or(IconName::Zap, |(_, i)| *i))
}

pub(super) fn icon_element(icon: Option<&str>, size: f32, color: Hsla) -> AnyElement {
    match parse_icon(icon) {
        Glyph::Icon(name) => chrome::small_icon(name, size, color).into_any_element(),
        Glyph::Emoji(e) => div().text_size(px(size)).line_height(px(size + 2.)).child(e).into_any_element(),
    }
}

/// Rótulo e ícone dos nativos (`INTERNAL_LABEL`/`INTERNAL_ICON` do web): vêm do app, não da config.
fn native_label(action: &str) -> String { tr(&format!("shortcuts_native_{action}")) }

fn native_icon(action: &str) -> &'static str {
    match action { "terminal" => "glifo:terminal", "modo" => "glifo:git", "navegador" => "glifo:globe", "anexos" => "glifo:folder", _ => "glifo:play" }
}

/// Até `max` unidades UTF-16, o que o `maxlength` do web conta.
fn clip(text: &str, max: usize) -> Option<String> {
    if text.encode_utf16().count() <= max { return None; }
    let mut out = String::new();
    for c in text.chars() {
        if out.encode_utf16().count() + c.len_utf16() > max { break; }
        out.push(c);
    }
    Some(out)
}

/// `formId` do web para um atalho novo: "a-<tempo base 36>-<4 aleatórios>".
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let base36 = |mut n: u128| {
        let mut s = String::new();
        loop {
            s.insert(0, char::from_digit((n % 36) as u32, 36).unwrap_or('0'));
            n /= 36;
            if n == 0 { break s; }
        }
    };
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(nanos);
    format!("a-{}-{}", base36(nanos / 1_000_000), base36(u128::from(hasher.finish()) % 1_679_616))
}

/// Formulário de adicionar/editar, aberto abaixo da lista (como o web, sem diálogo).
struct Form {
    /// Id do atalho sendo editado; `None` = novo.
    editing: Option<String>,
    /// O objeto gravado do atalho editado.
    original: Option<Map<String, Value>>,
    shell: bool,
    label: Entity<InputState>,
    emoji: Entity<InputState>,
    content: Entity<InputState>,
    glyph: String,
    direct: bool,
    confirm: bool,
    _subscriptions: Vec<Subscription>,
}

impl Form {
    fn value(input: &Entity<InputState>, cx: &App) -> String { input.read(cx).value().trim().to_owned() }
    /// `formValid`: rótulo e texto/comando preenchidos.
    fn valid(&self, cx: &App) -> bool { !Self::value(&self.label, cx).is_empty() && !Self::value(&self.content, cx).is_empty() }
}

/// O que o formulário confirmou, já lido dos campos.
struct Draft { shell: bool, label: String, content: String, icon: String, direct: bool, confirm: bool }

impl Draft {
    /// Editar parte do objeto gravado: campo que esta versão não conhece continua lá; os do formulário são reescritos.
    fn into_item(self, original: Option<Map<String, Value>>, id: String) -> Item {
        let mut o = original.unwrap_or_default();
        for key in ["send_direct", "confirm"] { o.remove(key); }
        o.insert("id".into(), json!(id));
        o.insert("type".into(), json!(if self.shell { "shell" } else { "send_text" }));
        o.insert((if self.shell { "command" } else { "text" }).into(), json!(self.content));
        if !self.shell && !self.direct { o.insert("send_direct".into(), json!(false)); }
        o.insert("label".into(), json!(self.label));
        o.insert("icon".into(), json!(self.icon));
        if self.confirm { o.insert("confirm".into(), json!(true)); }
        Item(o)
    }
}

/// Atalho novo vai para o fim; o editado troca de lugar com ele mesmo. Editado que saiu da lista com o formulário aberto
/// (removido, lista restaurada) não volta pela edição.
fn apply_edit(items: &mut Vec<Item>, editing: Option<&str>, item: Item) {
    match editing {
        Some(id) => if let Some(n) = items.iter().position(|i| i.id() == id) { items[n] = item; },
        None => items.push(item),
    }
}

/// Atalho sendo arrastado: o id para achar a posição e o rótulo para o que segue o ponteiro.
#[derive(Clone)]
struct Dragged { id: String, label: String }

impl Render for Dragged {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_3().py(px(6.)).rounded(px(8.)).border_1().border_color(theme::border_strong()).bg(theme::raised())
            .shadow(theme::popover_shadow()).text_sm().child(self.label.clone())
    }
}

#[derive(Default)]
pub(in crate::app) struct Shortcuts {
    load: Remote<()>,
    items: Vec<Item>,
    dirty: bool,
    saving: bool,
    /// Número do salvar cujo "Salvo" está na tela: some 2,5 s depois se nenhum outro o trocou.
    saved: Option<u64>,
    save_seq: u64,
    save_error: Option<String>,
    form: Option<Form>,
    suggestions: Vec<String>,
    suggesting: bool,
    /// Linha que começou o arrasto em curso; só vale enquanto a GPUI tem um arrasto ativo.
    dragging: Option<String>,
}

pub(super) enum ShortcutsReply {
    Loaded(u64, Result<Value, Failure>),
    /// Número do pedido e a lista gravada (`None` = restaurar padrão).
    Saved(u64, Option<Vec<Item>>, Result<Value, Failure>),
    Commands(Result<Vec<CommandInfo>, Failure>),
}

impl Hangar {
    fn shortcuts_send_later(&self) -> impl Fn(ShortcutsReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Shortcuts(reply) }).await; })
        }
    }

    /// Página aberta: edição de uma visita anterior não volta (o web desmonta o editor); relê do servidor.
    /// Os números dos pedidos ficam, para resposta de antes não passar por resposta de agora.
    pub(super) fn shortcuts_opened(&mut self, cx: &mut Context<Self>) {
        let (load, save_seq, saving) = (std::mem::take(&mut self.shortcuts.load.seq), self.shortcuts.save_seq, self.shortcuts.saving);
        self.shortcuts = Shortcuts { save_seq, saving, ..Shortcuts::default() };
        self.shortcuts.load.seq = load;
        self.load_shortcuts(cx);
    }

    fn load_shortcuts(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.shortcuts.load.start();
        let done = self.shortcuts_send_later();
        self.runtime.spawn(async move { done(ShortcutsReply::Loaded(seq, api.config().await)).await });
        cx.notify();
    }

    fn save_shortcuts(&mut self, restore: bool, cx: &mut Context<Self>) {
        let s = &mut self.shortcuts;
        if s.saving { return; }
        let Some(api) = self.api.clone() else { return };
        let list = (!restore).then(|| s.items.clone());
        s.save_seq += 1;
        (s.saving, s.save_error, s.saved) = (true, None, None);
        let (seq, done) = (s.save_seq, self.shortcuts_send_later());
        let body = json!({"shortcuts": list.as_deref().map(serialize)});
        self.runtime.spawn(async move {
            done(ShortcutsReply::Saved(seq, list, api.server_send(reqwest::Method::POST, &["config"], Some(body), 8).await)).await
        });
        cx.notify();
    }

    /// Sugestões de skill: comandos da primeira sessão viva deste servidor. Sem sessão, o campo fica livre.
    fn load_suggestions(&mut self) {
        if self.shortcuts.suggesting || !self.shortcuts.suggestions.is_empty() { return; }
        let Some(api) = self.api.clone() else { return };
        let Some(name) = self.sessions.iter().find(|s| s.state != "dead").map(|s| s.name.clone()) else { return };
        self.shortcuts.suggesting = true;
        let done = self.shortcuts_send_later();
        self.runtime.spawn(async move { done(ShortcutsReply::Commands(api.commands(&name).await)).await });
    }

    pub(super) fn receive_shortcuts(&mut self, reply: ShortcutsReply, cx: &mut Context<Self>) {
        let s = &mut self.shortcuts;
        match reply {
            ShortcutsReply::Loaded(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).and_then(|config| match config.pointer("/campos") {
                    Some(campos) => Ok(resolve(campos.pointer("/shortcuts/valor").and_then(Value::as_str).unwrap_or(""))),
                    None => Err(tr("invalid_response")),
                });
                let list = parsed.as_ref().ok().cloned();
                if !s.load.finish(seq, parsed.map(|_| ())) { return; }
                if let Some(list) = list {
                    (s.items, s.dirty) = (list.clone(), false);
                    self.side.set_shortcuts(&list);
                }
            }
            ShortcutsReply::Saved(seq, list, result) => {
                let saved = list.unwrap_or_else(defaults);
                // Gravado vale para o painel mesmo com a página já fechada.
                if result.is_ok() { self.side.set_shortcuts(&saved); }
                if seq != s.save_seq { return; }
                s.saving = false;
                match result {
                    Ok(_) => {
                        (s.items, s.dirty) = (saved, false);
                        s.saved = Some(seq);
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(2500)).await;
                            let _ = this.update(cx, |this, cx| if this.shortcuts.saved == Some(seq) { this.shortcuts.saved = None; cx.notify(); });
                        }).detach();
                    }
                    // Erro de validação do backend chega como veio ("shortcuts: item 2 …").
                    Err(error) => s.save_error = Some(Self::failure(&error)),
                }
            }
            ShortcutsReply::Commands(result) => {
                s.suggesting = false;
                if let Ok(commands) = result {
                    s.suggestions = commands.into_iter().map(|c| if c.display.is_empty() { format!("/{}", c.name) } else { c.display }).collect();
                }
            }
        }
        cx.notify();
    }

    fn shortcuts_edit(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Vec<Item>)) {
        if self.shortcuts.saving { return; }
        change(&mut self.shortcuts.items);
        self.shortcuts.dirty = true;
        cx.notify();
    }

    fn shortcut_position(&self, id: &str) -> Option<usize> { self.shortcuts.items.iter().position(|i| i.id() == id) }

    fn move_shortcut(&mut self, id: &str, delta: isize, cx: &mut Context<Self>) {
        let Some(i) = self.shortcut_position(id) else { return };
        let Some(j) = i.checked_add_signed(delta).filter(|j| *j < self.shortcuts.items.len()) else { return };
        self.shortcuts_edit(cx, |items| items.swap(i, j));
    }

    /// Soltar sobre outra linha: o arrastado passa a ocupar o lugar dela. Soltar fora de uma linha não muda nada.
    fn drop_shortcut(&mut self, from: &str, onto: &str, cx: &mut Context<Self>) {
        let (Some(i), Some(j)) = (self.shortcut_position(from), self.shortcut_position(onto)) else { return };
        if i == j { return; }
        self.shortcuts_edit(cx, |items| { let item = items.remove(i); items.insert(j, item); });
    }

    fn open_shortcut_form(&mut self, editing: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let item = editing.as_deref().and_then(|id| self.shortcuts.items.iter().find(|i| i.id() == id)).cloned();
        if editing.is_some() && item.as_ref().is_none_or(|i| i.kind() == "internal") { return; }
        let shell = item.as_ref().is_some_and(|i| i.kind() == "shell");
        let field = |value: String, placeholder: String, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(placeholder);
            state.set_value(value, window, cx);
            state
        });
        let label = field(item.as_ref().map(|i| i.label().to_owned()).unwrap_or_default(), String::new(), window, cx);
        let (glyph, emoji) = match item.as_ref().and_then(Item::icon) {
            Some(icon) if icon.starts_with("emoji:") => ("bolt".to_owned(), icon["emoji:".len()..].to_owned()),
            // Glifo que esta versão não conhece fica com o nome (sem seleção na grade) e volta igual ao gravar, como no web.
            icon => (icon.and_then(|i| i.strip_prefix("glifo:")).unwrap_or("bolt").to_owned(), String::new()),
        };
        let emoji = field(emoji, tr("shortcuts_emoji_hint"), window, cx);
        let content = field(item.as_ref().map(|i| i.content().to_owned()).unwrap_or_default(),
            tr(if shell { "shortcuts_command_hint" } else { "shortcuts_text_hint" }), window, cx);
        let mut subscriptions = Vec::new();
        for (input, max) in [(&label, Some(LABEL_MAX)), (&emoji, Some(EMOJI_MAX)), (&content, None)] {
            subscriptions.push(cx.subscribe_in(input, window, move |this: &mut Hangar, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(clipped) = max.and_then(|max| clip(&input.read(cx).value(), max)) {
                        input.update(cx, |state, cx| state.set_value(clipped, window, cx));
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.submit_shortcut_form(window, cx),
                _ => {}
            }));
        }
        label.update(cx, |state, cx| state.focus(window, cx));
        self.shortcuts.form = Some(Form { editing, original: item.as_ref().map(|i| i.0.clone()), shell, label, emoji, content, glyph, direct: item.as_ref().is_none_or(Item::sends_direct),
            confirm: item.as_ref().is_some_and(Item::confirm), _subscriptions: subscriptions });
        if !shell { self.load_suggestions(); }
        cx.notify();
    }

    fn submit_shortcut_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Com um salvar em voo a lista não muda: o formulário fica aberto com o que foi digitado até ele terminar.
        if self.shortcuts.saving { return; }
        let Some(form) = self.shortcuts.form.as_ref().filter(|f| f.valid(cx)) else { return };
        let emoji = Form::value(&form.emoji, cx);
        let icon = if emoji.is_empty() { format!("glifo:{}", form.glyph) } else { format!("emoji:{emoji}") };
        let draft = Draft { shell: form.shell, label: Form::value(&form.label, cx), content: Form::value(&form.content, cx), icon,
            direct: form.direct, confirm: form.confirm };
        let editing = form.editing.clone();
        let item = draft.into_item(form.original.clone(), editing.clone().unwrap_or_else(new_id));
        self.close_shortcut_form(window, cx);
        self.shortcuts_edit(cx, |items| apply_edit(items, editing.as_deref(), item));
    }

    /// O campo com foco sai junto com o formulário: o foco volta à raiz, senão o Esc seguinte não chega a lugar nenhum.
    fn close_shortcut_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shortcuts.form = None;
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    /// Esc com o formulário aberto fecha só ele.
    pub(super) fn shortcuts_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.settings != Some(super::settings::Page::Shortcuts) || self.shortcuts.form.is_none() { return false; }
        self.close_shortcut_form(window, cx);
        true
    }

    pub(super) fn render_shortcuts_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let page = div().flex().flex_col().child(self.page_top("settings_page_shortcuts", tr("shortcuts_lead")));
        let note = |text: String, color: Hsla| div().px_4().py(px(18.)).text_size(px(13.)).text_color(color).whitespace_normal().child(text);
        if self.api.is_none() { return page.child(settings_box().mt(px(24.)).child(note(tr("settings_offline"), theme::muted()))).into_any_element(); }
        let s = &self.shortcuts;
        match (&s.load.value, s.load.loading) {
            (None, _) => return page.child(settings_box().mt(px(24.)).child(note(tr("shortcuts_loading"), theme::muted()))).into_any_element(),
            (Some(Err(error)), loading) => {
                let retry = Button::new("shortcuts-retry").outline().small().icon(IconName::RefreshCw)
                    .label(tr(if loading { "shortcuts_loading" } else { "shortcuts_retry" })).disabled(loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load_shortcuts(cx)));
                return page.child(settings_box().mt(px(24.))
                    .child(note(format!("{} {error}", tr("shortcuts_load_failed")), theme::danger()))
                    .child(div().px_4().pb(px(16.)).flex().child(retry))).into_any_element();
            }
            _ => {}
        }
        let saving = s.saving;
        let count = s.items.len();
        let mut list = settings_box().mt(px(24.));
        if count == 0 { list = list.child(note(tr("shortcuts_empty"), theme::muted())); }
        for (n, item) in s.items.iter().enumerate() {
            list = list.child(self.render_shortcut_row(item, n, count, saving, cx));
        }
        let missing: Vec<&str> = NATIVES.into_iter().filter(|a| !s.items.iter().any(|i| i.kind() == "internal" && i.action() == *a)).collect();
        let restore_natives = (!missing.is_empty()).then(|| div().mt(px(12.)).flex().flex_wrap().items_center().gap(px(8.))
            .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("shortcuts_restore_native")))
            .children(missing.into_iter().map(|action| Button::new(SharedString::from(format!("shortcut-native-{action}"))).outline().small()
                .icon(IconName::Plus).label(native_label(action)).disabled(saving)
                .on_click(cx.listener(move |this, _, _, cx| this.shortcuts_edit(cx, |items| items.push(Item::native(action))))))));
        let form = match &s.form {
            Some(form) => self.render_shortcut_form(form, cx),
            None => self.mark(div().mt(px(16.)).flex(), "shortcuts_add").child(Button::new("shortcut-add").outline().small().icon(IconName::Plus).label(tr("shortcuts_add"))
                .disabled(saving).on_click(cx.listener(|this, _, window, cx| this.open_shortcut_form(None, window, cx)))),
        };
        let feedback = match (&s.save_error, s.saved) {
            (Some(error), _) => Some(div().text_color(theme::danger()).child(error.clone())),
            (None, Some(_)) => Some(div().text_color(theme::success()).child(tr("shortcuts_saved"))),
            _ => None,
        };
        let footer = self.mark(div().mt(px(24.)).pt(px(16.)), "shortcuts_restore").border_t_1().border_color(theme::border()).flex().items_center().gap(px(10.))
            .child(Button::new("shortcuts-restore").outline().small().label(tr("shortcuts_restore")).tooltip(tr("shortcuts_restore_help"))
                .disabled(saving).on_click(cx.listener(|this, _, _, cx| this.save_shortcuts(true, cx))))
            .child(div().flex_1().min_w_0().flex().justify_end().text_size(px(12.5)).whitespace_normal().children(feedback))
            .child(Button::new("shortcuts-save").primary().small().label(tr("shortcuts_save")).loading(saving).disabled(!s.dirty || saving)
                .on_click(cx.listener(|this, _, _, cx| this.save_shortcuts(false, cx))));
        page.child(list).children(restore_natives).child(form).child(footer).into_any_element()
    }

    fn render_shortcut_row(&self, item: &Item, n: usize, count: usize, saving: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let native = item.kind() == "internal";
        let (label, icon) = if native { (native_label(item.action()), Some(native_icon(item.action()))) } else { (item.label().to_owned(), item.icon()) };
        let id = item.id().to_owned();
        let button = |key: &str, icon: IconName, tip: &str, off: bool| chrome::icon_button(SharedString::from(format!("shortcut-{key}-{id}")), icon, tr(tip), cx)
            .small().disabled(off || saving);
        let (up, down, remove, edit) = (id.clone(), id.clone(), id.clone(), id.clone());
        let actions = div().flex().flex_shrink_0().gap(px(2.))
            .when(!native, |el| el.child(button("edit", IconName::Pencil, "shortcuts_edit", false)
                .on_click(cx.listener(move |this, _, window, cx| this.open_shortcut_form(Some(edit.clone()), window, cx)))))
            .child(button("up", IconName::ArrowUp, "shortcuts_up", n == 0).on_click(cx.listener(move |this, _, _, cx| this.move_shortcut(&up, -1, cx))))
            .child(button("down", IconName::ArrowDown, "shortcuts_down", n + 1 == count).on_click(cx.listener(move |this, _, _, cx| this.move_shortcut(&down, 1, cx))))
            .child(button("remove", IconName::Close, "shortcuts_remove", false)
                .on_click(cx.listener(move |this, _, _, cx| this.shortcuts_edit(cx, |items| items.retain(|i| i.id() != remove)))));
        let text = div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().font_weight(FontWeight::MEDIUM).truncate().child(label.clone()))
            .when(!native, |el| el.child(div().text_size(px(12.)).font_family(theme::MONO).text_color(theme::muted()).truncate().child(item.content().to_owned())));
        let dragged = Dragged { id: id.clone(), label };
        // A linha que saiu do lugar esmaece, como a `.linha.arrastando` do web.
        let lifted = cx.has_active_drag() && self.shortcuts.dragging.as_deref() == Some(id.as_str());
        let this = cx.entity().downgrade();
        // Divisória em cima de toda linha, como nas outras páginas; a da primeira some sob a borda da caixa.
        div().id(SharedString::from(format!("shortcut-row-{id}"))).mt(px(-1.)).border_t_1().border_color(theme::border())
            .flex().items_center().gap(px(12.)).px_4().py(px(10.))
            .child(chrome::small_icon(IconName::GripVertical, 16., theme::faint()))
            .child(div().size(px(36.)).flex_shrink_0().rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
                .flex().items_center().justify_center().child(icon_element(icon, 16., theme::muted())))
            .child(text)
            .child(actions)
            .when(lifted, |el| el.opacity(0.45))
            .when(!saving, |el| el.on_drag(dragged, move |d, _, _, cx| {
                    this.update(cx, |this, cx| { this.shortcuts.dragging = Some(d.id.clone()); cx.notify(); }).ok();
                    cx.new(|_| d.clone())
                })
                .drag_over::<Dragged>(|style, _, _, _| style.bg(theme::accent_dim()))
                .on_drop(cx.listener(move |this, d: &Dragged, _, cx| this.drop_shortcut(&d.id, &id, cx))))
    }

    fn render_shortcut_form(&self, form: &Form, cx: &mut Context<Self>) -> Div {
        let field = |key: &str, control: AnyElement| div().flex().flex_col().gap(px(6.))
            .child(div().text_size(px(13.)).text_color(theme::muted()).child(tr(key))).child(control);
        let editing = form.editing.is_some();
        let kinds = [tr("shortcuts_type_send"), tr("shortcuts_type_shell")];
        let kind = segments("shortcut-type", &kinds, form.shell as usize, if editing { 0 } else { 2 }, editing, String::new(),
            |this, n, window, cx| {
                let Some(form) = this.shortcuts.form.as_mut() else { return };
                form.shell = n == 1;
                let hint = tr(if form.shell { "shortcuts_command_hint" } else { "shortcuts_text_hint" });
                form.content.update(cx, |state, cx| state.set_placeholder(hint, window, cx));
                if n == 0 { this.load_suggestions(); }
                cx.notify();
            }, cx);
        let emoji_set = !Form::value(&form.emoji, cx).is_empty();
        let glyphs = div().flex().flex_wrap().items_center().gap(px(2.))
            .children(GLYPHS.iter().map(|(g, icon)| {
                let on = !emoji_set && form.glyph == *g;
                Button::new(SharedString::from(format!("shortcut-glyph-{g}")))
                    .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
                        .foreground(if on { theme::accent_text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
                    .size(px(34.)).rounded(px(8.)).icon(Icon::new(*icon).size(px(16.))).accessibility_label(*g).tooltip(*g)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let Some(form) = this.shortcuts.form.as_mut() else { return };
                        form.glyph = (*g).to_owned();
                        form.emoji.update(cx, |state, cx| state.set_value("", window, cx));
                        cx.notify();
                    }))
            }))
            .child(div().ml(px(8.)).w(px(110.)).child(Input::new(&form.emoji).small()));
        let typed = Form::value(&form.content, cx);
        let picks: Vec<String> = if form.shell { Vec::new() } else {
            self.shortcuts.suggestions.iter().filter(|s| **s != typed && s.to_lowercase().contains(&typed.to_lowercase())).take(6).cloned().collect()
        };
        let content = div().flex().flex_col().gap(px(6.)).child(Input::new(&form.content))
            .when(!picks.is_empty(), |el| el.child(div().flex().flex_wrap().gap(px(6.)).children(picks.into_iter().map(|pick| {
                Button::new(SharedString::from(format!("shortcut-pick-{pick}"))).ghost().small().label(pick.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(form) = this.shortcuts.form.as_ref() { form.content.update(cx, |state, cx| state.set_value(pick.clone(), window, cx)); }
                        cx.notify();
                    }))
            }))));
        let direct = (!form.shell).then(|| div().flex().flex_col().gap(px(2.))
            .child(Checkbox::new("shortcut-direct").label(tr("shortcuts_send_direct")).checked(form.direct)
                .on_click(cx.listener(|this, on: &bool, _, cx| { if let Some(f) = this.shortcuts.form.as_mut() { f.direct = *on; } cx.notify(); })))
            .child(div().pl(px(24.)).text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(tr("shortcuts_send_direct_help"))));
        let confirm = Checkbox::new("shortcut-confirm").label(tr("shortcuts_confirm")).checked(form.confirm)
            .on_click(cx.listener(|this, on: &bool, _, cx| { if let Some(f) = this.shortcuts.form.as_mut() { f.confirm = *on; } cx.notify(); }));
        settings_box().mt(px(16.)).p(px(16.)).gap(px(16.))
            .child(field("shortcuts_type", div().flex().child(kind).into_any_element()))
            .child(field("shortcuts_label", Input::new(&form.label).into_any_element()))
            .child(field("shortcuts_icon", glyphs.into_any_element()))
            .child(field(if form.shell { "shortcuts_command" } else { "shortcuts_text" }, content.into_any_element()))
            .children(direct)
            .child(confirm)
            .child(div().flex().justify_end().gap(px(8.))
                .child(Button::new("shortcut-form-cancel").ghost().small().label(tr("cancel"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_shortcut_form(window, cx))))
                .child(Button::new("shortcut-form-ok").primary().small().label(tr("shortcuts_form_ok")).disabled(!form.valid(cx) || self.shortcuts.saving)
                    .on_click(cx.listener(|this, _, window, cx| this.submit_shortcut_form(window, cx)))))
    }
}

#[cfg(test)]
mod tests {
    use super::{Draft, Item, apply_edit, clip, defaults, resolve, serialize};

    #[test]
    fn editing_keeps_unknown_fields_and_never_revives_a_removed_item() {
        let mut items = resolve(r#"[{"id":"a","type":"send_text","label":"R","text":"/r","send_direct":false,"confirm":true,"novo":1,"icon":"glifo:futuro"}]"#);
        let draft = Draft { shell: false, label: "R2".into(), content: "/r2".into(), icon: "glifo:futuro".into(), direct: true, confirm: false };
        let edited = draft.into_item(Some(items[0].0.clone()), "a".into());
        // O campo desconhecido e o glifo que esta versão não conhece ficam; as marcas desligadas saem.
        assert_eq!(edited.0.get("novo"), Some(&serde_json::json!(1)));
        assert_eq!((edited.icon(), edited.label(), edited.content()), (Some("glifo:futuro"), "R2", "/r2"));
        assert!(edited.sends_direct() && !edited.confirm() && !edited.0.contains_key("send_direct"));
        apply_edit(&mut items, Some("a"), edited.clone());
        assert_eq!(items, vec![edited.clone()]);
        items.clear();
        apply_edit(&mut items, Some("a"), edited);
        assert!(items.is_empty());
    }

    #[test]
    fn resolve_keeps_the_web_rules_and_unknown_fields() {
        let ids = |raw: &str| resolve(raw).iter().map(|i| i.id().to_owned()).collect::<Vec<_>>();
        assert_eq!(resolve(""), defaults());
        assert_eq!(resolve("{quebrado"), defaults());
        assert_eq!(resolve(r#"{"id":"x"}"#), defaults());
        assert!(resolve("[]").is_empty());
        let raw = r#"[{"id":"a","type":"send_text","label":"R","text":"/r","novo":1},{"id":"a","type":"shell","label":"d","command":"x"},
            {"id":"b","type":"shell","label":"B","command":"make","icon":7},{"id":"c","type":"internal","action":"modo"},
            {"id":"d","type":"internal","action":"outra"},{"id":" ","type":"shell","label":"x","command":"y"},
            {"id":"e","type":"send_text","label":"E","text":"t","send_direct":null}]"#;
        assert_eq!(ids(raw), ["a", "c"]);
        // O campo desconhecido volta na gravação.
        assert!(serialize(&resolve(raw)).contains(r#""novo":1"#));
        assert_eq!(resolve(&serialize(&defaults())), defaults());
        let direct = |raw: &str| resolve(raw)[0].sends_direct();
        assert!(direct(r#"[{"id":"a","type":"send_text","label":"R","text":"/r"}]"#));
        assert!(!direct(r#"[{"id":"a","type":"send_text","label":"R","text":"/r","send_direct":false}]"#));
        assert_eq!(Item::native("rodar").action(), "rodar");
    }

    #[test]
    fn clip_counts_like_maxlength() {
        assert_eq!(clip("🚀🚀", 4), None);
        assert_eq!(clip("🚀🚀🚀", 4).as_deref(), Some("🚀🚀"));
        assert_eq!(clip(&"a".repeat(25), 24).map(|s| s.len()), Some(24));
    }
}
