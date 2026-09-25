//! Configuração do servidor pelo app: as páginas Notificações, Anexos e Avançado editam UM rascunho, e o
//! Salvar do rodapé grava tudo o que foi mexido nelas num só `POST /api/config`. Porta de `serverConfig.svelte.ts`,
//! `ServerSettings.svelte` e `LinhaConfig.svelte`. As horas silenciosas têm leitura e Salvar próprios (`PushQuiet.svelte`),
//! fora do rascunho: são gravadas no servidor e silenciam o push que chega no celular. A Voz usa o mesmo rascunho.
use super::*;
use super::device::Remote;
use super::settings::{segments_with_hints, settings_box, Page};
use gpui_kit::base::AccordionTrigger;
use gpui_kit::component::{select::{Select, SelectEvent, SelectState}, searchable_list::SearchableListItem, switch::Switch, tooltip::Tooltip};
use serde_json::Map;

#[derive(Clone, Copy, PartialEq)]
enum Kind { Toggle, Number(&'static str), Text,
    /// Entra mas não sai: o servidor devolve só a máscara, e o campo nunca a recebe — tocado, ela voltaria como chave.
    Secret,
    /// Lista fechada (`escolha` do web): os valores e o rótulo do vazio, que é o "padrão".
    Choice(&'static [&'static str], &'static str) }

/// Opção de um campo `Choice`; o rótulo é lido a cada desenho, para seguir a troca de idioma.
#[derive(Clone)]
struct Choice { value: &'static str, empty: &'static str }

impl SearchableListItem for Choice {
    type Value = &'static str;
    fn title(&self) -> SharedString { if self.value.is_empty() { tr(self.empty).into() } else { self.value.into() } }
    fn value(&self) -> &&'static str { &self.value }
}

type Picker = Entity<SelectState<Vec<Choice>>>;

/// Estilos do ditado, na ordem do web (`estilosDitado`).
const STYLES: [&str; 3] = ["limpar", "prosa", "briefing"];
/// Campos de cada seção que abre e fecha na Voz: outro serviço de transcrição, outro para organizar, o do briefing.
const SECTIONS: [&[&str]; 3] = [&["transcription_base_url", "transcription_model"],
    &["llm_base_url", "llm_api_key", "llm_model", "llm_reasoning_effort"], &["llm_briefing_base_url", "llm_briefing_api_key", "llm_briefing_model"]];

struct Field { key: &'static str, label: &'static str, help: &'static str, icon: IconName, kind: Kind, page: Page }

/// Na ordem do `CAMPOS` do web, filtrada por página.
const FIELDS: [Field; 26] = [
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
    Field { key: "automations", label: "server_automations", help: "server_automations_help", icon: IconName::Zap, kind: Kind::Toggle, page: Page::Advanced },
    Field { key: "mostrar_pensamento", label: "server_thinking", help: "server_thinking_help", icon: IconName::Eye, kind: Kind::Toggle, page: Page::Advanced },
    Field { key: "traduzir_pensamento", label: "server_translate_thinking", help: "server_translate_thinking_help", icon: IconName::Languages,
        kind: Kind::Toggle, page: Page::Advanced },
    Field { key: "editor", label: "server_editor", help: "server_editor_help", icon: IconName::SquarePen, kind: Kind::Text, page: Page::Advanced },
    Field { key: "jev_api_key", label: "server_jev_key", help: "server_jev_key_help", icon: IconName::Key, kind: Kind::Secret, page: Page::Advanced },
    // Logo abaixo da chave, como no web: é o único campo cujo efeito depende dela.
    Field { key: "jev_padrao", label: "server_jev_default", help: "server_jev_default_help", icon: IconName::Rocket, kind: Kind::Toggle, page: Page::Advanced },
    Field { key: "jev_texto_base_url", label: "server_jev_endpoint", help: "server_jev_endpoint_help", icon: IconName::Globe, kind: Kind::Text,
        page: Page::Advanced },
    Field { key: "jev_texto_api_key", label: "server_jev_text_key", help: "server_jev_text_key_help", icon: IconName::Key, kind: Kind::Secret,
        page: Page::Advanced },
    Field { key: "jev_texto_modelo", label: "server_jev_model", help: "server_jev_model_help", icon: IconName::Bot, kind: Kind::Text, page: Page::Advanced },
    Field { key: "jev_texto_cmd", label: "server_jev_cmd", help: "server_jev_cmd_help", icon: IconName::SquareTerminal, kind: Kind::Text,
        page: Page::Advanced },
    // Voz, na ordem do `VozSettings.svelte`; a página as distribui pelas seções dela.
    Field { key: "groq_api_key", label: "voice_groq", help: "voice_groq_help", icon: IconName::Key, kind: Kind::Secret, page: Page::Voice },
    Field { key: "transcription_base_url", label: "voice_transcription_endpoint", help: "voice_transcription_endpoint_help", icon: IconName::Globe,
        kind: Kind::Text, page: Page::Voice },
    Field { key: "transcription_model", label: "voice_transcription_model", help: "voice_transcription_model_help", icon: IconName::Bot,
        kind: Kind::Text, page: Page::Voice },
    Field { key: "ditado_vocabulario", label: "voice_vocabulary", help: "voice_vocabulary_help", icon: IconName::BookOpen, kind: Kind::Text,
        page: Page::Voice },
    Field { key: "llm_base_url", label: "voice_llm_endpoint", help: "voice_llm_endpoint_help", icon: IconName::Globe, kind: Kind::Text, page: Page::Voice },
    Field { key: "llm_api_key", label: "voice_llm_key", help: "voice_llm_key_help", icon: IconName::Key, kind: Kind::Secret, page: Page::Voice },
    Field { key: "llm_model", label: "voice_llm_model", help: "voice_llm_model_help", icon: IconName::Bot, kind: Kind::Text, page: Page::Voice },
    Field { key: "llm_reasoning_effort", label: "voice_llm_effort", help: "voice_llm_effort_help", icon: IconName::SlidersHorizontal,
        kind: Kind::Choice(&["", "none", "low", "medium", "high"], "voice_llm_effort_default"), page: Page::Voice },
    Field { key: "llm_briefing_base_url", label: "voice_briefing_endpoint", help: "voice_briefing_endpoint_help", icon: IconName::Globe,
        kind: Kind::Text, page: Page::Voice },
    Field { key: "llm_briefing_api_key", label: "voice_briefing_key", help: "voice_briefing_key_help", icon: IconName::Key, kind: Kind::Secret,
        page: Page::Voice },
    Field { key: "llm_briefing_model", label: "voice_briefing_model", help: "voice_briefing_model_help", icon: IconName::Bot, kind: Kind::Text,
        page: Page::Voice },
];

fn field(key: &str) -> &'static Field { FIELDS.iter().find(|f| f.key == key).expect("campo declarado em FIELDS") }

/// Veredito com o "por quê?" que expande, como no web: Automações e o raciocínio da organização.
const VERDICTS: [(&str, &str, &str); 2] = [("automations", "server_recommended_on", "server_automations_why"),
    ("llm_reasoning_effort", "voice_llm_effort_verdict", "voice_llm_effort_why")];

/// Do bloco só leitura, o que Máquinas já mostra não se repete aqui.
const READ_IN_MACHINES: [&str; 5] = ["port", "lan_bind_ip", "server_id", "public_url", "terminal_origem_ok"];
const READ_LABELS: [&str; 3] = ["terminal_panel", "traducao_pensamento", "versao"];
/// Códigos de descrição do `.env` que o web traduz (`DESCRICAO_ENV`); código fora daqui não vira texto nenhum.
const ENV_CODES: [&str; 30] = ["terminal", "pricing_offline", "claude_config_dirs", "engines_file", "codex_sync_enabled", "auto_resume",
    "omp_plugin_sync", "omp_claude_context", "lan_bind_ip", "port", "auth_token", "projects_dir", "reload", "diag_term_input", "front_port",
    "public_url", "server_id", "vapid_public", "vapid_private", "vapid_subject", "stall_poll_seconds", "omp_plugin_sync_interval", "sync",
    "sync_bootstrap", "sync_data", "sync_session_secret", "sync_rate_max", "sync_rate_window", "forwarded_allow_ips", "deploy_secret"];
/// `ALERTA_ENV` do web: código do alerta e a mensagem dele.
const ENV_ALERTS: [(&str, &str); 1] = [("codex_sync_desligado", "config_server_env_alerta_codex_sync")];

/// Rascunho do servidor: só entra chave que a última leitura boa trouxe. Sem `insert` fora deste módulo, nenhuma página
/// grava por cima do que não foi lido (leitura em curso, que falhou ou reconexão deixam `campos` vazio).
mod draft {
    use serde_json::{Map, Value};

    #[derive(Default)]
    pub(super) struct Draft(Map<String, Value>);

    impl Draft {
        /// `read` são os `campos` da última leitura ou gravação boa.
        pub(super) fn stage(&mut self, read: &Map<String, Value>, key: &str, value: Value) -> bool {
            if !read.contains_key(key) { return false; }
            self.0.insert(key.to_owned(), value);
            true
        }
        pub(super) fn get(&self, key: &str) -> Option<&Value> { self.0.get(key) }
        /// "Desfazer": a chave sai do rascunho e a tela volta ao valor do servidor.
        pub(super) fn unstage(&mut self, key: &str) { self.0.remove(key); }
        pub(super) fn contains_key(&self, key: &str) -> bool { self.0.contains_key(key) }
        pub(super) fn is_empty(&self) -> bool { self.0.is_empty() }
        pub(super) fn snapshot(&self) -> Map<String, Value> { self.0.clone() }
        /// Sai só a chave cujo valor ainda é o enviado.
        pub(super) fn settle(&mut self, sent: &Map<String, Value>) {
            for (key, value) in sent {
                if self.0.get(key) == Some(value) { self.0.remove(key); }
            }
        }
    }
}

/// Páginas que leem e gravam o rascunho do servidor.
pub(super) fn is_server_page(page: Page) -> bool { matches!(page, Page::Voice | Page::Notifications | Page::Attachments | Page::Advanced) }

#[derive(Default)]
pub(in crate::app) struct ServerConfig {
    load: Remote<()>,
    /// `campos` da última leitura ou gravação: valor, se é segredo definido e de onde veio (`origem`).
    fields: Map<String, Value>,
    /// O que foi mexido e ainda não gravado, das páginas todas. Morre só na troca de servidor.
    draft: draft::Draft,
    saving: bool,
    save_seq: u64,
    /// Número do salvar cujo "salvo" está no rodapé: some 2,5 s depois se nenhum outro o trocou.
    saved: Option<u64>,
    save_error: Option<String>,
    inputs: Vec<(&'static str, Entity<InputState>)>,
    /// `somente_leitura` e `variaveis_env` da última leitura; o POST devolve só o primeiro.
    read: Map<String, Value>,
    env: Vec<Value>,
    /// Campo de "Adicionar" das pastas mapeadas.
    root: Option<Entity<InputState>>,
    /// Diálogo de pasta do sistema aberto: dois ao mesmo tempo voltariam fora de ordem.
    picking: bool,
    pick_error: Option<String>,
    choices: Vec<(&'static str, Picker)>,
    /// Chaves com o "por quê?" aberto.
    why: Vec<&'static str>,
    /// Seções da Voz abertas (`SECTIONS`). A primeira leitura boa abre as que já têm valor, uma vez: um Salvar depois
    /// não reabre o que a pessoa fechou.
    open: [bool; 3],
    opened_by_read: bool,
    /// Foco dos três disparadores de seção.
    section_focus: Vec<FocusHandle>,
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
        (self.read, self.env) = (Map::new(), Vec::new());
        (self.quiet.load, self.quiet.saving, self.quiet.note) = (Remote::default(), false, None);
    }

    /// Valor que a tela mostra: o do rascunho, senão o do servidor.
    fn current(&self, key: &str) -> Value {
        self.draft.get(key).cloned().or_else(|| self.fields.get(key).and_then(|f| f.get("valor")).cloned()).unwrap_or(Value::Null)
    }

    fn edited_in_app(&self, key: &str) -> bool { self.fields.get(key).and_then(|f| f.get("origem")).and_then(Value::as_str) == Some("app") }

    /// Texto do campo: o segredo mostra só o que foi digitado aqui, nunca o que veio do servidor.
    fn input_text(&self, key: &str, kind: Kind) -> String {
        if kind == Kind::Secret { return self.draft.get(key).map(text_of).unwrap_or_default(); }
        text_of(&self.current(key))
    }

    /// Máscara do segredo guardado (`gsk_••••1234`), se há um.
    fn secret_mask(&self, key: &str) -> Option<String> {
        let field = self.fields.get(key)?;
        (field.get("definido") == Some(&Value::Bool(true))).then(|| text_of(field.get("valor").unwrap_or(&Value::Null)))
    }

    /// `scan_roots` é a string "a,b" do `CP_SCAN_ROOTS`; a tela edita como lista.
    fn roots(&self) -> Vec<String> {
        text_of(&self.current("scan_roots")).split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned).collect()
    }

    /// Devolve se adicionou: vazio ou repetido não mexe, e quem digitou não perde o texto.
    fn add_root(&mut self, path: &str) -> bool {
        let (path, mut list) = (path.trim(), self.roots());
        if path.is_empty() || list.iter().any(|r| r == path) { return false; }
        list.push(path.to_owned());
        self.stage("scan_roots", Value::String(list.join(",")))
    }

    fn remove_root(&mut self, path: &str) {
        let list: Vec<String> = self.roots().into_iter().filter(|r| r != path).collect();
        self.stage("scan_roots", Value::String(list.join(",")));
    }

    /// A única porta de escrita no rascunho: devolve `false` quando a chave não veio da última leitura boa.
    fn stage(&mut self, key: &str, value: Value) -> bool { self.draft.stage(&self.fields, key, value) }

    /// "Remover" do web: `null` no rascunho apaga o override no Salvar, e o campo volta ao valor do ambiente.
    fn removing(&self, key: &str) -> bool { self.draft.get(key) == Some(&Value::Null) }

    fn filled(&self, key: &str) -> bool { !text_of(&self.current(key)).trim().is_empty() }

    /// Chave guardada no servidor e que não sai no próximo Salvar.
    fn key_set(&self, key: &str) -> bool { self.secret_mask(key).is_some() && !self.removing(key) }

    /// Estado da transcrição, `None` desativada. Contra o rascunho, sem esperar o Salvar, como o web.
    fn transcribe_status(&self) -> Option<&'static str> {
        self.key_set("groq_api_key").then(|| if self.filled("transcription_base_url") { "voice_status_custom" } else { "voice_status_on" })
    }

    /// Estado da organização: com endpoint próprio vale a chave dele; sem, o padrão reusa a da transcrição padrão.
    fn cleanup_status(&self) -> Option<&'static str> {
        if self.filled("llm_base_url") { return self.key_set("llm_api_key").then_some("voice_status_custom"); }
        (self.transcribe_status() == Some("voice_status_on")).then_some("voice_status_default")
    }

    /// A lista mostra o valor atual; valor que ela não conhece fica sem escolha, em vez de parecer o padrão.
    fn show_choice(&self, key: &str, picker: &Picker, window: &mut Window, cx: &mut App) {
        let current = text_of(&self.current(key));
        let Kind::Choice(values, _) = field(key).kind else { return };
        let known = values.iter().find(|v| **v == current).copied();
        picker.update(cx, |state, cx| match known {
            Some(value) => state.set_selected_value(&value, window, cx),
            None => state.set_selected_index(None, window, cx),
        });
    }

    /// Abre as seções que já têm valor; só abre, nunca fecha.
    fn open_filled(&mut self) {
        if self.opened_by_read || self.fields.is_empty() { return; }
        self.opened_by_read = true;
        for n in 0..3 { self.open[n] |= SECTIONS[n].iter().any(|k| self.filled(k)); }
        self.open[1] |= self.open[2];
    }

    /// A busca levou a uma linha de seção fechada: abre o caminho até ela.
    pub(super) fn reveal(&mut self, label: &str) {
        let Some(key) = FIELDS.iter().find(|f| f.label == label).map(|f| f.key) else { return };
        if let Some(n) = SECTIONS.iter().position(|keys| keys.contains(&key)) {
            self.open[n] = true;
            if n == 2 { self.open[1] = true; }
        }
    }

    /// Resposta da gravação: sai do rascunho só a chave cujo valor ainda é o enviado — o que foi mexido durante o
    /// salvar continua lá e vai no próximo.
    fn settle(&mut self, sent: &Map<String, Value>) {
        self.draft.settle(sent);
    }
}

fn chip(text: String, color: Hsla, bg: Hsla) -> Div {
    div().px(px(6.)).rounded_full().bg(bg).text_size(px(10.5)).font_weight(FontWeight::BOLD).text_color(color).child(text)
}

fn icon_box(icon: IconName) -> Div {
    div().size(px(36.)).flex_shrink_0().rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
        .flex().items_center().justify_center().child(chrome::small_icon(icon, 16., theme::muted()))
}

fn text_of(value: &Value) -> String {
    match value { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() }
}

/// Valor de uma linha só leitura: sim/não, "—" quando vazio.
fn shown(value: &Value) -> String {
    match value {
        Value::Bool(on) => tr(if *on { "server_yes" } else { "server_no" }),
        Value::Null => "—".into(),
        Value::String(s) if s.is_empty() => "—".into(),
        other => text_of(other),
    }
}

/// Valor de uma variável do `.env`: o segredo não vem do servidor, e a linha diz só se está definida.
fn env_value(var: &Value) -> String {
    if var["segredo"] == Value::Bool(true) { return tr(if var["definida"] == Value::Bool(true) { "server_env_set" } else { "server_env_unset" }); }
    shown(&var["valor"])
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
        (s.read, s.env) = (Map::new(), Vec::new());
        (s.save_error, s.saved) = (None, None);
        let done = self.server_config_send_later();
        self.runtime.spawn(async move { done(ServerConfigReply::Loaded(seq, api.config().await)).await });
        cx.notify();
    }

    fn save_server_config(&mut self, cx: &mut Context<Self>) {
        let s = &mut self.server_config;
        if s.saving || s.draft.is_empty() { return; }
        let Some(api) = self.api.clone() else { return };
        let sent = s.draft.snapshot();
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
                    Some(Value::Object(_)) => Ok(config),
                    _ => Err(tr("invalid_response")),
                });
                let config = parsed.as_ref().ok().cloned();
                if !self.server_config.load.finish(seq, parsed.map(|_| ())) { return; }
                if let Some(mut config) = config {
                    let s = &mut self.server_config;
                    if let Value::Object(campos) = config["campos"].take() { s.fields = campos; }
                    if let Value::Object(read) = config["somente_leitura"].take() { s.read = read; }
                    // Ausente num servidor mais antigo: o bloco some.
                    if let Value::Array(env) = config["variaveis_env"].take() { s.env = env; }
                    s.open_filled();
                    self.fill_config_inputs(window, cx);
                }
            }
            ServerConfigReply::Saved(seq, sent, result) => {
                let s = &mut self.server_config;
                if seq != s.save_seq { return; }
                s.saving = false;
                // Sem `campos` não dá para confirmar o que valeu: é falha, e o rascunho fica.
                let result = result.map_err(|e| Self::failure(&e)).and_then(|r| match r.get("campos") {
                    Some(Value::Object(campos)) => Ok((campos.clone(), r.get("somente_leitura").and_then(Value::as_object).cloned())),
                    _ => Err(tr("invalid_response")),
                });
                match result {
                    Ok((campos, read)) => {
                        s.fields = campos;
                        // A tradução do raciocínio disponível muda com campo editável: a linha só leitura não fica velha.
                        if let Some(read) = read { s.read = read; }
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

    /// Campos de texto com o valor atual. Criados na primeira leitura; nas seguintes só o que não está no rascunho muda.
    fn fill_config_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_config.inputs.is_empty() {
            for field in FIELDS.iter().filter(|f| matches!(f.kind, Kind::Number(_) | Kind::Text | Kind::Secret)) {
                let (key, kind) = (field.key, field.kind);
                let input = cx.new(|cx| match kind {
                    // Como o `type="number" min="0"` do web: só dígitos entram.
                    Kind::Number(_) => InputState::new(window, cx).validate(|text, _| text.chars().all(|c| c.is_ascii_digit())),
                    // O web mostra a chave digitada; aqui ela vira pontos, para não sair em captura de tela.
                    Kind::Secret => InputState::new(window, cx).masked(true),
                    _ => InputState::new(window, cx),
                });
                let sub = cx.subscribe_in(&input, window, move |this: &mut Hangar, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.server_config.stage(key, Value::String(input.read(cx).value().to_string()));
                        cx.notify();
                    }
                });
                self.server_config._subscriptions.push(sub);
                self.server_config.inputs.push((key, input));
            }
            let root = cx.new(|cx| InputState::new(window, cx).placeholder(tr("server_root_placeholder")));
            let sub = cx.subscribe_in(&root, window, |this: &mut Hangar, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.add_typed_root(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            });
            self.server_config._subscriptions.push(sub);
            self.server_config.root = Some(root);
            for field in FIELDS.iter() {
                let (key, Kind::Choice(values, empty)) = (field.key, field.kind) else { continue };
                let items = values.iter().map(|&value| Choice { value, empty }).collect::<Vec<_>>();
                let picker = cx.new(|cx| SelectState::new(items, None, window, cx));
                let sub = cx.subscribe_in(&picker, window, move |this: &mut Hangar, _, event: &SelectEvent<Vec<Choice>>, _, cx| {
                    let SelectEvent::Confirm(Some(value)) = event else { return };
                    this.server_config.stage(key, Value::String((*value).to_owned()));
                    cx.notify();
                });
                self.server_config._subscriptions.push(sub);
                self.server_config.choices.push((key, picker));
            }
            self.server_config.section_focus = (0..SECTIONS.len()).map(|_| cx.focus_handle().tab_stop(true)).collect();
        }
        let s = &self.server_config;
        for (key, input) in &s.inputs {
            let kind = field(key).kind;
            if kind == Kind::Secret {
                let hint = tr(if s.secret_mask(key).is_some() { "server_secret_paste_new" } else { "server_secret_paste" });
                input.update(cx, |state, cx| state.set_placeholder(hint, window, cx));
            }
            if s.draft.contains_key(*key) { continue; }
            let value = s.input_text(key, kind);
            input.update(cx, |state, cx| if state.value() != value.as_str() { state.set_value(value, window, cx) });
        }
        for (key, picker) in &s.choices {
            if s.draft.contains_key(*key) { continue; }
            s.show_choice(key, picker, window, cx);
        }
    }

    /// "Remover": o campo fica vazio (a lista volta ao "padrão") até o Salvar apagar o override, ou até o Desfazer.
    fn remove_config(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let s = &mut self.server_config;
        if !s.stage(key, Value::Null) { return; }
        if let Some((_, input)) = s.inputs.iter().find(|(k, _)| *k == key) { input.update(cx, |state, cx| state.set_value("", window, cx)); }
        if let Some((_, picker)) = s.choices.iter().find(|(k, _)| *k == key) { s.show_choice(key, picker, window, cx); }
        cx.notify();
    }

    fn undo_config(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.server_config.draft.unstage(key);
        self.fill_config_inputs(window, cx);
        cx.notify();
    }

    /// Só quem adicionou limpa o campo: digitar uma pasta que já está na lista não apaga o que foi escrito.
    fn add_typed_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.server_config.root.clone() else { return };
        let path = input.read(cx).value().to_string();
        if self.server_config.add_root(&path) { input.update(cx, |state, cx| state.set_value("", window, cx)); }
        cx.notify();
    }

    /// Seletor de pasta do sistema: a escolhida entra direto na lista, sem um segundo clique em "Adicionar". A pasta é
    /// da máquina deste app; com um servidor remoto ela pode não existir lá, e o Salvar mostra a recusa dele (como o web).
    fn pick_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = &mut self.server_config;
        if s.picking { return; }
        (s.picking, s.pick_error) = (true, None);
        let owner = s.owner.clone();
        let prompt = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let chosen = prompt.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let s = &mut this.server_config;
                // Trocou de servidor com o diálogo aberto: a pasta era para o rascunho do anterior.
                if s.owner != owner { return; }
                s.picking = false;
                match chosen {
                    // Sem leitura boa a lista não é a do servidor: a escolha espera no campo de "Adicionar".
                    Ok(Ok(Some(paths))) => if let Some(path) = paths.first().map(|p| p.to_string_lossy().into_owned()) {
                        if !s.fields.contains_key("scan_roots") {
                            if let Some(root) = s.root.clone() { root.update(cx, |state, cx| state.set_value(path, window, cx)); }
                        } else { s.add_root(&path); }
                    },
                    Ok(Ok(None)) => {}
                    // Como o web: a razão do sistema (portal ausente, recusa), não um "falhou" genérico.
                    Ok(Err(error)) => s.pick_error = Some(error.to_string()),
                    Err(_) => s.pick_error = Some(tr("picker_failed")),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
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
        self.server_config.stage(key, Value::Bool(on));
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
        } else if page == Page::Voice {
            self.render_voice(cx)
        } else {
            let rows = FIELDS.iter().filter(|f| f.page == page).map(|f| self.config_row(f, cx)).collect::<Vec<_>>();
            div().mt(px(24.)).child(settings_box().children(rows))
                .when(page == Page::Advanced, |el| el.child(self.render_roots(cx)).children(self.render_read()).children(self.render_env()))
        };
        div().flex().flex_col().child(top).child(body)
            .when(page == Page::Notifications && self.api.is_some(), |el| el.child(self.render_quiet(cx)))
            .into_any_element()
    }

    /// Chip de onde a linha grava, e "editado" quando o valor veio do app e não do `.env`.
    fn config_badges(&self, key: &str) -> Div {
        div().flex().items_center().gap(px(6.))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()))
            .when(self.server_config.edited_in_app(key) && !self.server_config.removing(key),
                |el| el.child(chip(tr("server_edited"), theme::accent_text(), theme::accent_dim())))
    }

    /// A linha de cada campo, no desenho do `row` das outras páginas, com as etiquetas ao lado do título.
    fn config_row(&self, field: &Field, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        let key = field.key;
        let input = s.inputs.iter().find(|(k, _)| *k == key).map(|(_, i)| i.clone());
        // Campo que a leitura não trouxe (servidor mais antigo) fica desligado: o `stage` recusaria, e o controle mostraria
        // uma escolha que o Salvar não leva.
        let off = !s.fields.contains_key(key);
        // Liga e número à direita; texto e segredo descem para baixo da ajuda, na largura da coluna: endereço e comando
        // não cabem num campo estreito ao lado.
        let control = match field.kind {
            Kind::Toggle => Some(Switch::new(SharedString::from(format!("server-{key}"))).checked(s.current(key) == Value::Bool(true))
                .accessibility_label(tr(field.label)).disabled(off)
                .on_click(cx.listener(move |this, on: &bool, _, cx| this.set_config_toggle(key, *on, cx))).into_any_element()),
            Kind::Number(suffix) => Some(div().flex().items_center().gap(px(8.))
                .children(input.clone().map(|i| div().w(px(96.)).child(Input::new(&i).small().disabled(off).aria_label(tr(field.label)))))
                .child(div().text_size(px(13.)).text_color(theme::muted()).child(tr(suffix)))
                .into_any_element()),
            Kind::Choice(..) => s.choices.iter().find(|(k, _)| *k == key).map(|(_, picker)| div().w(px(200.))
                .child(Select::new(picker).small().disabled(off).accessibility_label(tr(field.label))).into_any_element()),
            Kind::Text | Kind::Secret => None,
        };
        let removing = s.removing(key);
        let below = matches!(field.kind, Kind::Text | Kind::Secret).then_some(input).flatten().map(|i| {
            let mask = (field.kind == Kind::Secret && !removing).then(|| s.secret_mask(key)).flatten();
            div().mt(px(8.)).flex().items_center().gap(px(10.))
                .child(div().flex_1().min_w_0().child(Input::new(&i).small().disabled(off).aria_label(tr(field.label))))
                .children(mask.map(|mask| div().id(SharedString::from(format!("server-{key}-mask"))).flex_shrink_0().flex().items_center().gap(px(6.))
                    .text_size(px(12.5)).text_color(theme::muted())
                    .child(div().font_family(theme::MONO).child(mask)).child(tr("server_secret_set"))
                    .tooltip(|window, cx| Tooltip::new(tr("server_secret_no_return")).build(window, cx))))
        });
        let open = s.why.contains(&key);
        let verdict = VERDICTS.iter().find(|(k, ..)| *k == key).map(|&(_, verdict, why)| div().flex().flex_col().gap(px(6.))
            .child(div().flex().items_center().gap(px(6.)).text_size(px(13.))
                .child(div().text_color(theme::muted()).child(tr(verdict)))
                .child(Button::new(SharedString::from(format!("server-{key}-why"))).ghost().xsmall().label(tr("accounts_engine_why"))
                    .icon(if open { IconName::ChevronUp } else { IconName::ChevronDown })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let why = &mut this.server_config.why;
                        if let Some(n) = why.iter().position(|k| *k == key) { why.remove(n); } else { why.push(key); }
                        cx.notify();
                    }))))
            .when(open, |el| el.child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr(why)))));
        // "Remover"/"Desfazer" do `removivel` do web: só na Voz, e só com valor gravado pelo app.
        let removal = (field.page == Page::Voice).then(|| if removing {
            Some(div().id(SharedString::from(format!("server-{key}-removing"))).role(Role::Status).flex().items_center().gap(px(8.))
                .text_size(px(12.5)).text_color(theme::warning()).child(tr("server_remove_on_save"))
                .child(Button::new(SharedString::from(format!("server-{key}-undo"))).ghost().xsmall().label(tr("server_undo"))
                    .on_click(cx.listener(move |this, _, window, cx| this.undo_config(key, window, cx))))
                .into_any_element())
        } else if s.edited_in_app(key) {
            Some(div().flex().child(Button::new(SharedString::from(format!("server-{key}-remove"))).ghost().xsmall().label(tr("server_remove"))
                .text_color(theme::danger()).accessibility_label(tr("server_root_remove").replace("{p}", &tr(field.label)))
                .on_click(cx.listener(move |this, _, window, cx| this.remove_config(key, window, cx))))
                .into_any_element())
        } else { None }).flatten();
        // Com o campo embaixo a linha fica alta: o ícone sobe para junto do título em vez de flutuar no meio dela.
        let tall = below.is_some() || removal.is_some();
        let head = div().flex_1().min_w_0().flex().gap(px(14.)).map(|el| if tall { el.items_start() } else { el.items_center() })
            .child(icon_box(field.icon))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().flex().flex_wrap().items_center().gap(px(8.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(tr(field.label))).child(self.config_badges(key)))
                .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr(field.help)))
                .children(verdict).children(below).children(removal));
        let row = div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_center().gap(px(14.)).px_4().py(px(14.))
            .child(head).children(control.map(|c| div().flex_shrink_0().child(c)));
        self.mark(row, field.label)
    }

    /// Título de um bloco do Avançado, com as etiquetas ao lado, e a ajuda embaixo.
    fn block_head(&self, key: &'static str, help: &'static str, chips: Div) -> Div {
        div().mt(px(28.)).mb(px(10.)).flex().flex_col().gap(px(4.))
            .child(self.mark(div().flex().flex_wrap().items_center().gap(px(8.))
                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr(key))).child(chips), key))
            .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr(help)))
    }

    /// Pastas mapeadas (`scan_roots`): lista, remover, adicionar pelo campo ou pelo seletor de pasta do sistema.
    fn render_roots(&self, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        let line = || div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_center().px_4();
        let roots = s.roots();
        let rows = roots.iter().map(|path| {
            let owned = path.clone();
            line().gap(px(10.)).py(px(8.))
                .child(chrome::small_icon(IconName::Folder, 16., theme::muted()))
                .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(12.5)).child(path.clone()))
                .child(Button::new(SharedString::from(format!("server-root-remove-{path}"))).ghost().small().icon(IconName::Close)
                    .label(tr("server_remove")).accessibility_label(tr("server_root_remove").replace("{p}", path))
                    .on_click(cx.listener(move |this, _, _, cx| { this.server_config.remove_root(&owned); cx.notify(); })))
        }).collect::<Vec<_>>();
        let typed = s.root.as_ref().is_some_and(|i| !i.read(cx).value().trim().is_empty());
        let add = line().gap(px(8.)).py(px(10.))
            .children(s.root.as_ref().map(|i| div().flex_1().min_w_0().child(Input::new(i).small().aria_label(tr("server_root_new")))))
            .child(Button::new("server-root-add").outline().small().label(tr("server_root_add")).disabled(!typed)
                .on_click(cx.listener(|this, _, window, cx| this.add_typed_root(window, cx))))
            .child(Button::new("server-root-pick").outline().small().icon(IconName::FolderOpen).label(tr("server_root_pick"))
                .loading(s.picking).disabled(s.picking).on_click(cx.listener(|this, _, window, cx| this.pick_root(window, cx))));
        let chips = self.config_badges("scan_roots");
        div().child(self.block_head("server_roots", "server_roots_help", chips))
            .child(settings_box()
                .when(roots.is_empty(), |el| el.child(line().py(px(12.)).text_sm().text_color(theme::muted()).whitespace_normal().child(tr("server_roots_empty"))))
                .children(rows).child(add))
            .when_some(s.pick_error.clone(), |el, error| el.child(div().mt_2().text_sm().text_color(theme::danger()).child(error)))
    }

    /// O que só a máquina decide: o app lê e não grava. Sem etiqueta de escopo, como no web.
    fn render_read(&self) -> Option<Div> {
        let rows = self.server_config.read.iter().filter(|(k, _)| !READ_IN_MACHINES.contains(&k.as_str())).map(|(k, v)| {
            let label = if READ_LABELS.contains(&k.as_str()) { tr(&format!("server_read_{k}")) } else { k.clone() };
            div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_start().justify_between().gap(px(16.)).px_4().py(px(10.))
                .child(div().flex_1().min_w_0().text_sm().whitespace_normal().child(label))
                .child(div().flex_shrink_0().max_w(px(280.)).text_right().whitespace_normal().font_family(theme::MONO).text_size(px(12.5))
                    .text_color(theme::muted()).child(shown(v)))
        }).collect::<Vec<_>>();
        (!rows.is_empty()).then(|| div().child(self.block_head("server_machine_only", "server_machine_only_help", div())).child(settings_box().children(rows)))
    }

    /// Variáveis do `.env`: o servidor manda códigos e a tela traduz pelas mensagens do web; código que ela não conhece
    /// não vira texto nenhum, e a linha fica só com o nome.
    fn render_env(&self) -> Option<Div> {
        let env = &self.server_config.env;
        if env.is_empty() { return None; }
        let none = HashMap::new();
        let rows = env.iter().map(|var| {
            let code = |k: &str| var[k].as_str().unwrap_or_default().to_owned();
            let description = Some(code("descricao")).filter(|c| ENV_CODES.contains(&c.as_str()))
                .and_then(|c| crate::i18n::tr_web(&format!("config_server_env_{c}"), &none));
            let alert = ENV_ALERTS.iter().find(|(c, _)| *c == code("alerta")).and_then(|(_, key)| crate::i18n::tr_web(key, &none));
            let (set, name) = (var["definida"] == Value::Bool(true), code("nome"));
            // Lista de definição, como o `<dl>` do web: o leitor de tela liga nome, valor, descrição e alerta.
            let id = |part: &str| SharedString::from(format!("server-env-{name}-{part}"));
            div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().flex_col().gap(px(4.)).px_4().py(px(10.))
                .child(div().flex().items_start().justify_between().gap(px(16.))
                    .child(div().id(id("term")).role(Role::Term).when_some(alert.clone(), |el, a| el.aria_description(a))
                        .flex_shrink_0().font_family(theme::MONO).text_size(px(12.5)).child(name.clone()))
                    .child(div().id(id("value")).role(Role::Definition).flex_1().min_w_0().text_right().whitespace_normal()
                        .font_family(theme::MONO).text_size(px(12.5)).text_color(theme::muted()).when(!set, |el| el.italic()).child(env_value(var))))
                .children(description.map(|d| div().id(id("desc")).role(Role::Definition).text_size(px(12.5)).text_color(theme::muted())
                    .whitespace_normal().child(d)))
                .children(alert.map(|a| div().id(id("alert")).role(Role::Note).text_size(px(12.5)).text_color(theme::warning())
                    .whitespace_normal().child(a)))
        }).collect::<Vec<_>>();
        let chips = chip(tr("server_env_scope"), theme::muted(), theme::raised());
        Some(div().child(self.block_head("server_env", "server_env_help", div().child(chips)))
            .child(settings_box().id("server-env-list").role(Role::DescriptionList).aria_label(tr("server_env")).children(rows)))
    }

    /// Voz (`VozSettings.svelte`), na ordem do caminho do áudio: transcrever, o que vem depois, organizar o texto.
    fn render_voice(&self, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        let rows = |keys: &[&str], cx: &mut Context<Self>| settings_box().children(keys.iter().map(|k| self.config_row(field(k), cx)).collect::<Vec<_>>());
        let transcribe = s.transcribe_status();
        div()
            .child(self.voice_head("voice_transcribe", "voice_transcribe_help", transcribe))
            .when(transcribe.is_none(), |el| el.child(div().mb(px(10.)).text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr("voice_transcribe_no_key"))))
            .child(rows(&["groq_api_key"], cx))
            .child(div().mt(px(8.)).flex().flex_wrap().items_center().gap(px(12.))
                .child(Button::new("voice-create-key").link().small().icon(IconName::ExternalLink).label(tr("voice_create_key"))
                    .on_click(|_, _, cx| cx.open_url("https://console.groq.com/keys")))
                .child(self.section_toggle(0, "voice_transcribe_other", cx)))
            .when(s.open[0], |el| el.child(div().mt(px(8.)).child(rows(SECTIONS[0], cx))))
            .child(div().mt(px(28.)).mb(px(10.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr("voice_after")))
            .child(settings_box().child(self.hands_free_row()).child(self.style_row(cx)).child(self.config_row(field("ditado_vocabulario"), cx)))
            .child(self.voice_head("voice_cleanup", "voice_cleanup_help", s.cleanup_status()))
            .child(div().flex().child(self.section_toggle(1, "voice_cleanup_other", cx)))
            .when(s.open[1], |el| el.child(div().mt(px(8.)).child(rows(SECTIONS[1], cx)))
                .child(div().mt(px(8.)).flex().child(self.section_toggle(2, "voice_briefing_own", cx)))
                .when(s.open[2], |el| el.child(div().mt(px(8.)).child(rows(SECTIONS[2], cx)))))
    }

    /// Título de seção da Voz com o estado dela à direita; o texto diz o estado, a cor só reforça.
    fn voice_head(&self, title: &'static str, help: &'static str, status: Option<&'static str>) -> Div {
        let pill = match status {
            Some(key) => chip(tr(key), theme::success(), theme::success().alpha(0.14)),
            None => chip(tr("voice_status_off"), theme::muted(), theme::raised()),
        };
        div().mt(px(28.)).mb(px(10.)).flex().items_start().gap(px(16.))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(4.))
                .child(self.mark(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr(title)), title))
                .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr(help))))
            .child(div().flex_shrink_0().child(pill))
    }

    /// Abre e fecha uma seção de `SECTIONS`, como o `<details>` do web: o `AccordionTrigger` do kit anuncia aberto/fechado
    /// (o `Button` não expõe esse estado), e o foco próprio o põe na ordem do Tab com Enter e Espaço.
    fn section_toggle(&self, n: usize, label: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let open = self.server_config.open[n];
        let this = cx.entity().downgrade();
        AccordionTrigger::new(SharedString::from(format!("voice-section-{n}"))).open(open)
            .when_some(self.server_config.section_focus.get(n), |el, focus| el.track_focus(focus))
            .flex().items_center().gap(px(6.)).h(px(28.)).px(px(8.)).rounded(px(6.)).border_1().border_color(transparent_black())
            .text_sm().font_weight(FontWeight::MEDIUM).text_color(theme::text()).cursor_pointer()
            .hover(|el| el.bg(theme::hover())).focus_visible(|el| el.border_color(theme::accent_focus()))
            .child(chrome::small_icon(if open { IconName::ChevronUp } else { IconName::ChevronDown }, 14., theme::muted()))
            .child(tr(label))
            .on_change(move |open, _, _, cx| { let _ = this.update(cx, |this, cx| { this.server_config.open[n] = open; cx.notify(); }); })
            .into_any_element()
    }

    /// Mãos-livres é do aparelho (localStorage no web), e o app ainda não dita: aparece desligado, dizendo por quê.
    fn hands_free_row(&self) -> Div {
        let switch = div().id("voice-hands-free").child(Switch::new("voice-hands-free-switch").checked(false).disabled(true)
            .accessibility_label(tr("voice_hands_free")))
            .tooltip(|window, cx| Tooltip::new(tr("settings_next_version")).build(window, cx));
        // As etiquetas no lugar do "Este servidor" das vizinhas: onde vale e por que está desligado.
        let head = div().flex_1().min_w_0().flex().items_center().gap(px(14.))
            .child(icon_box(IconName::Mic))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().flex().flex_wrap().items_center().gap(px(8.))
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(tr("voice_hands_free")))
                    .child(chip(tr("settings_group_device"), theme::muted(), theme::raised()))
                    .child(chip(tr("settings_next_version").trim_end_matches('.').to_owned(), theme::muted(), theme::raised())))
                .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr("voice_hands_free_help"))));
        let row = div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_center().gap(px(14.)).px_4().py(px(14.))
            .child(head).child(div().flex_shrink_0().child(switch));
        self.mark(row, "voice_hands_free")
    }

    /// Estilo do ditado: grava pelo rascunho, no Salvar do rodapé (o web grava no clique). Cada opção explica a si mesma no
    /// tooltip e no nome acessível, como o `SegmentedPicker` do web.
    fn style_row(&self, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        let current = text_of(&s.current("ditado_estilo"));
        let selected = STYLES.iter().position(|v| *v == current).unwrap_or(usize::MAX);
        let labels = STYLES.map(|v| tr(&format!("voice_style_{v}"))).to_vec();
        let hints = STYLES.map(|v| tr(&format!("voice_style_{v}_hint"))).to_vec();
        let available = if s.fields.contains_key("ditado_estilo") { STYLES.len() } else { 0 };
        let control = segments_with_hints("voice-style", &labels, &hints, selected, available, false, tr("settings_next_version"),
            |this, n, _, cx| { this.server_config.stage("ditado_estilo", Value::String(STYLES[n].to_owned())); cx.notify(); }, cx);
        let head = div().flex_1().min_w_0().flex().items_start().gap(px(14.))
            .child(icon_box(IconName::Type))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().flex().flex_wrap().items_center().gap(px(8.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(tr("voice_style"))).child(chip(tr("server_scope"), theme::muted(), theme::raised())))
                .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr("voice_style_help")))
                .child(div().mt(px(8.)).flex().child(control)));
        self.mark(div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().px_4().py(px(14.)).child(head), "voice_style")
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
        // O erro do Salvar mora aqui, junto do botão, e não no fim da página: no Avançado ele ficava abaixo do `.env`, fora
        // da vista de quem clicou Salvar no topo. O web o põe no fim da coluna, que lá é curta.
        let column = div().w(px(720.)).max_w_full().px_4().flex().items_center().justify_end().gap(px(12.))
            .when_some(s.save_error.clone(), |el, error| el.child(div().id("server-config-error").flex_1().min_w_0().truncate().text_right()
                .text_size(px(12.5)).text_color(theme::danger()).child(error.clone())
                .tooltip(move |window, cx| Tooltip::new(error.clone()).build(window, cx))))
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
    use super::{Kind, Quiet, ServerConfig, env_value, shown, text_of};
    use serde_json::{Value, json};

    #[test]
    fn secret_field_never_shows_what_the_server_holds() {
        let mut s = ServerConfig::default();
        s.fields.insert("jev_api_key".into(), json!({"valor": "sk-s••••••••abcd", "definido": true, "origem": "app"}));
        assert_eq!(s.input_text("jev_api_key", Kind::Secret), "", "a máscara nunca entra no campo");
        assert_eq!(s.secret_mask("jev_api_key").as_deref(), Some("sk-s••••••••abcd"));
        assert!(s.stage("jev_api_key", json!("digitada")));
        assert_eq!(s.input_text("jev_api_key", Kind::Secret), "digitada");
        s.fields.insert("jev_texto_api_key".into(), json!({"valor": "", "definido": false, "origem": "env"}));
        assert_eq!(s.secret_mask("jev_texto_api_key"), None);
    }

    #[test]
    fn roots_add_and_remove_like_the_web() {
        let mut s = ServerConfig::default();
        s.fields.insert("scan_roots".into(), json!({"valor": "/a, /b", "origem": "env"}));
        assert_eq!(s.roots(), ["/a", "/b"]);
        assert!(!s.add_root("  "), "vazio não adiciona");
        assert!(!s.add_root("/b"), "repetido não adiciona");
        assert!(s.draft.is_empty(), "e nada vai ao rascunho");
        assert!(s.add_root(" /c "));
        assert_eq!(s.draft.get("scan_roots"), Some(&json!("/a,/b,/c")));
        s.remove_root("/a");
        assert_eq!(s.roots(), ["/b", "/c"]);
        s.remove_root("/b");
        s.remove_root("/c");
        assert_eq!(s.draft.get("scan_roots"), Some(&json!("")), "lista vazia grava vazio, não some do rascunho");
    }

    #[test]
    fn the_draft_only_takes_what_a_read_brought() {
        let mut s = ServerConfig::default();
        assert!(!s.stage("notify_dead", json!(false)), "nunca lido");
        assert!(!s.add_root("/x"), "sem leitura a lista vazia não vira a do servidor");
        s.remove_root("/x");
        assert!(s.draft.is_empty());
        s.fields.insert("scan_roots".into(), json!({"valor": "/a"}));
        assert!(s.add_root("/x"));
        assert_eq!(s.draft.get("scan_roots"), Some(&json!("/a,/x")));
    }

    #[test]
    fn read_only_values_never_show_a_secret() {
        assert_eq!(env_value(&json!({"valor": null, "definida": true, "segredo": true})), crate::i18n::tr("server_env_set"));
        assert_eq!(env_value(&json!({"valor": "vazou?", "definida": false, "segredo": true})), crate::i18n::tr("server_env_unset"));
        assert_eq!(env_value(&json!({"valor": "", "definida": false, "segredo": false})), "—");
        assert_eq!(env_value(&json!({"valor": 8765, "definida": true, "segredo": false})), "8765");
        assert_eq!(shown(&json!(false)), crate::i18n::tr("server_no"));
    }

    #[test]
    fn reconnecting_keeps_the_draft_only_for_the_same_owner() {
        let mut s = ServerConfig::default();
        s.reconnected("http://a/\nt".into());
        s.fields.insert("notify_dead".into(), json!({"valor": true}));
        assert!(s.stage("notify_dead", json!(false)));
        (s.quiet.confirmed, s.saving) = (true, true);
        s.reconnected("http://a/\nt".into());
        assert!(s.draft.contains_key("notify_dead") && s.quiet.confirmed && !s.saving, "Reconectar ao mesmo: fica, e o voo antigo morre");
        s.reconnected("http://b/\nt".into());
        assert!(s.draft.is_empty() && !s.quiet.confirmed, "outro servidor: nada passa");
        s.fields.insert("notify_dead".into(), json!({"valor": true}));
        assert!(s.stage("notify_dead", json!(false)));
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
        s.fields.insert("stall_seconds".into(), json!({"valor": 900}));
        s.fields.insert("notify_dead".into(), json!({"valor": true}));
        s.stage("stall_seconds", json!("600"));
        s.stage("notify_dead", json!(false));
        let sent = s.draft.snapshot();
        // Durante o salvar a pessoa muda um dos campos de novo.
        s.stage("stall_seconds", json!("700"));
        s.settle(&sent);
        assert_eq!(s.draft.get("stall_seconds"), Some(&json!("700")));
        assert!(!s.draft.contains_key("notify_dead"));
    }

    #[test]
    fn remove_goes_as_null_and_undo_brings_the_server_value_back() {
        let mut s = ServerConfig::default();
        assert!(!s.stage("groq_api_key", Value::Null), "sem leitura não há o que remover");
        s.fields.insert("groq_api_key".into(), json!({"valor": "sint••••••••wxyz", "definido": true, "origem": "app"}));
        assert!(s.transcribe_status().is_some());
        assert!(s.stage("groq_api_key", Value::Null));
        assert!(s.removing("groq_api_key") && s.draft.snapshot()["groq_api_key"].is_null(), "o Salvar manda null");
        assert_eq!(s.transcribe_status(), None, "a seção já diz desativada antes do Salvar, como o web");
        s.draft.unstage("groq_api_key");
        assert!(!s.removing("groq_api_key") && s.draft.is_empty());
        assert_eq!(s.transcribe_status(), Some("voice_status_on"));
    }

    #[test]
    fn voice_statuses_follow_the_web() {
        let mut s = ServerConfig::default();
        for key in ["groq_api_key", "llm_api_key"] { s.fields.insert(key.into(), json!({"valor": "", "definido": false})); }
        for key in ["transcription_base_url", "llm_base_url"] { s.fields.insert(key.into(), json!({"valor": ""})); }
        assert_eq!((s.transcribe_status(), s.cleanup_status()), (None, None));
        s.fields.insert("groq_api_key".into(), json!({"valor": "sint••••••••wxyz", "definido": true}));
        assert_eq!((s.transcribe_status(), s.cleanup_status()), (Some("voice_status_on"), Some("voice_status_default")));
        s.stage("transcription_base_url", json!("http://x/v1"));
        assert_eq!((s.transcribe_status(), s.cleanup_status()), (Some("voice_status_custom"), None), "o padrão só reusa a chave do padrão");
        s.stage("llm_base_url", json!("http://y/v1"));
        assert_eq!(s.cleanup_status(), None, "endpoint próprio sem chave própria");
        s.fields.insert("llm_api_key".into(), json!({"valor": "sint••••••••abcd", "definido": true}));
        assert_eq!(s.cleanup_status(), Some("voice_status_custom"));
    }

    #[test]
    fn sections_open_once_from_the_read_and_for_the_search() {
        let mut s = ServerConfig::default();
        s.fields.insert("llm_briefing_model".into(), json!({"valor": "modelo-sintetico"}));
        s.open_filled();
        assert_eq!(s.open, [false, true, true], "o briefing abre junto com a seção que o contém");
        s.open = [false; 3];
        s.open_filled();
        assert_eq!(s.open, [false; 3], "leitura seguinte não reabre o que foi fechado");
        s.reveal("voice_transcription_model");
        assert_eq!(s.open, [true, false, false]);
        s.reveal("voice_style");
        assert_eq!(s.open, [true, false, false], "linha fora de seção não mexe em nada");
    }

    #[test]
    fn current_prefers_the_draft_and_reads_the_origin() {
        let mut s = ServerConfig::default();
        s.fields.insert("stall_seconds".into(), json!({"valor": 900, "origem": "app"}));
        s.fields.insert("notify_dead".into(), json!({"valor": true, "origem": "env"}));
        assert_eq!(text_of(&s.current("stall_seconds")), "900");
        s.stage("stall_seconds", json!("30"));
        assert_eq!(text_of(&s.current("stall_seconds")), "30");
        assert!(s.edited_in_app("stall_seconds") && !s.edited_in_app("notify_dead"));
        assert_eq!(s.current("missing"), Value::Null);
    }
}
