//! Configuração do servidor pelo app: as páginas Notificações, Anexos e Avançado editam UM rascunho, e o
//! Salvar do rodapé grava tudo o que foi mexido nelas num só `POST /api/config`. Porta de `serverConfig.svelte.ts`,
//! `ServerSettings.svelte` e `LinhaConfig.svelte`. As horas silenciosas têm leitura e Salvar próprios (`PushQuiet.svelte`),
//! fora do rascunho: são gravadas no servidor e silenciam o push que chega no celular. A Voz usa o mesmo rascunho.
use super::*;
use super::device::Remote;
use super::settings::{segments_with_hints, settings_box, Disclosure, Page};
use gpui_kit::component::{select::{Select, SelectEvent, SelectState}, searchable_list::SearchableListItem, slider::{Slider, SliderEvent, SliderState},
    switch::Switch, tooltip::Tooltip};
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

/// Uma voz da conta ElevenLabs (`/api/tts/voices`); o id vazio é o "Padrão da máquina".
#[derive(Clone)]
struct Voice { id: String, name: String }

impl SearchableListItem for Voice {
    type Value = String;
    fn title(&self) -> SharedString { if self.id.is_empty() { tr("voice_machine_default").into() } else { self.name.clone().into() } }
    fn value(&self) -> &String { &self.id }
}

/// Estilos do ditado, na ordem do web (`estilosDitado`).
const STYLES: [&str; 3] = ["limpar", "prosa", "briefing"];
/// Campos de cada seção que abre e fecha na Voz: outro serviço de transcrição, outro para organizar, o do briefing, a
/// ElevenLabs, os ajustes da voz (dentro da ElevenLabs) e o leitor instalado.
const SECTIONS: [&[&str]; 6] = [&["transcription_base_url", "transcription_model"],
    &["llm_base_url", "llm_api_key", "llm_model", "llm_reasoning_effort"], &["llm_briefing_base_url", "llm_briefing_api_key", "llm_briefing_model"],
    &["elevenlabs_api_key", "elevenlabs_voice_id"], &["tts_stability", "tts_similarity_boost", "tts_style", "tts_speed"], &["tts_local_cmd"]];
/// Seção que contém outra: abrir a de dentro abre o caminho até ela.
const PARENTS: [(usize, usize); 2] = [(2, 1), (4, 3)];

/// Lista ordenada de serviços de transcrição; vazia, vale o serviço único das chaves de cima.
const PROVIDERS: &str = "transcription_providers";
/// Campos de texto de um serviço, na ordem dos campos em `provider_inputs`.
const PROVIDER_FIELDS: [&str; 7] = ["base_url", "api_key", "model", "executable_path", "model_path", "language", "converter_path"];

/// Espera de cota de um serviço, como `/api/transcription/providers/status` manda.
#[derive(Clone, Debug, PartialEq)]
struct ProviderStatus { id: String, name: String, waiting_until: Option<f64>, reason: Option<String>, state: Option<String>, error: Option<String> }

fn parse_provider_status(value: &Value) -> Result<Vec<ProviderStatus>, String> {
    let list = value.get("providers").and_then(Value::as_array).ok_or_else(|| tr("invalid_response"))?;
    Ok(list.iter().filter_map(|p| Some(ProviderStatus {
        id: p.get("id")?.as_str()?.to_owned(),
        name: p.get("name").and_then(Value::as_str).unwrap_or("").to_owned(),
        waiting_until: p.get("waiting_until").and_then(Value::as_f64),
        reason: p.get("reason").and_then(Value::as_str).filter(|r| !r.is_empty()).map(str::to_owned),
        state: p.get("state").and_then(Value::as_str).map(str::to_owned),
        error: p.get("error").and_then(Value::as_str).map(str::to_owned),
    })).collect())
}

fn provider_text<'a>(item: &'a Value, field: &str) -> &'a str { item.get(field).and_then(Value::as_str).unwrap_or("") }
/// O servidor só troca a máscara pela chave guardada com o mesmo tipo e endpoint do item salvo.
fn provider_keeps_key(item: &Value, saved: &Value) -> bool {
    let kind = provider_text(item, "kind");
    kind == provider_text(saved, "kind")
        && (kind == "elevenlabs" || provider_text(item, "base_url").trim() == provider_text(saved, "base_url"))
}

/// Um ajuste da voz (`AJUSTES_VOZ` do web): chave, nome das mensagens, padrão, mínimo e máximo.
struct Tune { key: &'static str, name: &'static str, default: i64, min: i64, max: i64 }

const TUNES: [Tune; 4] = [
    Tune { key: "tts_stability", name: "stability", default: 50, min: 0, max: 100 },
    Tune { key: "tts_similarity_boost", name: "similarity", default: 75, min: 0, max: 100 },
    Tune { key: "tts_style", name: "style", default: 0, min: 0, max: 100 },
    Tune { key: "tts_speed", name: "speed", default: 100, min: 70, max: 120 },
];

struct Field { key: &'static str, label: &'static str, help: &'static str, icon: IconName, kind: Kind, page: Page }

/// Na ordem do `CAMPOS` do web, filtrada por página.
const FIELDS: [Field; 33] = [
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
    Field { key: "jev_api_key", label: "server_jev_key", help: "server_jev_key_help", icon: IconName::Key, kind: Kind::Secret, page: Page::Jev },
    // Logo abaixo da chave, como no web: é o único campo cujo efeito depende dela.
    Field { key: "jev_padrao", label: "server_jev_default", help: "server_jev_default_help", icon: IconName::Rocket, kind: Kind::Toggle, page: Page::Jev },
    Field { key: "jev_endpoint", label: "server_jev_endpoint", help: "server_jev_endpoint_help", icon: IconName::Globe, kind: Kind::Text,
        page: Page::Jev },
    Field { key: "jev_model", label: "server_jev_model", help: "server_jev_model_help", icon: IconName::Bot, kind: Kind::Text, page: Page::Jev },
    Field { key: "jev_texto_base_url", label: "server_jev_text_endpoint", help: "server_jev_text_endpoint_help", icon: IconName::Globe,
        kind: Kind::Text, page: Page::Jev },
    Field { key: "jev_texto_api_key", label: "server_jev_text_key", help: "server_jev_text_key_help", icon: IconName::Key, kind: Kind::Secret,
        page: Page::Jev },
    Field { key: "jev_texto_modelo", label: "server_jev_text_model", help: "server_jev_text_model_help", icon: IconName::Bot, kind: Kind::Text,
        page: Page::Jev },
    Field { key: "jev_texto_cmd", label: "server_jev_cmd", help: "server_jev_cmd_help", icon: IconName::SquareTerminal, kind: Kind::Text,
        page: Page::Jev },
    // Voz, na ordem do `VozSettings.svelte`; a página as distribui pelas seções dela.
    Field { key: "groq_api_key", label: "voice_groq", help: "voice_groq_help", icon: IconName::Key, kind: Kind::Secret, page: Page::Voice },
    Field { key: "transcription_base_url", label: "voice_transcription_endpoint", help: "voice_transcription_endpoint_help", icon: IconName::Globe,
        kind: Kind::Text, page: Page::Voice },
    Field { key: "transcription_model", label: "voice_transcription_model", help: "voice_transcription_model_help", icon: IconName::Bot,
        kind: Kind::Text, page: Page::Voice },
    Field { key: "ditado_vocabulario", label: "voice_vocabulary", help: "voice_vocabulary_help", icon: IconName::BookOpen, kind: Kind::Text,
        page: Page::Voice },
    Field { key: "llm_base_url", label: "voice_llm_endpoint", help: "voice_llm_endpoint_help", icon: IconName::Globe, kind: Kind::Text, page: Page::Voice },
    Field { key: "dictation_include_recent_messages", label: "voice_recent_messages", help: "voice_recent_messages_help", icon: IconName::MessageSquare, kind: Kind::Toggle, page: Page::Voice },
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
    Field { key: "elevenlabs_api_key", label: "voice_elevenlabs_key", help: "voice_elevenlabs_key_help", icon: IconName::Key, kind: Kind::Secret,
        page: Page::Voice },
    Field { key: "tts_local_cmd", label: "voice_local_cmd", help: "voice_local_cmd_help", icon: IconName::SquareTerminal, kind: Kind::Text,
        page: Page::Voice },
    Field { key: "tts_max_chars", label: "voice_max_chars", help: "voice_max_chars_help", icon: IconName::Hash, kind: Kind::Number("voice_chars"),
        page: Page::Voice },
    // No Avançado do detalhe desta máquina, em Máquinas (`MaquinasSettings.svelte`, `CAMPO_TERM_ORIGINS`).
    Field { key: "term_origins", label: "server_term_origins", help: "server_term_origins_help", icon: IconName::SquareTerminal, kind: Kind::Text,
        page: Page::Servers },
];

fn field(key: &str) -> &'static Field { FIELDS.iter().find(|f| f.key == key).expect("campo declarado em FIELDS") }

/// Veredito com o "por quê?" que expande, como no web: Automações e o raciocínio da organização.
const VERDICTS: [(&str, &str, &str); 3] = [("automations", "server_recommended_on", "server_automations_why"),
    ("llm_reasoning_effort", "voice_llm_effort_verdict", "voice_llm_effort_why"),
    ("tts_local_cmd", "voice_local_cmd_verdict", "voice_local_cmd_why")];

/// Do bloco só leitura, o que Máquinas já mostra não se repete aqui.
const READ_IN_MACHINES: [&str; 5] = ["port", "lan_bind_ip", "server_id", "public_url", "terminal_origem_ok"];
const READ_LABELS: [&str; 3] = ["terminal_panel", "traducao_pensamento", "versao"];
/// Códigos de descrição do `.env` que o web traduz (`DESCRICAO_ENV`); código fora daqui não vira texto nenhum.
const ENV_CODES: [&str; 31] = ["terminal", "pricing_offline", "claude_config_dirs", "engines_file", "codex_sync_enabled", "auto_resume",
    "omp_plugin_sync", "omp_claude_context", "lan_bind_ip", "port", "auth_token", "projects_dir", "reload", "diag_term_input", "front_port",
    "public_url", "server_id", "vapid_public", "vapid_private", "vapid_subject", "stall_poll_seconds", "omp_plugin_sync_interval", "sync",
    "sync_bootstrap", "sync_data", "sync_session_secret", "sync_rate_max", "sync_rate_window", "forwarded_allow_ips", "deploy_secret", "update_branch"];
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
pub(super) fn is_server_page(page: Page) -> bool { matches!(page, Page::Voice | Page::Jev | Page::Notifications | Page::Attachments | Page::Advanced) }

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
    open: [bool; 6],
    opened_by_read: bool,
    jev_advanced: bool,
    /// Havia chave da ElevenLabs e comando local no último olhar: a seção de cada um abre quando isso passa a valer.
    readers: [bool; 2],
    /// Vozes da conta e consumo do mês: só lidos no clique, nunca ao abrir a página.
    voices: Remote<Vec<Voice>>,
    usage: Remote<[Option<i64>; 2]>,
    voice_picker: Option<(Entity<SelectState<Vec<Voice>>>, Subscription)>,
    /// Um slider por ajuste, com o foco do invólucro que dá teclado e nome a ele.
    tunes: Vec<(&'static str, Entity<SliderState>, FocusHandle)>,
    quiet: Quiet,
    /// Campos de cada serviço da lista, pela identidade dele: subir e descer não troca o que está digitado.
    provider_inputs: Vec<(String, [Entity<InputState>; 7], Vec<Subscription>)>,
    /// Espera de cota de cada serviço, relida ao abrir a Voz.
    provider_status: Remote<Vec<ProviderStatus>>,
    dictation_catalog: Remote<Value>,
    dictation_catalog_target: Option<String>,
    provider_tests: std::collections::HashMap<String, Remote<String>>,
    /// Serviços da última confirmação do servidor. Sobrevive à limpeza de `fields` ao reler: é com ela que um teste é comparado.
    confirmed_providers: Vec<Value>,
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
    Voices(u64, Result<Value, Failure>),
    Usage(u64, Result<Value, Failure>),
    ProviderStatus(u64, Result<Value, Failure>),
    ProviderTest(String, u64, Option<Result<Value, Failure>>),
    DictationModels(u64, String, Result<Value, Failure>),
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
        (self.voices, self.usage, self.voice_picker) = (Remote::default(), Remote::default(), None);
        self.provider_status = Remote::default();
        self.dictation_catalog.reset();
        self.dictation_catalog_target = None;
        self.provider_tests.clear();
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

    /// Começo da chave do Jev: a digitada agora, senão a máscara guardada (`sk-o••••` já diz o provedor).
    fn jev_key_head(&self) -> Option<String> {
        if self.removing("jev_api_key") { return None; }
        let typed = self.draft.get("jev_api_key").map(text_of).filter(|t| !t.trim().is_empty());
        let head = typed.or_else(|| self.secret_mask("jev_api_key"))?;
        Some(if head.starts_with("sk-o") { "sk-or-".to_owned() } else { head })
    }

    /// Provedor, endereço e modelo efetivos do Jev pela regra do servidor; `None` sem chave.
    fn jev_route(&self) -> Option<(bool, String, String)> {
        let key = self.jev_key_head()?;
        let setting = |name: &str| Some(text_of(&self.current(name))).filter(|v| !v.trim().is_empty());
        let endpoint = setting("jev_endpoint");
        let openrouter = crate::voice::jev::is_openrouter(&key, endpoint.as_deref());
        let (url, model) = crate::voice::jev::destination(&key, endpoint.as_deref(), setting("jev_model").as_deref());
        Some((openrouter, url.unwrap_or_default(), model.unwrap_or_default()))
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
    fn stage(&mut self, key: &str, value: Value) -> bool {
        let staged = self.draft.stage(&self.fields, key, value);
        self.follow_readers();
        staged
    }

    /// Como os `$effect` do web: a seção da ElevenLabs abre quando passa a haver chave, e a do leitor quando o comando passa a
    /// ter valor; fechada à mão, fica fechada até isso mudar de novo.
    fn follow_readers(&mut self) {
        let now = [self.key_set("elevenlabs_api_key"), self.filled("tts_local_cmd")];
        for (n, (on, was)) in [3, 5].into_iter().zip(now.into_iter().zip(self.readers)) {
            if on && !was { self.open[n] = true; }
        }
        self.readers = now;
    }

    /// Dá para ler em voz alta (`podeLerCriterio` do web), contra o rascunho: chave que fica ou comando local.
    fn can_read(&self) -> bool { self.key_set("elevenlabs_api_key") || self.filled("tts_local_cmd") }

    /// Estado da leitura, `None` desativada; a ElevenLabs vence o comando local, como no web.
    fn read_status(&self) -> Option<&'static str> {
        if self.key_set("elevenlabs_api_key") { Some("voice_status_elevenlabs") } else if self.filled("tts_local_cmd") { Some("voice_status_local") } else { None }
    }

    /// Valor de um ajuste: o do rascunho ou do servidor; ausente, removido ou fora de número, o padrão (`ajusteValor` do web).
    fn tune_value(&self, tune: &Tune) -> i64 {
        let value = self.current(tune.key);
        value.as_i64().or_else(|| value.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(tune.default)
    }

    /// Nome da voz escolhida: o da lista carregada, senão o id, senão o padrão da máquina.
    fn voice_name(&self) -> String {
        let id = text_of(&self.current("elevenlabs_voice_id")).trim().to_owned();
        if id.is_empty() { return tr("voice_machine_default"); }
        self.voices.ok().and_then(|v| v.iter().find(|v| v.id == id)).map_or(id, |v| v.name.clone())
    }

    /// "Remover" do web: `null` no rascunho apaga o override no Salvar, e o campo volta ao valor do ambiente.
    fn removing(&self, key: &str) -> bool { self.draft.get(key) == Some(&Value::Null) }

    fn filled(&self, key: &str) -> bool { !text_of(&self.current(key)).trim().is_empty() }

    /// Chave guardada no servidor e que não sai no próximo Salvar.
    fn key_set(&self, key: &str) -> bool { self.secret_mask(key).is_some() && !self.removing(key) }

    /// Estado do serviço único (chave e endpoint de cima), contra o rascunho.
    fn legacy_status(&self) -> Option<&'static str> {
        self.key_set("groq_api_key").then(|| if self.filled("transcription_base_url") { "voice_status_custom" } else { "voice_status_on" })
    }

    /// Estado da transcrição, `None` desativada. Com a lista, quem transcreve é ela.
    fn transcribe_status(&self) -> Option<&'static str> {
        if self.providers().is_empty() { self.legacy_status() } else { Some("voice_status_custom") }
    }

    /// Estado da organização: com endpoint próprio vale a chave dele; sem, o padrão reusa a chave do serviço único, que a
    /// lista de transcrição não muda.
    fn cleanup_status(&self) -> Option<&'static str> {
        match text_of(&self.current("dictation_organization_mode")).as_str() {
            "harness"=>Some("voice_organization_harness"),
            "external_api"=>self.key_set("llm_api_key").then_some("voice_status_custom"),
            _=>Some("voice_organization_none"),
        }
    }

    fn organization_mode(&self)->&'static str {
        match self.current("dictation_organization_mode").as_str(){Some("harness")=>"harness",Some("external_api")=>"external_api",_=>"none"}
    }

    /// Lista de serviços de transcrição (rascunho ou servidor).
    fn providers(&self) -> Vec<Value> { self.current(PROVIDERS).as_array().cloned().unwrap_or_default() }

    /// O serviço como a última leitura trouxe (chave mascarada).
    fn provider_saved(&self, id: &str) -> Option<&Value> {
        self.fields.get(PROVIDERS)?.get("valor")?.as_array()?.iter().find(|p| provider_text(p, "id") == id)
    }

    /// Campos que o servidor confirmou. O resultado de um teste vale só para a configuração testada: some quando a
    /// leitura traz outra, ou quando um Salvar confirmou serviços (a máscara não distingue duas chaves).
    fn replace_fields(&mut self, fields: Map<String, Value>, providers_saved: bool) {
        self.fields = fields;
        let confirmed = std::mem::take(&mut self.confirmed_providers);
        for (id, test) in &mut self.provider_tests {
            let old = confirmed.iter().find(|p| provider_text(p, "id") == id);
            let new = self.fields.get(PROVIDERS).and_then(|f| f.get("valor")).and_then(Value::as_array)
                .and_then(|list| list.iter().find(|p| provider_text(p, "id") == id));
            if providers_saved || old != new { test.reset(); }
        }
        self.confirmed_providers = self.fields.get(PROVIDERS).and_then(|f| f.get("valor")).and_then(Value::as_array).cloned().unwrap_or_default();
    }

    /// Máscara da chave guardada de um serviço, como a última leitura trouxe.
    fn provider_mask(&self, id: &str) -> Option<String> {
        self.provider_saved(id).map(|p| provider_text(p, "api_key").to_owned()).filter(|mask| !mask.is_empty())
    }

    /// Novo serviço no fim da lista; a identidade nasce aqui e não muda mais.
    fn add_provider(&mut self, id: String) -> bool {
        let mut list = self.providers();
        list.push(json!({"id": id, "kind": "openai", "name": "", "base_url": "", "api_key": "", "model": ""}));
        self.stage(PROVIDERS, Value::Array(list))
    }

    fn remove_provider(&mut self, id: &str) -> bool {
        let list = self.providers().into_iter().filter(|p| provider_text(p, "id") != id).collect();
        self.stage(PROVIDERS, Value::Array(list))
    }

    /// Troca de lugar com o vizinho de cima (`up`) ou de baixo; na ponta, nada muda.
    fn move_provider(&mut self, id: &str, up: bool) -> bool {
        let mut list = self.providers();
        let Some(at) = list.iter().position(|p| provider_text(p, "id") == id) else { return false };
        let Some(other) = (if up { at.checked_sub(1) } else { Some(at + 1).filter(|n| *n < list.len()) }) else { return false };
        list.swap(at, other);
        self.stage(PROVIDERS, Value::Array(list))
    }

    /// Um campo de um serviço. Chave apagada volta à máscara guardada: o servidor lê a máscara como "mantém a chave".
    /// Ele só a aceita com o tipo e o endpoint salvos: trocados, a máscara sai e o "falta a chave" segura o Salvar em
    /// vez do 400; de volta ao salvo, ela volta (`editTranscriptionProviderTarget` do core).
    fn edit_provider(&mut self, id: &str, field: &str, text: String) -> bool {
        let saved = self.provider_saved(id).cloned();
        let mask = self.provider_mask(id);
        let mut list = self.providers();
        let Some(item) = list.iter_mut().find(|p| provider_text(p, "id") == id) else { return false };
        // Mesmo valor (tipo já escolhido, campo reposto pela leitura) não suja o rascunho.
        if field != "api_key" && provider_text(item, field) == text { return false; }
        let keeps = |item: &Value| saved.as_ref().is_some_and(|saved| provider_keeps_key(item, saved));
        if field == "api_key" {
            let value = if text.is_empty() && keeps(&*item) { mask.unwrap_or_default() } else { text };
            item[field] = Value::String(value);
        } else {
            item[field] = Value::String(text);
            if let Some(mask) = mask {
                let (key, kept) = (provider_text(item, "api_key").to_owned(), keeps(&*item));
                if !kept && key == mask { item["api_key"] = Value::String(String::new()); }
                else if kept && key.is_empty() { item["api_key"] = Value::String(mask); }
            }
        }
        self.stage(PROVIDERS, Value::Array(list))
    }

    /// Até quando o serviço espera a cota voltar, e por quê, se ainda espera.
    fn provider_waiting(&self, id: &str, now: f64) -> Option<(f64, Option<String>)> {
        let status = self.provider_status.ok()?.iter().find(|s| s.id == id)?;
        status.waiting_until.filter(|until| *until > now).map(|until| (until, status.reason.clone()))
    }

    /// Serviço da lista no rascunho sem chave: o servidor recusaria (400) o Salvar inteiro, de todas as páginas, porque
    /// o rascunho é um só. O Salvar espera a chave.
    fn provider_missing_key(&self) -> bool {
        self.draft.contains_key(PROVIDERS) && self.providers().iter().any(|p| provider_text(p, "kind") == "elevenlabs" && provider_text(p, "api_key").trim().is_empty())
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
        let key = FIELDS.iter().find(|f| f.label == label).map(|f| f.key)
            .or_else(|| TUNES.iter().find(|t| label == format!("voice_tune_{}", t.name)).map(|t| t.key))
            .or((label == "voice_voice").then_some("elevenlabs_voice_id"));
        let Some(key) = key else { return };
        if key.starts_with("jev_texto_") {
            self.jev_advanced = true;
        }
        if let Some(n) = SECTIONS.iter().position(|keys| keys.contains(&key)) {
            self.open[n] = true;
            for (child, parent) in PARENTS { if n == child { self.open[parent] = true; } }
        }
    }

    /// Resposta da gravação: sai do rascunho só a chave cujo valor ainda é o enviado — o que foi mexido durante o
    /// salvar continua lá e vai no próximo.
    fn settle(&mut self, sent: &Map<String, Value>) {
        self.draft.settle(sent);
    }
}

pub(super) fn chip(text: String, color: Hsla, bg: Hsla) -> Div {
    div().px(px(6.)).rounded_full().bg(bg).text_size(px(10.5)).font_weight(FontWeight::BOLD).text_color(color).child(text)
}

fn icon_box(icon: IconName) -> Div {
    div().size(px(36.)).flex_shrink_0().rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
        .flex().items_center().justify_center().child(chrome::small_icon(icon, 16., theme::muted()))
}

fn text_of(value: &Value) -> String {
    match value { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum JevModel { Latest, Fixed }

/// Versões fixas sugeridas; o vazio é o mais novo do provedor.
const JEV_FIXED: [&str; 2] = ["jev-1.13.0", "typesafe/jev-1.13-20260917"];

fn jev_model_value(which: JevModel, openrouter: bool) -> &'static str {
    match which { JevModel::Latest => "", JevModel::Fixed => JEV_FIXED[usize::from(openrouter)] }
}

/// Qual sugestão o modelo atual é; `None` para um modelo próprio. O apelido do mais novo também conta como ele.
fn jev_model_preset(current: &str, openrouter: bool) -> Option<JevModel> {
    let latest = if openrouter { crate::voice::jev::OPENROUTER_MODEL } else { crate::voice::jev::TYPESAFE_MODEL };
    match current.trim() {
        "" => Some(JevModel::Latest),
        v if v == latest || (openrouter && v == "typesafe/jev-latest") => Some(JevModel::Latest),
        v if v == JEV_FIXED[usize::from(openrouter)] => Some(JevModel::Fixed),
        _ => None,
    }
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
        if page == Page::Voice { self.load_provider_status(cx); }
    }

    fn load_server_config(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let s = &mut self.server_config;
        let seq = s.load.start();
        // Como o web: os campos da leitura anterior saem, para uma falha agora aparecer como falha e não como a lista velha.
        s.fields.clear();
        (s.read, s.env) = (Map::new(), Vec::new());
        (s.save_error, s.saved) = (None, None);
        // O web remonta a página a cada abertura, e os `$effect` reabrem as seções de quem já tem leitor.
        s.readers = [false; 2];
        let done = self.server_config_send_later();
        self.runtime.spawn(async move { done(ServerConfigReply::Loaded(seq, api.config().await)).await });
        cx.notify();
    }

    /// "Carregar vozes da conta": as vozes e o consumo do mês, juntos, como o `carregarVozes` do web.
    fn load_voices(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let s = &mut self.server_config;
        if s.voices.loading { return; }
        let (voices, usage) = (s.voices.start(), s.usage.start());
        let (done, done_usage) = (self.server_config_send_later(), self.server_config_send_later());
        let usage_api = api.clone();
        self.runtime.spawn(async move { done(ServerConfigReply::Voices(voices, api.server_read(&["tts", "voices"], &[], 15).await)).await });
        self.runtime.spawn(async move { done_usage(ServerConfigReply::Usage(usage, usage_api.server_read(&["tts", "saldo"], &[], 15).await)).await });
        cx.notify();
    }

    /// Quem está em espera de cota e até quando: só leitura, nada vai ao rascunho.
    fn load_provider_status(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.server_config.provider_status.start();
        let done = self.server_config_send_later();
        self.runtime.spawn(async move {
            done(ServerConfigReply::ProviderStatus(seq, api.server_read(&["transcription", "providers", "status"], &[], 15).await)).await
        });
        cx.notify();
    }

    /// Um campo por texto de cada serviço, criado quando o serviço aparece e solto quando ele sai. Sem a lista no
    /// rascunho, os campos voltam ao que o servidor tem; a chave mostra só o que for digitado aqui.
    fn sync_provider_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let providers = self.server_config.providers();
        self.server_config.provider_inputs.retain(|(id, ..)| providers.iter().any(|p| provider_text(p, "id") == id));
        for item in &providers {
            let id = provider_text(item, "id").to_owned();
            if id.is_empty() || self.server_config.provider_inputs.iter().any(|(known, ..)| *known == id) { continue; }
            let mut subscriptions = Vec::new();
            let inputs = PROVIDER_FIELDS.map(|field| {
                let secret = field == "api_key";
                let input = cx.new(|cx| InputState::new(window, cx).masked(secret));
                let value = if secret { String::new() } else { provider_text(item, field).to_owned() };
                input.update(cx, |state, cx| state.set_value(value, window, cx));
                let id = id.clone();
                subscriptions.push(cx.subscribe_in(&input, window, move |this: &mut Hangar, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.server_config.edit_provider(&id, field, input.read(cx).value().to_string());
                        cx.notify();
                    }
                }));
                input
            });
            self.server_config.provider_inputs.push((id, inputs, subscriptions));
        }
        let staged = self.server_config.draft.contains_key(PROVIDERS);
        let s = &self.server_config;
        for (id, inputs, _) in &s.provider_inputs {
            let [endpoint, key, model, _, _, language, converter] = inputs;
            let Some(item) = providers.iter().find(|p| provider_text(p, "id") == id) else { continue };
            let key_hint = tr(if s.provider_mask(id).is_some() { "server_secret_paste_new" } else { "server_secret_paste" });
            key.update(cx, |state, cx| state.set_placeholder(key_hint, window, cx));
            // Modelo e endpoint padrão do servidor: identificadores, não frase de tela.
            let model_hint = if provider_text(item, "kind") == "elevenlabs" { "scribe_v2" } else { "whisper-large-v3" };
            model.update(cx, |state, cx| state.set_placeholder(model_hint, window, cx));
            endpoint.update(cx, |state, cx| state.set_placeholder("https://api.groq.com/openai/v1", window, cx));
            language.update(cx, |state, cx| state.set_placeholder("pt", window, cx));
            converter.update(cx, |state, cx| state.set_placeholder("ffmpeg", window, cx));
            if staged { continue; }
            for (field, input) in PROVIDER_FIELDS.iter().zip(inputs) {
                let value = if *field == "api_key" { String::new() } else { provider_text(item, field).to_owned() };
                input.update(cx, |state, cx| if state.value() != value.as_str() { state.set_value(value, window, cx) });
            }
        }
    }

    fn add_transcription_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // ponytail: id pelo relógio em nanossegundos; dois cliques nunca caem no mesmo instante.
        let id = format!("p{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        if self.server_config.add_provider(id) { self.sync_provider_inputs(window, cx); }
        cx.notify();
    }

    fn remove_transcription_provider(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_config.remove_provider(id) { self.sync_provider_inputs(window, cx); }
        cx.notify();
    }

    fn move_transcription_provider(&mut self, id: &str, up: bool, cx: &mut Context<Self>) {
        self.server_config.move_provider(id, up);
        cx.notify();
    }

    fn set_provider_kind(&mut self, id: &str, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_config.edit_provider(id, "kind", kind.to_owned()) { self.sync_provider_inputs(window, cx); }
        cx.notify();
    }

    fn test_provider(&mut self, id: String, cx: &mut Context<Self>) {
        if self.server_config.draft.contains_key(PROVIDERS) || self.server_config.provider_tests.get(&id).is_some_and(|t| t.loading) { return; }
        let Some(api) = self.api.clone() else { return };
        let seq = self.server_config.provider_tests.entry(id.clone()).or_default().start();
        let prompt = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        let runtime = self.runtime.clone();
        let done = self.server_config_send_later();
        cx.spawn(async move |_, _| {
            let result = match prompt.await {
                Ok(Ok(Some(paths))) => if let Some(path) = paths.into_iter().next() {
                    let provider = id.clone();
                    Some(runtime.spawn(async move {
                        if tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0) > crate::api::MAX_BYTES {
                            return Err(Failure::local("attach_too_big"));
                        }
                        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("audio.wav").to_owned();
                        let bytes = tokio::fs::read(path).await.map_err(|_| Failure::local("picker_failed"))?;
                        api.test_transcription_provider(&provider, &filename, bytes).await
                    }).await.unwrap_or_else(|_| Err(Failure::local("invalid_response"))))
                } else { None },
                Ok(Ok(None)) => None,
                _ => Some(Err(Failure::local("picker_failed"))),
            };
            done(ServerConfigReply::ProviderTest(id, seq, result)).await;
        }).detach();
        cx.notify();
    }

    fn save_server_config(&mut self, cx: &mut Context<Self>) {
        let s = &mut self.server_config;
        if s.saving || s.draft.is_empty() || s.provider_missing_key() { return; }
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

    fn load_dictation_models(&mut self,cx:&mut Context<Self>) {
        if self.server_config.organization_mode()!="harness"||self.server_config.dictation_catalog.loading{return;}
        let Some(api)=self.session_api() else{return;};
        let Some(session)=self.selected.as_ref() else {
            let seq=self.server_config.dictation_catalog.start();
            self.server_config.dictation_catalog.finish(seq,Err(tr("voice_models_destination")));cx.notify();return;
        };
        let target=format!("{}|{}|{:?}|{:?}",api.identity(),session.name,session.lifecycle_id,session.conta);
        let name=session.name.clone();
        let seq=self.server_config.dictation_catalog.start();
        self.server_config.dictation_catalog_target=Some(target.clone());
        let done=self.server_config_send_later();
        self.runtime.spawn(async move{done(ServerConfigReply::DictationModels(seq,target,api.dictation_models(&name).await)).await;});
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
                    if let Value::Object(campos) = config["campos"].take() { s.replace_fields(campos, false); }
                    if let Value::Object(read) = config["somente_leitura"].take() { s.read = read; }
                    // Ausente num servidor mais antigo: o bloco some.
                    if let Value::Array(env) = config["variaveis_env"].take() { s.env = env; }
                    s.open_filled();
                    s.follow_readers();
                    self.fill_config_inputs(window, cx);
                    self.load_dictation_style(cx);
                    self.load_dictation_models(cx);
                }
            }
            ServerConfigReply::Saved(seq, sent, result) => {
                let s = &mut self.server_config;
                if seq != s.save_seq { return; }
                s.saving = false;
                // Sem `campos` não dá para confirmar o que valeu: é falha, e o rascunho fica. Um 5xx traz a frase do servidor, não a
                // da entrega de mensagem.
                let result = result.map_err(|e| Self::fetch_failure(&e)).and_then(|r| match r.get("campos") {
                    Some(Value::Object(campos)) => Ok((campos.clone(), r.get("somente_leitura").and_then(Value::as_object).cloned())),
                    _ => Err(tr("invalid_response")),
                });
                match result {
                    Ok((campos, read)) => {
                        s.replace_fields(campos, sent.contains_key(PROVIDERS));
                        // A tradução do raciocínio disponível muda com campo editável: a linha só leitura não fica velha.
                        if let Some(read) = read { s.read = read; }
                        s.settle(&sent);
                        s.follow_readers();
                        s.saved = Some(seq);
                        self.fill_config_inputs(window, cx);
                        self.load_dictation_style(cx);
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
                    Err(error) => (Self::fetch_failure(&error), true),
                });
            }
            ServerConfigReply::Voices(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).and_then(|r| match r.get("voices") {
                    Some(Value::Array(list)) => Ok(list.iter().filter_map(|v| {
                        let id = v.get("id")?.as_str()?.to_owned();
                        Some(Voice { name: v.get("nome").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(&id).to_owned(), id })
                    }).collect::<Vec<_>>()),
                    _ => Err(tr("invalid_response")),
                });
                let list = parsed.as_ref().ok().cloned();
                if !self.server_config.voices.finish(seq, parsed) { return; }
                // Lista vazia volta ao botão, como o web; a lista anterior não fica no lugar dela.
                self.server_config.voice_picker = list.filter(|l| !l.is_empty()).map(|list| {
                    let items = std::iter::once(Voice { id: String::new(), name: String::new() }).chain(list).collect::<Vec<_>>();
                    let picker = cx.new(|cx| SelectState::new(items, None, window, cx));
                    let sub = cx.subscribe_in(&picker, window, |this: &mut Hangar, _, event: &SelectEvent<Vec<Voice>>, window, cx| {
                        let SelectEvent::Confirm(Some(id)) = event else { return };
                        if !this.server_config.stage("elevenlabs_voice_id", Value::String(id.clone())) { this.show_voice(window, cx); }
                        cx.notify();
                    });
                    (picker, sub)
                });
                self.show_voice(window, cx);
            }
            ServerConfigReply::DictationModels(seq,target,result)=>{
                let current=self.selected.as_ref().zip(self.session_api()).map(|(session,api)|format!("{}|{}|{:?}|{:?}",api.identity(),session.name,session.lifecycle_id,session.conta));
                if current.as_deref()!=Some(target.as_str()){
                    self.server_config.dictation_catalog.finish(seq,Err(tr("voice_models_destination")));return;
                }
                let result=result.map_err(|error|Self::failure(&error)).and_then(|value|if value["models"].is_array(){Ok(value)}else{Err(tr("invalid_response"))});
                self.server_config.dictation_catalog.finish(seq,result);
            }
            ServerConfigReply::Usage(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).map(|r| ["usados", "limite"].map(|k| r.get(k).and_then(Value::as_i64)));
                self.server_config.usage.finish(seq, parsed);
            }
            ServerConfigReply::ProviderStatus(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).and_then(|r| parse_provider_status(&r));
                self.server_config.provider_status.finish(seq, parsed);
            }
            ServerConfigReply::ProviderTest(id, seq, result) => {
                if let Some(result) = result {
                    self.server_config.provider_tests.entry(id).or_default().finish(seq,
                        result.map_err(|error| Self::failure(&error)).and_then(|value|
                            value.get("text").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
                                .map(str::to_owned).ok_or_else(|| tr("invalid_response"))));
                } else if let Some(test) = self.server_config.provider_tests.get_mut(&id).filter(|t| t.seq == seq) {
                    // Seletor cancelado: volta ao vazio sem reusar o número, que um pedido anterior ainda pode trazer.
                    test.reset();
                }
                self.load_provider_status(cx);
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
                let sub = cx.subscribe_in(&input, window, move |this: &mut Hangar, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.server_config.stage(key, Value::String(input.read(cx).value().to_string()));
                        // Colar a chave escolhe o provedor: o endereço padrão do outro sai do caminho.
                        if key == "jev_api_key" {
                            if let Some((openrouter, ..)) = this.server_config.jev_route() { this.follow_jev_provider(openrouter, false, window, cx); }
                        }
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
            for tune in &TUNES {
                let key = tune.key;
                let state = cx.new(|_| SliderState::new().min(tune.min as f32).max(tune.max as f32).step(1.).default_value(tune.default as f32));
                // Como o `oninput` do web: cada passo do arrasto vai ao rascunho.
                let sub = cx.subscribe_in(&state, window, move |this: &mut Hangar, _, event: &SliderEvent, window, cx| {
                    let (SliderEvent::Change(v) | SliderEvent::Release(v)) = event;
                    // Recusado (campo fora da leitura): o slider volta, para não mostrar um valor que o Salvar não leva.
                    if !this.server_config.stage(key, json!(v.start().round() as i64)) { this.show_tune(key, window, cx); }
                    cx.notify();
                });
                self.server_config._subscriptions.push(sub);
                self.server_config.tunes.push((key, state, cx.focus_handle().tab_stop(true)));
            }
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
        for (key, _, _) in s.tunes.clone() {
            if !self.server_config.draft.contains_key(key) { self.show_tune(key, window, cx); }
        }
        if !self.server_config.draft.contains_key("elevenlabs_voice_id") { self.show_voice(window, cx); }
        self.sync_provider_inputs(window, cx);
    }

    /// O slider no valor atual (o do rascunho ou do servidor, senão o padrão).
    fn show_tune(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let s = &self.server_config;
        let Some(tune) = TUNES.iter().find(|t| t.key == key) else { return };
        let Some((_, state, _)) = s.tunes.iter().find(|(k, ..)| *k == key) else { return };
        let value = s.tune_value(tune) as f32;
        state.update(cx, |state, cx| state.set_value(value, window, cx));
    }

    /// A lista de vozes na voz atual; id que ela não conhece fica sem escolha, como o `show_choice`.
    fn show_voice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = &self.server_config;
        let Some((picker, _)) = &s.voice_picker else { return };
        let id = text_of(&s.current("elevenlabs_voice_id")).trim().to_owned();
        let known = s.voices.ok().is_some_and(|v| id.is_empty() || v.iter().any(|v| v.id == id));
        picker.update(cx, |state, cx| if known { state.set_selected_value(&id, window, cx) } else { state.set_selected_index(None, window, cx) });
    }

    /// Teclado do slider, que o kit não dá: setas ±1, Home e End nas pontas. O valor vai ao rascunho pela mesma porta.
    fn tune_key(&mut self, key: &'static str, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tune) = TUNES.iter().find(|t| t.key == key) else { return };
        if !self.server_config.fields.contains_key(key) { return; }
        let now = self.server_config.tune_value(tune);
        let next = match event.keystroke.key.as_str() {
            "left" | "down" => now - 1,
            "right" | "up" => now + 1,
            "home" => tune.min,
            "end" => tune.max,
            _ => return,
        }.clamp(tune.min, tune.max);
        cx.stop_propagation();
        if next == now { return; }
        self.server_config.stage(key, json!(next));
        self.show_tune(key, window, cx);
        cx.notify();
    }

    /// "voltar ao padrão" dos ajustes e da voz: `null` no rascunho, como o `removerRascunho` do web.
    fn back_to_default(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.server_config.stage(key, Value::Null) { return; }
        if key == "elevenlabs_voice_id" { self.show_voice(window, cx); } else { self.show_tune(key, window, cx); }
        cx.notify();
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
        self.server_config.follow_readers();
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
        } else if page == Page::Jev {
            self.render_jev(cx)
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

    fn render_jev(&self, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        let route = s.jev_route();
        let openrouter = route.as_ref().map(|(openrouter, ..)| *openrouter);
        let provider = |openrouter: bool| if openrouter { "OpenRouter" } else { "TypeSafe" };
        let card_title = |text: String, aside: Option<Div>| div().px_4().pt(px(14.)).pb(px(4.)).flex().items_center().justify_between().gap(px(8.))
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(text)).children(aside);
        let line = || div().mt(px(-1.)).border_t_1().border_color(theme::border()).px_4().py(px(14.)).flex().flex_col().gap(px(8.));
        let label = |key: &str, chip_text: Option<String>| div().flex().flex_wrap().items_center().gap(px(8.))
            .child(div().font_weight(FontWeight::MEDIUM).child(tr(key)))
            .children(chip_text.map(|t| chip(t, theme::accent_text(), theme::accent_dim())));
        let help = |text: String| div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(text);
        // Provedor não é campo gravado: sai da chave e do endereço, como no servidor.
        let names = [provider(false).to_owned(), provider(true).to_owned()];
        let provider_row = line()
            .child(label("server_jev_provider", route.as_ref().filter(|_| !s.filled("jev_endpoint")).map(|_| tr("server_jev_provider_from_key"))))
            .child(div().flex().child(segments_with_hints("jev-provider", &names, &[], openrouter.map_or(usize::MAX, usize::from), 2, false,
                String::new(), |this, n, window, cx| { this.follow_jev_provider(n == 1, true, window, cx); cx.notify(); }, cx)))
            .child(help(tr("server_jev_provider_help")));
        let default_chip = |value_is_default: bool| openrouter.filter(|_| value_is_default)
            .map(|o| tr(if o { "server_jev_default_of_openrouter" } else { "server_jev_default_of_typesafe" }));
        let in_use = |value: &str| route.as_ref().map(|_| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
            .child(tr("server_jev_in_use").replace("{value}", value)));
        let input = |key: &str| s.inputs.iter().find(|(k, _)| *k == key).map(|(_, i)| i.clone());
        let url = route.as_ref().map(|(_, url, _)| url.clone()).unwrap_or_default();
        let url_row = line()
            .child(label("server_jev_endpoint", default_chip(!s.filled("jev_endpoint"))))
            .child(help(tr("server_jev_endpoint_help")))
            .children(input("jev_endpoint").map(|i| Input::new(&i).small().aria_label(tr("server_jev_endpoint"))))
            .child(div().flex().items_center().gap(px(10.)).children(in_use(&url))
                .when(s.filled("jev_endpoint"), |el| el.child(Button::new("jev-endpoint-default").ghost().xsmall().label(tr("server_jev_back_to_default"))
                    .on_click(cx.listener(|this, _, window, cx| { this.set_jev_text("jev_endpoint", "", window, cx); cx.notify(); })))));
        let model = route.as_ref().map(|(_, _, model)| model.clone()).unwrap_or_default();
        let current_model = text_of(&s.current("jev_model"));
        let preset = jev_model_preset(&current_model, openrouter.unwrap_or(false));
        let presets = [("jev-model-latest", "server_jev_model_latest", JevModel::Latest), ("jev-model-fixed", "server_jev_model_fixed", JevModel::Fixed)];
        let model_row = line()
            .child(label("server_jev_model", default_chip(current_model.trim().is_empty())))
            .child(help(tr("server_jev_model_help")))
            .child(div().flex().gap(px(6.)).children(presets.into_iter().map(|(id, text, which)| {
                let selected = preset == Some(which);
                Button::new(id).ghost().small().rounded_full().selected(selected).aria_selected(selected).label(tr(text))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let openrouter = this.server_config.jev_route().is_some_and(|(openrouter, ..)| openrouter);
                        this.set_jev_text("jev_model", jev_model_value(which, openrouter), window, cx);
                        cx.notify();
                    }))
            })))
            .children(input("jev_model").map(|i| Input::new(&i).small().aria_label(tr("server_jev_model"))))
            .children(in_use(&model));
        let config_card = settings_box()
            .child(card_title(tr("server_jev_config_title"), None))
            .child(self.config_row(field("jev_api_key"), cx))
            .child(self.mark(provider_row, "server_jev_provider"))
            .child(self.mark(url_row, "server_jev_endpoint"))
            .child(self.mark(model_row, "server_jev_model"));
        let summary = route.as_ref().map(|(openrouter, _, model)| chip(format!("{} · {model}", provider(*openrouter)), theme::muted(), theme::raised()));
        let uses = [("server_jev_use_browser", "server_jev_use_browser_help"), ("server_jev_use_orchestration", "server_jev_use_orchestration_help"),
            ("server_jev_use_voice", "server_jev_use_voice_help"), ("server_jev_use_computer", "server_jev_use_computer_help")];
        let has_key = route.is_some();
        let status = move || if has_key { chip(tr("server_jev_uses"), theme::accent_text(), theme::accent_dim()) }
            else { chip(tr("server_jev_no_key"), theme::warning(), theme::raised()) };
        let cell = |(name, about): (&str, &str)| div().flex_1().min_w(px(220.)).p_3().rounded(px(8.)).border_1().border_color(theme::border())
            .flex().flex_col().gap(px(4.))
            .child(div().flex().items_center().justify_between().gap(px(8.))
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(tr(name))).child(status()))
            .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr(about)));
        let computer = cell(uses[3]).child(div().flex().child(Button::new("jev-open-computer").ghost().xsmall().label(tr("server_jev_open_computer"))
            .icon(IconName::ArrowRight).on_click(cx.listener(|this, _, window, cx| this.open_settings(Page::Windows, window, cx)))));
        let users_card = settings_box()
            .child(card_title(tr("server_jev_users_title"), summary))
            .child(div().px_4().pb(px(14.)).pt(px(6.)).flex().flex_wrap().gap(px(10.))
                .children(uses[..3].iter().copied().map(cell)).child(computer))
            .child(self.config_row(field("jev_padrao"), cx));
        let open = s.jev_advanced;
        let owner = cx.entity().downgrade();
        div().mt_4().flex().flex_col().gap_4()
            .child(help(tr("server_jev_intro")))
            .child(config_card)
            .child(users_card)
            .child(Disclosure::new("jev-advanced", open, tr("server_jev_text_advanced"), false)
                .on_change(move |open, cx| { let _ = owner.update(cx, |this, cx| {
                    this.server_config.jev_advanced = open; cx.notify();
                }); }))
            .when(open, |el| el
                .child(help(tr_shared("jev_text_help", &[])))
                .child(settings_box().children(["jev_texto_base_url", "jev_texto_api_key", "jev_texto_modelo", "jev_texto_cmd"]
                    .into_iter().map(|key| self.config_row(field(key), cx)))))
    }

    /// Grava um texto do Jev no rascunho e mostra no campo.
    fn set_jev_text(&mut self, key: &'static str, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.server_config.stage(key, Value::String(value.to_owned())) { return; }
        if let Some((_, input)) = self.server_config.inputs.iter().find(|(k, _)| *k == key) {
            input.update(cx, |state, cx| if state.value() != value { state.set_value(value.to_owned(), window, cx) });
        }
    }

    /// Escolher o provedor (`force`) troca o endereço; colar a chave só tira do caminho o endereço padrão do outro. Modelo
    /// sugerido pelo outro provedor volta ao padrão; modelo próprio fica.
    fn follow_jev_provider(&mut self, openrouter: bool, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let s = &self.server_config;
        let key = s.jev_key_head().unwrap_or_default();
        let endpoint = text_of(&s.current("jev_endpoint")).trim().to_owned();
        let defaults = [crate::voice::jev::TYPESAFE_URL, crate::voice::jev::OPENROUTER_URL];
        if force || defaults.contains(&endpoint.as_str()) {
            let by_key = crate::voice::jev::is_openrouter(&key, None);
            let wanted = if openrouter == by_key { "" } else { defaults[usize::from(openrouter)] };
            if endpoint != wanted { self.set_jev_text("jev_endpoint", wanted, window, cx); }
        }
        let model = text_of(&self.server_config.current("jev_model"));
        if !model.trim().is_empty() && jev_model_preset(&model, !openrouter).is_some() {
            self.set_jev_text("jev_model", "", window, cx);
        }
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
        let error = s.fields.get(key).and_then(|f| f.get("erro")).and_then(Value::as_str).filter(|error| !error.is_empty());
        let off = !s.fields.contains_key(key) || error.is_some();
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
        let this = cx.entity().downgrade();
        let verdict = VERDICTS.iter().find(|(k, ..)| *k == key).map(|&(_, verdict, why)| div().flex().flex_col().gap(px(6.))
            .child(div().flex().items_center().gap(px(6.)).text_size(px(13.))
                .child(div().text_color(theme::muted()).child(tr(verdict)))
                .child(Disclosure::new(format!("server-{key}-why"), open, tr("accounts_engine_why"), true)
                    // Na Voz dois "por quê?" ficam à vista juntos: o nome diz de qual linha é, como nas Contas.
                    .name(tr("accounts_engine_why_of").replace("{name}", &tr(field.label)))
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| {
                        let why = &mut this.server_config.why;
                        why.retain(|k| *k != key);
                        if open { why.push(key); }
                        cx.notify();
                    }); })))
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
                .children(verdict).children(below).children(removal)
                .children(error.map(|error| div().id(SharedString::from(format!("server-{key}-error"))).role(Role::Alert)
                    .text_sm().text_color(theme::danger()).whitespace_normal().child(error.to_owned()))));
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
            .children(self.render_providers(cx))
            .child(div().mt(px(28.)).mb(px(10.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr("voice_after")))
            .child(settings_box().child(self.hands_free_row(cx)).child(self.config_row(field("ditado_vocabulario"), cx)))
            .when_some(self.appearance_note.clone(), |el, note| el.child(div().id("voice-hands-free-save-error").role(Role::Alert)
                .mt_2().text_sm().text_color(theme::danger()).whitespace_normal().child(note)))
            .child(self.voice_head("voice_cleanup", "voice_cleanup_help", s.cleanup_status()))
            .child(self.organization_row(cx))
            .when(s.organization_mode()!="none",|el|el.child(settings_box().child(self.style_row(cx)).child(self.config_row(field("dictation_include_recent_messages"),cx))))
            .when(s.organization_mode()=="harness",|el|el.child(self.harness_model_rows(cx)))
            .when(s.organization_mode()=="external_api",|el|el.child(div().mt(px(8.)).child(rows(SECTIONS[1], cx)))
                .child(div().mt(px(8.)).flex().child(self.section_toggle(2, "voice_briefing_own", cx)))
                .when(s.open[2], |el| el.child(div().mt(px(8.)).child(rows(SECTIONS[2], cx)))))
            .child(self.render_read_aloud(cx))
    }

    fn organization_row(&self,cx:&mut Context<Self>)->Div {
        let modes=["none","harness","external_api"];
        let labels=modes.map(|mode|tr(&format!("voice_organization_{mode}"))).to_vec();
        let hints=modes.map(|mode|tr(&format!("voice_organization_{mode}_hint"))).to_vec();
        let selected=modes.iter().position(|mode|*mode==self.server_config.organization_mode()).unwrap_or(0);
        let available=if self.server_config.fields.contains_key("dictation_organization_mode"){3}else{0};
        settings_box().p_4().child(segments_with_hints("voice-organization-mode",&labels,&hints,selected,available,false,tr("settings_next_version"),
            move|this,index,_,cx|{this.server_config.stage("dictation_organization_mode",json!(modes[index]));this.load_dictation_models(cx);cx.notify();},cx))
    }

    fn harness_model_rows(&self,cx:&mut Context<Self>)->Div {
        let state=&self.server_config;
        let catalog=state.dictation_catalog.ok();
        let mut body=settings_box().p_4().flex().flex_col().gap_4();
        for (provider,key) in [("claude","dictation_claude_model"),("codex","dictation_codex_model")] {
            let selected=text_of(&state.current(key));
            let models=catalog.filter(|catalog|catalog["provider"]==provider).and_then(|catalog|catalog["models"].as_array()).cloned().unwrap_or_default();
            let label=models.iter().find(|model|model["id"].as_str()==Some(selected.as_str()))
                .and_then(|model|model["name"].as_str()).map(str::to_owned).unwrap_or_else(||if selected.is_empty(){tr("voice_model_choose")}else{selected.clone()});
            let entity=cx.entity().downgrade();
            let disabled=models.is_empty()||!state.fields.contains_key(key)||state.dictation_catalog.loading;
            body=body.child(div().flex().flex_col().gap_2()
                .child(div().font_weight(FontWeight::MEDIUM).child(tr(&format!("voice_harness_model_{provider}"))))
                .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("voice_harness_model_help")))
                .child(Button::new(SharedString::from(format!("voice-model-{provider}"))).outline().label(label).disabled(disabled)
                    .dropdown_menu(move|menu,_,_|models.iter().cloned().fold(menu,|menu,model|{
                        let id=model["id"].as_str().unwrap_or("").to_owned();
                        let label=model["name"].as_str().unwrap_or(&id).to_owned();
                        let owner=entity.clone();let checked=id==selected;
                        menu.item(PopupMenuItem::new(label).checked(checked).on_click(move|_,_,cx|{let _=owner.update(cx,|this,cx|{this.server_config.stage(key,json!(id));cx.notify();});}))
                    }))));
        }
        body
            .when(state.dictation_catalog.loading,|el|el.child(div().id("voice-models-loading").role(Role::Status).child(tr("voice_harness_models_loading"))))
            .when_some(state.dictation_catalog.value.as_ref().and_then(|value|value.as_ref().err()).cloned(),|el,error|el.child(div().id("voice-models-error").role(Role::Alert).text_sm().text_color(theme::warning()).whitespace_normal().child(error)))
            .when(catalog.is_some_and(|catalog|catalog["models"].as_array().is_some_and(Vec::is_empty)),|el|el.child(tr("voice_models_empty")))
            .child(Button::new("voice-models-refresh").ghost().small().label(tr("voice_models_refresh")).disabled(state.dictation_catalog.loading)
                .on_click(cx.listener(|this,_,_,cx|this.load_dictation_models(cx))))
    }

    /// Serviços de transcrição em ordem: o primeiro transcreve, e quem falha ou fica sem cota passa a vez. Servidor cuja
    /// leitura não trouxe a lista não mostra o bloco.
    fn render_providers(&self, cx: &mut Context<Self>) -> Option<Div> {
        let s = &self.server_config;
        if !s.fields.contains_key(PROVIDERS) { return None; }
        let providers = s.providers();
        let now = chrono::Utc::now().timestamp() as f64;
        let last = providers.len().saturating_sub(1);
        let line = || div().mt(px(-1.)).border_t_1().border_color(theme::border()).px_4().py(px(14.));
        let rows = providers.iter().enumerate().map(|(n, item)| {
            let id = provider_text(item, "id").to_owned();
            let kind = provider_text(item, "kind");
            let status_name = s.provider_status.ok().and_then(|list| list.iter().find(|st| st.id == id)).map(|st| st.name.clone());
            let name = Some(provider_text(item, "name").to_owned()).filter(|n| !n.is_empty()).or(status_name.filter(|n| !n.is_empty()))
                .unwrap_or_else(|| tr(if kind == "elevenlabs" { "voice_provider_kind_elevenlabs" }
                    else if kind == "whisper_cpp" { "voice_provider_kind_whisper" } else { "voice_provider_kind_openai" }));
            let title = format!("{}. {name}", n + 1);
            let waiting = s.provider_waiting(&id, now).map(|(until, reason)| {
                // Mesmo formato da hora das mensagens: só a hora hoje, com o dia traduzido antes disso.
                let when = super::stamp(Some(until)).unwrap_or_default();
                let text = tr("voice_provider_waiting").replace("{until}", &when);
                match reason { Some(reason) => format!("{text}: {reason}"), None => text }
            });
            let mask = s.provider_mask(&id).filter(|mask| provider_text(item, "api_key") == mask);
            let no_key = kind == "elevenlabs" && provider_text(item, "api_key").trim().is_empty();
            let inputs = s.provider_inputs.iter().find(|(known, ..)| *known == id).map(|(_, inputs, _)| inputs.clone());
            let labeled = |label: &'static str, input: Entity<InputState>| div().flex().flex_col().gap(px(4.))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr(label)))
                .child(Input::new(&input).small().aria_label(format!("{}, {title}", tr(label))));
            let fields = inputs.map(|[endpoint, key, model, executable, model_file, language, converter]| {
                let el = div().flex().flex_col().gap(px(8.));
                if kind == "whisper_cpp" {
                    let address = self.api.as_ref().map(|api| api.server_address()).unwrap_or_else(|| tr("voice_provider_current_server"));
                    return el.child(div().text_sm().text_color(theme::muted()).whitespace_normal()
                        .child(tr("voice_provider_local_help").replace("{server}", &address)))
                        .child(Button::new(SharedString::from(format!("provider-{id}-official"))).ghost().small()
                            .label(tr("voice_provider_official")).on_click(|_, _, cx| cx.open_url("https://github.com/ggml-org/whisper.cpp")))
                        .child(labeled("voice_provider_executable", executable)).child(labeled("voice_provider_model_file", model_file))
                        .child(labeled("voice_provider_language", language)).child(labeled("voice_provider_converter", converter));
                }
                el.when(kind == "openai", |el| el.child(labeled("voice_provider_endpoint", endpoint)))
                .child(labeled(if kind == "openai" { "voice_provider_key_optional" } else { "voice_provider_key" }, key))
                .children(mask.map(|mask| div().flex().items_center().gap(px(6.)).text_size(px(12.5)).text_color(theme::muted())
                    .child(div().font_family(theme::MONO).child(mask)).child(tr("server_secret_set"))))
                .when(no_key, |el| el.child(div().id(SharedString::from(format!("provider-{id}-no-key"))).role(Role::Status)
                    .text_size(px(12.5)).text_color(theme::warning()).child(tr("voice_provider_missing_key"))))
                .child(labeled("voice_provider_model", model))
            });
            let kinds = div().flex().flex_wrap().gap(px(6.)).children([("openai", "voice_provider_kind_openai"), ("elevenlabs", "voice_provider_kind_elevenlabs"), ("whisper_cpp", "voice_provider_kind_whisper")]
                .map(|(value, label)| {
                    let id = id.clone();
                    let on = value == kind;
                    // O fundo de `selected` some nos temas com vidro: o escolhido vai como primário para se distinguir.
                    // O leitor de tela sabe o tipo escolhido pelo estado de alternância.
                    let button = Button::new(SharedString::from(format!("provider-{id}-kind-{value}"))).xsmall();
                    if on { button.primary() } else { button.ghost() }.label(tr(label)).toggled(on)
                        .on_click(cx.listener(move |this, _, window, cx| this.set_provider_kind(&id, value, window, cx)))
                }));
            let (up, down, remove) = (id.clone(), id.clone(), id.clone());
            let up_label = tr("voice_provider_up").replace("{name}", &name);
            let down_label = tr("voice_provider_down").replace("{name}", &name);
            let test = s.provider_tests.get(&id);
            let test_busy = test.is_some_and(|t| t.loading);
            let test_id = id.clone();
            let local = s.provider_status.ok().and_then(|list| list.iter().find(|p| p.id == id));
            line().flex().flex_col().gap(px(10.))
                .child(div().flex().items_center().gap(px(6.))
                    .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(title.clone()))
                    .child(Button::new(SharedString::from(format!("provider-{id}-up"))).ghost().xsmall().icon(IconName::ArrowUp)
                        .disabled(n == 0).tooltip(up_label.clone()).accessibility_label(up_label)
                        .on_click(cx.listener(move |this, _, _, cx| this.move_transcription_provider(&up, true, cx))))
                    .child(Button::new(SharedString::from(format!("provider-{id}-down"))).ghost().xsmall().icon(IconName::ArrowDown)
                        .disabled(n == last).tooltip(down_label.clone()).accessibility_label(down_label)
                        .on_click(cx.listener(move |this, _, _, cx| this.move_transcription_provider(&down, false, cx))))
                    .child(Button::new(SharedString::from(format!("provider-{id}-remove"))).ghost().xsmall().icon(IconName::Close)
                        .label(tr("server_remove")).accessibility_label(tr("voice_provider_remove").replace("{name}", &name))
                        .on_click(cx.listener(move |this, _, window, cx| this.remove_transcription_provider(&remove, window, cx)))))
                .children(waiting.map(|text| div().id(SharedString::from(format!("provider-{id}-waiting"))).role(Role::Status)
                    .text_size(px(12.5)).text_color(theme::warning()).whitespace_normal().child(text)))
                .child(kinds)
                .children(fields)
                .when(kind == "whisper_cpp", |el| el.children(local.and_then(|p| p.state.as_deref()).map(|state|
                    div().id(SharedString::from(format!("provider-{id}-local-state"))).role(Role::Status).text_sm().text_color(theme::muted()).child(tr(match state {
                        "ready" => "voice_provider_local_ready", "starting" => "voice_provider_local_starting",
                        "failed" => "voice_provider_local_failed", _ => "voice_provider_local_not_started",
                    }))))
                    .children(local.and_then(|p| p.error.clone()).map(|error| div().id(SharedString::from(format!("provider-{id}-local-error"))).role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error))))
                .child(Button::new(SharedString::from(format!("provider-{id}-test"))).outline().small()
                    .disabled(s.draft.contains_key(PROVIDERS) || test_busy)
                    .label(tr(if test_busy { "voice_provider_test_running" } else { "voice_provider_test_audio" }))
                    .on_click(cx.listener(move |this, _, _, cx| this.test_provider(test_id.clone(), cx))))
                .when(s.draft.contains_key(PROVIDERS), |el| el.child(div().text_sm().text_color(theme::muted()).child(tr("voice_provider_test_saved"))))
                .children(test.and_then(|t| t.value.as_ref()).map(|value| match value {
                    Ok(text) => div().id(SharedString::from(format!("provider-{id}-test-result"))).role(Role::Status).text_sm().whitespace_normal().child(tr("voice_provider_test_success")).child(text.clone()),
                    Err(error) => div().id(SharedString::from(format!("provider-{id}-test-error"))).role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone()),
                }))
        }).collect::<Vec<_>>();
        let staged = s.draft.contains_key(PROVIDERS);
        let status_error = s.provider_status.value.as_ref().and_then(|v| v.as_ref().err()).cloned();
        Some(div().mt(px(16.))
            .child(div().mb(px(10.)).flex().items_start().gap(px(16.))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(4.))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr("voice_providers")))
                    .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr("voice_providers_help"))))
                .when(staged, |el| el.child(Button::new("voice-providers-undo").ghost().xsmall().label(tr("server_undo"))
                    .on_click(cx.listener(|this, _, window, cx| this.undo_config(PROVIDERS, window, cx))))))
            .child(settings_box()
                .when(providers.is_empty(), |el| el.child(line().text_sm().text_color(theme::muted()).whitespace_normal()
                    .child(tr("voice_providers_empty"))))
                .children(rows)
                .child(line().py(px(10.)).flex().child(Button::new("voice-provider-add").outline().small().icon(IconName::Plus)
                    .label(tr("voice_provider_add")).on_click(cx.listener(|this, _, window, cx| this.add_transcription_provider(window, cx))))))
            .when_some(status_error, |el, error| el.child(div().mt_2().flex().items_center().gap(px(8.))
                .child(div().id("voice-providers-status-error").role(Role::Alert).flex_1().min_w_0().text_sm().text_color(theme::danger())
                    .whitespace_normal().child(tr("voice_providers_status_failed").replace("{error}", &error)))
                .child(Button::new("voice-providers-status-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_provider_status(cx)))))))
    }

    /// Ler em voz alta (`VozSettings.svelte`, "Ler em voz alta"): ElevenLabs, leitor instalado e o limite da confirmação.
    fn render_read_aloud(&self, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        let can_read = s.can_read();
        let eleven = settings_box().child(self.config_row(field("elevenlabs_api_key"), cx))
            .when(s.key_set("elevenlabs_api_key"), |el| el.child(self.voice_row(cx)).child(self.sample_row()));
        div()
            .child(self.voice_head("voice_read", "voice_read_help", s.read_status()))
            .when(!can_read, |el| el.child(div().mb(px(10.)).text_sm().text_color(theme::muted()).whitespace_normal().child(tr("voice_read_no_reader"))))
            .child(div().flex().child(self.section_toggle(3, "voice_elevenlabs_section", cx)))
            .when(s.open[3], |el| el.child(div().mt(px(8.)).child(eleven))
                .when(s.key_set("elevenlabs_api_key"), |el| el
                    .child(div().mt(px(8.)).flex().child(self.section_toggle(4, "voice_tune", cx)))
                    .when(s.open[4], |el| el.child(div().mt(px(8.)).child(settings_box()
                        .children(TUNES.iter().map(|t| self.tune_row(t, cx)).collect::<Vec<_>>()))))))
            .child(div().mt(px(8.)).flex().child(self.section_toggle(5, "voice_local_section", cx)))
            .when(s.open[5], |el| el.child(div().mt(px(8.)).child(settings_box().child(self.config_row(field("tts_local_cmd"), cx)))))
            .when(can_read, |el| el.child(div().mt(px(12.)).child(settings_box().child(self.config_row(field("tts_max_chars"), cx)))))
    }

    /// Cabeça de uma linha própria da Voz (voz, ajuste): ícone, título com as etiquetas e a ajuda.
    fn own_head(&self, icon: IconName, key: &str, label: &'static str, help: &'static str) -> Div {
        div().flex_1().min_w_0().flex().items_start().gap(px(14.))
            .child(icon_box(icon))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().flex().flex_wrap().items_center().gap(px(8.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(tr(label))).child(self.config_badges(key)))
                .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr(help))))
    }

    /// "voltar ao padrão": só com valor gravado pelo app e que não está saindo no Salvar.
    fn back_button(&self, key: &'static str, label: &str, cx: &mut Context<Self>) -> Option<Button> {
        let s = &self.server_config;
        (s.edited_in_app(key) && !s.removing(key)).then(|| Button::new(SharedString::from(format!("server-{key}-default"))).ghost().xsmall()
            .label(tr("voice_back_to_default")).accessibility_label(format!("{label}, {}", tr("voice_back_to_default")))
            .on_click(cx.listener(move |this, _, window, cx| this.back_to_default(key, window, cx))))
    }

    /// Voz da conta: carrega sob demanda (vozes e consumo), escolhe na lista e grava no rascunho.
    fn voice_row(&self, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        let key = "elevenlabs_voice_id";
        let off = !s.fields.contains_key(key);
        let loading = s.voices.loading;
        let error = s.voices.value.as_ref().and_then(|v| v.as_ref().err()).cloned();
        let control = match (&s.voice_picker, &error) {
            (Some((picker, _)), None) => div().w(px(200.)).child(Select::new(picker).small().disabled(off).accessibility_label(tr("voice_voice"))),
            _ => div().child(Button::new("voice-load").outline().small().loading(loading).disabled(loading)
                .label(tr(if loading { "server_loading" } else if error.is_some() { "server_retry" } else { "voice_load_voices" }))
                .on_click(cx.listener(|this, _, _, cx| this.load_voices(cx)))),
        };
        // O resultado do clique fica na linha do botão: erro das vozes, consumo do mês ou o erro dele.
        let usage = match &s.usage.value {
            Some(Ok([used, limit])) => Some((tr("voice_usage").replace("{usados}", &used.map_or("?".into(), |n| n.to_string()))
                .replace("{limite}", &limit.map_or("?".into(), |n| n.to_string())), false)),
            Some(Err(e)) => Some((e.clone(), true)),
            None => None,
        };
        let notes = error.map(|e| (e, true)).into_iter().chain(usage).map(|(text, bad)| div().text_size(px(12.5)).whitespace_normal()
            .text_color(if bad { theme::danger() } else { theme::muted() }).child(text));
        let head = self.own_head(IconName::AudioLines, key, "voice_voice", "voice_voice_help");
        let row = div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().flex_col().gap(px(8.)).px_4().py(px(14.))
            .child(div().flex().items_start().gap(px(14.)).child(head).child(div().flex_shrink_0().child(control)))
            .child(div().pl(px(50.)).flex().flex_col().gap(px(4.))
                .child(div().flex().items_center().gap(px(8.)).text_size(px(13.))
                    .child(div().id("voice-current").role(Role::Status).child(tr("voice_current").replace("{valor}", &s.voice_name())))
                    .children(self.back_button(key, &tr("voice_voice"), cx)))
                .children(notes));
        self.mark(row, "voice_voice")
    }

    /// "Ouvir amostra": o app ainda não lê em voz alta, então a linha aparece desligada dizendo por quê, no desenho do
    /// mãos-livres.
    fn sample_row(&self) -> Div {
        let next = tr("settings_next_version");
        let head = div().flex_1().min_w_0().flex().items_center().gap(px(14.))
            .child(icon_box(IconName::Volume2))
            .child(div().flex().flex_wrap().items_center().gap(px(8.))
                .child(div().font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(tr("voice_sample")))
                .child(chip(next.trim_end_matches('.').to_owned(), theme::muted(), theme::raised())));
        // O kit não dá descrição ao `Button`: o motivo entra no nome, para o leitor de tela não anunciar só "desligado".
        let button = Button::new("voice-sample").outline().small().icon(IconName::Volume2).label(tr("voice_sample")).disabled(true)
            .accessibility_label(format!("{}. {next}", tr("voice_sample")));
        div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_center().gap(px(14.)).px_4().py(px(14.))
            .child(head).child(div().flex_shrink_0().child(button))
    }

    /// Um ajuste da voz: título com o valor, "voltar ao padrão", ajuda e o slider entre as pontas. O invólucro dá ao slider
    /// o nome e o teclado que o kit não dá.
    fn tune_row(&self, tune: &Tune, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        let (key, name) = (tune.key, tune.name);
        let label = tr(&format!("voice_tune_{name}"));
        let off = !s.fields.contains_key(key);
        let value = s.tune_value(tune);
        let slider = s.tunes.iter().find(|(k, ..)| *k == key).map(|(_, state, focus)| div().id(SharedString::from(format!("voice-tune-{key}")))
            .track_focus(focus).role(Role::Group).aria_label(label.clone()).flex_1().min_w_0().px(px(6.)).py(px(4.)).rounded(px(6.))
            .border_1().border_color(transparent_black()).focus_visible(|el| el.border_color(theme::accent_focus()))
            .when(!off, |el| el.on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| this.tune_key(key, event, window, cx))))
            .child(Slider::new(state).aria_label(label.clone()).bg(theme::accent()).text_color(theme::text()).disabled(off)));
        // Pontas de largura fixa: os quatro sliders começam e terminam na mesma coluna, qualquer que seja o texto.
        let end = |side: &str| div().w(px(96.)).flex_shrink_0().text_size(px(12.)).text_color(theme::muted()).whitespace_normal()
            .when(side == "right", |el| el.text_right()).child(tr(&format!("voice_tune_{name}_{side}")));
        let title = div().flex().flex_wrap().items_center().gap(px(8.))
            .child(div().font_weight(FontWeight::MEDIUM).child(label.clone()))
            .child(div().font_family(theme::MONO).text_size(px(13.)).text_color(theme::accent_text()).child(value.to_string()))
            .child(self.config_badges(key))
            .children(self.back_button(key, &label, cx));
        let row = div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().flex_col().gap(px(6.)).px_4().py(px(14.))
            .child(title)
            .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr(&format!("voice_tune_{name}_help"))))
            .child(div().flex().items_center().gap(px(10.)).child(end("left")).children(slider).child(end("right")));
        self.mark(row, &format!("voice_tune_{name}"))
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

    /// Abre e fecha uma seção de `SECTIONS`, como o `<details>` do web.
    fn section_toggle(&self, n: usize, label: &'static str, cx: &mut Context<Self>) -> Disclosure {
        let this = cx.entity().downgrade();
        Disclosure::new(format!("voice-section-{n}"), self.server_config.open[n], tr(label), false)
            .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.server_config.open[n] = open; cx.notify(); }); })
    }

    /// A preferência é deste aparelho e já é gravada no clique.
    fn hands_free_row(&self, cx: &mut Context<Self>) -> Div {
        let switch = div().id("voice-hands-free").child(Switch::new("voice-hands-free-switch").checked(appearance::get().hands_free)
            .accessibility_label(tr("voice_hands_free"))
            .on_change(cx.listener(|this, on: &bool, _, cx| {
                let mut next = appearance::get();
                next.hands_free = *on;
                this.apply_appearance(next, true, cx);
            })));
        let head = div().flex_1().min_w_0().flex().items_center().gap(px(14.))
            .child(icon_box(IconName::Mic))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().flex().flex_wrap().items_center().gap(px(8.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(tr("voice_hands_free")))
                    .child(chip(tr("settings_group_device"), theme::muted(), theme::raised())))
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
        let this = cx.entity().downgrade();
        let why = div().flex().items_center().gap(px(6.)).text_size(px(13.))
            .child(div().text_color(theme::muted()).child(tr("server_quiet_verdict")))
            .child(Disclosure::new("server-quiet-why", q.why, tr("accounts_engine_why"), true)
                .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.server_config.quiet.why = open; cx.notify(); }); }));
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
    /// `term_origins` no detalhe desta máquina, com o Salvar e o resultado ao lado dela, como o web: o rascunho é o das outras
    /// páginas, e o rodapé delas ficaria atrás do diálogo.
    pub(super) fn term_origins_block(&self, cx: &mut Context<Self>) -> Div {
        let s = &self.server_config;
        // Leitura boa sem o campo (servidor antigo) cai na linha desligada, como nas outras páginas.
        if s.fields.is_empty() {
            match &s.load.value {
                Some(Err(error)) if !s.load.loading => return div().flex().flex_col().items_start().gap(px(8.))
                    .child(div().id("machines-config-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal()
                        .child(error.clone()))
                    .child(Button::new("machines-config-retry").outline().small().label(tr("server_retry"))
                        .on_click(cx.listener(|this, _, _, cx| this.load_server_config(cx)))),
                Some(Ok(())) if !s.load.loading => {}
                _ => return div().text_sm().text_color(theme::muted()).child(tr("server_loading")),
            }
        }
        let dirty = !s.draft.is_empty();
        let blocked = s.provider_missing_key();
        div().flex().flex_col().gap(px(8.))
            .child(settings_box().child(self.config_row(field("term_origins"), cx)))
            .when_some(s.save_error.clone(), |el, error| el.child(div().id("machines-save-error").role(Role::Alert)
                .text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error)))
            // O rodapé das páginas do servidor não aparece aqui: o motivo do Salvar desligado fica junto do botão.
            .when(blocked, |el| el.child(div().id("machines-config-blocked").role(Role::Status).text_size(px(12.5))
                .text_color(theme::warning()).whitespace_normal().child(tr("server_save_blocked_provider_key"))))
            .when(dirty || s.saving || s.saved.is_some(), |el| el.child(div().flex().items_center().justify_end().gap(px(12.))
                .when(s.saved.is_some(), |el| el.child(div().id("machines-saved").role(Role::Status).text_size(px(12.5))
                    .text_color(theme::success()).child(tr("server_saved"))))
                .when(dirty || s.saving, |el| el.child(Button::new("machines-config-save").primary().small()
                    .label(tr(if s.saving { "server_saving" } else { "server_save" })).loading(s.saving).disabled(s.saving || blocked)
                    .on_click(cx.listener(|this, _, _, cx| this.save_server_config(cx)))))))
    }

    pub(super) fn server_config_footer(&self, page: Page, cx: &mut Context<Self>) -> Option<Div> {
        let s = &self.server_config;
        let dirty = !s.draft.is_empty();
        let blocked = s.provider_missing_key();
        if !is_server_page(page) || s.load.loading || s.fields.is_empty() || !(dirty || s.saving || s.saved.is_some()) { return None; }
        // O botão fica na mesma coluna de 720px do conteúdo, não na borda da janela.
        // O erro do Salvar mora aqui, junto do botão, e não no fim da página: no Avançado ele ficava abaixo do `.env`, fora
        // da vista de quem clicou Salvar no topo. O web o põe no fim da coluna, que lá é curta.
        let column = div().w(px(720.)).max_w_full().px_4().flex().items_center().justify_end().gap(px(12.))
            .when_some(s.save_error.clone(), |el, error| el.child(div().id("server-config-error").flex_1().min_w_0().truncate().text_right()
                .text_size(px(12.5)).text_color(theme::danger()).child(error.clone())
                .tooltip(move |window, cx| Tooltip::new(error.clone()).build(window, cx))))
            .when(blocked, |el| el.child(div().id("server-config-blocked").role(Role::Status).flex_1().min_w_0().text_right()
                .text_size(px(12.5)).text_color(theme::warning()).whitespace_normal().child(tr("server_save_blocked_provider_key"))))
            .when(s.saved.is_some(), |el| el.child(div().text_size(px(12.5)).text_color(theme::success()).child(tr("server_saved"))))
            .when(dirty || s.saving, |el| el.child(Button::new("server-config-save").primary().small()
                .label(tr(if s.saving { "server_saving" } else { "server_save" })).loading(s.saving).disabled(s.saving || blocked)
                .on_click(cx.listener(|this, _, _, cx| this.save_server_config(cx)))));
        Some(div().h(px(56.)).flex_shrink_0().border_t_1().border_color(theme::border()).bg(theme::chrome())
            .flex().items_center().justify_center().child(column))
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, PROVIDERS, Quiet, ServerConfig, TUNES, Voice, env_value, shown, text_of};
    use serde_json::{Map, Value, json};

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
    fn jev_fields_follow_the_web_order_and_labels() {
        let jev: Vec<(&str, &str)> = super::FIELDS.iter().filter(|f| f.key.starts_with("jev_")).map(|f| (f.key, f.label)).collect();
        assert_eq!(jev, [("jev_api_key", "server_jev_key"), ("jev_padrao", "server_jev_default"), ("jev_endpoint", "server_jev_endpoint"),
            ("jev_model", "server_jev_model"), ("jev_texto_base_url", "server_jev_text_endpoint"), ("jev_texto_api_key", "server_jev_text_key"),
            ("jev_texto_modelo", "server_jev_text_model"), ("jev_texto_cmd", "server_jev_cmd")]);
        assert!(super::FIELDS.iter().filter(|f| f.key.starts_with("jev_")).all(|f| f.page == super::Page::Jev));
        let mut s = ServerConfig::default();
        s.reveal("server_jev_model");
        assert!(!s.jev_advanced, "endereço e modelo ficam no cartão principal");
        s.reveal("server_jev_text_model");
        assert!(s.jev_advanced, "a busca abre os ajustes avançados do Jev");
        // `reveal` acha o campo pelo rótulo: rótulo repetido abriria a seção errada.
        let labels: std::collections::HashSet<&str> = super::FIELDS.iter().map(|f| f.label).collect();
        assert_eq!(labels.len(), super::FIELDS.len());
        use crate::i18n::{tr, tr_shared};
        assert_eq!(tr("server_jev_endpoint"), tr_shared("config_server_jev_endpoint", &[]));
        assert_eq!(tr("server_jev_model"), tr_shared("config_server_jev_modelo", &[]));
        assert_eq!(tr("server_jev_text_endpoint"), tr_shared("config_server_jev_texto_endpoint", &[]));
        assert_eq!(tr("server_jev_text_model"), tr_shared("config_server_jev_texto_modelo", &[]));
    }

    #[test]
    fn jev_model_suggestions_follow_the_provider() {
        use super::{jev_model_preset, jev_model_value, JevModel};
        assert_eq!(jev_model_preset("", true), Some(JevModel::Latest), "vazio = o mais novo");
        assert_eq!(jev_model_preset("~typesafe/jev-latest", true), Some(JevModel::Latest));
        assert_eq!(jev_model_preset("typesafe/jev-latest", true), Some(JevModel::Latest), "sem til conta como o mais novo");
        assert_eq!(jev_model_preset("jev-latest", false), Some(JevModel::Latest));
        assert_eq!(jev_model_preset(jev_model_value(JevModel::Fixed, true), true), Some(JevModel::Fixed));
        assert_eq!(jev_model_preset(jev_model_value(JevModel::Fixed, false), true), None, "versão da TypeSafe não é a do OpenRouter");
        assert_eq!(jev_model_preset("meu-modelo", false), None);
    }

    #[test]
    fn jev_route_reads_the_provider_from_the_key_or_its_mask() {
        let mut s = ServerConfig::default();
        assert_eq!(s.jev_route(), None, "sem chave");
        s.fields.insert("jev_api_key".into(), json!({"valor": "sk-o••••••••25ba", "definido": true}));
        s.fields.insert("jev_endpoint".into(), json!({"valor": ""}));
        s.fields.insert("jev_model".into(), json!({"valor": ""}));
        assert_eq!(s.jev_route(), Some((true, crate::voice::jev::OPENROUTER_URL.into(), "~typesafe/jev-latest".into())));
        assert!(s.stage("jev_api_key", json!("apik-123")));
        assert_eq!(s.jev_route(), Some((false, crate::voice::jev::TYPESAFE_URL.into(), "jev-latest".into())));
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
    fn reconnecting_allows_a_new_dictation_catalog_after_the_old_request_is_discarded() {
        let mut state = ServerConfig::default();
        state.reconnected("servidor".into());
        let old = state.dictation_catalog.start();
        state.dictation_catalog_target = Some("conversa-antiga".into());
        state.reconnected("servidor".into());
        assert!(!state.dictation_catalog.loading, "a resposta descartada não pode bloquear uma nova consulta");
        assert!(state.dictation_catalog_target.is_none());
        let next = state.dictation_catalog.start();
        assert!(state.dictation_catalog.finish(next, Ok(json!({"models":[]}))));
        assert!(!state.dictation_catalog.finish(old, Ok(json!({"models":["antigo"]}))));
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
    fn provider_test_result_follows_the_saved_configuration() {
        let read = |url: &str| { let mut m = Map::new(); m.insert(PROVIDERS.into(), json!({"valor": [{"id": "a", "base_url": url}]})); m };
        let mut s = ServerConfig::default();
        s.replace_fields(read("http://x"), false);
        let seq = s.provider_tests.entry("a".into()).or_default().start();
        s.provider_tests.get_mut("a").unwrap().finish(seq, Ok("ok".into()));
        s.fields.clear(); // como `load_server_config` faz antes de reler
        s.replace_fields(read("http://x"), false);
        assert!(s.provider_tests["a"].value.is_some(), "reler a mesma configuração: o resultado fica");
        s.replace_fields(read("http://y"), false);
        assert!(s.provider_tests["a"].value.is_none(), "outra configuração lida: o resultado some");
        let seq = s.provider_tests.get_mut("a").unwrap().start();
        s.provider_tests.get_mut("a").unwrap().finish(seq, Ok("ok".into()));
        s.replace_fields(read("http://y"), true);
        assert!(s.provider_tests["a"].value.is_none(), "Salvar confirmou serviços: o resultado some");
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
        s.fields.insert("dictation_organization_mode".into(),json!({"valor":"external_api"}));
        assert_eq!((s.transcribe_status(), s.cleanup_status()), (None, None));
        s.fields.insert("groq_api_key".into(), json!({"valor": "sint••••••••wxyz", "definido": true}));
        assert_eq!((s.transcribe_status(), s.cleanup_status()), (Some("voice_status_on"), None));
        s.stage("transcription_base_url", json!("http://x/v1"));
        assert_eq!((s.transcribe_status(), s.cleanup_status()), (Some("voice_status_custom"), None), "o padrão só reusa a chave do padrão");
        s.stage("llm_base_url", json!("http://y/v1"));
        assert_eq!(s.cleanup_status(), None, "endpoint próprio sem chave própria");
        s.fields.insert("llm_api_key".into(), json!({"valor": "sint••••••••abcd", "definido": true}));
        assert_eq!(s.cleanup_status(), Some("voice_status_custom"));
    }

    #[test]
    fn organization_is_off_even_when_an_old_external_service_is_configured() {
        let mut config = ServerConfig::default();
        config.fields.insert("llm_base_url".into(), json!({"valor":"https://exemplo.invalid/v1"}));
        config.fields.insert("llm_api_key".into(), json!({"valor":"masc••••••••abcd", "definido":true}));
        assert_eq!(config.cleanup_status(), Some("voice_organization_none"));
    }

    #[test]
    fn sections_open_once_from_the_read_and_for_the_search() {
        let mut s = ServerConfig::default();
        s.fields.insert("llm_briefing_model".into(), json!({"valor": "modelo-sintetico"}));
        s.open_filled();
        assert_eq!(s.open, [false, true, true, false, false, false], "o briefing abre junto com a seção que o contém");
        s.open = [false; 6];
        s.open_filled();
        assert_eq!(s.open, [false; 6], "leitura seguinte não reabre o que foi fechado");
        s.reveal("voice_transcription_model");
        assert_eq!(s.open, [true, false, false, false, false, false]);
        s.reveal("voice_style");
        assert_eq!(s.open, [true, false, false, false, false, false], "linha fora de seção não mexe em nada");
        s.open = [false; 6];
        s.reveal("voice_tune_speed");
        assert_eq!(s.open, [false, false, false, true, true, false], "o ajuste abre a ElevenLabs que o contém");
        s.reveal("voice_voice");
        s.reveal("voice_local_cmd");
        assert_eq!(s.open, [false, false, false, true, true, true]);
    }

    #[test]
    fn reader_sections_open_when_a_reader_appears_like_the_web() {
        let mut s = ServerConfig::default();
        s.fields.insert("elevenlabs_api_key".into(), json!({"valor": "", "definido": false}));
        s.fields.insert("tts_local_cmd".into(), json!({"valor": ""}));
        s.follow_readers();
        assert_eq!((s.open[3], s.open[5], s.can_read(), s.read_status()), (false, false, false, None));
        assert!(s.stage("tts_local_cmd", json!("kokoro")));
        assert_eq!((s.open[5], s.can_read(), s.read_status()), (true, true, Some("voice_status_local")), "o comando digitado já conta, sem Salvar");
        s.open[5] = false;
        assert!(s.stage("tts_local_cmd", json!("kokoro --pt")));
        assert!(!s.open[5], "fechada à mão, fica fechada enquanto o comando continua lá");
        s.fields.insert("elevenlabs_api_key".into(), json!({"valor": "sint••••••••wxyz", "definido": true, "origem": "app"}));
        s.follow_readers();
        assert_eq!((s.open[3], s.read_status()), (true, Some("voice_status_elevenlabs")), "a ElevenLabs vence o comando");
        assert!(s.stage("elevenlabs_api_key", Value::Null));
        assert_eq!(s.read_status(), Some("voice_status_local"), "chave saindo no Salvar já não conta");
    }

    #[test]
    fn tunes_fall_back_to_the_default_and_the_voice_name_to_the_id() {
        let mut s = ServerConfig::default();
        let speed = TUNES.iter().find(|t| t.key == "tts_speed").unwrap();
        assert_eq!(s.tune_value(speed), 100, "sem leitura, o padrão");
        s.fields.insert("tts_speed".into(), json!({"valor": 110, "origem": "app"}));
        assert_eq!(s.tune_value(speed), 110);
        assert!(s.stage("tts_speed", Value::Null));
        assert_eq!(s.tune_value(speed), 100, "voltar ao padrão mostra o padrão até o Salvar");
        assert_eq!(s.voice_name(), crate::i18n::tr("voice_machine_default"));
        s.fields.insert("elevenlabs_voice_id".into(), json!({"valor": "voz-sintetica-1"}));
        assert_eq!(s.voice_name(), "voz-sintetica-1", "sem a lista carregada, o id");
        s.voices.value = Some(Ok(vec![Voice { id: "voz-sintetica-1".into(), name: "Sintética".into() }]));
        assert_eq!(s.voice_name(), "Sintética");
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

    #[test]
    fn transcription_providers_edit_as_one_list_and_keep_the_stored_key() {
        let mut s = ServerConfig::default();
        assert!(!s.add_provider("a".into()), "sem leitura não há lista para editar");
        s.fields.insert("transcription_providers".into(), json!({"valor": [
            {"id": "a", "kind": "elevenlabs", "name": "", "base_url": "", "api_key": "sk_e••••1234", "model": ""}], "origem": "app"}));
        assert!(!s.provider_missing_key(), "lista do servidor, sem rascunho");
        assert!(s.add_provider("b".into()));
        assert!(!s.provider_missing_key(), "serviço compatível pode funcionar sem chave");
        assert!(s.edit_provider("b", "base_url", "https://api.groq.com/openai/v1".into()));
        assert!(s.edit_provider("b", "api_key", "gsk_nova".into()));
        assert!(!s.provider_missing_key());
        assert!(s.move_provider("b", true));
        assert!(!s.move_provider("b", true), "já é o primeiro");
        let ids: Vec<String> = s.providers().iter().map(|p| super::provider_text(p, "id").to_owned()).collect();
        assert_eq!(ids, ["b", "a"]);
        assert!(s.edit_provider("a", "api_key", "digitada".into()));
        assert!(s.edit_provider("a", "api_key", String::new()));
        assert_eq!(super::provider_text(&s.providers()[1], "api_key"), "sk_e••••1234", "apagar o digitado volta à máscara: o servidor mantém a chave");
        assert_eq!(super::provider_text(&s.providers()[0], "api_key"), "gsk_nova");
        assert_eq!(s.transcribe_status(), Some("voice_status_custom"));
        assert!(s.remove_provider("b") && s.remove_provider("a"));
        assert_eq!(s.draft.get("transcription_providers"), Some(&json!([])), "lista vazia volta ao serviço único");
        assert_eq!(s.transcribe_status(), None);
    }

    #[test]
    fn only_elevenlabs_requires_a_transcription_key() {
        let mut s = ServerConfig::default();
        s.fields.insert(super::PROVIDERS.into(), json!({"valor": []}));
        s.stage(super::PROVIDERS, json!([{"id":"local", "kind":"whisper_cpp", "api_key":""},
            {"id":"compatible", "kind":"openai", "api_key":""}]));
        assert!(!s.provider_missing_key());
        s.stage(super::PROVIDERS, json!([{"id":"eleven", "kind":"elevenlabs", "api_key":""}]));
        assert!(s.provider_missing_key());
    }

    #[test]
    fn changing_a_saved_providers_kind_or_endpoint_asks_for_the_key_again() {
        let mut s = ServerConfig::default();
        s.fields.insert("transcription_providers".into(), json!({"valor": [
            {"id": "a", "kind": "openai", "name": "", "base_url": "https://a/v1", "api_key": "gsk_••••1234", "model": ""}], "origem": "app"}));
        let key = |s: &ServerConfig| super::provider_text(&s.providers()[0], "api_key").to_owned();
        assert!(!s.edit_provider("a", "kind", "openai".into()), "o tipo já escolhido não suja o rascunho");
        assert!(!s.draft.contains_key("transcription_providers"));
        assert!(s.edit_provider("a", "kind", "elevenlabs".into()));
        assert_eq!(key(&s), "", "a máscara não vale para outro serviço");
        assert!(s.provider_missing_key(), "o Salvar espera a chave nova em vez do 400");
        assert!(s.edit_provider("a", "api_key", String::new()));
        assert_eq!(key(&s), "", "apagar não devolve a máscara de outro serviço");
        assert!(s.edit_provider("a", "kind", "openai".into()));
        assert_eq!(key(&s), "gsk_••••1234", "de volta ao salvo, a chave guardada volta a valer");
        assert!(s.edit_provider("a", "base_url", "https://b/v1".into()));
        assert_eq!(key(&s), "");
        assert!(s.edit_provider("a", "base_url", "https://a/v1 ".into()));
        assert_eq!(key(&s), "gsk_••••1234");
        assert!(s.edit_provider("a", "kind", "elevenlabs".into()));
        assert!(s.edit_provider("a", "api_key", "  ".into()));
        assert!(s.provider_missing_key(), "ElevenLabs exige uma chave que não seja só espaços");
    }

    #[test]
    fn provider_status_reads_the_wait_and_ignores_what_already_passed() {
        let parsed = super::parse_provider_status(&json!({"providers": [
            {"id": "a", "name": "ElevenLabs", "kind": "elevenlabs", "waiting_until": 2000.0, "reason": "cota esgotada"},
            {"id": "b", "name": "api.groq.com · whisper-large-v3-turbo", "kind": "openai", "waiting_until": null, "reason": null}]})).unwrap();
        assert_eq!(parsed.len(), 2);
        let mut s = ServerConfig::default();
        s.provider_status.value = Some(Ok(parsed));
        assert_eq!(s.provider_waiting("a", 1000.), Some((2000., Some("cota esgotada".into()))));
        assert_eq!(s.provider_waiting("a", 3000.), None, "o prazo passou");
        assert_eq!(s.provider_waiting("b", 1000.), None);
        assert!(super::parse_provider_status(&json!({"erro": 1})).is_err());
    }
}
