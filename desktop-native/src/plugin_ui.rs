//! Faixa acima do prompt e painéis que os mods do Claude Code desenham (SSE `plugin_ui`). A árvore
//! chega como o engine a monta (`Box`, `Text`, `Raster`, `Svg`...) e é traduzida aqui sem saber de
//! que mod veio: mod novo aparece sem código novo.
use std::{borrow::Cow, collections::{HashMap, HashSet}, rc::Rc};
use std::sync::Arc;
use std::time::Duration;
use gpui_kit::*;
use gpui_kit::component::{input::{Input, InputEvent, InputState}, Sizable};
use gpui_kit::prelude::FluentBuilder;
use serde_json::{json, Value};
use crate::theme;

mod btw;

/// Medidas dos mods são em células de terminal; estas são as da fonte mono de 12 px.
pub const CELL_W: f32 = 7.2;
const CELL_H: f32 = 17.;
const TEXT_PX: f32 = 12.;

/// Faixa sem nada para mostrar: ninguém desenhou, ou só o marcador do próprio engine.
pub fn is_empty(tree: &Value) -> bool { !tree.is_object() || tree["type"] == "engine" }

/// Clique num botão de mod: (site, botão). O site é `above-prompt` ou o id do painel.
pub type Press = Rc<dyn Fn(&str, &Control, &mut Window, &mut App)>;

/// Uma ação sobre o painel de id dado: trocar de aba (`Show`) ou fechar pelo `✕` (`Close`).
pub type PaneAction = Rc<dyn Fn(&str, &mut Window, &mut App)>;
pub type Close = PaneAction;

/// O que o app manda ao backend para achar um botão ou um campo de mod: o mod que o desenhou e a `key`, que só é
/// única dentro do mod.
#[derive(Clone, Debug, PartialEq)]
pub struct Control { pub plugin: String, pub key: String }
pub const BAND_SITE: &str = "above-prompt";

/// Troca de aba pedida no app: o id do painel.
pub type Show = PaneAction;

/// O ponteiro entrou (`true`) ou saiu de um escopo de hover ou de um cartão absoluto; o id é o lugar e o caminho.
pub type Hover = Rc<dyn Fn(&str, bool, &mut Window, &mut App)>;

/// Envio de um `Input` pelo rótulo de envio: (lugar, campo).
pub type Submit = Rc<dyn Fn(&str, &Control, &mut Window, &mut App)>;

/// Um `Input` de mod como a árvore o traz.
#[derive(Debug, PartialEq)]
pub struct FieldSpec { pub control: Control, pub placeholder: String, pub value: String }

/// O campo de texto que o app mantém para um `Input`, pela `key`. `seen` é o desenho do mod (contador de eventos
/// `plugin_ui`) que o campo já conferiu; `sync` decide quando o valor desenhado entra; `outbox` põe em ordem o que o
/// campo manda à rota; `placeholder` é o texto de ajuda posto no campo, para repô-lo só quando o mod o muda.
pub struct Field {
    pub state: Entity<InputState>, pub sync: FieldSync, pub outbox: Outbox, pub placeholder: String, pub seen: u64,
    pub _changes: Subscription,
}

/// Quando o valor que o mod desenha entra no campo. Só conta como posto quando é posto: com a pessoa no campo ele fica
/// pendente (se mudou em relação ao desenho anterior) e entra quando o campo perde o foco, salvo se a pessoa digitou
/// depois que ele chegou. Logo depois do envio do próprio campo, o desenho seguinte entra mesmo com foco: é como o mod
/// limpa o campo depois do envio, e o valor pode ser igual ao de antes (vazio). Os ecos atrasados dos `change` de antes
/// do envio não tomam a vez da resposta: `sent` guarda o que o campo mandou como `change` desde o último valor
/// aplicado, e um desenho com um desses valores é eco velho. A mesma regra está no web, em
/// `packages/core/src/pluginField.ts`, com os mesmos casos de teste.
#[derive(Default)]
pub struct FieldSync { pending: Option<String>, submitted: bool, sent: Vec<String>, last: Option<String> }

impl FieldSync {
    /// Campo criado com o valor `first` já desenhado: o próximo desenho com o mesmo valor não é mudança.
    pub fn new(first: &str) -> FieldSync { FieldSync { last: Some(first.to_owned()), ..FieldSync::default() } }

    /// Um desenho do app. `drawn` é o valor de um desenho novo do mod (`None` num redesenho do app sem evento novo),
    /// `shown` o que o campo mostra e `focused` se a pessoa está nele. Devolve o valor a pôr no campo agora.
    pub fn draw(&mut self, drawn: Option<&str>, shown: &str, focused: bool) -> Option<String> {
        let Some(drawn) = drawn else {
            if focused { return None; }
            let pending = self.pending.take()?;
            return self.apply(pending, shown);
        };
        // Depois do envio, um desenho igual ao que se vê ou a um `change` mandado (eco atrasado de antes do envio) não é
        // a resposta: a vez fica, e ele espera como pendente. Limite: uma resposta igual a um valor digitado antes (o
        // vazio depois de a pessoa apagar tudo) só entra quando o campo perde o foco.
        let answer = self.submitted && drawn != shown && !self.sent.iter().any(|v| v == drawn);
        let changed = self.last.as_deref() != Some(drawn);
        if changed { self.last = Some(drawn.to_owned()); }
        if !focused || answer {
            self.pending = None;
            self.submitted = false;
            return self.apply(drawn.to_owned(), shown);
        }
        // Só vira pendente o valor que mudou em relação ao desenho anterior: um redesenho sem mudança (outro mod que
        // redesenha o lugar) não traz nada do mod para este campo, e no blur apagaria o que se digitou num mod que não
        // ecoa o `value`. O pendente que já havia fica. Na vez da resposta ao envio vale qualquer desenho: a pessoa não
        // digitou depois do envio, e a resposta pode repetir o valor de antes (o vazio que limpa o campo).
        if changed || self.submitted { self.pending = Some(drawn.to_owned()); }
        None
    }

    /// O valor do mod entra: o que se mandou antes dele deixa de contar como eco.
    fn apply(&mut self, value: String, shown: &str) -> Option<String> {
        self.sent.clear();
        (value != shown).then_some(value)
    }

    /// O campo mandou `submit` (Enter ou o rótulo de envio): o próximo desenho do mod entra mesmo com foco. O pendente
    /// sai: o que foi enviado é o que está no campo, e um eco de antes do envio não volta ao perder o foco.
    pub fn submitted(&mut self) {
        self.pending = None;
        self.submitted = true;
    }

    /// A pessoa digitou, e o campo mandou `sent` como `change`: o pendente é descartado (perder o foco nunca apaga texto
    /// digitado e não enviado), acaba a vez do desenho que responde ao envio, e um desenho com `sent` passa a ser eco.
    pub fn typed(&mut self, sent: &str) {
        if !self.sent.iter().any(|v| v == sent) { self.sent.push(sent.to_owned()); }
        self.pending = None;
        self.submitted = false;
    }
}

/// Um pedido à rota `plugin/input`: o tipo (`change` ou `submit`) e o valor.
pub type InputRequest = (&'static str, String);

/// Ordem do que um campo manda ao mod: um pedido em voo por vez. Enquanto um voa, só o `change` mais recente fica
/// guardado (os de antes já não dizem nada ao mod), e um `submit` sai sempre depois dos `change` que o antecederam. Sem
/// isso, cada tecla seria um pedido solto, e o `change "ab"` poderia chegar depois do `change "abc"`. Espelha a
/// `InputOutbox` do web, em `packages/core/src/pluginField.ts`.
#[derive(Default)]
pub struct Outbox { busy: bool, queue: std::collections::VecDeque<InputRequest> }

impl Outbox {
    /// O campo quer mandar `kind`/`value`. Devolve o pedido a mandar agora, ou `None` quando ele ficou na fila.
    pub fn push(&mut self, kind: &'static str, value: String) -> Option<InputRequest> {
        if !self.busy {
            self.busy = true;
            return Some((kind, value));
        }
        match self.queue.back_mut() {
            Some(last) if kind == "change" && last.0 == "change" => last.1 = value,
            _ => self.queue.push_back((kind, value)),
        }
        None
    }

    /// O pedido em voo voltou (com ou sem erro). Devolve o próximo a mandar, ou `None` quando a fila acabou.
    pub fn done(&mut self) -> Option<InputRequest> {
        let next = self.queue.pop_front();
        self.busy = next.is_some();
        next
    }
}

/// Os `Input` com `key` e mod de uma árvore, na ordem dela.
pub fn fields(tree: &Value) -> Vec<FieldSpec> {
    fn walk(v: &Value, out: &mut Vec<FieldSpec>) {
        if let Some(control) = control_of(v, "Input") {
            out.push(FieldSpec { control, placeholder: text_of(&v["props"]["placeholder"]), value: text_of(&v["props"]["value"]) });
        }
        for k in children(v) { walk(k, out); }
    }
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

/// Chave do campo no app: o lugar, o mod e a `key`, que só é única dentro do mod.
pub fn field_id(site: &str, field: &Control) -> String { id_of(site, &field.plugin, &field.key) }

fn id_of(site: &str, plugin: &str, key: &str) -> String { format!("{site}\u{1f}{plugin}\u{1f}{key}") }

/// A sessão aceita digitação pelo app: interface vinda da superfície (sessão sem terminal) e fora do só leitura. Com
/// terminal, ou com servidor que não diz a fonte, o campo do mod só aceita digitação no terminal.
pub fn accepts_typing(source: Option<UiSource>, read_only: bool) -> bool { source == Some(UiSource::Surface) && !read_only }

/// O que o campo manda à rota `plugin/input`: só `change` (cada mudança) e `submit` (Enter).
pub fn input_kind(event: &InputEvent) -> Option<&'static str> {
    match event { InputEvent::Change => Some("change"), InputEvent::PressEnter { .. } => Some("submit"), _ => None }
}

/// O corpo do `plugin/input`, ou `None` quando a sessão não aceita digitação pelo app. Não se compara com o valor
/// desenhado: o `set_value` que repõe o campo não emite `Change`, então todo `change` que chega é da pessoa.
pub fn input_request(source: Option<UiSource>, read_only: bool, site: &str, field: &Control, kind: &str, value: &str) -> Option<Value> {
    accepts_typing(source, read_only).then(|| json!({"site": site, "plugin": field.plugin, "key": field.key, "kind": kind, "value": value}))
}

/// A nova tentativa com um servidor de antes destas rotas: sem a rota `close` (404 ou 405), o `✕` vai pelo `press` com
/// a `key` reservada; com o corpo estrito que recusa o mod (422), `press` e `input` vão uma vez sem ele, e o servidor
/// acha o mod pela `key`, como antes. `None`: a falha é do pedido, não da versão.
pub fn older_server_retry(action: &'static str, body: &Value, status: Option<u16>) -> Option<(&'static str, Value)> {
    match (action, status) {
        ("close", Some(404 | 405)) => Some(("press", json!({"site": body["site"], "key": "__close__"}))),
        ("press" | "input", Some(422)) if body.get("plugin").is_some() => {
            let mut older = body.clone();
            older.as_object_mut()?.remove("plugin");
            Some((action, older))
        }
        _ => None,
    }
}

/// O que o app passa para desenhar a faixa e os painéis: quem atende o clique e a troca de aba, a largura da faixa
/// (`columns`) e o hover: quem avisa o app do ponteiro e os trechos com o ponteiro em cima. Sem `press`, botão é só
/// rótulo e não há `✕`. `columns` é a largura, em colunas, para a qual a faixa foi desenhada.
pub struct View<'a> {
    pub press: Option<Press>,
    /// Sem ele (sessão só leitura), o painel não tem `✕`.
    pub close: Option<Close>,
    pub show: Show,
    /// Rolagem da fileira de abas: o app manda rolar até a aba ativa quando ela muda.
    pub tabs_scroll: &'a ScrollHandle,
    pub columns: Option<f64>,
    pub hover: Option<Hover>,
    pub hovered: &'a HashSet<String>,
    /// Campos dos `Input`, por `field_id`.
    pub fields: &'a HashMap<String, Field>,
    /// Presente só quando a sessão aceita digitação pelo app (fonte superfície e fora do só leitura).
    pub submit: Option<Submit>,
}

/// Onde a árvore está desenhada e o que o app oferece. `links` numera os links na ordem da árvore: o mesmo endereço
/// duas vezes não repete o id do elemento.
struct Ctx<'a> { site: &'a str, view: &'a View<'a>, place: Option<f64>, links: std::cell::Cell<usize> }

impl Ctx<'_> {
    /// Id de um trecho: o lugar e o caminho dele na árvore.
    fn spot(&self, at: &Spot) -> String { format!("{}{}", self.site, at.path) }
}

/// Onde o nó está: o caminho na árvore, que vira o id do escopo de hover, e se o escopo mais próximo está aceso.
struct Spot { path: String, lit: bool }

impl Spot {
    fn root() -> Spot { Spot { path: String::new(), lit: false } }
    fn child(&self, i: usize, lit: bool) -> Spot { Spot { path: format!("{}/{i}", self.path), lit } }
}

/// Box com `key` é escopo de hover.
fn is_scope(v: &Value) -> bool { v["type"] == "Box" && v["props"]["key"].as_str().is_some_and(|k| !k.is_empty()) }

/// A subárvore tem algum `hover`: só então o escopo precisa avisar o app do ponteiro.
pub fn wants_hover(v: &Value) -> bool { v["hover"].is_object() || children(v).iter().any(wants_hover) }

/// Um escopo está aceso com o ponteiro nele ou num trecho dentro dele (o cartão absoluto, que pode sair da área do
/// escopo). A comparação é por segmento do caminho.
pub fn scope_active(hovered: &HashSet<String>, scope: &str) -> bool {
    hovered.iter().any(|h| h == scope || h.strip_prefix(scope).is_some_and(|rest| rest.starts_with('/')))
}

/// O `hover` que vale para o nó: `hover` com `scope` (grupo entre lugares) fica para depois, e o nó segue sem hover.
fn own_hover(v: &Value) -> Option<&serde_json::Map<String, Value>> {
    v["hover"].as_object().filter(|h| !h.contains_key("scope"))
}

/// As props do nó com o `hover` aplicado quando o escopo está aceso. Sem hover aplicado, as props são as do nó, sem
/// cópia: um Raster carrega as células inteiras nelas.
pub fn hover_props(v: &Value, lit: bool) -> Cow<'_, Value> {
    let Some(hover) = own_hover(v).filter(|_| lit) else { return Cow::Borrowed(&v["props"]) };
    let mut p = if v["props"].is_object() { v["props"].clone() } else { Value::Object(Default::default()) };
    for (k, value) in hover { p[k] = value.clone(); }
    Cow::Owned(p)
}

/// O Box avisa o app do ponteiro: escopo com algum `hover` dentro, ou cartão absoluto (com ou sem o hover aplicado).
fn tracks_hover(v: &Value) -> bool {
    v["type"] == "Box" && ((is_scope(v) && wants_hover(v)) || hover_props(v, false)["position"] == "absolute"
        || hover_props(v, true)["position"] == "absolute")
}

/// Ids de trecho que a árvore desenhada no lugar pode ter no conjunto de hover.
fn tracked(site: &str, path: &mut String, v: &Value, out: &mut HashSet<String>) {
    if tracks_hover(v) { out.insert(format!("{site}{path}")); }
    for (i, k) in children(v).iter().enumerate() {
        let len = path.len();
        path.push_str(&format!("/{i}"));
        tracked(site, path, k, out);
        path.truncate(len);
    }
}

/// Depois de um evento novo, de uma troca de aba ou de fechar um painel, fica no conjunto só o trecho que ainda está
/// desenhado num dos lugares à vista (`(lugar, árvore)`): o gpui não avisa a saída do ponteiro de um trecho que sumiu
/// debaixo dele, e o hover ficaria preso.
pub fn keep_hovered(hovered: &mut HashSet<String>, places: &[(&str, &Value)]) {
    if hovered.is_empty() { return; }
    let mut drawn = HashSet::new();
    for (site, tree) in places { tracked(site, &mut String::new(), tree, &mut drawn); }
    hovered.retain(|h| drawn.contains(h));
}

/// Pinta o filho depois do resto da árvore, recortado onde ele está: é o `position: absolute` dos mods, que no terminal
/// fica por cima dos vizinhos sem sair do lugar. O gpui não tem z-index, e o `deferred` dele pinta sem recorte.
///
/// O recorte vai pelo `Clip`, que embrulha o filho: o `defer_draw` só aplica a máscara na pintura, e o prepaint adiado
/// roda sem máscara, então o hitbox do trecho cortado pegaria hover e clique fora do lugar (sobre o compositor ou o
/// cabeçalho do painel). A máscara é a do lugar, lida no prepaint do `OnTop` e passada ao `Clip` pela célula.
struct OnTop { clip: Option<AnyElement>, mask: Rc<std::cell::Cell<Option<ContentMask<Pixels>>>> }

impl OnTop {
    fn new(child: AnyElement) -> OnTop {
        let mask = Rc::new(std::cell::Cell::new(None));
        OnTop { clip: Some(Clip { child, mask: mask.clone() }.into_any_element()), mask }
    }
}

impl IntoElement for OnTop {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for OnTop {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App)
        -> (LayoutId, ()) {
        (self.clip.as_mut().expect("filho antes do prepaint").request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (),
        window: &mut Window, _: &mut App) {
        let clip = self.clip.take().expect("prepaint uma vez só");
        let (offset, mask) = (window.element_offset(), window.content_mask());
        self.mask.set(Some(mask));
        window.defer_draw(clip, offset, 1, Some(mask));
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (),
        _: &mut Window, _: &mut App) {}
}

/// Roda o prepaint e a pintura do filho dentro da máscara do lugar: o hitbox nasce recortado, como o desenho.
struct Clip { child: AnyElement, mask: Rc<std::cell::Cell<Option<ContentMask<Pixels>>>> }

impl IntoElement for Clip {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for Clip {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App)
        -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (),
        window: &mut Window, cx: &mut App) {
        let child = &mut self.child;
        window.with_content_mask(self.mask.get(), |window| { child.prepaint(window, cx); });
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (),
        window: &mut Window, cx: &mut App) {
        let child = &mut self.child;
        window.with_content_mask(self.mask.get(), |window| child.paint(window, cx));
    }
}

/// O controle (`Button` ou `Input`) que o app aciona: sem `key` ou sem o mod do `press`, não há a quem mandar.
fn control_of(v: &Value, kind: &str) -> Option<Control> {
    control_ref(v, kind).map(|(plugin, key)| Control { plugin: plugin.to_owned(), key: key.to_owned() })
}

/// O `(mod, key)` do controle, emprestado da árvore: o desenho por quadro só aloca o `Control` quando precisa guardá-lo.
fn control_ref<'a>(v: &'a Value, kind: &str) -> Option<(&'a str, &'a str)> {
    if v["type"] != kind { return None; }
    let key = v["props"]["key"].as_str().filter(|k| !k.is_empty())?;
    let plugin = v["press"]["plugin"].as_str().filter(|p| !p.is_empty())?;
    Some((plugin, key))
}

pub fn button_control(v: &Value) -> Option<Control> { control_of(v, "Button") }

/// Só http(s) vira link, como no web: `javascript:` ou `file:` abririam o que o mod não deveria.
pub fn safe_href(v: &Value) -> Option<String> {
    v.as_str().filter(|h| h.starts_with("https://") || h.starts_with("http://")).map(str::to_owned)
}

/// Aviso (`$.ui.toast`) que um mod mostrou no terminal; `plugin` é o mod que o emitiu.
#[derive(Debug, PartialEq)]
pub struct Toast { pub id: String, pub text: String, pub plugin: String, pub timeout: Duration }

/// O dado do SSE `plugin_toast`; sem id, sem texto ou sem prazo não é aviso.
pub fn toast(data: &Value) -> Option<Toast> {
    let id = data["id"].as_str().filter(|id| !id.is_empty())?;
    let text = data["text"].as_str().filter(|text| !text.trim().is_empty())?;
    let ms = data["timeoutMs"].as_u64().filter(|ms| *ms > 0)?;
    Some(Toast { id: id.to_owned(), text: short(text), plugin: data["plugin"].as_str().unwrap_or("").to_owned(), timeout: Duration::from_millis(ms) })
}

/// A notificação cresce com o texto: aviso longo vira no máximo 4 linhas e 300 caracteres.
fn short(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: String = lines.iter().take(4).copied().collect::<Vec<_>>().join("\n");
    let mut cut = lines.len() > 4;
    if out.chars().count() > 300 { out = out.chars().take(300).collect(); cut = true; }
    if cut { out.push('…'); }
    out
}

/// De onde vem a interface dos mods: superfície remota (sessão sem terminal) ou o plugin no terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiSource { Surface, Terminal }

/// O evento `plugin_ui` lido. `shown_id`: `None` quando o servidor não manda (antigo), `Some(None)` sem painel.
/// `columns` e `source` ausentes ou estranhos valem como "o servidor não mandou".
#[derive(Debug, Default, PartialEq)]
pub struct Surfaces { pub above: Value, pub panes: Vec<Value>, pub shown_id: Option<Option<String>>, pub columns: Option<f64>, pub source: Option<UiSource>, pub caps: Vec<String> }

/// O dado do SSE `plugin_ui`, por valor: a árvore é movida, não copiada. Painel sem id fica de fora, como no web.
pub fn surfaces(mut data: Value) -> Surfaces {
    if !data.is_object() { return Surfaces::default(); }
    let shown_id = match data.get("shown_id") {
        Some(Value::String(id)) if !id.is_empty() => Some(Some(id.clone())),
        Some(Value::Null) => Some(None),
        _ => None,
    };
    let panes = match data["panes"].take() {
        Value::Array(panes) => panes.into_iter().filter(|p| p["id"].as_str().is_some_and(|id| !id.is_empty())).collect(),
        _ => Vec::new(),
    };
    Surfaces {
        above: data["above"].take(),
        panes,
        shown_id,
        columns: data["columns"].as_f64().filter(|c| c.is_finite() && *c > 0.),
        source: match data["source"].as_str() { Some("surface") => Some(UiSource::Surface), Some("terminal") => Some(UiSource::Terminal), _ => None },
        caps: data["caps"].as_array().map(|caps| caps.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(),
    }
}

/// A vista inteira a partir da anterior e do `plugin_ui_delta`, por valor: a faixa ausente fica a de antes, e o
/// painel `{id, same: true}` volta ao de mesmo id na anterior. O servidor só manda a diferença a quem já tem a
/// vista de que ela parte.
pub fn apply_delta(above: Value, panes: Vec<Value>, mut delta: Value) -> Value {
    if !delta.is_object() { return delta; }
    let mut before: std::collections::HashMap<String, Value> =
        panes.into_iter().filter_map(|pane| Some((pane["id"].as_str()?.to_owned(), pane))).collect();
    if delta.get("above").is_none() { delta["above"] = above; }
    for pane in delta["panes"].as_array_mut().into_iter().flatten() {
        if pane["same"] == true && let Some(old) = pane["id"].as_str().and_then(|id| before.remove(id)) { *pane = old; }
    }
    delta
}

pub fn pane_ids(panes: &[Value]) -> Vec<String> { panes.iter().filter_map(|p| p["id"].as_str().map(str::to_owned)).collect() }

/// O servidor diz qual painel está na frente, e ele está na lista: a aba segue o servidor.
pub fn follows_server(ids: &[String], shown: &Option<Option<String>>) -> bool {
    matches!(shown, Some(Some(id)) if ids.contains(id))
}

/// O painel desenhado: o do servidor quando ele diz um da lista; senão a escolha local; senão o último aberto.
pub fn active_pane(ids: &[String], shown: &Option<Option<String>>, local: Option<&str>) -> Option<String> {
    if let Some(Some(id)) = shown.as_ref().filter(|_| follows_server(ids, shown)) { return Some(id.clone()); }
    local.filter(|l| ids.iter().any(|id| id == l)).map(str::to_owned).or_else(|| ids.last().cloned())
}

/// A escolha local depois de um evento novo. Painel que acabou de abrir vai para a frente, como no terminal; fechado o
/// escolhido, fica o vizinho anterior (o seguinte, se não houver anterior); senão ela sobrevive ao redesenho.
pub fn follow_local(prev: &[String], next: &[String], local: Option<&str>) -> Option<String> {
    if next.is_empty() { return None; }
    if let Some(opened) = next.iter().rev().find(|id| !prev.contains(id)) { return Some(opened.clone()); }
    if let Some(local) = local.filter(|l| next.iter().any(|id| id == l)) { return Some(local.to_owned()); }
    let Some(at) = local.and_then(|l| prev.iter().position(|id| id == l)) else { return next.last().cloned() };
    prev[..at].iter().rev().find(|id| next.contains(id)).or_else(|| next.first()).cloned()
}

/// O que fazer com a rolagem da fileira de abas num desenho.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabScroll {
    /// Nada a rolar: sem fileira (menos de dois painéis), aba ativa já vista ou fora da lista.
    Idle,
    /// Há aba ativa nova, mas a fileira ainda não tem geometria (primeiro quadro): tentar no quadro seguinte.
    Wait,
    /// Rolar até o índice e marcar a aba como vista.
    To(usize),
}

/// Decide a rolagem da fileira: a aba ativa mudou desde a última vista (abrir, troca pelo servidor, clique, aba ativa
/// fechada) e a fileira já tem geometria. Sem troca não rola, para respeitar a roda do mouse.
pub fn tab_scroll_target(seen: Option<&str>, ids: &[String], active: Option<&str>, laid_out: bool) -> TabScroll {
    let Some(active) = active.filter(|a| ids.len() > 1 && seen != Some(*a)) else { return TabScroll::Idle };
    match ids.iter().position(|id| id == active) {
        Some(_) if !laid_out => TabScroll::Wait,
        Some(ix) => TabScroll::To(ix),
        None => TabScroll::Idle,
    }
}

fn frame() -> Div {
    div().px(px(10.)).py(px(6.)).rounded(px(8.)).bg(theme::inset())
        .font_family(theme::MONO).text_size(px(TEXT_PX)).line_height(px(CELL_H)).text_color(theme::text())
}

pub fn band(tree: &Value, view: &View) -> Option<AnyElement> {
    if is_empty(tree) { return None; }
    let c = Ctx { site: BAND_SITE, view, place: view.columns, links: Default::default() };
    Some(frame().w_full().mb(px(4.)).overflow_hidden().child(node(tree, &c, &Spot::root())).into_any_element())
}

/// O `✕` do lugar: fecha o painel da frente, como a marca do engine no terminal.
fn close_mark(site: &str, view: &View) -> Option<AnyElement> {
    let close = view.close.clone()?;
    let site = site.to_owned();
    Some(div().id(SharedString::from(format!("plg-close-{site}"))).flex_shrink_0().cursor_pointer().px(px(4.))
        .text_color(theme::muted()).child("✕")
        .on_click(move |_, window, cx| close(&site, window, cx)).into_any_element())
}

/// Largura máxima de uma aba, em células: título maior que isso sai cortado com reticências.
const TAB_MAX_CELLS: f32 = 24.;

/// Fileira das abas: um título por painel, o ativo em destaque, e um `✕` só à direita. Cada aba tem largura máxima,
/// com o título cortado, e a fileira rola de lado quando não cabe (como o `overflow-x: auto` do web): toda aba fica ao
/// alcance, e o `✕` não sai da tela.
fn tabs(panes: &[Value], active: &str, view: &View) -> AnyElement {
    let row = div().id("plg-tabs").flex().flex_row().items_center().gap_1().flex_1().min_w_0().overflow_x_scroll().track_scroll(view.tabs_scroll)
        .children(panes.iter().map(|p| {
            let id = p["id"].as_str().unwrap_or("").to_owned();
            let title = p["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(&id).to_owned();
            let on = id == active;
            let tab = div().id(SharedString::from(format!("plg-tab-{id}"))).flex_shrink_0().max_w(px(TAB_MAX_CELLS * CELL_W))
                .px(px(CELL_W)).rounded(px(4.))
                .when(on, |el| el.bg(theme::raised()).font_weight(FontWeight::SEMIBOLD))
                .when(!on, |el| el.text_color(theme::muted()).cursor_pointer())
                .child(div().min_w_0().truncate().child(title));
            match (!on).then(|| view.show.clone()) {
                Some(show) => tab.on_click(move |_, window, cx| show(&id, window, cx)).into_any_element(),
                None => tab.into_any_element(),
            }
        }));
    div().flex().items_center().justify_between().gap_2().child(row).children(close_mark(active, view)).into_any_element()
}

/// Os painéis dos mods, acima da faixa como o terminal os abre. Com mais de um, uma fileira de abas e só o ativo
/// desenhado; com um só, título e `✕`. O corpo rola dentro de `max_h`.
pub fn panes(panes: &[Value], active: Option<&str>, view: &View, max_h: f32) -> Option<AnyElement> {
    let pane = panes.iter().find(|p| p["id"].as_str() == active)?;
    let id = pane["id"].as_str().unwrap_or("").to_owned();
    // O `/btw` manda o próprio estado: o app o desenha com o tema dele, e a árvore fica para o terminal.
    if id == btw::SITE && pane["data"].is_object() {
        return btw::panel(pane, view, max_h, (panes.len() > 1).then(|| tabs(panes, &id, view)));
    }
    let header = if panes.len() > 1 { tabs(panes, &id, view) } else {
        let title = pane["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(&id).to_owned();
        div().flex().items_center().justify_between().gap_2()
            .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(title))
            .children(close_mark(&id, view)).into_any_element()
    };
    let c = Ctx { site: &id, view, place: pane["columns"].as_f64(), links: Default::default() };
    // Recorta o que passa da largura: no gpui, filho maior que a coluna desenha por cima do vizinho.
    Some(frame().flex().flex_col().gap_1().min_h_0().max_h(px(max_h)).w_full().overflow_hidden()
        .child(header)
        .child(div().id(SharedString::from(format!("plg-body-{id}"))).flex_1().min_h_0().overflow_y_scroll()
            .child(node(&pane["tree"], &c, &Spot::root())))
        .into_any_element())
}

fn node(v: &Value, c: &Ctx, at: &Spot) -> AnyElement {
    match v {
        Value::String(s) => div().flex_shrink_0().child(s.clone()).into_any_element(),
        Value::Number(n) => div().flex_shrink_0().child(n.to_string()).into_any_element(),
        Value::Object(_) => element(v, c, at),
        _ => div().into_any_element(),
    }
}

fn text_of(v: &Value) -> String { v.as_str().unwrap_or("").to_owned() }

fn children(v: &Value) -> &[Value] { v["children"].as_array().map(Vec::as_slice).unwrap_or(&[]) }

/// Texto direto dos filhos, para rótulos de botão e link.
fn plain(v: &Value) -> String {
    children(v).iter().map(|c| match c { Value::String(s) => s.clone(), Value::Number(n) => n.to_string(), _ => String::new() }).collect()
}

fn element(v: &Value, c: &Ctx, at: &Spot) -> AnyElement {
    // Box com `key` é escopo: o hover dele e o dos filhos valem com o ponteiro nele; os outros seguem o de cima.
    let lit = if is_scope(v) { scope_active(c.view.hovered, &c.spot(at)) } else { at.lit };
    let props = hover_props(v, lit);
    let p: &Value = &props;
    match v["type"].as_str().unwrap_or("") {
        "Box" => boxed(v, p, c, at, lit),
        "Text" => text(p, children(v), c, at),
        "Raster" => raster(p),
        "Svg" => svg(p),
        "Markdown" => {
            let source = text_of(&p["text"]);
            // Painel é leitura (a resposta do `/btw`): markdown de verdade. A faixa segue sem as marcações.
            let body = if c.site == BAND_SITE { div().whitespace_normal().child(unmark(&source)).into_any_element() } else {
                gpui_kit::component::text::TextView::markdown(SharedString::from(format!("plg-md-{}", c.spot(at))), source)
                    .selectable(true).scrollable(false).into_any_element()
            };
            div().w_full().min_w_0().when(p["dimColor"] == true, |el| el.opacity(0.6)).child(body).into_any_element()
        }
        "Code" => div().whitespace_normal().text_color(theme::muted()).child(text_of(&p["source"])).into_any_element(),
        "Link" => {
            let label = Some(text_of(&p["label"])).filter(|s| !s.is_empty()).unwrap_or_else(|| plain(v));
            let shown = if label.is_empty() { text_of(&p["href"]) } else { label };
            let base = div().text_color(theme::accent()).underline();
            match safe_href(&p["href"]) {
                Some(href) => {
                    let n = c.links.get();
                    c.links.set(n + 1);
                    base.id(SharedString::from(format!("lnk-{}-{n}", c.site))).cursor_pointer()
                        .on_click(move |_, _, cx| cx.open_url(&href)).child(shown).into_any_element()
                }
                None => base.child(shown).into_any_element(),
            }
        }
        "Button" => {
            let label = Some(text_of(&p["label"])).filter(|s| !s.is_empty()).unwrap_or_else(|| plain(v));
            // `plain` sai como no terminal: texto, sem pílula.
            let base = div().flex_shrink_0()
                .when(p["plain"] != true, |el| el.px(px(CELL_W)).rounded(px(4.)).bg(theme::raised()))
                .when(p["dimColor"] == true, |el| el.opacity(0.6))
                .when(p["variant"] == "primary", |el| el.text_color(theme::accent()))
                // Estilo do rótulo como no web, das props ou do `hover` do escopo; o fundo vence o da pílula.
                .when_some(color(&p["color"]), |el, fg| el.text_color(fg))
                .when_some(color(&p["backgroundColor"]), |el, bg| el.bg(bg))
                .when(p["bold"] == true, |el| el.font_weight(FontWeight::BOLD))
                .when(p["italic"] == true, |el| el.italic())
                .when(p["underline"] == true, |el| el.underline())
                .when(p["strikethrough"] == true, |el| el.line_through());
            match (button_control(v), c.view.press.clone()) {
                (Some(button), Some(press)) => {
                    let site = c.site.to_owned();
                    base.id(SharedString::from(format!("plg-{site}-{}-{}", button.plugin, button.key))).cursor_pointer()
                        .hover(|el| el.underline())
                        .on_click(move |_, window, cx| press(&site, &button, window, cx))
                        .child(label).into_any_element()
                }
                _ => base.child(label).into_any_element(),
            }
        }
        "Input" => field(v, p, c),
        "Image" => div().text_color(theme::muted()).child(text_of(&p["alt"])).into_any_element(),
        _ => div().flex().children(children(v).iter().enumerate().map(|(i, k)| node(k, c, &at.child(i, lit)))).into_any_element(),
    }
}

/// Largura mínima e base do campo, em pixels: as do web (`min-width: 12ch`, `flex: 1 1 16ch`) em células.
fn field_width() -> (f32, f32) { (12. * CELL_W, 16. * CELL_W) }

/// O que o campo desabilitado de um `Input` sem `key` mostra: o valor desenhado, ou o texto de ajuda (`true`, em cor
/// apagada) quando o valor é vazio.
fn keyless_text(p: &Value) -> (String, bool) {
    let value = text_of(&p["value"]);
    if value.is_empty() { (text_of(&p["placeholder"]), true) } else { (value, false) }
}

/// `Input` de mod: rótulo, campo e rótulo de envio. Sem `submit` (sessão com terminal, servidor que não diz a fonte ou só
/// leitura) o campo fica desabilitado, com a dica de digitar no terminal; sem `key`, desabilitado e sem a dica.
fn field(v: &Value, p: &Value, c: &Ctx) -> AnyElement {
    let label = text_of(&p["label"]);
    let control = control_ref(v, "Input");
    // Sem `min_w_0`: a linha não fica menor que rótulo, campo mínimo e envio. Encolhida, os filhos dela saíam por cima
    // dos vizinhos; assim, numa faixa estreita a linha passa da borda e o lugar a recorta.
    let row = div().flex().flex_row().items_center().gap_2()
        .when(!label.is_empty(), |el| el.child(div().flex_shrink_0().child(label)));
    let (min, basis) = field_width();
    let Some(state) = control.and_then(|(plugin, key)| c.view.fields.get(&id_of(c.site, plugin, key))) else {
        // `Input` sem `key` ou sem mod: não há o que mandar ao mod em nenhuma sessão, e o campo fica desabilitado, sem a
        // dica do terminal, como no web. (Com os dois o campo já existe aqui: o app o cria antes de desenhar a faixa e os
        // painéis.)
        let (text, hint) = keyless_text(p);
        return row.child(div().flex_grow(1.).flex_shrink(1.).flex_basis(px(basis)).min_w(px(min)).px(px(CELL_W / 2.))
            .rounded(px(4.)).border_1().border_color(theme::border()).bg(theme::raised()).opacity(0.6)
            .overflow_hidden().whitespace_nowrap().when(hint, |el| el.text_color(theme::muted())).child(text))
            .into_any_element();
    };
    let typing = c.view.submit.clone().zip(control.map(|(plugin, key)| Control { plugin: plugin.to_owned(), key: key.to_owned() }));
    // Sem largura mínima o campo ficava com 18 px (só o enfeite) numa linha com textos ao lado, e o clique caía no
    // enfeite, que tem foco próprio: o anel acendia, mas o texto não recebia a digitação.
    let row = row.child(div().flex_grow(1.).flex_shrink(1.).flex_basis(px(basis)).min_w(px(min))
        .child(Input::new(&state.state).small().disabled(typing.is_none())));
    match typing {
        Some((submit, field)) => {
            let site = c.site.to_owned();
            let send = Some(text_of(&p["submitLabel"])).filter(|s| !s.is_empty())
                .unwrap_or_else(|| crate::i18n::tr_shared("plugin_input_enviar", &[]));
            row.child(div().id(SharedString::from(format!("plg-enviar-{site}-{}-{}", field.plugin, field.key))).flex_shrink_0().cursor_pointer()
                .px(px(CELL_W)).rounded(px(4.)).bg(theme::raised()).child(send)
                // Sem isto, o `mousedown` passa o foco à raiz da janela, o campo perde o foco antes do clique, e o
                // redesenho do blur poria o valor pendente do mod no campo: o envio mandaria esse valor, não o digitado.
                // Com o foco no campo, o envio pelo rótulo segue o mesmo caminho do Enter.
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(move |_, window, cx| submit(&site, &field, window, cx))).into_any_element()
        }
        None => div().flex().flex_col().child(row)
            .child(div().text_color(theme::muted()).whitespace_normal().child(crate::i18n::tr_shared("plugin_input_no_terminal", &[])))
            .into_any_element(),
    }
}

/// `width` em colunas que alcança a largura do lugar ocupa o lugar inteiro: o mod desenhou para a coluna do terminal, e
/// o app pode ser mais largo. Menor continua teto. Sem a largura do lugar (servidor antigo), sempre teto.
pub fn fills_place(width: Option<f64>, place: Option<f64>) -> bool {
    matches!((width, place), (Some(w), Some(p)) if p > 0. && w >= p)
}

fn cols(v: &Value) -> Option<f32> { v.as_f64().map(|n| n as f32 * CELL_W) }
fn lines(v: &Value) -> Option<f32> { v.as_f64().map(|n| n as f32 * CELL_H) }

/// O primeiro número entre as chaves, na ordem: a mais específica vence (`paddingLeft` > `paddingX` > `padding`).
fn first(p: &Value, keys: &[&str]) -> Value { keys.iter().map(|k| p[*k].clone()).find(Value::is_number).unwrap_or(Value::Null) }

/// Fundo do `Box`: a cor do mod, ou, no cartão `absolute` sem cor, o fundo opaco do lugar (`place`). No terminal as
/// células do cartão substituem as de baixo; sem fundo, o texto dele e o da linha se embaralham.
fn box_background(p: &Value, place: Hsla) -> Option<Hsla> {
    color(&p["backgroundColor"]).or_else(|| (p["position"] == "absolute").then_some(place))
}

/// `Box` do Ink em flexbox do gpui. O padrão do Ink é linha, não coluna.
fn boxed(v: &Value, p: &Value, c: &Ctx, at: &Spot, lit: bool) -> AnyElement {
    if p["display"] == "none" { return div().into_any_element(); }
    let kids = children(v);
    let mut el = div().flex().min_w_0();
    el = match p["flexDirection"].as_str() {
        Some("column") => el.flex_col(),
        Some("column-reverse") => el.flex_col_reverse(),
        Some("row-reverse") => el.flex_row_reverse(),
        _ => el.flex_row(),
    };
    el = match p["justifyContent"].as_str() {
        Some("space-between") => el.justify_between(),
        Some("space-around") => el.justify_around(),
        Some("space-evenly") => el.justify_evenly(),
        Some("center") => el.justify_center(),
        Some("flex-end") => el.justify_end(),
        _ => el,
    };
    el = match p["alignItems"].as_str() {
        Some("center") => el.items_center(),
        Some("flex-end") => el.items_end(),
        Some("flex-start") => el.items_start(),
        _ => el,
    };
    if let Some(g) = p["flexGrow"].as_f64() { el = el.flex_grow(g as f32); }
    if p["flexWrap"] == "wrap" { el = el.flex_wrap(); }
    // Largura fixa do terminal vira teto (a coluna da conversa pode ser mais estreita que o pane), salvo quando ela
    // alcança a largura do lugar: aí o mod quis a linha inteira.
    if let Some(w) = cols(&p["width"]) {
        el = if fills_place(p["width"].as_f64(), c.place) { el.w_full() } else { el.w_full().max_w(px(w)) };
    }
    if let Some(w) = cols(&p["minWidth"]) { el = el.min_w(px(w)); }
    if let Some(g) = cols(&first(p, &["columnGap", "gap"])) { el = el.gap_x(px(g)); }
    if let Some(g) = lines(&first(p, &["rowGap", "gap"])) { el = el.gap_y(px(g)); }
    if let Some(v) = lines(&first(p, &["paddingTop", "paddingY", "padding"])) { el = el.pt(px(v)); }
    if let Some(v) = lines(&first(p, &["paddingBottom", "paddingY", "padding"])) { el = el.pb(px(v)); }
    if let Some(v) = cols(&first(p, &["paddingLeft", "paddingX", "padding"])) { el = el.pl(px(v)); }
    if let Some(v) = cols(&first(p, &["paddingRight", "paddingX", "padding"])) { el = el.pr(px(v)); }
    if let Some(v) = lines(&first(p, &["marginTop", "marginY", "margin"])) { el = el.mt(px(v)); }
    if let Some(v) = lines(&first(p, &["marginBottom", "marginY", "margin"])) { el = el.mb(px(v)); }
    if let Some(v) = cols(&first(p, &["marginLeft", "marginX", "margin"])) { el = el.ml(px(v)); }
    if let Some(v) = cols(&first(p, &["marginRight", "marginX", "margin"])) { el = el.mr(px(v)); }
    // O fundo do lugar, opaco: com o vidro o `inset` fica translúcido, e a linha de baixo atravessaria o cartão.
    if let Some(c) = box_background(p, theme::inset().alpha(1.)) { el = el.bg(c); }
    if p["borderStyle"].is_string() {
        el = el.border_1().rounded(px(4.)).border_color(color(&p["borderColor"]).unwrap_or_else(theme::border));
    }
    if p["overflow"] == "hidden" { el = el.overflow_hidden(); }
    // `absolute` sai do fluxo; deslocamento em células, negativo permitido.
    let absolute = p["position"] == "absolute";
    if absolute {
        // Por cima, o cartão fica com o ponteiro, como o `z-index` do web: hover e clique não chegam ao que está embaixo
        // (o escopo dono segue aceso pelo id do cartão). A rolagem passa.
        el = el.absolute().block_mouse_except_scroll();
        if let Some(n) = lines(&p["top"]) { el = el.top(px(n)); }
        if let Some(n) = lines(&p["bottom"]) { el = el.bottom(px(n)); }
        if let Some(n) = cols(&p["left"]) { el = el.left(px(n)); }
        if let Some(n) = cols(&p["right"]) { el = el.right(px(n)); }
    }
    // Linha de texto logo abaixo de um Raster (os rótulos sob os traços da barra de progresso)
    // segue a escala dele; sem isso o Raster cabe na coluna estreita e os rótulos saem do lugar.
    // Linha com hover fica no desenho comum, o único que aplica o hover.
    let el = el.children(kids.iter().enumerate().map(|(i, k)| {
        let at = at.child(i, lit);
        match i.checked_sub(1).filter(|_| text_row(k) && !wants_hover(k)).and_then(|j| raster_row(&kids[j])) {
            Some(frame) => aligned_row(k, frame, c, &at),
            None => node(k, c, &at),
        }
    }));
    // Escopo e cartão absoluto avisam o app do ponteiro: o cartão conta como parte do escopo que o contém.
    let id = c.spot(at);
    let el = match c.view.hover.clone().filter(|_| absolute || (is_scope(v) && wants_hover(v))) {
        Some(hover) => el.id(SharedString::from(format!("plg-hv-{id}")))
            .on_hover(move |on, window, cx| hover(&id, *on, window, cx)).into_any_element(),
        None => el.into_any_element(),
    };
    if absolute { OnTop::new(el).into_any_element() } else { el }
}

/// Props do `Box` que o `boxed` desenha e o molde do Raster não reproduz: com qualquer uma, a
/// linha segue o desenho comum em vez de perder o recuo, o espaçamento ou o fundo.
const BOX_LAYOUT: &[&str] = &[
    "justifyContent", "alignItems", "flexGrow", "flexWrap", "width", "minWidth", "gap", "columnGap", "rowGap",
    "padding", "paddingX", "paddingY", "paddingTop", "paddingBottom", "paddingLeft", "paddingRight",
    "margin", "marginX", "marginY", "marginTop", "marginBottom", "marginLeft", "marginRight",
    "backgroundColor", "borderStyle", "overflow", "position",
];

/// `Box` em linha sem nada além dos filhos: cada filho ocupa as suas células, como no terminal.
fn plain_row(v: &Value) -> bool {
    let p = &v["props"];
    v["type"] == "Box" && matches!(p["flexDirection"].as_str(), None | Some("row"))
        && BOX_LAYOUT.iter().all(|k| p[*k].is_null()) && p["display"] != "none"
        && !children(v).is_empty()
}

/// Texto de um `Text` sem corte e sem texto aninhado, o único que se mede em células.
fn flat_text(v: &Value) -> Option<String> {
    let flat = v["type"] == "Text" && !v["props"]["wrap"].is_string()
        && children(v).iter().all(|k| k.is_string() || k.is_number());
    flat.then(|| plain(v))
}

fn text_cells(v: &Value) -> Option<usize> { flat_text(v).map(|t| t.chars().count()) }

fn text_row(v: &Value) -> bool { plain_row(v) && children(v).iter().all(|k| flat_text(k).is_some()) }

/// Molde de uma linha com Raster: células de texto antes, colunas do Raster e o texto depois.
struct RasterFrame<'a> { before: usize, columns: usize, after: &'a [Value] }

fn raster_row(v: &Value) -> Option<RasterFrame<'_>> {
    if !plain_row(v) { return None; }
    let kids = children(v);
    let at = kids.iter().position(|k| k["type"] == "Raster")?;
    let (before, after) = (&kids[..at], &kids[at + 1..]);
    if after.iter().any(|k| flat_text(k).is_none()) { return None; }
    let columns = kids[at]["props"]["columns"].as_u64().filter(|&n| n > 0)? as usize;
    Some(RasterFrame { before: before.iter().map(text_cells).sum::<Option<usize>>()?, columns, after })
}

/// Trilho do Raster: ocupa o que sobra da linha até a largura natural das colunas. O Raster e a
/// linha alinhada a ele usam o mesmo, para encolherem juntos.
fn raster_track(columns: usize) -> Div {
    div().flex().flex_basis(px(0.)).flex_grow(1.).min_w_0().max_w(px(columns as f32 * CELL_W))
}

/// Monta a linha de texto no molde do Raster de cima: o começo com a largura natural, o trecho
/// sob o Raster na mesma escala dele e, no fim, o texto de depois invisível, só para ocupar o
/// mesmo espaço. No trecho escalado cada palavra corta onde começa a próxima, não antes.
fn aligned_row(row: &Value, frame: RasterFrame, c: &Ctx, at: &Spot) -> AnyElement {
    let piece = |k: &Value, t: &[char]| text(&k["props"], &[Value::from(t.iter().collect::<String>())], c, at);
    let (mut head, mut words): (Vec<AnyElement>, Vec<(Vec<AnyElement>, usize)>) = (Vec::new(), Vec::new());
    let mut seen = 0;
    for k in children(row) {
        let chars: Vec<char> = plain(k).chars().collect();
        let cut = frame.before.saturating_sub(seen).min(chars.len());
        seen += chars.len();
        if cut > 0 { head.push(piece(k, &chars[..cut])); }
        let rest = &chars[cut..];
        if rest.is_empty() { continue; }
        match words.last_mut() {
            Some((els, cells)) if rest.iter().all(|ch| ch.is_whitespace()) => { els.push(piece(k, rest)); *cells += rest.len(); }
            _ => words.push((vec![piece(k, rest)], rest.len())),
        }
    }
    let last = words.len().saturating_sub(1);
    let scaled = raster_track(frame.columns).flex_row()
        .children(words.into_iter().enumerate().map(|(i, (els, cells))| {
            div().flex().flex_row().flex_shrink_0().whitespace_nowrap().w(relative(cells as f32 / frame.columns as f32))
                .when(i < last, |el| el.overflow_hidden())
                .children(els)
        }));
    div().flex().flex_row().min_w_0()
        .children(head)
        .child(scaled)
        .child(div().flex().flex_row().flex_shrink_0().opacity(0.).children(frame.after.iter().enumerate().map(|(i, n)| node(n, c, &at.child(i, at.lit)))))
        .into_any_element()
}

/// `Text` do Ink: cor, ênfase e corte. `dimColor` é opacidade, como no terminal.
fn text(p: &Value, kids: &[Value], c: &Ctx, at: &Spot) -> AnyElement {
    let fg = color(&p["color"]);
    let bg = color(&p["backgroundColor"]);
    let (fg, bg) = if p["inverse"] == true { (bg.or(Some(theme::background())), fg.or(Some(theme::text()))) } else { (fg, bg) };
    let truncate = p["wrap"].as_str().is_some_and(|w| w.starts_with("truncate") || w == "end" || w == "middle");
    // Texto dentro de texto vira trechos lado a lado: o gpui não tem span em linha.
    let el = div().flex().flex_row().min_w_0()
        .when(!truncate, |el| el.flex_shrink_0())
        .when_some(fg, |el, c| el.text_color(c))
        .when_some(bg, |el, c| el.bg(c))
        .when(p["bold"] == true, |el| el.font_weight(FontWeight::BOLD))
        .when(p["italic"] == true, |el| el.italic())
        .when(p["underline"] == true, |el| el.underline())
        .when(p["strikethrough"] == true, |el| el.line_through())
        .when(p["dimColor"] == true, |el| el.opacity(0.6))
        .when(truncate, |el| el.overflow_hidden().whitespace_nowrap());
    // Juntar em texto puro tiraria o clique de link e botão: com eles, a linha cortada só recorta, sem reticências.
    if let Some(text) = truncate.then(|| plain_deep(kids)).flatten() {
        return el.child(div().truncate().child(text)).into_any_element();
    }
    el.children(kids.iter().enumerate().map(|(i, k)| node(k, c, &at.child(i, at.lit)))).into_any_element()
}

/// Texto de uma subárvore inteira, para o corte com reticências que o gpui só faz num texto só; `None` quando ela tem
/// algo que se clica.
fn plain_deep(kids: &[Value]) -> Option<String> {
    kids.iter().map(|c| match c {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(_) if matches!(c["type"].as_str(), Some("Link" | "Button")) => None,
        Value::Object(_) => plain_deep(children(c)),
        _ => Some(String::new()),
    }).collect()
}

/// Células do `Raster`: base64 de triplas u32 little-endian `[código, frente, fundo]`, por linha.
/// Vizinhas da mesma cor viram um trecho só. O Raster vem com a largura do pane do terminal: em
/// coluna mais estreita cada trecho encolhe na proporção das suas células, sem rolar de lado.
fn raster(p: &Value) -> AnyElement {
    let columns = p["columns"].as_u64().unwrap_or(0) as usize;
    let mut grid = raster_track(columns).flex_col();
    for runs in raster_runs(p) {
        grid = grid.child(div().flex().flex_row().w_full().min_w_0().whitespace_nowrap().children(runs.into_iter().map(|(t, fg, bg)| {
            div().flex_basis(px(0.)).flex_grow(t.chars().count() as f32).flex_shrink(1.).min_w_0().overflow_hidden()
                .when_some(fg, |el, c| el.text_color(c)).when_some(bg, |el, c| el.bg(c)).child(t)
        })));
    }
    grid.into_any_element()
}

type Run = (String, Option<Hsla>, Option<Hsla>);

/// Trechos de cada linha do Raster. `rows` vem do mod: sem o teto pelos bytes que chegaram, um
/// número enorme prende a thread de desenho criando linha vazia.
fn raster_runs(p: &Value) -> Vec<Vec<Run>> {
    use base64::Engine as _;
    let columns = p["columns"].as_u64().unwrap_or(0) as usize;
    let bytes = base64::engine::general_purpose::STANDARD.decode(p["cells"].as_str().unwrap_or("")).unwrap_or_default();
    let cells = bytes.len() / 12;
    let rows = if columns == 0 { 0 } else { (p["rows"].as_u64().unwrap_or(0) as usize).min(cells.div_ceil(columns)) };
    let word = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    (0..rows).map(|r| {
        let mut runs: Vec<Run> = Vec::new();
        for i in (r * columns..).take(columns).take_while(|&i| i < cells) {
            let at = i * 12;
            let ch = char::from_u32(word(at)).filter(|c| !c.is_control()).unwrap_or(' ');
            let (fg, bg) = (cell_color(word(at + 4)), cell_color(word(at + 8)));
            match runs.last_mut() {
                Some(run) if run.1 == fg && run.2 == bg => run.0.push(ch),
                _ => runs.push((ch.to_string(), fg, bg)),
            }
        }
        runs
    }).collect()
}

/// Bit 24 sozinho é a cor padrão do terminal; o resto é 0x00RRGGBB.
fn cell_color(v: u32) -> Option<Hsla> { (v & 0x0100_0000 == 0).then(|| rgb(v & 0x00ff_ffff).into()) }

/// O SVG vira imagem; as animações de CSS dele ficam paradas no primeiro quadro.
fn svg(p: &Value) -> AnyElement {
    let source = text_of(&p["source"]);
    if source.is_empty() { return div().into_any_element(); }
    let image = Arc::new(Image::from_bytes(ImageFormat::Svg, source.into_bytes()));
    let mut el = img(image).max_w_full();
    if let Some(w) = p["width"].as_f64() { el = el.w(px(w as f32)); }
    if let Some(h) = p["height"].as_f64() { el = el.h(px(h as f32)); }
    el.into_any_element()
}

/// Cor de um `Text`/`Box`: `#rrggbb` ou nome do Ink, no tom do terminal.
fn color(v: &Value) -> Option<Hsla> {
    let name = v.as_str()?;
    if let Some(hex) = name.strip_prefix('#') {
        let hex = if hex.len() == 3 { hex.chars().flat_map(|c| [c, c]).collect() } else { hex.to_owned() };
        return u32::from_str_radix(&hex, 16).ok().map(|n| rgb(n).into());
    }
    let value = match name {
        "black" => 0x000000, "red" => 0xcd3131, "green" => 0x0dbc79, "yellow" => 0xe5e510,
        "blue" => 0x2472c8, "magenta" => 0xbc3fbc, "cyan" => 0x11a8cd, "white" => 0xe5e5e5,
        "gray" | "grey" => 0x808080, "blackBright" => 0x666666, "redBright" => 0xf14c4c,
        "greenBright" => 0x23d18b, "yellowBright" => 0xf5f543, "blueBright" => 0x3b8eea,
        "magentaBright" => 0xd670d6, "cyanBright" => 0x29b8db, "whiteBright" => 0xffffff,
        _ => return None,
    };
    Some(rgb(value).into())
}

/// Markdown de mod na faixa, sem as marcações: o nativo não abre um leitor de markdown para uma faixa.
fn unmark(text: &str) -> String {
    text.lines().map(|l| l.trim_start_matches('#').trim_start().replace("**", "").replace("__", "").replace('`', ""))
        .collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    // Importação explícita: `super::*` traz o `test` do gpui_kit, e o `#[test]` passaria a ser o dele.
    use super::{accepts_typing, active_pane, apply_delta, box_background, button_control, cell_color, color, field_id, fields, FieldSync, fills_place, follow_local, follows_server,
        hover_props, input_kind, input_request, is_empty, older_server_retry, keep_hovered, keyless_text, Outbox, pane_ids, tab_scroll_target, TabScroll, plain_deep, raster_row, raster_runs, safe_href,
        scope_active, surfaces, text_row, toast, wants_hover, Control, FieldSpec, Surfaces, Toast, UiSource};
    use gpui_kit::component::input::InputEvent;
    use gpui_kit::{rgb, Hsla};
    use serde_json::{json, Value};
    use std::borrow::Cow;
    use std::collections::HashSet;
    use std::time::Duration;

    #[test]
    fn toast_keeps_the_mod_and_its_timeout_and_refuses_what_is_not_a_toast() {
        assert_eq!(toast(&json!({"id": "ab-1", "text": "Jenkins configurado.", "plugin": "demo", "timeoutMs": 9000})),
            Some(Toast { id: "ab-1".into(), text: "Jenkins configurado.".into(), plugin: "demo".into(), timeout: Duration::from_millis(9000) }));
        assert_eq!(toast(&json!({"id": "ab-2", "text": "oi", "timeoutMs": 1})).map(|t| t.plugin), Some(String::new()));
        assert_eq!(toast(&json!({"text": "oi", "timeoutMs": 4000})), None);
        assert_eq!(toast(&json!({"id": "ab-3", "text": "  ", "timeoutMs": 4000})), None);
        assert_eq!(toast(&json!({"id": "ab-4", "text": "oi"})), None);
        let long = toast(&json!({"id": "ab-5", "text": "x".repeat(2000), "timeoutMs": 1})).unwrap().text;
        assert_eq!((long.chars().count(), long.ends_with('…')), (301, true));
        assert_eq!(toast(&json!({"id": "ab-6", "text": "1\n2\n3\n4\n5", "timeoutMs": 1})).unwrap().text, "1\n2\n3\n4…");
    }

    fn cells(words: &[u32]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>())
    }

    #[test]
    fn raster_never_builds_rows_beyond_the_cells_that_arrived() {
        let three = cells(&[0x41, 0x0100_0000, 0x0100_0000, 0x42, 0x0100_0000, 0x0100_0000, 0x43, 0x0100_0000, 0x0100_0000]);
        let runs = raster_runs(&json!({"columns": 2, "rows": 4_000_000_000_000u64, "cells": three}));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0][0].0, "AB");
        assert_eq!(runs[1][0].0, "C");
        assert!(raster_runs(&json!({"columns": 0, "rows": 1_000_000, "cells": three})).is_empty());
        assert!(raster_runs(&json!({"columns": u64::MAX, "rows": u64::MAX, "cells": three}))[0].len() == 1);
    }

    #[test]
    fn label_row_follows_the_raster_frame_and_only_plain_text_counts() {
        let text = |s: &str| json!({"type": "Text", "children": [s]});
        let bar = json!({"type": "Box", "props": {"flexDirection": "row"}, "children": [
            text("  "), {"type": "Raster", "props": {"columns": 20, "rows": 1, "cells": ""}}, text("  29%")]});
        let frame = raster_row(&bar).unwrap();
        assert_eq!((frame.before, frame.columns, frame.after.len()), (2, 20, 1));
        let labels = json!({"type": "Box", "children": [text("  "), {"type": "Text", "props": {"bold": true}, "children": ["Correção"]}, text("   Entrega")]});
        assert!(text_row(&labels) && raster_row(&labels).is_none());
        // Largura, distribuição, corte e texto aninhado não são células: seguem o desenho comum.
        assert!(!text_row(&json!({"type": "Box", "props": {"width": 30}, "children": [text("a")]})));
        assert!(!text_row(&json!({"type": "Box", "props": {"justifyContent": "space-between"}, "children": [text("a")]})));
        assert!(!text_row(&json!({"type": "Box", "children": [{"type": "Text", "props": {"wrap": "truncate-end"}, "children": ["a"]}]})));
        assert!(!text_row(&json!({"type": "Box", "children": [{"type": "Text", "children": [text("a")]}]})));
        assert!(!text_row(&json!({"type": "Box", "children": []})));
        // Recuo, espaçamento e fundo o molde não reproduz: a linha fica no desenho comum.
        for prop in ["paddingLeft", "marginLeft", "gap", "columnGap", "backgroundColor", "borderStyle"] {
            let row = json!({"type": "Box", "props": {prop: 1}, "children": [text("a")]});
            assert!(!text_row(&row), "{prop}");
            let bar = json!({"type": "Box", "props": {prop: 1}, "children": [text("  "), {"type": "Raster", "props": {"columns": 20}}]});
            assert!(raster_row(&bar).is_none(), "{prop}");
        }
        assert!(raster_row(&json!({"type": "Box", "children": [{"type": "Raster", "props": {"columns": 0}}]})).is_none());
    }

    #[test]
    fn empty_band_is_the_engine_marker_or_nothing() {
        assert!(is_empty(&Value::Null));
        assert!(is_empty(&json!({ "type": "engine", "ref": 1 })));
        assert!(!is_empty(&json!({ "type": "Box", "children": [] })));
    }

    #[test]
    fn default_terminal_color_is_none() {
        assert_eq!(cell_color(0x0100_0000), None);
        assert!(cell_color(0x5aa6ff).is_some());
    }

    #[test]
    fn button_control_reads_only_buttons_with_key_and_mod() {
        let press = json!({"plugin": "pm-mock", "handle": 1});
        assert_eq!(button_control(&json!({"type": "Button", "props": {"key": "cp-1"}, "press": press})), Some(ctl("pm-mock", "cp-1")));
        assert_eq!(button_control(&json!({"type": "Button", "props": {"key": "cp-1"}})), None);
        assert_eq!(button_control(&json!({"type": "Button", "props": {}, "press": press})), None);
        assert_eq!(button_control(&json!({"type": "Text", "props": {"key": "x"}, "press": press})), None);
    }

    #[test]
    fn only_http_links_open() {
        assert_eq!(safe_href(&json!("https://gitlab.exemplo/mr/1")), Some("https://gitlab.exemplo/mr/1".to_owned()));
        assert_eq!(safe_href(&json!("javascript:alert(1)")), None);
        assert_eq!(safe_href(&json!("file:///etc/passwd")), None);
    }

    #[test]
    fn cut_text_keeps_links_and_buttons_clickable() {
        let link = json!({"type": "Link", "props": {"href": "https://gitlab.exemplo/pm/PM-1"}, "children": ["PM-1"]});
        assert_eq!(plain_deep(&[json!({"type": "Text", "children": ["PM ", link]})]), None);
        assert_eq!(plain_deep(&[json!({"type": "Button", "props": {"key": "k"}})]), None);
        assert_eq!(plain_deep(&[json!("texto "), json!({"type": "Text", "children": ["só texto"]})]).as_deref(), Some("texto só texto"));
    }

    #[test]
    fn colors_accept_hex_and_ink_names() {
        assert!(color(&json!("#5aa6ff")).is_some());
        assert!(color(&json!("redBright")).is_some());
        assert_eq!(color(&json!("nope")), None);
    }

    #[test]
    fn delta_keeps_the_band_and_brings_back_unchanged_panes() {
        let a = json!({"id": "a", "tree": {"type": "Text", "children": ["grande"]}});
        let full = apply_delta(json!({"type": "Text"}), vec![a.clone(), json!({"id": "b"})],
            json!({"panes": [{"id": "c", "tree": null}, {"id": "a", "same": true}], "shown_id": "c"}));
        assert_eq!(full, json!({"above": {"type": "Text"}, "panes": [{"id": "c", "tree": null}, a], "shown_id": "c"}));
        assert_eq!(apply_delta(json!({"type": "Text"}), vec![a], json!({"above": null, "panes": []})), json!({"above": null, "panes": []}));
    }

    fn amostras() -> Value { serde_json::from_str(include_str!("../../packages/core/src/__fixtures__/plugin-ui-arvores.json")).unwrap() }

    #[test]
    fn plugin_ui_event_reads_the_new_fields_and_tolerates_their_absence() {
        let rol = amostras()["rolPm"]["panes"].clone();
        let old = surfaces(json!({"above": amostras()["faixaPm"], "panes": rol}));
        assert_eq!(pane_ids(&old.panes), ["pm-mock-pm", "pm-mock-mr", "pm-mock-jenkins"]);
        assert_eq!((old.shown_id, old.columns, old.source), (None, None, None));
        let new = surfaces(json!({"above": null, "panes": [{"title": "sem id"}], "shown_id": null, "columns": 110, "source": "surface"}));
        assert!(new.panes.is_empty());
        assert_eq!((new.shown_id, new.columns, new.source), (Some(None), Some(110.), Some(UiSource::Surface)));
        let odd = surfaces(json!({"shown_id": 7, "columns": -1, "source": "mobile"}));
        assert_eq!((odd.shown_id, odd.columns, odd.source), (None, None, None));
        assert_eq!(surfaces(json!("texto")), Surfaces::default());
        assert!(old.caps.is_empty(), "servidor antigo não anuncia nada");
        assert_eq!(surfaces(json!({"caps": ["btw", 3, "x"], "source": "terminal"})).caps, ["btw", "x"]);
    }

    #[test]
    fn active_pane_follows_the_server_only_when_it_names_a_pane_in_the_list() {
        let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        assert_eq!(active_pane(&ids, &Some(Some("a".into())), Some("b")).as_deref(), Some("a"));
        // shown_id de painel que ainda não chegou: vale a escolha local, nunca o corpo vazio.
        assert_eq!(active_pane(&ids, &Some(Some("z".into())), Some("b")).as_deref(), Some("b"));
        assert_eq!(active_pane(&ids, &None, None).as_deref(), Some("c"));
        assert_eq!(active_pane(&[], &Some(None), Some("a")), None);
        assert!(follows_server(&ids, &Some(Some("c".into()))));
        assert!(!follows_server(&ids, &None) && !follows_server(&ids, &Some(None)));
    }

    #[test]
    fn width_that_reaches_the_place_fills_it_and_a_missing_place_keeps_the_cap() {
        assert!(fills_place(Some(110.), Some(110.)) && fills_place(Some(120.), Some(110.)));
        assert!(!fills_place(Some(24.), Some(58.)));
        // Servidor de hoje, sem `columns`: continua teto, nada vira 100% por engano.
        assert!(!fills_place(Some(110.), None) && !fills_place(None, Some(110.)) && !fills_place(Some(5.), Some(0.)));
    }

    #[test]
    fn local_tab_starts_on_the_newest_survives_redraws_and_falls_back_to_the_previous_neighbour() {
        let v = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(follow_local(&[], &v(&["a", "b", "c"]), None).as_deref(), Some("c"));
        assert_eq!(follow_local(&v(&["a", "b", "c"]), &v(&["a", "b", "c"]), Some("a")).as_deref(), Some("a"));
        assert_eq!(follow_local(&v(&["a", "b"]), &v(&["a", "b", "d"]), Some("a")).as_deref(), Some("d"));
        assert_eq!(follow_local(&v(&["a", "b", "c"]), &v(&["a", "c"]), Some("b")).as_deref(), Some("a"));
        assert_eq!(follow_local(&v(&["a", "b"]), &v(&["b"]), Some("a")).as_deref(), Some("b"));
        assert_eq!(follow_local(&v(&["a"]), &[], Some("a")), None);
    }

    #[test]
    fn tab_row_scrolls_to_the_active_tab_only_when_it_changes_and_the_row_has_geometry() {
        let v = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let ids = v(&["a", "b", "c"]);
        // Abrir: nada visto ainda, a ativa é a última.
        assert_eq!(tab_scroll_target(None, &ids, Some("c"), true), TabScroll::To(2));
        // Troca pelo servidor e por clique: a ativa mudou.
        assert_eq!(tab_scroll_target(Some("c"), &ids, Some("a"), true), TabScroll::To(0));
        assert_eq!(tab_scroll_target(Some("a"), &ids, Some("b"), true), TabScroll::To(1));
        // Redesenho sem troca: a rolagem da pessoa é respeitada.
        assert_eq!(tab_scroll_target(Some("b"), &ids, Some("b"), true), TabScroll::Idle);
        // Ativa removida: a vizinha assume e entra na vista.
        assert_eq!(tab_scroll_target(Some("b"), &v(&["a", "c"]), Some("a"), true), TabScroll::To(0));
        // Primeiro quadro, sem geometria: espera, sem rolar nem marcar.
        assert_eq!(tab_scroll_target(None, &ids, Some("c"), false), TabScroll::Wait);
        // Sem aba ativa, ativa fora da lista ou uma aba só: nada.
        assert_eq!(tab_scroll_target(None, &ids, None, true), TabScroll::Idle);
        assert_eq!(tab_scroll_target(Some("a"), &[], None, true), TabScroll::Idle);
        assert_eq!(tab_scroll_target(None, &ids, Some("z"), true), TabScroll::Idle);
        assert_eq!(tab_scroll_target(None, &v(&["a"]), Some("a"), true), TabScroll::Idle);
        assert_eq!(tab_scroll_target(None, &v(&["a"]), Some("a"), false), TabScroll::Idle);
    }

    #[test]
    fn absolute_card_without_a_color_takes_the_opaque_background_of_the_place() {
        let place: Hsla = rgb(0x101010).into();
        // No terminal as células do cartão substituem as de baixo: sem fundo, o texto dele e o da linha se misturam.
        assert_eq!(box_background(&json!({"position": "absolute", "top": 1}), place), Some(place));
        assert_eq!(box_background(&json!({"position": "absolute", "backgroundColor": "#30363d"}), place), color(&json!("#30363d")));
        assert_eq!(box_background(&json!({"flexDirection": "row"}), place), None);
        assert_eq!(box_background(&json!({"backgroundColor": "red"}), place), color(&json!("red")));
    }

    #[test]
    fn hover_scope_is_lit_by_itself_or_by_an_absolute_box_inside_it() {
        let card: HashSet<String> = ["above-prompt/1/2".to_owned()].into();
        assert!(scope_active(&card, "above-prompt/1"));
        assert!(scope_active(&card, "above-prompt/1/2"));
        assert!(!scope_active(&card, "above-prompt/1/2/0"));
        // O caminho é por segmento: "/12" não acende "/1".
        let other: HashSet<String> = ["above-prompt/12".to_owned()].into();
        assert!(!scope_active(&other, "above-prompt/1"));
    }

    #[test]
    fn hover_props_apply_only_when_lit_and_skip_cross_place_scopes() {
        let card = amostras()["hoverV29"]["children"][1].clone();
        assert_eq!(hover_props(&card, false)["display"], "none");
        assert_eq!(hover_props(&card, true)["display"], "flex");
        assert_eq!(hover_props(&card, true)["position"], "absolute");
        let v30 = json!({"type": "Text", "hover": {"scope": "vitrine-V30", "color": "#e8a33d"}});
        assert!(hover_props(&v30, true)["color"].is_null());
        assert!(wants_hover(&amostras()["hoverV29"]) && wants_hover(&amostras()["hoverV28"]));
        assert!(!wants_hover(&json!({"type": "Box", "props": {"key": "k"}, "children": [{"type": "Text", "children": ["a"]}]})));
    }

    #[test]
    fn hover_props_copy_the_props_only_when_a_hover_applies() {
        let card = amostras()["hoverV29"]["children"][1].clone();
        assert!(matches!(hover_props(&card, false), Cow::Borrowed(_)));
        assert!(matches!(hover_props(&card, true), Cow::Owned(_)));
        let v30 = json!({"type": "Text", "props": {"bold": true}, "hover": {"scope": "vitrine-V30", "color": "#e8a33d"}});
        assert!(matches!(hover_props(&v30, true), Cow::Borrowed(_)));
        // Hover aplicado sobre nó sem props: as props nascem só com o hover.
        assert_eq!(*hover_props(&json!({"type": "Text", "hover": {"bold": true}}), true), json!({"bold": true}));
    }

    #[test]
    fn hovered_pieces_survive_a_redraw_only_while_they_are_still_drawn_in_a_shown_place() {
        let v29 = amostras()["hoverV29"].clone();
        let (scope, card) = ("above-prompt".to_owned(), "above-prompt/1".to_owned());
        let lit = || -> HashSet<String> { [scope.clone(), card.clone(), "painel/0".to_owned()].into() };
        // Mesma árvore na faixa, painel fora da tela (outra aba): fica só o que a faixa desenha.
        let mut hovered = lit();
        keep_hovered(&mut hovered, &[("above-prompt", &v29)]);
        assert_eq!(hovered, [scope.clone(), card.clone()].into());
        // O cartão saiu da árvore debaixo do ponteiro: o id dele sai junto e o escopo segue aceso.
        let without_card = json!({"type": "Box", "props": {"key": "V29-escopo"}, "hover": {"borderColor": "#5aa6ff"}, "children": []});
        let mut hovered = lit();
        keep_hovered(&mut hovered, &[("above-prompt", &without_card)]);
        assert_eq!(hovered, [scope.clone()].into());
        // Escopo sem nenhum hover não avisa o app do ponteiro: não pode ficar no conjunto.
        let mut hovered = lit();
        keep_hovered(&mut hovered, &[("above-prompt", &json!({"type": "Box", "props": {"key": "k"}, "children": []}))]);
        assert!(hovered.is_empty());
    }

    fn ctl(plugin: &str, key: &str) -> Control { Control { plugin: plugin.into(), key: key.into() } }

    #[test]
    fn fields_are_the_inputs_with_a_key_and_a_mod_in_tree_order() {
        let tree = json!({"type": "Box", "children": [amostras()["campoV18"], {"type": "Input", "props": {"label": "sem key"}},
            {"type": "Input", "props": {"key": "sem-mod"}}]});
        assert_eq!(fields(&tree), vec![FieldSpec { control: ctl("vitrine", "V18-campo"), placeholder: "digite e tecle Enter".into(), value: String::new() }]);
        assert!(fields(&json!({"type": "Text", "children": ["a"]})).is_empty());
        // Ordem da árvore, inclusive dentro de filhos aninhados.
        let press = json!({"plugin": "m", "handle": 1});
        let two = json!({"type": "Box", "children": [
            {"type": "Box", "children": [{"type": "Input", "props": {"key": "b", "value": "x"}, "press": press}]},
            {"type": "Input", "props": {"key": "a", "placeholder": "p"}, "press": press}]});
        assert_eq!(fields(&two).iter().map(|f| f.control.key.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        // A `key` só é única dentro do mod, e o mod só dentro do lugar.
        assert_ne!(field_id("vitrine-campos", &ctl("vitrine", "V18-campo")), field_id("above-prompt", &ctl("vitrine", "V18-campo")));
        assert_ne!(field_id("above-prompt", &ctl("um", "k")), field_id("above-prompt", &ctl("outro", "k")));
    }

    #[test]
    fn typing_reaches_the_route_only_from_a_surface_session_that_is_not_read_only() {
        assert!(accepts_typing(Some(UiSource::Surface), false));
        // Com terminal, servidor que não diz a fonte, ou só leitura: campo desabilitado, nada vai para a rota.
        assert!(!accepts_typing(Some(UiSource::Terminal), false));
        assert!(!accepts_typing(None, false));
        assert!(!accepts_typing(Some(UiSource::Surface), true));
        assert_eq!(input_request(Some(UiSource::Terminal), false, "s", &ctl("m", "k"), "change", "a"), None);
        assert_eq!(input_request(None, false, "s", &ctl("m", "k"), "submit", "a"), None);
        assert_eq!(input_request(Some(UiSource::Surface), true, "s", &ctl("m", "k"), "submit", "a"), None);
    }

    #[test]
    fn keyless_input_shows_the_drawn_value_or_the_muted_placeholder() {
        assert_eq!(keyless_text(&json!({"placeholder": "p", "value": "v"})), ("v".to_owned(), false));
        assert_eq!(keyless_text(&json!({"placeholder": "p", "value": ""})), ("p".to_owned(), true));
        assert_eq!(keyless_text(&json!({})), (String::new(), true));
    }

    #[test]
    fn an_older_server_gets_one_retry_in_its_own_contract() {
        let press = json!({"site": "faixa", "plugin": "m", "key": "k"});
        assert_eq!(older_server_retry("press", &press, Some(422)), Some(("press", json!({"site": "faixa", "key": "k"}))));
        let input = json!({"site": "p", "plugin": "m", "key": "k", "kind": "change", "value": "a"});
        assert_eq!(older_server_retry("input", &input, Some(422)), Some(("input", json!({"site": "p", "key": "k", "kind": "change", "value": "a"}))));
        for status in [404, 405] {
            assert_eq!(older_server_retry("close", &json!({"site": "p"}), Some(status)), Some(("press", json!({"site": "p", "key": "__close__"}))));
        }
        // A segunda tentativa já vai sem o mod, e uma recusa de verdade (409, sem resposta) não é versão.
        assert_eq!(older_server_retry("press", &json!({"site": "faixa", "key": "k"}), Some(422)), None);
        assert_eq!(older_server_retry("press", &press, Some(409)), None);
        assert_eq!(older_server_retry("close", &json!({"site": "p"}), Some(409)), None);
        assert_eq!(older_server_retry("show", &json!({"site": "p"}), Some(404)), None);
        assert_eq!(older_server_retry("press", &press, None), None);
    }

    #[test]
    fn input_request_carries_site_mod_key_kind_and_value() {
        assert_eq!(input_request(Some(UiSource::Surface), false, "vitrine-campos", &ctl("vitrine", "V18-campo"), "change", ""),
            Some(json!({"site": "vitrine-campos", "plugin": "vitrine", "key": "V18-campo", "kind": "change", "value": ""})));
    }

    #[test]
    fn focused_redraw_keeps_the_typed_text_and_blur_applies_the_pending_value() {
        let mut sync = FieldSync::default();
        // Desenho novo do mod com a pessoa digitando: o texto fica, o valor fica pendente.
        assert_eq!(sync.draw(Some("ab"), "abc", true), None);
        // Redesenho do app ainda em foco: nada muda.
        assert_eq!(sync.draw(None, "abc", true), None);
        // Fora de foco, o pendente entra uma vez só.
        assert_eq!(sync.draw(None, "abc", false).as_deref(), Some("ab"));
        assert_eq!(sync.draw(None, "ab", false), None);
        // O desenho que chega com o campo fora de foco entra na hora; igual ao que se vê, nada a fazer.
        assert_eq!(sync.draw(Some("x"), "ab", false).as_deref(), Some("x"));
        assert_eq!(sync.draw(Some("x"), "x", false), None);
    }

    #[test]
    fn typing_after_the_pending_value_arrived_discards_it_so_blur_keeps_the_typed_text() {
        let mut sync = FieldSync::default();
        assert_eq!(sync.draw(Some("ab"), "abc", true), None);
        sync.typed("abcd");
        // Fora de foco, o texto digitado e não enviado fica.
        assert_eq!(sync.draw(None, "abcd", false), None);
        // Um desenho novo depois disso volta a ficar pendente e entra ao perder o foco.
        assert_eq!(sync.draw(Some("x"), "abcd", true), None);
        assert_eq!(sync.draw(None, "abcd", false).as_deref(), Some("x"));
    }

    #[test]
    fn the_redraw_right_after_the_own_submit_applies_even_with_focus() {
        // É assim que o mod limpa o campo depois do envio: o valor desenhado é o mesmo de antes (vazio), mas entra.
        let mut sync = FieldSync::default();
        sync.submitted();
        assert_eq!(sync.draw(Some(""), "abc", true).as_deref(), Some(""));
        // Só o primeiro: o seguinte, em foco, volta a esperar.
        assert_eq!(sync.draw(Some("zz"), "d", true), None);
        // O eco da digitação que chega depois do Enter (igual ao que se vê) não gasta a vez da resposta ao envio.
        let mut sync = FieldSync::default();
        sync.submitted();
        assert_eq!(sync.draw(Some("abc"), "abc", true), None);
        assert_eq!(sync.draw(Some(""), "abc", true).as_deref(), Some(""));
        // Um eco atrasado que chega antes do Enter fica pendente e não atrapalha a resposta ao envio.
        let mut sync = FieldSync::default();
        assert_eq!(sync.draw(Some("ab"), "abc", true), None);
        sync.submitted();
        // Esse eco não volta ao perder o foco antes da resposta: o texto enviado fica.
        assert_eq!(sync.draw(None, "abc", false), None);
        assert_eq!(sync.draw(Some(""), "abc", true).as_deref(), Some(""));
        // Digitar de novo fecha a vez: o desenho seguinte não apaga o que se digita.
        let mut sync = FieldSync::default();
        sync.submitted();
        sync.typed("abcd");
        assert_eq!(sync.draw(Some(""), "abcd", true), None);
    }

    #[test]
    fn after_the_submit_late_echoes_of_the_changes_sent_do_not_take_the_turn_of_the_answer() {
        // `a`, `ab` e `abc` vão como `change` com o eco atrasado, e o Enter sai antes de qualquer eco.
        let typed = |sync: &mut FieldSync| for v in ["a", "ab", "abc"] { sync.typed(v); };
        let mut sync = FieldSync::default();
        typed(&mut sync);
        sync.submitted();
        // Os ecos de antes do envio chegam um a um: o campo segue com o texto enviado.
        assert_eq!(sync.draw(Some("a"), "abc", true), None);
        assert_eq!(sync.draw(Some("ab"), "abc", true), None);
        // A resposta, fora do que foi mandado, entra mesmo com foco.
        assert_eq!(sync.draw(Some(""), "abc", true).as_deref(), Some(""));
        // Ecos e resposta no mesmo desenho: vale o último valor, que é a resposta.
        let mut sync = FieldSync::default();
        typed(&mut sync);
        sync.submitted();
        assert_eq!(sync.draw(Some(""), "abc", true).as_deref(), Some(""));
        // O histórico zera quando um valor entra: depois da resposta, `a` volta a ser um valor como outro qualquer.
        sync.submitted();
        assert_eq!(sync.draw(Some("a"), "", true).as_deref(), Some("a"));
    }

    #[test]
    fn an_answer_equal_to_a_value_typed_before_waits_for_the_blur() {
        // Limite conhecido: a pessoa apagou tudo (`change ""`) antes de digitar `x`. A resposta `""` ao envio é igual a
        // um valor mandado, então passa por eco velho, fica pendente em foco e só entra quando o campo perde o foco.
        // O mod não ecoou: o desenho anterior também era `""`, e mesmo assim a resposta espera o blur.
        let mut sync = FieldSync::new("");
        for v in ["a", "", "x"] { sync.typed(v); }
        sync.submitted();
        assert_eq!(sync.draw(Some(""), "x", true), None);
        assert_eq!(sync.draw(None, "x", true), None);
        assert_eq!(sync.draw(None, "x", false).as_deref(), Some(""));
    }

    #[test]
    fn a_redraw_without_a_change_of_the_drawn_value_does_not_create_a_pending_value() {
        // O mod desenha `""` e não devolve o que se digita; outro mod redesenha a faixa no meio da digitação.
        let mut sync = FieldSync::new("");
        sync.typed("abc");
        assert_eq!(sync.draw(Some(""), "abc", true), None);
        // A pessoa clica fora: o texto digitado fica.
        assert_eq!(sync.draw(None, "abc", false), None);
        // Valor que mudou em relação ao desenho anterior continua pendente e entra no blur.
        assert_eq!(sync.draw(Some("x"), "abc", true), None);
        assert_eq!(sync.draw(None, "abc", false).as_deref(), Some("x"));
    }

    #[test]
    fn a_redraw_without_a_change_keeps_the_pending_value_already_there() {
        let mut sync = FieldSync::new("");
        assert_eq!(sync.draw(Some("ab"), "abc", true), None);
        assert_eq!(sync.draw(Some("ab"), "abc", true), None);
        assert_eq!(sync.draw(None, "abc", false).as_deref(), Some("ab"));
    }

    #[test]
    fn without_a_previous_draw_the_first_value_counts_as_a_change() {
        let mut sync = FieldSync::default();
        assert_eq!(sync.draw(Some(""), "abc", true), None);
        assert_eq!(sync.draw(None, "abc", false).as_deref(), Some(""));
    }

    #[test]
    fn the_answer_to_the_submit_equal_to_the_previous_draw_still_applies() {
        // O mod limpa o campo com o mesmo vazio que já desenhava: a vez da resposta não depende de o valor mudar.
        let mut sync = FieldSync::new("");
        sync.typed("abc");
        sync.submitted();
        assert_eq!(sync.draw(Some(""), "abc", true).as_deref(), Some(""));
    }

    // Os mesmos casos da `InputOutbox` em `packages/core/src/pluginField.test.ts`.
    #[test]
    fn one_request_in_flight_and_only_the_latest_change_waits() {
        let mut outbox = Outbox::default();
        assert_eq!(outbox.push("change", "a".into()), Some(("change", "a".to_owned())));
        assert_eq!(outbox.push("change", "ab".into()), None);
        assert_eq!(outbox.push("change", "abc".into()), None);
        assert_eq!(outbox.done(), Some(("change", "abc".to_owned())));
        assert_eq!(outbox.done(), None);
        // Livre de novo: o próximo sai na hora.
        assert_eq!(outbox.push("change", "abcd".into()), Some(("change", "abcd".to_owned())));
    }

    #[test]
    fn the_submit_goes_after_the_pending_changes_and_what_is_typed_after_it_goes_after() {
        let mut outbox = Outbox::default();
        assert_eq!(outbox.push("change", "a".into()), Some(("change", "a".to_owned())));
        for (kind, value) in [("change", "ab"), ("submit", "ab"), ("change", "abc"), ("change", "abcd")] {
            assert_eq!(outbox.push(kind, value.into()), None);
        }
        assert_eq!(outbox.done(), Some(("change", "ab".to_owned())));
        assert_eq!(outbox.done(), Some(("submit", "ab".to_owned())));
        assert_eq!(outbox.done(), Some(("change", "abcd".to_owned())));
        assert_eq!(outbox.done(), None);
    }

    #[test]
    fn two_submits_in_a_row_both_go_in_order() {
        let mut outbox = Outbox::default();
        for value in ["a", "b", "c"] { outbox.push("submit", value.into()); }
        assert_eq!(outbox.done(), Some(("submit", "b".to_owned())));
        assert_eq!(outbox.done(), Some(("submit", "c".to_owned())));
        assert_eq!(outbox.done(), None);
    }

    #[test]
    fn change_and_enter_are_the_only_input_events_sent() {
        assert_eq!(input_kind(&InputEvent::Change), Some("change"));
        assert_eq!(input_kind(&InputEvent::PressEnter { secondary: false, shift: false }), Some("submit"));
        assert_eq!(input_kind(&InputEvent::Focus), None);
        assert_eq!(input_kind(&InputEvent::Blur), None);
    }
}
