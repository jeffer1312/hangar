//! Modelos do Claude Code e chaves de outros agentes: o formulário do modelo (editar e criar, o `MotorForm` do web), o
//! formulário curto da chave (`NovaCredencialSheet` do web), o catálogo de provedores do "Adicionar…" e o cookie do
//! painel do OpenCode. Cada escrita vai uma vez; a resposta só mexe no formulário que a pediu (número do pedido), e a
//! chave digitada sai do campo assim que o servidor a guarda.
use super::*;
use crate::app::settings::Disclosure;
use gpui_kit::component::{IndexPath, WindowExt, dialog::DialogButtonProps, select::{Select, SelectEvent, SelectState},
    searchable_list::{SearchableListItem, SearchableVec}, switch::Switch};

/// Um provedor do catálogo: o endereço é a raiz, sem `/v1` (o servidor monta o caminho).
pub(super) struct Provider { pub(super) id: &'static str, name: &'static str, desc: &'static str, url: &'static str }

/// Os provedores que o web oferece para modelo e para chave, na mesma ordem.
pub(super) const PROVIDERS: &[Provider] = &[
    Provider { id: "opencode", name: "OpenCode Zen", desc: "accounts_provider_opencode", url: "https://opencode.ai/zen" },
    Provider { id: "kimi", name: "Kimi Code", desc: "accounts_provider_kimi", url: "https://api.kimi.com/coding" },
    Provider { id: "omni", name: "OmniRoute", desc: "accounts_provider_omni", url: "https://ai.omniwise.com.br" },
    Provider { id: "anthropic", name: "Anthropic", desc: "accounts_provider_anthropic", url: "https://api.anthropic.com" },
    Provider { id: "openrouter", name: "OpenRouter", desc: "accounts_provider_openrouter", url: "https://openrouter.ai/api" },
    Provider { id: "groq", name: "Groq", desc: "accounts_provider_groq", url: "https://api.groq.com/openai" },
    Provider { id: "deepseek", name: "DeepSeek", desc: "accounts_provider_deepseek", url: "https://api.deepseek.com" },
    Provider { id: "custom", name: "", desc: "accounts_provider_custom_desc", url: "" },
];

/// Os dois atalhos de endereço do web: provedores cujo endereço não se adivinha.
const HINTS: [(&str, &str); 2] = [("Kimi Code", "https://api.kimi.com/coding"), ("OmniRoute", "https://ai.omniwise.com.br")];

impl Provider {
    fn name(&self) -> String { if self.name.is_empty() { tr("accounts_provider_custom") } else { self.name.to_owned() } }
}

/// Selo do provedor: a cor da marca quando o app a conhece, e duas letras.
fn provider_badge(url: &str, name: &str, size: f32) -> Div {
    let (color, _) = theme::provider(if url.contains("kimi") { "kimi" } else { "" });
    let letters: String = name.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase();
    div().size(px(size)).flex_shrink_0().rounded(px(size * 0.28)).bg(color.opacity(0.16)).flex().items_center().justify_center()
        .text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(letters)
}

/// Uma linha do catálogo no diálogo "Adicionar…": selo, nome, o que é e o botão.
pub(super) fn provider_choice(provider: &Provider, action: Button) -> Div {
    let name = provider.name();
    div().flex().items_center().gap(px(12.)).py(px(10.)).border_t_1().border_color(theme::border())
        .child(provider_badge(provider.url, &name, 30.))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().font_weight(FontWeight::MEDIUM).child(name))
            .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr(provider.desc))))
        .child(div().flex_shrink_0().child(action))
}

/// Texto com trechos de código entre crases: as crases saem e o trecho ganha fundo de código.
fn coded(text: &str) -> StyledText {
    let (mut plain, mut marks, mut start) = (String::new(), Vec::new(), None);
    for ch in text.chars() {
        if ch != '`' { plain.push(ch); continue; }
        match start.take() { Some(at) => marks.push(at..plain.len()), None => start = Some(plain.len()) }
    }
    let style = HighlightStyle { color: Some(theme::text()), background_color: Some(theme::raised()), ..Default::default() };
    StyledText::new(plain).with_highlights(marks.into_iter().map(|range| (range, style)))
}

/// Nome digitado → id no `engines.json` (minúsculas, números, `-`, `_`), como o `idDe` do web.
fn engine_id(text: &str) -> String {
    let id = actions::account_slug(text);
    if id.is_empty() { "chave".into() } else { id }
}

/// Número de um campo: vazio é "em branco" (`Some(None)`); `None` é texto que não é inteiro maior que zero.
fn number(text: &str) -> Option<Option<u64>> {
    let text = text.trim();
    if text.is_empty() { return Some(None); }
    text.parse::<u64>().ok().filter(|n| *n > 0).map(Some)
}

/// "256k" a partir de tokens.
fn thousands(tokens: u64) -> String { format!("{}k", (tokens as f64 / 1000.).round()) }

#[derive(Clone, Deserialize)]
struct ProviderModel { id: String, context_length: Option<u64>, vision: Option<bool> }

/// Item do seletor de modelo: o id e a janela ao lado.
#[derive(Clone)]
struct ModelChoice { id: String, label: String, hint: String }

impl SearchableListItem for ModelChoice {
    type Value = String;
    fn title(&self) -> SharedString { self.label.clone().into() }
    fn value(&self) -> &String { &self.id }
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().w_full().flex().items_center().gap(px(8.)).child(div().flex_1().min_w_0().truncate().child(self.label.clone()))
            .when(!self.hint.is_empty(), |el| el.child(div().flex_shrink_0().text_color(theme::muted()).child(self.hint.clone())))
    }
}

type Picker = Entity<SelectState<SearchableVec<ModelChoice>>>;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum FormKind { Edit, Model, Key }

/// Interruptores do Avançado, na ordem do web; os dois números vêm depois.
const FLAGS: [(&str, &str); 8] = [
    ("bundled_skills", "accounts_engine_skills"), ("experimental_betas", "accounts_engine_betas"), ("prompt_caching", "accounts_engine_cache"),
    ("adaptive_thinking", "accounts_engine_thinking"), ("tool_search", "accounts_engine_tool_search"),
    ("gateway_model_discovery", "accounts_engine_discovery"), ("fine_grained_tool_streaming", "accounts_engine_streaming"),
    ("auth_via_api_key", "accounts_engine_x_api_key"),
];
const BETAS: usize = 1;
const THINKING: usize = 3;
const TOOL_SEARCH: usize = 4;

/// O que a gravação nos outros agentes devolveu, por agente.
struct Synced { lines: Vec<(String, bool, String)>, codex_var: Option<String> }

/// Tudo que se mostra e se decide a partir dos campos, refeito a cada edição (nunca no desenho).
#[derive(Default)]
struct Derived {
    id: String,
    /// O que a ajuda do nome curto mostra: campo vazio mostra o exemplo, não o `chave` de reserva.
    terminal_id: String,
    can_test: bool,
    can_save: bool,
    url_moved: bool,
    moonshot: bool,
    no_vision: bool,
    bad_number: bool,
    taken: bool,
    /// Criando sem a lista de modelos: salvar poderia substituir um modelo de mesmo nome sem aviso.
    names_unknown: bool,
}

/// O formulário aberto no lugar da lista.
pub(super) struct EngineForm {
    kind: FormKind,
    /// Id no disco depois que o modelo existe: na edição desde o começo, na criação depois do primeiro Salvar.
    saved: Option<String>,
    title: String,
    lead: Option<String>,
    badge_url: String,
    label: String,
    name: Entity<InputState>,
    url: Entity<InputState>,
    key: Entity<InputState>,
    model: Entity<InputState>,
    subagent: Entity<InputState>,
    window: Entity<InputState>,
    compact: Entity<InputState>,
    output: Entity<InputState>,
    flags: [bool; 8],
    /// Endereço gravado e se há chave gravada: com os dois, testar usa a chave do disco.
    saved_url: String,
    key_set: bool,
    vision: Option<bool>,
    /// Modelos que o provedor listou; `None` = ainda não testou.
    models: Option<Vec<ProviderModel>>,
    picks: Option<(Picker, Picker)>,
    testing: Option<u64>,
    tested: Option<Result<usize, String>>,
    saving: Option<u64>,
    error: Option<String>,
    syncing: Option<u64>,
    synced: Option<Result<Synced, String>>,
    why: Option<usize>,
    derived: Derived,
    _subscriptions: Vec<Subscription>,
    /// As dos dois seletores: trocadas junto com eles a cada teste.
    pick_subscriptions: Vec<Subscription>,
}

impl EngineForm {
    pub(super) fn busy(&self) -> bool { self.saving.is_some() || self.syncing.is_some() }
    fn value(input: &Entity<InputState>, cx: &App) -> String { input.read(cx).value().trim().to_owned() }
    fn model_now(&self, cx: &App) -> Option<&ProviderModel> {
        let id = Self::value(&self.model, cx);
        self.models.as_ref()?.iter().find(|m| m.id == id)
    }

    /// Refaz o que depende dos campos: chamado a cada tecla e a cada resposta.
    fn refresh(&mut self, taken: &Option<HashSet<String>>, cx: &App) {
        let (name, url, key, model) = (Self::value(&self.name, cx), Self::value(&self.url, cx), Self::value(&self.key, cx), Self::value(&self.model, cx));
        let id = self.saved.clone().unwrap_or_else(|| engine_id(if name.is_empty() && self.kind == FormKind::Key { &self.title } else { &name }));
        let bad_number = [&self.window, &self.compact, &self.output].iter().any(|i| number(&i.read(cx).value()).is_none());
        let creating = self.saved.is_none();
        let no_vision = self.model_now(cx).and_then(|m| m.vision) == Some(false);
        self.derived = Derived {
            terminal_id: if name.is_empty() { "kimi".into() } else { id.clone() },
            no_vision,
            can_test: !url.is_empty() && (!key.is_empty() || (self.kind != FormKind::Key && self.key_set)),
            can_save: !url.is_empty() && !bad_number && !(creating && taken.is_none()) && match self.kind {
                FormKind::Key => !name.is_empty() && !key.is_empty(),
                _ => !model.is_empty() && (!creating || !name.is_empty()),
            },
            url_moved: !creating && self.key_set && key.is_empty() && url != self.saved_url,
            moonshot: format!("{url} {model}").to_lowercase().contains("moonshot") || format!("{url} {model}").to_lowercase().contains("kimi"),
            taken: creating && taken.as_ref().is_some_and(|t| t.contains(&id)),
            names_unknown: creating && taken.is_none(),
            bad_number, id,
        };
    }
}

/// Cookie do painel do OpenCode sendo digitado.
pub(super) struct CookieForm {
    pub(super) id: String,
    workspace: Entity<InputState>,
    cookie: Entity<InputState>,
    pub(super) saving: bool,
    seq: u64,
    error: Option<String>,
    ready: bool,
    _subscriptions: Vec<Subscription>,
}

pub(in crate::app) enum KeysReply {
    Tested(u64, Result<Value, Failure>),
    Saved(u64, String, Result<Value, Failure>),
    Synced(u64, Result<Value, Failure>),
    Cookie(u64, Result<Value, Failure>),
    Cleared(String, String, Result<Value, Failure>),
}

fn input(window: &mut Window, cx: &mut Context<Hangar>, value: String, placeholder: String) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
}

fn field(label: String, control: impl IntoElement) -> Div {
    div().flex().flex_col().gap(px(6.)).child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(label)).child(control)
}

fn help(text: impl IntoElement) -> Div { div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text) }

/// Linha de aviso ou de erro: a cor mora num filho, a caixa pinta o texto de cinza por cima.
fn tone(text: String, color: Hsla) -> Div { div().text_size(px(12.5)).whitespace_normal().child(div().text_color(color).child(text)) }

impl Hangar {
    /// Nomes já no disco; `None` quando a lista não chegou ou o arquivo está ilegível: aí não dá para saber se um
    /// nome novo pisaria em outro modelo.
    fn engine_names(&self) -> Option<HashSet<String>> {
        self.accounts.engines.ok().filter(|e| e.broken_file.is_none()).map(|e| e.map.keys().cloned().collect())
    }

    fn keys_send(&mut self, request: impl Future<Output = KeysReply> + Send + 'static) {
        let done = self.accounts_send_later();
        self.runtime.spawn(async move { done(AccountsReply::Keys(request.await)).await });
    }

    /// Editar…: o formulário abre com o que o disco tem, e a chave vazia (tocar nela mandaria a máscara de volta).
    pub(super) fn open_engine_form(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let Some(row) = self.find_row(&id) else { return };
        let Some(name) = id.strip_prefix("chave:").map(str::to_owned) else { return };
        let Some(engine) = self.accounts.engines.ok().and_then(|e| e.map.get(&name)).cloned() else { return };
        let title = row.name.clone();
        let flags = [engine.bundled_skills == Some(true), engine.experimental_betas == Some(true), engine.prompt_caching != Some(false),
            engine.adaptive_thinking != Some(false), engine.tool_search == Some(true), engine.gateway_model_discovery == Some(true),
            engine.fine_grained_tool_streaming == Some(true), engine.auth_via_api_key == Some(true)];
        let label = engine.label.clone().unwrap_or_else(|| name.clone());
        self.build_form(FormKind::Edit, Some(name.clone()), title, None, engine, flags, label, window, cx);
    }

    /// "+ Conectar" num provedor do catálogo: modelo novo (formulário completo) ou chave (formulário curto).
    pub(super) fn open_new_key(&mut self, provider: &str, model: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let Some(p) = PROVIDERS.iter().find(|p| p.id == provider) else { return };
        let engine = Engine { base_url: p.url.into(), ..Default::default() };
        let kind = if model { FormKind::Model } else { FormKind::Key };
        // Como no web: os ligados por padrão nascem ligados, o resto desligado.
        let flags = [false, false, true, true, false, false, false, false];
        self.build_form(kind, None, p.name(), Some(tr(p.desc)), engine, flags, String::new(), window, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn build_form(&mut self, kind: FormKind, saved: Option<String>, title: String, lead: Option<String>, engine: Engine, flags: [bool; 8],
        label: String, window: &mut Window, cx: &mut Context<Self>) {
        let numeric = |n: Option<u64>| n.map(|n| n.to_string()).unwrap_or_default();
        let key_set = engine.api_key_definida;
        let name = input(window, cx, if kind == FormKind::Key { title.clone() } else { String::new() },
            if kind == FormKind::Key { title.clone() } else { "kimi".into() });
        let url = input(window, cx, engine.base_url.clone(), if kind == FormKind::Key { "https://api.exemplo.com".into() } else { "https://…".into() });
        // O formulário curto, como no web, não tem texto de fundo na chave.
        let key = cx.new(|cx| {
            let state = InputState::new(window, cx).masked(true);
            if kind == FormKind::Key { state } else { state.placeholder(tr(if key_set { "accounts_engine_key_replace" } else { "accounts_engine_key_paste" })) }
        });
        let model = input(window, cx, engine.model.clone(), tr("accounts_engine_model_id"));
        let subagent = input(window, cx, engine.subagent_model.clone().unwrap_or_default(), tr("accounts_engine_subagent_empty"));
        let context = input(window, cx, numeric(engine.context_window), tr("accounts_engine_tokens"));
        let compact = input(window, cx, numeric(engine.auto_compact_window), tr("accounts_engine_default"));
        let output = input(window, cx, numeric(engine.max_output_tokens), tr("accounts_engine_default"));
        let mut subscriptions = Vec::new();
        for (field, which) in [(&name, 0), (&url, 1), (&key, 2), (&model, 3), (&subagent, 4), (&context, 5), (&compact, 6), (&output, 7)] {
            subscriptions.push(cx.subscribe_in(field, window, move |this: &mut Hangar, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) { this.form_edited(which == 1 || which == 2, cx); }
            }));
        }
        let mut form = EngineForm { kind, saved: saved.clone(), title, lead, badge_url: engine.base_url.clone(), label, name, url, key, model, subagent,
            window: context, compact, output, flags, saved_url: engine.base_url, key_set, vision: engine.vision, models: None, picks: None,
            testing: None, tested: None, saving: None, error: None, syncing: None, synced: None, why: None, derived: Derived::default(),
            _subscriptions: subscriptions, pick_subscriptions: Vec::new() };
        form.refresh(&self.engine_names(), cx);
        self.accounts.outcome = None;
        self.accounts.cookie = None;
        let focus = if kind == FormKind::Edit { form.url.clone() } else { form.name.clone() };
        self.accounts.form = Some(form);
        focus.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// A lista de modelos chegou (ou mudou): "nome em uso" e "lista indisponível" são refeitos com ela.
    pub(super) fn refresh_engine_form(&mut self, cx: &mut Context<Self>) {
        let taken = self.engine_names();
        if let Some(form) = self.accounts.form.as_mut() { form.refresh(&taken, cx); }
    }

    /// Um campo mudou (digitado ou pelo atalho de endereço). Endereço ou chave novos: o teste em voo e a lista já
    /// lida eram de outro provedor, e saem; o modelo digitado fica no campo.
    fn form_edited(&mut self, provider_changed: bool, cx: &mut Context<Self>) {
        let taken = self.engine_names();
        let Some(form) = self.accounts.form.as_mut() else { return };
        if provider_changed { (form.testing, form.tested, form.models, form.picks, form.pick_subscriptions) = (None, None, None, None, Vec::new()); }
        form.refresh(&taken, cx);
        cx.notify();
    }

    fn close_engine_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts.form.as_ref().is_some_and(EngineForm::busy) { return; }
        self.accounts.form = None;
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    /// Testar e listar: com chave digitada vai o endereço e a chave; sem ela, o nome (o servidor usa a chave dele).
    fn test_models(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.accounts.keys_seq += 1;
        let seq = self.accounts.keys_seq;
        let Some(form) = self.accounts.form.as_mut().filter(|f| f.derived.can_test && f.testing.is_none()) else { return };
        let key = EngineForm::value(&form.key, cx);
        let body = if key.is_empty() { json!({"nome": form.derived.id}) } else { json!({"base_url": EngineForm::value(&form.url, cx), "api_key": key}) };
        (form.testing, form.tested) = (Some(seq), None);
        if form.kind == FormKind::Key { (form.models, form.picks) = (None, None); }
        self.keys_send(async move { KeysReply::Tested(seq, api.server_send(reqwest::Method::POST, &["engines", "modelos"], Some(body), 40).await) });
        cx.notify();
    }

    /// Escolher um modelo traz a janela dele; modelo sem janela limpa o campo (a do anterior passaria da real).
    fn choose_model(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let taken = self.engine_names();
        let Some(form) = self.accounts.form.as_mut() else { return };
        let context = form.models.as_ref().and_then(|m| m.iter().find(|m| m.id == id)).and_then(|m| m.context_length);
        form.model.update(cx, |input, cx| input.set_value(id, window, cx));
        if form.kind != FormKind::Key {
            form.window.update(cx, |input, cx| input.set_value(context.map(|n| n.to_string()).unwrap_or_default(), window, cx));
        }
        form.refresh(&taken, cx);
        cx.notify();
    }

    fn tested(&mut self, list: Vec<ProviderModel>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.accounts.form.as_mut() else { return };
        let current = EngineForm::value(&form.model, cx);
        let pick = list.iter().find(|m| m.id == current).or(list.first()).map(|m| m.id.clone());
        let choices: Vec<ModelChoice> = list.iter()
            .map(|m| ModelChoice { id: m.id.clone(), label: m.id.clone(), hint: m.context_length.map(thousands).unwrap_or_default() }).collect();
        let subagent = EngineForm::value(&form.subagent, cx);
        let mut sub_choices = vec![ModelChoice { id: String::new(), label: tr("accounts_engine_same_as_main"), hint: String::new() }];
        sub_choices.extend(list.iter().map(|m| ModelChoice { id: m.id.clone(), label: m.id.clone(), hint: String::new() }));
        let at = |items: &[ModelChoice], id: &str| items.iter().position(|c| c.id == id).map(IndexPath::new);
        let (main_at, sub_at) = (pick.as_deref().and_then(|id| at(&choices, id)), at(&sub_choices, &subagent));
        let main = cx.new(|cx| SelectState::new(SearchableVec::new(choices), main_at, window, cx));
        let sub = cx.new(|cx| SelectState::new(SearchableVec::new(sub_choices), sub_at, window, cx));
        form.pick_subscriptions = vec![cx.subscribe_in(&main, window, |this: &mut Hangar, _, event: &SelectEvent<SearchableVec<ModelChoice>>, window, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            this.choose_model(id.clone(), window, cx);
        })];
        form.pick_subscriptions.push(cx.subscribe_in(&sub, window, |this: &mut Hangar, _, event: &SelectEvent<SearchableVec<ModelChoice>>, window, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            if let Some(form) = this.accounts.form.as_mut() { form.subagent.update(cx, |input, cx| input.set_value(id.clone(), window, cx)); }
            cx.notify();
        }));
        (form.tested, form.models, form.picks) = (Some(Ok(list.len())), Some(list), Some((main, sub)));
        match pick {
            // Formulário curto: só preenche quando não havia escolha, como no web.
            Some(id) if form.kind != FormKind::Key || current.is_empty() => self.choose_model(id, window, cx),
            _ => {}
        }
    }

    /// Salvar: `PUT /api/engines/{id}` e, só com ele confirmado, a gravação nos outros agentes. Os campos opcionais vão
    /// sempre (vazio é como o servidor limpa); a chave só quando digitada.
    fn save_engine(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.accounts.keys_seq += 1;
        let seq = self.accounts.keys_seq;
        let Some(form) = self.accounts.form.as_mut().filter(|f| f.derived.can_save && !f.busy()) else { return };
        if form.derived.taken {
            form.error = Some(tr("accounts_engine_name_taken"));
            cx.notify();
            return;
        }
        let id = form.derived.id.clone();
        let (name, url, key, model) = (EngineForm::value(&form.name, cx), EngineForm::value(&form.url, cx), EngineForm::value(&form.key, cx),
            EngineForm::value(&form.model, cx));
        let mut body = json!({
            "label": match form.kind { FormKind::Edit => form.label.clone(), _ if name.is_empty() => form.title.clone(), _ => name },
            "base_url": url, "model": model,
        });
        if !key.is_empty() { body["api_key"] = json!(key); }
        if form.kind != FormKind::Key {
            let blank = |n: Option<Option<u64>>| n.flatten().map_or(json!(""), |n| json!(n));
            body["subagent_model"] = json!(EngineForm::value(&form.subagent, cx));
            body["context_window"] = blank(number(&form.window.read(cx).value()));
            body["auto_compact_window"] = blank(number(&form.compact.read(cx).value()));
            body["max_output_tokens"] = blank(number(&form.output.read(cx).value()));
            // Visão: a do modelo recém-testado; senão a gravada; senão nada.
            if let Some(vision) = form.model_now(cx).and_then(|m| m.vision).or(form.vision) { body["vision"] = json!(vision); }
            for ((key, _), on) in FLAGS.iter().zip(form.flags) { body[*key] = json!(on); }
        }
        (form.saving, form.error, form.synced, form.syncing) = (Some(seq), None, None, None);
        let done = self.accounts_send_later();
        self.runtime.spawn(async move {
            let saved = api.server_send(reqwest::Method::PUT, &["engines", &id], Some(body), 30).await;
            let ok = saved.is_ok();
            done(AccountsReply::Keys(KeysReply::Saved(seq, id.clone(), saved))).await;
            // A chave vale para o Claude Code de qualquer jeito; publicá-la nos outros agentes é um passo à parte, e só
            // depois de o servidor confirmar que o modelo existe.
            if ok {
                let synced = api.server_send(reqwest::Method::POST, &["credenciais", "sincronizar"], Some(json!({"id": format!("chave:{id}")})), 60).await;
                done(AccountsReply::Keys(KeysReply::Synced(seq, synced))).await;
            }
        });
        cx.notify();
    }

    pub(super) fn start_cookie(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let workspace = input(window, cx, String::new(), String::new());
        let cookie = cx.new(|cx| InputState::new(window, cx).masked(true));
        let mut subscriptions = Vec::new();
        for field in [&workspace, &cookie] {
            subscriptions.push(cx.subscribe_in(field, window, |this: &mut Hangar, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(c) = this.accounts.cookie.as_mut() {
                        c.ready = !EngineForm::value(&c.workspace, cx).is_empty() && !EngineForm::value(&c.cookie, cx).is_empty();
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.save_cookie(window, cx),
                _ => {}
            }));
        }
        workspace.update(cx, |input, cx| input.focus(window, cx));
        self.accounts.outcome = None;
        self.accounts.cookie = Some(CookieForm { id, workspace, cookie, saving: false, seq: 0, error: None, ready: false, _subscriptions: subscriptions });
        cx.notify();
    }

    fn cancel_cookie(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts.cookie.as_ref().is_some_and(|c| c.saving) { return; }
        self.accounts.cookie = None;
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    fn save_cookie(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.accounts.keys_seq += 1;
        let seq = self.accounts.keys_seq;
        let Some(c) = self.accounts.cookie.as_mut().filter(|c| c.ready && !c.saving) else { return };
        let body = json!({"id": c.id, "workspace_id": EngineForm::value(&c.workspace, cx), "auth_cookie": EngineForm::value(&c.cookie, cx)});
        (c.saving, c.seq, c.error) = (true, seq, None);
        self.keys_send(async move { KeysReply::Cookie(seq, api.server_send(reqwest::Method::PUT, &["credenciais", "cookie"], Some(body), 20).await) });
        cx.notify();
    }

    /// Parar de ler a cota pelo painel apaga o cookie guardado: pergunta antes.
    pub(super) fn confirm_clear_cookie(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let Some(row) = self.find_row(&id) else { return };
        let title = tr("accounts_cookie_clear_title").replace("{name}", &row.name);
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, id) = (this.clone(), id.clone());
            alert.title(SharedString::from(title.clone())).description(tr("accounts_cookie_clear_desc"))
                .button_props(DialogButtonProps::default().show_cancel(true).ok_text(tr("accounts_cookie_clear_ok")).ok_variant(ButtonVariant::Danger)
                    .cancel_text(tr("cancel")))
                .on_ok(move |_, _, cx| { let _ = this.update(cx, |this, cx| this.clear_cookie(id.clone(), cx)); true })
        });
    }

    fn clear_cookie(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.accounts_busy() { return; }
        let Some(name) = self.find_row(&id).map(|r| r.name.clone()) else { return };
        (self.accounts.outcome, self.accounts.cookie_clearing) = (None, Some(id.clone()));
        let body = json!({"id": id, "workspace_id": "", "auth_cookie": ""});
        self.keys_send(async move {
            KeysReply::Cleared(id, name, api.server_send(reqwest::Method::PUT, &["credenciais", "cookie"], Some(body), 20).await)
        });
        cx.notify();
    }

    pub(super) fn receive_keys(&mut self, reply: KeysReply, window: &mut Window, cx: &mut Context<Self>) {
        let taken = self.engine_names();
        match reply {
            KeysReply::Tested(seq, result) => {
                let Some(form) = self.accounts.form.as_mut().filter(|f| f.testing == Some(seq)) else { return };
                form.testing = None;
                // Testar só lê: resposta de erro é a do provedor (o servidor devolve 502 com a mensagem dele), e só a
                // falta de resposta vira "sem resposta".
                let parsed = result.map_err(|e| match e.status { None => tr("network_error"), Some(401 | 403 | 429) => Self::failure(&e), Some(_) => tr(&e.detail) })
                    .and_then(|v| v.get("modelos").cloned().map(serde_json::from_value::<Vec<ProviderModel>>).and_then(Result::ok)
                        .ok_or_else(|| tr("invalid_response")));
                match parsed {
                    Ok(list) => self.tested(list, window, cx),
                    // A mensagem do provedor é o que diz o que corrigir (401, host errado): ela aparece crua.
                    Err(error) => form.tested = Some(Err(error)),
                }
            }
            KeysReply::Saved(seq, id, result) => {
                // Deu certo, falhou ou ficou incerto: as listas relidas mostram o que o servidor guardou.
                self.load_engines(cx);
                self.load_accounts(false, cx);
                let Some(form) = self.accounts.form.as_mut().filter(|f| f.saving == Some(seq)) else { return };
                form.saving = None;
                match result {
                    Ok(value) => {
                        form.key_set = value.pointer(&format!("/motores/{id}/api_key_definida")).and_then(Value::as_bool)
                            .unwrap_or(form.key_set || !EngineForm::value(&form.key, cx).is_empty());
                        form.key.update(cx, |input, cx| input.set_value("", window, cx));
                        form.saved_url = EngineForm::value(&form.url, cx);
                        // Existe no disco a partir daqui: o Salvar seguinte é edição dele, com o nome travado.
                        form.saved = Some(id);
                        form.syncing = Some(seq);
                    }
                    Err(error) => form.error = Some(if error.uncertain { tr("accounts_engine_save_uncertain") } else { Self::failure(&error) }),
                }
            }
            KeysReply::Synced(seq, result) => {
                let Some(form) = self.accounts.form.as_mut().filter(|f| f.syncing == Some(seq)) else { return };
                form.syncing = None;
                form.synced = Some(match result.map(|value| value.get("resultado").and_then(Value::as_object).cloned()) {
                    // Resposta sem o resultado por agente não é "nada a mostrar": é resposta que não se leu.
                    Ok(None) => Err(tr("accounts_engine_sync_failed").replace("{reason}", &tr("invalid_response"))),
                    Ok(Some(agents)) => {
                        let mut lines: Vec<(String, bool, String)> = agents.iter().map(|(agent, r)| {
                            let ok = r.get("ok").and_then(Value::as_bool) == Some(true);
                            let why = r.get("motivo").and_then(Value::as_str).filter(|w| !w.is_empty() || ok)
                                .map_or_else(|| tr("accounts_engine_sync_no_reason"), str::to_owned);
                            (agent.clone(), ok, why)
                        }).collect();
                        lines.sort_by(|a, b| a.0.cmp(&b.0));
                        // O Codex guarda só o nome da variável: sem exportá-la a chave não vale lá.
                        let codex_var = lines.iter().find(|(agent, ok, _)| agent == "codex" && *ok)
                            .and_then(|(_, _, why)| why.split_once("exporte ").and_then(|(_, rest)| rest.split_whitespace().next()).map(str::to_owned));
                        Ok(Synced { lines, codex_var })
                    }
                    Err(error) => Err(tr("accounts_engine_sync_failed").replace("{reason}", &Self::setting_failure(&error))),
                });
            }
            KeysReply::Cookie(seq, result) => {
                // Relê mesmo sem o formulário (página deixada no meio): a leitura de quem voltou pode ter chegado antes da gravação.
                self.load_accounts(true, cx);
                let Some(c) = self.accounts.cookie.as_mut().filter(|c| c.seq == seq && c.saving) else { return };
                c.saving = false;
                match result {
                    Ok(_) => { self.accounts.cookie = None; self.root_focus.focus(window, cx); }
                    Err(error) => c.error = Some(if error.uncertain { tr("accounts_cookie_uncertain") } else { Self::failure(&error) }),
                }
            }
            KeysReply::Cleared(id, name, result) => {
                if self.accounts.cookie_clearing.as_deref() == Some(id.as_str()) { self.accounts.cookie_clearing = None; }
                self.accounts.outcome = Some(match result {
                    Ok(_) => (tr("accounts_cookie_cleared").replace("{name}", &name), false),
                    Err(error) if error.uncertain => (tr("accounts_change_uncertain").replace("{name}", &name), true),
                    Err(error) => (tr("accounts_cookie_clear_failed").replace("{name}", &name).replace("{reason}", &Self::failure(&error)), true),
                });
                self.load_accounts(true, cx);
            }
        }
        if let Some(form) = self.accounts.form.as_mut() { form.refresh(&taken, cx); }
    }

    /// O formulário no lugar da lista, como o login: campos do web, na ordem dele.
    pub(super) fn render_engine_form(&self, cx: &mut Context<Self>) -> Option<Div> {
        let f = self.accounts.form.as_ref()?;
        let d = &f.derived;
        let busy = f.busy();
        let short = f.kind == FormKind::Key;
        let creating = f.saved.is_none();
        let subtitle = match &f.lead {
            Some(lead) => lead.clone(),
            None => tr("accounts_login_server").replace("{server}", &self.server_label(cx)),
        };
        let head = div().flex().items_center().gap(px(12.)).child(provider_badge(&f.badge_url, &f.title, 36.))
            .child(div().flex().flex_col().gap(px(2.)).min_w_0()
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(match f.kind {
                    FormKind::Edit => tr("accounts_engine_edit_title").replace("{name}", &f.title),
                    _ => f.title.clone(),
                }))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(subtitle)));
        let mut body = div().flex().flex_col().gap(px(18.)).max_w(px(640.));

        // Nome: o curto do terminal (modelo) ou o nome da chave. Gravado, vira o id no disco e trava.
        if f.kind != FormKind::Edit {
            let label = tr(if short { "accounts_key_name" } else { "accounts_engine_short_name" });
            let mut name = field(label.clone(), Input::new(&f.name).disabled(busy || !creating).aria_label(label));
            if !short { name = name.child(help(coded(&tr("accounts_engine_terminal_new").replace("{id}", &d.terminal_id)))); }
            body = body.child(name);
        }

        let url_label = tr(if short { "accounts_key_url" } else { "accounts_engine_url" });
        let mut url = field(url_label.clone(), Input::new(&f.url).disabled(busy).aria_label(url_label));
        if !short {
            url = url.child(help(tr("accounts_engine_url_help")))
                .child(div().flex().gap(px(6.)).children(HINTS.iter().map(|(name, address)| {
                    let address = *address;
                    Button::new(SharedString::from(format!("accounts-engine-hint-{name}"))).outline().xsmall().label(*name).disabled(busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let Some(url) = this.accounts.form.as_ref().map(|f| f.url.clone()) else { return };
                            url.update(cx, |input, cx| input.set_value(address, window, cx));
                            this.form_edited(true, cx);
                        }))
                })));
        }
        body = body.child(url);

        let key_label = tr(if short { "accounts_key_secret" } else { "accounts_engine_key" });
        body = body.child(field(key_label.clone(), Input::new(&f.key).disabled(busy).aria_label(key_label))
            .when(f.key_set && !short, |el| el.child(tone(tr("accounts_engine_key_set"), theme::success()))));

        // Testar e listar: no formulário curto é o bloco "Modelos" com a lista.
        let testing = f.testing.is_some();
        let test = Button::new("accounts-engine-test").outline().small().icon(IconName::RefreshCw)
            .label(tr(if testing { "accounts_engine_testing" } else if short { "accounts_key_fetch" } else { "accounts_engine_test" }))
            .disabled(busy || testing || !d.can_test).on_click(cx.listener(|this, _, _, cx| this.test_models(cx)));
        let mut test_block = div().flex().flex_col().gap(px(6.)).when(short, |el| el.child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM)
            .child(tr("accounts_key_models")))).child(div().flex().child(test));
        match (&f.tested, f.models.as_ref()) {
            (Some(Err(error)), _) => test_block = test_block.child(tone(error.clone(), theme::danger())),
            (Some(Ok(n)), _) if !short => test_block = test_block.child(tone(tr("accounts_engine_models_ok").replace("{n}", &n.to_string()), theme::success())),
            (_, Some(list)) if short && list.is_empty() => test_block = test_block.child(help(tr("accounts_key_models_none"))),
            (_, None) if short && !testing => test_block = test_block.child(help(tr("accounts_key_models_hint"))),
            _ => {}
        }
        if d.url_moved { test_block = test_block.child(tone(tr("accounts_engine_url_moved"), theme::danger())); }
        body = body.child(test_block);

        let picks = f.picks.as_ref().filter(|_| f.models.as_ref().is_some_and(|m| !m.is_empty()));
        if short {
            if let (Some((main, _)), Some(list)) = (picks, f.models.as_ref()) {
                body = body.child(field(tr("accounts_engine_model"), Select::new(main).disabled(busy).accessibility_label(tr("accounts_engine_model")))
                    .child(tone(tr("accounts_key_models_found").replace("{n}", &list.len().to_string()), theme::success())));
            }
        } else {
            let model = match picks {
                Some((main, _)) => Select::new(main).disabled(busy).accessibility_label(tr("accounts_engine_model")).into_any_element(),
                None => div().flex().flex_col().gap(px(6.)).child(Input::new(&f.model).disabled(busy).aria_label(tr("accounts_engine_model")))
                    .child(help(tr("accounts_engine_model_help"))).into_any_element(),
            };
            body = body.child(field(tr("accounts_engine_model"), model)
                .when(d.no_vision, |el| el.child(tone(tr("accounts_engine_no_vision"), theme::danger()))));
            let subagent = match picks {
                Some((_, sub)) => Select::new(sub).disabled(busy).accessibility_label(tr("accounts_engine_subagent")).into_any_element(),
                None => Input::new(&f.subagent).disabled(busy).aria_label(tr("accounts_engine_subagent")).into_any_element(),
            };
            body = body.child(field(tr("accounts_engine_subagent"), subagent).child(help(tr("accounts_engine_subagent_help"))))
                .child(field(tr("accounts_engine_window"), Input::new(&f.window).disabled(busy).aria_label(tr("accounts_engine_window")))
                    .child(help(coded(&tr("accounts_engine_window_help")))));
        }
        let mut panel = settings_box().mt(px(24.)).p(px(20.)).gap(px(20.)).child(head).child(body);
        if !short { panel = panel.child(self.render_advanced(f, busy, cx)); }

        // Resultado: erro da gravação, a gravação nos outros agentes e o que ela devolveu.
        let mut result = div().flex().flex_col().gap(px(6.)).max_w(px(640.));
        if d.bad_number { result = result.child(tone(tr("accounts_engine_bad_number"), theme::danger())); }
        if d.names_unknown { result = result.child(tone(tr("accounts_engine_names_unknown"), theme::danger())); }
        if let Some(error) = &f.error { result = result.child(tone(error.clone(), theme::danger())); }
        if f.syncing.is_some() { result = result.child(help(tr("accounts_engine_syncing"))); }
        match &f.synced {
            Some(Err(error)) => result = result.child(tone(error.clone(), theme::danger())),
            Some(Ok(synced)) => {
                result = result.child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(tr("accounts_engine_sync_title")))
                    .children(synced.lines.iter().map(|(agent, ok, why)| {
                        let skipped = !ok && why == "nao-instalado";
                        let (text, color) = if *ok { (tr("accounts_engine_sync_ok"), theme::muted()) }
                            else if skipped { (tr("accounts_engine_sync_missing"), theme::faint()) } else { (why.clone(), theme::danger()) };
                        div().flex().gap(px(8.)).text_size(px(12.5))
                            .child(div().w(px(56.)).flex_shrink_0().font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(agent.clone()))
                            .child(div().flex_1().min_w_0().whitespace_normal().child(div().text_color(color).child(text)))
                    }))
                    .when_some(synced.codex_var.clone().filter(|_| short), |el, var| el.child(help(tr("accounts_engine_sync_codex_var").replace("{v}", &var))));
            }
            None => {}
        }
        if !short {
            if let Some(id) = f.saved.as_ref().filter(|_| f.kind == FormKind::Edit) {
                result = result.child(help(coded(&tr("accounts_engine_terminal").replace("{id}", id))));
            }
            result = result.child(help(tr("accounts_engine_open_sessions"))).child(help(tr("accounts_engine_sync_note")));
        }
        let finished = f.synced.is_some();
        let footer = div().flex().justify_end().gap(px(8.))
            .child(Button::new("accounts-engine-cancel").outline().small().label(tr(if finished { "accounts_engine_close" } else { "cancel" })).disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.close_engine_form(window, cx))))
            // O formulário curto, como no web, não salva de novo depois da gravação: o que resta é ler o resultado.
            .when(!(short && finished), |el| el.child(Button::new("accounts-engine-save").primary().small()
                .label(tr(if f.saving.is_some() { "accounts_saving" } else { "accounts_save" })).disabled(busy || !d.can_save)
                .on_click(cx.listener(|this, _, _, cx| this.save_engine(cx)))));
        Some(panel.child(result).child(footer))
    }

    /// Avançado: uma linha por recurso com o veredito à vista e o "por quê?" que abre um motivo por vez.
    fn render_advanced(&self, f: &EngineForm, busy: bool, cx: &mut Context<Self>) -> Div {
        let d = &f.derived;
        let tested = f.models.as_ref().map_or(0, Vec::len);
        let betas = f.flags[BETAS];
        let why = |index: usize| -> String {
            match index {
                THINKING => {
                    let mut text = tr("accounts_engine_why_thinking");
                    if d.moonshot { text = format!("{text} {}", tr("accounts_engine_why_thinking_moonshot")); }
                    format!("{text} {}", tr("accounts_engine_why_thinking_2"))
                }
                TOOL_SEARCH if !betas => tr("accounts_engine_why_tool_search_off"),
                _ => tr(&format!("accounts_engine_why_{}", FLAGS.get(index).map_or(if index == 8 { "compact" } else { "output" }, |(key, _)| *key))),
            }
        };
        let verdict = |index: usize| -> (String, Option<Hsla>, bool) {
            let off = (tr("accounts_engine_rec_off"), None, false);
            match index {
                2 => (tr("accounts_engine_rec_on"), Some(theme::success()), false),
                THINKING if d.moonshot => (tr("accounts_engine_required"), Some(theme::warning()), true),
                THINKING => (tr("accounts_engine_rec_on"), Some(theme::success()), false),
                TOOL_SEARCH if !betas => (tr("accounts_engine_no_effect"), None, false),
                5 if tested > 0 => (tr("accounts_engine_rec_on_n").replace("{n}", &tested.to_string()), Some(theme::success()), false),
                5 => (tr("accounts_engine_test_first"), None, false),
                7 => (tr("accounts_engine_on_401"), None, false),
                8 | 9 => (tr("accounts_engine_rec_blank"), None, false),
                _ => off,
            }
        };
        let row = |index: usize, cx: &mut Context<Self>| -> Div {
            let dead = index == TOOL_SEARCH && !betas;
            let title = tr(FLAGS.get(index).map_or(if index == 8 { "accounts_engine_compact" } else { "accounts_engine_output" }, |(_, key)| *key));
            let (text, color, strong) = verdict(index);
            let open = f.why == Some(index);
            let control = match index {
                0..=7 => Switch::new(SharedString::from(format!("accounts-engine-flag-{index}"))).checked(f.flags[index]).disabled(busy || dead)
                    .accessibility_label(title.clone())
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        if let Some(form) = this.accounts.form.as_mut() { form.flags[index] = *on; }
                        cx.notify();
                    })).into_any_element(),
                8 => div().w(px(132.)).child(Input::new(&f.compact).small().disabled(busy).aria_label(title.clone())).into_any_element(),
                _ => div().w(px(132.)).child(Input::new(&f.output).small().disabled(busy).aria_label(title.clone())).into_any_element(),
            };
            let this = cx.entity().downgrade();
            let toggle_why = Disclosure::new(format!("accounts-engine-why-{index}"), open, tr("accounts_engine_why"), true)
                .name(tr("accounts_engine_why_of").replace("{name}", &title))
                .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| {
                    if let Some(form) = this.accounts.form.as_mut() { form.why = if open { Some(index) } else if form.why == Some(index) { None } else { form.why }; }
                    cx.notify();
                }); });
            div().flex().flex_col().border_b_1().border_color(theme::border())
                .child(div().flex().items_center().gap(px(16.)).py(px(10.))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                        .child(div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).text_color(if dead { theme::muted() } else { theme::text() }).child(title))
                        // O veredito longo quebra a linha em vez de empurrar o "por quê?" para baixo do controle.
                        .child(div().flex().flex_wrap().items_center().gap_x(px(6.)).text_size(px(12.))
                            .child(div().min_w_0().whitespace_normal().text_color(color.unwrap_or(theme::muted()))
                                .when(strong, |el| el.font_weight(FontWeight::SEMIBOLD)).child(text))
                            .child(toggle_why)))
                    .child(div().flex_shrink_0().child(control)))
                .when(open, |el| el.child(div().pb(px(12.)).text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(coded(&why(index)))))
        };
        // Duas colunas independentes: o motivo aberto numa não abre buraco na outra.
        let (left, right): (Vec<usize>, Vec<usize>) = (0..10).partition(|n| *n < 5);
        let column = |rows: Vec<usize>, cx: &mut Context<Self>| div().flex_1().min_w_0().flex().flex_col().children(rows.into_iter().map(|n| row(n, cx)));
        div().flex().flex_col().gap(px(10.)).pt(px(4.)).border_t_1().border_color(theme::border())
            .child(div().pt(px(14.)).text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(tr("accounts_engine_advanced")))
            .child(help(tr("accounts_engine_advanced_help")).max_w(px(640.)))
            .child(div().flex().gap(px(32.)).child(column(left, cx)).child(column(right, cx)))
    }

    /// O cookie abaixo da linha: o que é, onde copiar, os dois campos e o Salvar.
    pub(super) fn render_cookie(&self, c: &CookieForm, size: f32, cx: &mut Context<Self>) -> Div {
        let saving = c.saving;
        div().px_4().pb(px(16.)).pl(px(16. + size + 12.)).flex().flex_col().gap(px(10.)).max_w(px(16. + size + 12. + 560.))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| { cx.stop_propagation(); this.cancel_cookie(window, cx); }))
            .child(help(tr("accounts_cookie_legend")))
            .child(help(tr("accounts_cookie_how")))
            .child(field(tr("accounts_cookie_workspace"), Input::new(&c.workspace).small().disabled(saving).aria_label(tr("accounts_cookie_workspace"))))
            .child(field(tr("accounts_cookie_value"), Input::new(&c.cookie).small().disabled(saving).aria_label(tr("accounts_cookie_value"))))
            .children(c.error.clone().map(|error| tone(error, theme::danger())))
            .child(div().flex().items_center().gap(px(6.))
                .child(Button::new("accounts-cookie-save").primary().small().label(tr(if saving { "accounts_saving" } else { "accounts_save" }))
                    .disabled(saving || !c.ready).on_click(cx.listener(|this, _, window, cx| this.save_cookie(window, cx))))
                .child(Button::new("accounts-cookie-cancel").ghost().small().label(tr("cancel")).disabled(saving)
                    .on_click(cx.listener(|this, _, window, cx| this.cancel_cookie(window, cx)))))
    }
}

#[cfg(test)]
mod tests {
    use super::{engine_id, number};

    #[test]
    fn engine_ids_and_numbers_follow_the_server() {
        assert_eq!(engine_id("Meu Provedor"), "meu-provedor");
        assert_eq!(engine_id("  "), "chave");
        assert_eq!(number(""), Some(None));
        assert_eq!(number(" 256000 "), Some(Some(256_000)));
        assert_eq!(number("0"), None);
        assert_eq!(number("12k"), None);
    }
}
