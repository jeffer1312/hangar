//! Contas e modelos: a lista única de credenciais do servidor (`GET /api/credenciais`) em três grupos,
//! com a cota de cada uma, e os modelos do Claude Code (`GET /api/engines`). O que a tela mostra é
//! montado na chegada da resposta e a cada meio minuto (o "há X" e os prazos envelhecem); o desenho só lê.
//! Renomear escreve daqui; entrar, sair, remover e adicionar conta Claude moram em `actions`; o formulário do modelo, a
//! chave de outro agente e o cookie do OpenCode, em `keys`; entrar no Codex, importar ou herdar e usar a redefinição, em `codex`.
mod actions;
mod codex;
mod keys;
mod usage;

use super::*;
use super::device::Remote;
use actions::{ActionReply, AddAccount, AddStep, Change, ChangeKind, SignIn};
use codex::{CodexFlow, CodexReply, ResetOffer, ResetTry};
use keys::{CookieForm, EngineForm, KeysReply};
pub(super) use keys::ModelChoice;
use super::settings::{Page, section_head, segments, settings_box};
use chrono::{Datelike, Local, TimeZone, Timelike};
use gpui_kit::component::menu::DropdownMenu;
use serde::Deserialize;

/// Leitura de cota mais velha que isto sai esmaecida e diz a idade (o backend relê a cada 5 min).
const STALE_AFTER: f64 = 600.;
/// Prazo de login a partir do qual a linha oferece renovar.
const RENEW_DAYS: i64 = 3;

#[derive(Clone, Deserialize)]
struct Credential {
    id: String,
    #[serde(rename = "tipo")] kind: String,
    auth_method: Option<String>,
    codex_account: Option<String>,
    codex_sync: Option<String>,
    #[serde(rename = "nome")] name: String,
    #[serde(rename = "nome_natural", default)] natural: String,
    #[serde(rename = "apelido")] alias: Option<String>,
    #[serde(rename = "ativa", default)] active: bool,
    login: Option<Login>,
    base_url: Option<String>,
    #[serde(rename = "chave_mascarada")] masked_key: Option<String>,
    #[serde(rename = "usos", default)] uses: Vec<String>,
    #[serde(rename = "cota")] quota: Option<Quota>,
    #[serde(rename = "aceita_cookie", default)] accepts_cookie: bool,
    #[serde(rename = "cookie_definido", default)] cookie_set: bool,
    #[serde(rename = "gerenciada")] managed: Option<bool>,
}

#[derive(Clone, Deserialize)]
struct Login {
    #[serde(rename = "estado")] state: String,
    #[serde(rename = "loggedIn")] logged_in: Option<bool>,
    email: Option<String>,
    #[serde(rename = "plano")] plan: Option<String>,
    #[serde(rename = "motivo")] reason: Option<String>,
    #[serde(rename = "refreshExpiresAt")] refresh_expires_at: Option<f64>,
}

#[derive(Clone, Deserialize)]
struct Quota {
    #[serde(rename = "estado")] state: String,
    #[serde(rename = "janelas", default)] windows: Vec<QuotaWindow>,
    #[serde(rename = "idade_s")] age: Option<f64>,
    #[serde(rename = "motivo")] reason: Option<String>,
    reset_credits: Option<ResetCredits>,
}

#[derive(Clone, Deserialize)]
struct QuotaWindow {
    #[serde(rename = "rotulo")] label: String,
    pct: f64,
    #[serde(rename = "reset_ts")] reset_at: Option<f64>,
    /// Janela de um modelo só (o rótulo é o nome do modelo).
    #[serde(default)] por_modelo: bool,
}

#[derive(Clone, Deserialize)]
/// `credits` pode vir ausente ou `null` (o web aceita os dois): a oferta sai de `available_count`.
struct ResetCredits { available_count: u64, #[serde(default)] credits: Option<Vec<ResetCredit>> }

#[derive(Clone, Deserialize)]
struct ResetCredit { id: String, expires_at: Option<f64>, status: String }

/// Um modelo do Claude Code (`engines.json`): o que a lista mostra e o que o formulário de edição abre. A chave nunca
/// vem, só se ela existe.
#[derive(Clone, Default, Deserialize)]
struct Engine {
    label: Option<String>,
    #[serde(default)] base_url: String,
    #[serde(default)] model: String,
    subagent_model: Option<String>,
    context_window: Option<u64>,
    vision: Option<bool>,
    bundled_skills: Option<bool>,
    experimental_betas: Option<bool>,
    prompt_caching: Option<bool>,
    adaptive_thinking: Option<bool>,
    tool_search: Option<bool>,
    gateway_model_discovery: Option<bool>,
    fine_grained_tool_streaming: Option<bool>,
    auth_via_api_key: Option<bool>,
    auto_compact_window: Option<u64>,
    max_output_tokens: Option<u64>,
    #[serde(default)] api_key_definida: bool,
    cliproxy_accounts: Option<Vec<crate::api::dto::CliProxyAccount>>,
    cliproxy_error: Option<String>,
}

pub(super) struct Engines { map: HashMap<String, Engine>, broken_file: Option<String> }

#[derive(Clone, Copy, PartialEq)]
enum Group { Subscriptions, Models, Others }

#[derive(Clone, Copy, PartialEq)]
enum Tone { Muted, Warn, Danger }

struct Bar { label: String, reset: String, pct: f64, reset_at: Option<f64>, per_model: bool }

enum QuotaView { Bars { bars: Vec<Bar>, stale: Option<String> }, Note(String), Nothing }

/// Uma linha pronta para desenhar.
struct Row {
    id: String,
    name: String,
    /// Nome no disco (`nome_natural`): é ele que as rotas de login e de saída esperam.
    label: String,
    natural: String,
    alias: String,
    active: bool,
    /// Provider do selo (cor da marca) e a letra dele.
    glyph: (&'static str, String),
    /// Tipo, login e o resto, separados por " · "; os trechos de aviso levam a cor deles.
    subtitle: (String, Vec<(std::ops::Range<usize>, Tone)>),
    chips: Vec<String>,
    /// Plano da assinatura ("Max", "Pro"), numa ficha ao lado do nome.
    plan: Option<String>,
    /// Dias até o login da conta Claude vencer, para o resumo do topo.
    login_days: Option<i64>,
    /// Redefinições guardadas da conta Codex, para o resumo do topo; `None` sem cota lida.
    resets: Option<u64>,
    quota: QuotaView,
    /// Conta Codex: o id dela e se a ação da linha é herdar da padrão (senão, entrar).
    codex: Option<(String, bool)>,
    reset: Option<ResetOffer>,
    /// Editar o modelo: `Some(false)` quando os detalhes dele não chegaram (a leitura dos modelos falhou).
    edit: Option<bool>,
    /// Cookie do painel do OpenCode: `Some(já guardado)` na credencial que aceita.
    cookie: Option<bool>,
    /// Entrar ou Renovar login, quando a conta Claude precisa.
    sign_in: Option<String>,
    /// Rota do POST de sair, conforme o tipo.
    sign_out: Option<Vec<String>>,
    /// Rota do DELETE conforme o tipo, o prazo dele e o texto da confirmação.
    remove: Option<(Vec<String>, u64, &'static str)>,
}

struct Section { group: Group, rows: Vec<Row>, labels: Vec<String> }

struct Rename { id: String, input: Entity<InputState>, saving: bool, error: Option<String>, seq: u64, _subscription: Subscription }

#[derive(Default)]
pub(super) struct Accounts {
    list: Remote<Vec<Credential>>,
    /// Contas da máquina da sessão aberta quando ela é de outro servidor, e de qual servidor vieram.
    session_list: Remote<Vec<Credential>>,
    session_server: Option<String>,
    session_engines: Remote<Engines>,
    engine_server: Option<String>,
    engines: Remote<Engines>,
    sections: Vec<Section>,
    /// Quando a lista na tela foi lida pela última vez.
    read_at: Option<Instant>,
    /// Falha da última releitura da lista, com a lista anterior ainda na tela; some com a próxima leitura boa.
    notice: Option<String>,
    /// O mesmo para os modelos: cada leitura apaga só o próprio aviso.
    engines_notice: Option<String>,
    rename: Option<Rename>,
    rename_seq: u64,
    clock: bool,
    sign_in: Option<SignIn>,
    login_attempt: u64,
    change: Option<Change>,
    /// Resultado da última saída, remoção ou conta criada; some com a próxima ação ou quando a página reabre.
    outcome: Option<(String, bool)>,
    add: Option<Entity<AddAccount>>,
    /// Formulário do modelo ou da chave aberto no lugar da lista.
    form: Option<EngineForm>,
    /// Cookie do OpenCode sendo digitado (abaixo da linha) e o que está sendo apagado.
    cookie: Option<CookieForm>,
    cookie_clearing: Option<String>,
    /// Número de cada pedido do formulário, do cookie e do painel do Codex: resposta de outro pedido não mexe no atual.
    keys_seq: u64,
    codex: Option<CodexFlow>,
    reset: Option<ResetTry>,
    /// Leitura da lista pedida depois de uma redefinição (número dela, se a redefinição valeu): a falha dela entra no aviso.
    reset_refresh: Option<(u64, bool)>,
    /// Cartão de contas aberto pelo anel de uso do compositor.
    pub(super) card: bool,
    /// O cartão aberto veio da pílula da barra do topo: preso a ela, com a conta padrão do Claude em vez da da sessão.
    pub(super) card_top: bool,
}

pub(super) enum AccountsReply {
    List(u64, Result<Value, Failure>),
    Engines(u64, Result<Value, Failure>),
    /// Lista da máquina da sessão aberta (outro servidor): número do pedido e o servidor dela.
    SessionList(u64, String, Result<Value, Failure>),
    SessionEngines(u64, String, Result<Value, Failure>),
    Renamed(u64, Result<Value, Failure>),
    Action(ActionReply),
    Keys(KeysReply),
    Codex(CodexReply),
}

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.) }

/// "1 min", "12 min", "3 h", "2 d": um minuto é o piso, como no web.
fn age(seconds: f64) -> String {
    match seconds.max(0.) {
        s if s < 3600. => format!("{} min", (s / 60.).floor().max(1.)),
        s if s < 86_400. => format!("{} h", (s / 3600.).floor()),
        s => format!("{} d", (s / 86_400.).floor()),
    }
}

/// Até o reinício: na janela curta, quanto falta ("1h20", "35m"); passando de um dia, o dia ("sáb 27/09 15h").
pub(super) fn reset_text(at: Option<f64>, now: f64) -> String {
    let Some(at) = at.filter(|at| at.is_finite() && *at > now) else { return String::new() };
    let left = at - now;
    if left > 86_400. {
        let Some(day) = Local.timestamp_opt(at as i64, 0).single() else { return String::new() };
        let names = if crate::i18n::english() { ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] }
            else { ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"] };
        return format!("{} {:02}/{:02} {}h", names[day.weekday().num_days_from_sunday() as usize], day.day(), day.month(), day.hour());
    }
    let minutes = (left / 60.).floor() as u64;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m:02}"),
    }
}

fn window_label(label: &str) -> String {
    match label { "5h" => tr("accounts_window_5h"), "7d" => tr("accounts_window_7d"), other => other.to_owned() }
}

fn window_order(label: &str) -> u8 { match label { "5h" => 0, "7d" => 1, _ => 2 } }

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or("")
}

impl Credential {
    fn auth(&self) -> &str { self.auth_method.as_deref().unwrap_or(if self.kind == "claude" { "oauth" } else { "unknown" }) }
    fn engine_name(&self) -> Option<&str> { self.id.strip_prefix("chave:") }
    fn logged_in(&self) -> Option<bool> { self.login.as_ref().filter(|l| l.state == "ok").and_then(|l| l.logged_in) }
    fn expired(&self) -> bool { self.quota.as_ref().is_some_and(|q| q.state == "expirada") }
    /// Dias até o login vencer (arredonda para cima, como o CLI). Sem prazo no arquivo, nada.
    fn login_days(&self, now: f64) -> Option<i64> {
        let login = self.login.as_ref().filter(|l| l.logged_in == Some(true))?;
        login.refresh_expires_at.map(|at| ((at - now) / 86_400.).ceil() as i64)
    }
    fn read_windows(&self) -> Option<&Vec<QuotaWindow>> {
        self.quota.as_ref().filter(|q| q.state == "lida" && !q.windows.is_empty()).map(|q| &q.windows)
    }
}

/// Nomes dos modelos do Claude Code. A própria credencial diz que roda o Claude Code: sem isso, uma falha ao ler
/// os modelos jogaria cada um deles em "Outros agentes", o grupo que diz que a chave não vale para o Claude Code.
fn model_names<'a>(list: &'a [Credential], engines: &'a HashMap<String, Engine>) -> HashSet<&'a str> {
    list.iter().filter(|c| c.uses.iter().any(|u| u == "claude_code")).filter_map(Credential::engine_name)
        .chain(engines.keys().map(String::as_str)).collect()
}

/// Em que grupo a credencial entra; `None` é a cópia que o app gravou no Kimi de um modelo já listado.
fn group_of(c: &Credential, models: &HashSet<&str>) -> Option<Group> {
    if c.id.strip_prefix("kimi:").is_some_and(|name| models.contains(name)) { return None; }
    if c.auth() == "oauth" || (c.kind == "codex" && c.auth() == "none") { return Some(Group::Subscriptions); }
    if c.engine_name().is_some_and(|name| models.contains(name)) { return Some(Group::Models); }
    Some(Group::Others)
}

fn build_row(c: &Credential, engines: &HashMap<String, Engine>, has_kimi_copy: bool, now: f64) -> Row {
    let engine = c.engine_name().and_then(|name| engines.get(name));
    let logged = c.logged_in();
    let muted = |text: String| (text, Tone::Muted);
    let mut subtitle = Vec::new();
    let mut chips = Vec::new();
    let mut codex = None;
    let mut edit = None;
    let mut sign_in = None;
    let initial = c.name.chars().find(|ch| ch.is_alphanumeric()).map(|ch| ch.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    let glyph = match c.kind.as_str() {
        "claude" => ("claude", "C".into()),
        "codex" => ("codex", "X".into()),
        _ if c.base_url.as_deref().is_some_and(|u| u.contains("kimi")) || c.id.starts_with("kimi:") => ("kimi", initial),
        _ => ("", initial),
    };
    match c.kind.as_str() {
        "claude" => {
            subtitle.push(muted("Claude".into()));
            match (logged, c.login_days(now)) {
                (Some(false), _) => subtitle.push(muted(tr("accounts_not_connected"))),
                (Some(true), Some(days)) if days <= 0 => subtitle.push((tr("accounts_login_expired"), Tone::Danger)),
                (Some(true), Some(days)) => subtitle.push((tr("accounts_login_expires").replace("{n}", &days.to_string()),
                    if days <= RENEW_DAYS { Tone::Warn } else { Tone::Muted })),
                _ => {}
            }
            if let Some(email) = c.login.as_ref().filter(|_| logged == Some(true)).and_then(|l| l.email.clone()) { subtitle.push(muted(email)); }
            if logged == Some(false) || c.expired() { sign_in = Some(tr("accounts_sign_in")); }
            else if logged == Some(true) && c.login_days(now).is_some_and(|d| d <= RENEW_DAYS) { sign_in = Some(tr("accounts_renew")); }
        }
        "codex" => {
            subtitle.push(muted("Codex".into()));
            let cli_missing = c.login.as_ref().and_then(|l| l.reason.as_deref()) == Some("cli-ausente");
            let auth = match c.auth() {
                "oauth" => tr("accounts_codex_oauth"),
                "api_key" => tr("accounts_api_key"),
                _ if cli_missing => tr("accounts_codex_cli_missing"),
                _ => String::new(),
            };
            if !auth.is_empty() { subtitle.push(muted(auth)); }
            if let Some(login) = c.login.as_ref().filter(|_| logged == Some(true)) {
                subtitle.extend(login.email.clone().map(muted));
            } else if logged == Some(false) {
                subtitle.push(muted(tr("accounts_not_connected")));
            }
            if !c.active && let Some(sync) = c.codex_sync.as_deref() {
                subtitle.push(muted(tr(match sync { "ready" => "accounts_codex_inherited", "running" => "accounts_codex_preparing",
                    "idle" => "accounts_codex_not_inherited", _ => "accounts_codex_inherit_failed" })));
            }
            // Como no web: sem login, Entrar; logada e sem nunca ter herdado, Herdar ("Depois" do login desembarca aqui).
            codex = c.codex_account.clone().and_then(|id| match (c.auth(), c.codex_sync.as_deref(), logged) {
                ("none", _, _) => Some((id, false)),
                (_, Some("idle"), Some(true)) => Some((id, true)),
                _ => None,
            });
        }
        _ => match engine {
            // Como no mock: só o que foge do padrão vira ficha; o endereço e o resto ficam no Editar.
            Some(engine) => {
                if has_kimi_copy { subtitle.push(muted(tr("accounts_kimi_copy"))); }
                if !engine.model.is_empty() { chips.push(engine.model.clone()); }
                chips.extend(engine.context_window.map(|n| format!("{}k", (n as f64 / 1000.).round())));
                chips.extend(engine.subagent_model.as_deref().filter(|m| !m.is_empty()).map(|m| tr("accounts_chip_subagent").replace("{id}", m)));
                if engine.adaptive_thinking != Some(false) { chips.push(tr("accounts_chip_thinking_on")); }
                edit = Some(true);
            }
            None => {
                subtitle.push(muted(tr("accounts_api_key")));
                if let Some(url) = &c.base_url { subtitle.push(muted(host(url).to_owned())); }
                subtitle.extend(c.masked_key.clone().map(muted));
                if c.cookie_set { subtitle.push(muted(tr("accounts_cookie_set"))); }
                if c.managed == Some(false) { subtitle.push(muted(tr("accounts_agent_key"))); }
                // Modelo do Claude Code cujos detalhes não chegaram: o Editar fica, desligado dizendo por quê. Abrir o
                // formulário em branco e salvar apagaria o que o disco tem.
                if c.engine_name().is_some() && c.uses.iter().any(|u| u == "claude_code") { edit = Some(false); }
            }
        },
    }
    subtitle.retain(|(text, _)| !text.is_empty());
    let mut line = String::new();
    let mut marks = Vec::new();
    for (text, tone) in subtitle {
        if !line.is_empty() { line.push_str(" · "); }
        if tone != Tone::Muted { marks.push((line.len()..line.len() + text.len(), tone)); }
        line.push_str(&text);
    }
    let subtitle = (line, marks);

    let quota = match (c.read_windows(), c.quota.as_ref()) {
        (Some(windows), Some(q)) => {
            let mut bars: Vec<Bar> = windows.iter().map(|w| Bar {
                label: w.label.clone(), reset: reset_text(w.reset_at, now), pct: w.pct, reset_at: w.reset_at, per_model: w.por_modelo,
            }).collect();
            bars.sort_by_key(|b| window_order(&b.label));
            let stale = q.age.filter(|a| *a > STALE_AFTER).map(|a| tr("accounts_last_read").replace("{n}", &age(a)));
            QuotaView::Bars { bars, stale }
        }
        _ if c.login.as_ref().and_then(|l| l.reason.as_deref()) == Some("cli-ausente") => QuotaView::Nothing,
        (_, Some(q)) if q.state == "expirada" || q.state == "sem_credencial" => QuotaView::Note(tr(match q.reason.as_deref() {
            Some("sessao-viva") => "accounts_quota_live_session",
            Some("renovacao-falhou") => "accounts_quota_open_session",
            _ => "accounts_quota_sign_in",
        })),
        _ => QuotaView::Note(tr("accounts_quota_none")),
    };
    let codex_extra = c.kind == "codex" && c.codex_account.is_some() && !c.active;
    let can_remove = codex_extra || engine.is_some() || (c.kind != "codex" && c.managed != Some(false));
    // Cada tipo tem a rota de sempre, com o nome no disco (como no web).
    let remove = can_remove.then(|| match (c.id.strip_prefix("kimi:"), c.kind.as_str()) {
        (Some(name), _) => (vec!["credenciais".into(), "kimi".into(), name.into()], 30, "accounts_remove_desc_key"),
        (_, "chave") => (vec!["engines".into(), c.engine_name().unwrap_or(&c.natural).into()], 30,
            if engine.is_some() { "accounts_remove_desc_model" } else { "accounts_remove_desc_key" }),
        (_, "codex") => (vec!["codex-contas".into(), c.codex_account.clone().unwrap_or_default()], 120, "accounts_remove_desc_codex"),
        _ => (vec!["claude-configs".into(), c.natural.clone()], 60, "accounts_remove_desc_claude"),
    });
    let plan = c.login.as_ref().filter(|_| logged == Some(true)).and_then(|l| l.plan.as_deref()).filter(|p| !p.is_empty())
        .map(|p| { let mut chars = p.chars(); chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default() });
    // Codex sem cota lida não diz quantas guardou: `None`, não zero.
    let resets = match c.kind.as_str() {
        "codex" => c.read_windows().map(|_| c.quota.as_ref().and_then(|q| q.reset_credits.as_ref()).map_or(0, |r| r.available_count)),
        _ => Some(0),
    };
    Row {
        id: c.id.clone(), name: c.name.clone(), label: c.natural.clone(), natural: c.natural.clone(), alias: c.alias.clone().unwrap_or_default(),
        active: c.active, glyph, subtitle, chips, plan, login_days: if c.kind == "claude" { c.login_days(now) } else { None }, resets, quota, codex, reset: codex::reset_offer(c, now), edit, cookie: c.accepts_cookie.then_some(c.cookie_set), sign_in,
        sign_out: match c.kind.as_str() {
            "claude" if c.managed != Some(false) && logged == Some(true) && !c.expired() =>
                Some(vec!["claude-configs".into(), c.natural.clone(), "logout".into()]),
            "codex" if logged == Some(true) => c.codex_account.clone().map(|id| vec!["codex-contas".into(), id, "logout".into()]),
            _ => None,
        },
        remove,
    }
}

/// Os três grupos, sempre presentes depois da leitura: um grupo que some esconde onde a coisa nova vai aparecer.
fn build_sections(list: &[Credential], engines: &HashMap<String, Engine>, now: f64) -> Vec<Section> {
    let models = model_names(list, engines);
    [Group::Subscriptions, Group::Models, Group::Others].into_iter().map(|group| {
        let rows: Vec<Row> = list.iter().filter(|c| group_of(c, &models) == Some(group)).map(|c| {
            let copy = c.engine_name().is_some_and(|name| list.iter().any(|x| x.id == format!("kimi:{name}")));
            build_row(c, engines, copy, now)
        }).collect();
        // Uma coluna por janela na lista compacta, a mesma em toda linha: 5h e semana primeiro.
        let mut labels: Vec<String> = Vec::new();
        for row in &rows {
            if let QuotaView::Bars { bars, .. } = &row.quota {
                for bar in bars { if !labels.contains(&bar.label) { labels.push(bar.label.clone()); } }
            }
        }
        labels.sort_by_key(|l| window_order(l));
        Section { group, rows, labels }
    }).collect()
}

/// O resumo do topo, tirado das próprias linhas.
#[derive(Default)]
struct Summary {
    /// Provider e nome de cada conta em uso.
    in_use: Vec<(&'static str, String)>,
    /// Contas com a semana em 100%: nome e reinício, a que volta primeiro na frente.
    week_full: Vec<(String, String)>,
    /// Menor prazo de login e as contas que vencem nele.
    login: Option<(i64, Vec<String>)>,
    /// Alguma conta contada no limite vem de leitura antiga.
    week_stale: bool,
    resets: u64,
    /// Alguma conta Codex sem cota lida: o total pode ser maior.
    resets_unread: bool,
}

fn build_summary(sections: &[Section]) -> Summary {
    let mut summary = Summary::default();
    let mut full = Vec::new();
    for row in sections.iter().flat_map(|s| &s.rows) {
        if row.active { summary.in_use.push((row.glyph.0, row.name.clone())); }
        if let QuotaView::Bars { bars, stale } = &row.quota && let Some(week) = bars.iter().find(|b| b.label == "7d" && b.pct >= 100.) {
            full.push((week.reset_at.unwrap_or(f64::MAX), row.name.clone(), week.reset.clone()));
            summary.week_stale |= stale.is_some();
        }
        if let Some(days) = row.login_days {
            match &mut summary.login {
                Some((least, names)) if days == *least => names.push(row.name.clone()),
                Some((least, _)) if days > *least => {}
                _ => summary.login = Some((days, vec![row.name.clone()])),
            }
        }
        match row.resets { Some(n) => summary.resets += n, None => summary.resets_unread = true }
    }
    full.sort_by(|a, b| a.0.total_cmp(&b.0));
    summary.week_full = full.into_iter().map(|(_, name, reset)| (name, reset)).collect();
    summary
}

fn parse_engines(value: &Value) -> Result<Engines, String> {
    let map = value.get("motores").cloned().map(serde_json::from_value::<HashMap<String, Engine>>).transpose()
        .map_err(|_| tr("invalid_response"))?.unwrap_or_default();
    let broken = value.get("arquivo_corrompido").and_then(Value::as_bool) == Some(true);
    Ok(Engines { map, broken_file: broken.then(|| value.get("arquivo_caminho").and_then(Value::as_str).unwrap_or("engines.json").to_owned()) })
}

fn proxy_account_targets(session: &SessionInfo, list: &[Credential], engine: &Engine) -> Vec<super::sidebar::AccountTarget> {
    if engine.cliproxy_error.is_some() { return Vec::new(); }
    engine.cliproxy_accounts.as_deref().unwrap_or_default().iter().filter_map(|a| {
        let c = list.iter().find(|c| c.kind == "codex" && c.id == a.credential_id && c.id.starts_with("codex:")
            && c.codex_account.as_deref() == Some(a.account.as_str()) && c.logged_in() == Some(true))?;
        if session.engine_account.as_deref() == Some(a.account.as_str()) && session.conta.as_deref() == Some(c.id.as_str()) { return None; }
        let pct = c.read_windows().and_then(|windows| windows.iter().filter(|w| matches!(w.label.as_str(), "5h" | "7d")
            && w.reset_at.is_none_or(|at| at > now())).map(|w| w.pct).reduce(f64::max));
        let label = c.alias.clone().filter(|a| !a.is_empty()).unwrap_or_else(||
            if a.label.is_empty() { a.email.clone() } else { a.label.clone() });
        Some(super::sidebar::AccountTarget { path: String::new(), label, pct, low: pct.is_some_and(|p| p >= 95.),
            full: pct.is_some_and(|p| p >= 99.), engine_account: Some(a.account.clone()) })
    }).collect()
}

pub(super) fn session_proxy_targets(session: &SessionInfo, credentials: Value, engines: Value) -> Result<Option<Vec<super::sidebar::AccountTarget>>, String> {
    let list: Vec<Credential> = serde_json::from_value(credentials).map_err(|_| tr("invalid_response"))?;
    let engines = parse_engines(&engines)?;
    let engine = session.engine.as_deref().and_then(|name| engines.map.get(name)).ok_or_else(|| tr("create_proxy_no_accounts"))?;
    if let Some(error) = &engine.cliproxy_error { return Err(tr("create_proxy_error").replace("{reason}", error)); }
    if engine.cliproxy_accounts.is_none() { return Ok(None); }
    Ok(Some(proxy_account_targets(session, &list, engine)))
}

/// Cor da barra pelo quanto já foi usado.
fn level(pct: f64) -> Hsla { if pct > 90. { theme::danger() } else if pct > 80. { theme::warning() } else { theme::accent() } }

impl Hangar {
    /// Página aberta: lê do servidor (a lista anterior fica na tela enquanto chega) e liga o relógio do "há X".
    pub(super) fn accounts_opened(&mut self, cx: &mut Context<Self>) {
        self.accounts.outcome = None;
        if !self.accounts.list.loading { self.load_accounts(false, cx); }
        if !self.accounts.engines.loading { self.load_engines(cx); }
        if self.accounts.clock { return; }
        self.accounts.clock = true;
        // A troca de servidor zera as contas e liga outro relógio: o desta conexão para ali.
        let connection = self.connection;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            let open = this.update(cx, |this, cx| {
                if this.connection != connection { return false; }
                let open = this.settings == Some(Page::Accounts);
                if open { this.rebuild_accounts(); cx.notify(); } else { this.accounts.clock = false; }
                open
            });
            if !matches!(open, Ok(true)) { break; }
        }).detach();
    }

    fn accounts_send_later(&self) -> impl Fn(AccountsReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Accounts(reply) }).await; })
        }
    }

    /// `force`: o botão Atualizar, que pede a cota de agora em vez da guardada há até 5 min.
    fn load_accounts(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.accounts.list.start();
        let done = self.accounts_send_later();
        self.runtime.spawn(async move {
            let query: &[(&str, &str)] = if force { &[("forcar", "true")] } else { &[] };
            done(AccountsReply::List(seq, api.server_read(&["credenciais"], query, 30).await)).await
        });
        cx.notify();
    }

    /// Contas da máquina da sessão aberta: a lista do servidor ativo, ou a lida da outra máquina quando a sessão é de lá.
    pub(super) fn load_session_accounts(&mut self, cx: &mut Context<Self>) {
        if self.selected.as_ref().is_some_and(|s| s.engine.as_deref().is_some_and(|e| !e.is_empty()) || s.uses_engine_account()) {
            self.load_session_engines(cx);
        }
        let Some(api) = self.open_api.clone() else {
            if !self.accounts.list.loading { self.load_accounts(false, cx); }
            return;
        };
        let server = self.open_server();
        // Outra máquina: a lista da anterior não pode aparecer como se fosse desta enquanto a nova não chega.
        if self.accounts.session_server.as_deref() != Some(server.as_str()) {
            // `reset`, não `default`: o número do pedido segue subindo, e a resposta de uma ida anterior a esta máquina
            // não passa pela nova.
            self.accounts.session_list.reset();
            self.accounts.session_server = Some(server.clone());
        } else if self.accounts.session_list.loading { return; }
        let seq = self.accounts.session_list.start();
        let done = self.accounts_send_later();
        self.runtime.spawn(async move { done(AccountsReply::SessionList(seq, server, api.server_read(&["credenciais"], &[], 30).await)).await });
        cx.notify();
    }

    /// A lista que o cartão de contas e a pílula do topo leem: a da máquina da sessão aberta.
    pub(super) fn session_accounts(&self) -> &Remote<Vec<Credential>> {
        if self.open_api.is_some() { &self.accounts.session_list } else { &self.accounts.list }
    }

    fn load_session_engines(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.open_api.clone().or_else(|| self.api.clone()) else { return; };
        let server = self.open_server();
        if self.accounts.engine_server.as_deref() != Some(server.as_str()) {
            self.accounts.session_engines.reset();
            self.accounts.engine_server = Some(server.clone());
        } else if self.accounts.session_engines.loading { return; }
        let seq = self.accounts.session_engines.start();
        let done = self.accounts_send_later();
        self.runtime.spawn(async move { done(AccountsReply::SessionEngines(seq, server, api.server_read(&["engines"], &[], 15).await)).await; });
        cx.notify();
    }

    fn session_engine(&self) -> Option<&Engine> {
        if self.accounts.engine_server.as_deref() != Some(self.open_server().as_str()) { return None; }
        let engine = self.selected.as_ref().filter(|s| s.provider == "claude")?.engine.as_deref()?;
        self.accounts.session_engines.ok()?.map.get(engine)
    }

    fn load_engines(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.accounts.engines.start();
        let done = self.accounts_send_later();
        self.runtime.spawn(async move { done(AccountsReply::Engines(seq, api.server_read(&["engines"], &[], 15).await)).await });
        cx.notify();
    }

    fn refresh_accounts(&mut self, cx: &mut Context<Self>) {
        if self.accounts.list.loading { return; }
        // A falha da vez anterior sai quando se tenta de novo; a desta volta com a resposta.
        self.accounts.notice = None;
        self.load_accounts(true, cx);
        if !self.accounts.engines.loading {
            self.accounts.engines_notice = None;
            self.load_engines(cx);
        }
    }

    /// Remonta as linhas: na chegada de uma resposta, na troca de idioma e a cada meio minuto.
    pub(super) fn rebuild_accounts(&mut self) {
        let accounts = &mut self.accounts;
        let empty = HashMap::new();
        let engines = accounts.engines.ok().map_or(&empty, |e| &e.map);
        accounts.sections = accounts.list.ok().map(|list| build_sections(list, engines, now())).unwrap_or_default();
    }

    fn start_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        // Um nome sendo salvo segura o campo: trocá-lo perderia a resposta, inclusive a recusa.
        if self.accounts.rename.as_ref().is_some_and(|r| r.saving) { return; }
        let Some(row) = self.accounts.sections.iter().flat_map(|s| &s.rows).find(|r| r.id == id) else { return };
        // O texto de fundo é o nome original: é ele que volta quando o campo fica vazio.
        let (alias, natural) = (row.alias.clone(), row.natural.clone());
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(natural));
        input.update(cx, |state, cx| { state.set_value(alias, window, cx); state.focus(window, cx); });
        let subscription = cx.subscribe_in(&input, window, |this: &mut Hangar, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event { this.save_rename(window, cx); }
        });
        self.accounts.rename = Some(Rename { id, input, saving: false, error: None, seq: 0, _subscription: subscription });
        cx.notify();
    }

    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts.rename.as_ref().is_some_and(|r| r.saving) { return; }
        self.accounts.rename = None;
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    /// Apelido vazio devolve o nome original: é o que o servidor faz com ele.
    fn save_rename(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.accounts.rename_seq += 1;
        let seq = self.accounts.rename_seq;
        let Some(rename) = self.accounts.rename.as_mut().filter(|r| !r.saving) else { return };
        let alias = rename.input.read(cx).value().trim().to_owned();
        (rename.saving, rename.error, rename.seq) = (true, None, seq);
        let body = json!({"id": rename.id, "apelido": alias});
        let done = self.accounts_send_later();
        self.runtime.spawn(async move {
            done(AccountsReply::Renamed(seq, api.server_send(reqwest::Method::PUT, &["credenciais", "apelido"], Some(body), 20).await)).await
        });
        cx.notify();
    }

    pub(super) fn receive_accounts(&mut self, reply: AccountsReply, window: &mut Window, cx: &mut Context<Self>) {
        let accounts = &mut self.accounts;
        match reply {
            AccountsReply::List(seq, result) => {
                // A lista relida depois de uma redefinição não veio: o aviso diz que o que está na tela é anterior a ela.
                if let Some((_, applied)) = accounts.reset_refresh.take_if(|(s, _)| *s == seq)
                    && !matches!(&result, Ok(list) if Vec::<Credential>::deserialize(list).is_ok())
                    && let Some((text, error)) = accounts.outcome.as_mut() {
                    *text = format!("{text} {}", tr(if applied { "accounts_reset_refresh_failed" } else { "accounts_reset_refresh_failed_neutral" }));
                    *error = true;
                }
                match result.map(serde_json::from_value::<Vec<Credential>>) {
                // `finish` só aceita o último pedido: resposta atrasada não apaga a falha de uma releitura mais nova.
                Ok(Ok(list)) => if accounts.list.finish(seq, Ok(list)) { (accounts.read_at, accounts.notice) = (Some(Instant::now()), None); },
                // Falhou com a lista anterior na tela: ela fica, e a falha aparece acima dela.
                failed if accounts.list.ok().is_some() => if seq == accounts.list.seq {
                    accounts.list.loading = false;
                    accounts.notice = Some(match failed { Err(e) => Self::failure(&e), Ok(_) => tr("invalid_response") });
                },
                Err(e) => { accounts.list.finish(seq, Err(Self::failure(&e))); }
                Ok(Err(_)) => { accounts.list.finish(seq, Err(tr("invalid_response"))); }
                }
            }
            AccountsReply::SessionList(seq, server, result) => {
                if accounts.session_server.as_deref() == Some(server.as_str()) {
                    accounts.session_list.finish(seq, result.map_err(|e| Self::failure(&e))
                        .and_then(|v| serde_json::from_value::<Vec<Credential>>(v).map_err(|_| tr("invalid_response"))));
                }
            }
            AccountsReply::SessionEngines(seq, server, result) => {
                if accounts.engine_server.as_deref() == Some(server.as_str()) {
                    accounts.session_engines.finish(seq, result.map_err(|e| Self::failure(&e)).and_then(|v| parse_engines(&v)));
                }
            }
            AccountsReply::Engines(seq, result) => {
                match result.map_err(|e| Self::failure(&e)).and_then(|v| parse_engines(&v)) {
                    // Releitura que falhou com os modelos já na tela: eles ficam, e a falha aparece como a da lista.
                    Err(error) if accounts.engines.ok().is_some() => if seq == accounts.engines.seq {
                        accounts.engines.loading = false;
                        accounts.engines_notice = Some(error);
                    },
                    parsed => if accounts.engines.finish(seq, parsed) { accounts.engines_notice = None; },
                }
                // O formulário de criação aberto antes da lista chegar refaz o "nome em uso" com ela.
                self.refresh_engine_form(cx);
            }
            AccountsReply::Renamed(seq, result) => {
                let Some(rename) = accounts.rename.as_mut().filter(|r| r.seq == seq) else { return };
                match result {
                    Ok(_) => { accounts.rename = None; self.root_focus.focus(window, cx); }
                    Err(error) => (rename.saving, rename.error) = (false, Some(if error.uncertain { tr("accounts_rename_uncertain") } else { Self::failure(&error) })),
                }
                // Deu certo, falhou ou ficou incerto: a lista relida mostra o nome que o servidor guardou.
                self.load_accounts(false, cx);
            }
            AccountsReply::Action(reply) => self.receive_action(reply, window, cx),
            AccountsReply::Keys(reply) => self.receive_keys(reply, window, cx),
            AccountsReply::Codex(reply) => self.receive_codex(reply, window, cx),
        }
        self.rebuild_accounts();
        cx.notify();
    }

    /// `wide`: a janela tem largura para uma coluna por janela de cota; sem ela, a linha empilha as barras como antes.
    pub(super) fn render_accounts(&mut self, wide: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let server = self.server_label(cx);
        let chip = div().mt(px(12.)).flex().child(div().h(px(24.)).px(px(9.)).flex().items_center().gap(px(6.)).rounded_full().border_1()
            .border_color(theme::border_strong()).text_size(px(12.5)).text_color(theme::muted())
            .child(chrome::small_icon(IconName::Server, 13., theme::muted())).child(tr("accounts_this_server").replace("{server}", &server)));
        let intro = div().flex_1().min_w(px(280.)).flex().flex_col().child(self.page_top("settings_page_accounts", tr("accounts_lead"))).child(chip);
        let top = |tools: Option<Div>| div().flex().flex_wrap().items_end().gap(px(16.)).child(intro).children(tools);
        let accounts = &self.accounts;
        if self.api.is_none() {
            return div().flex().flex_col().child(top(None)).child(self.heading("accounts_subscriptions"))
                .child(settings_box().child(div().px_4().py(px(18.)).text_size(px(13.)).text_color(theme::muted()).child(tr("settings_offline"))))
                .into_any_element();
        }
        let page = div().flex().flex_col();
        if let Some(panel) = self.render_sign_in(cx) { return page.child(top(None)).child(panel).into_any_element(); }
        if let Some(panel) = self.render_engine_form(cx) { return page.child(top(None)).child(panel).into_any_element(); }
        if let Some(panel) = self.render_codex(cx) { return page.child(top(None)).child(panel).into_any_element(); }
        match (&accounts.list.value, accounts.list.loading) {
            (None, _) => return page.child(top(None)).child(settings_box().mt(px(20.))
                .child(div().px_4().py(px(18.)).flex().items_center().gap(px(10.)).text_size(px(13.)).text_color(theme::muted())
                    .child(chrome::Spinner::new("accounts-loading", IconName::LoaderCircle, px(15.), theme::accent())).child(tr("accounts_loading"))))
                .into_any_element(),
            (Some(Err(error)), loading) => {
                let retry = Button::new("accounts-retry").outline().small().icon(IconName::RefreshCw)
                    .label(tr(if loading { "accounts_loading_short" } else { "accounts_retry" })).disabled(loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load_accounts(false, cx)));
                return page.child(top(None))
                    .child(banner(theme::danger(), IconName::CircleAlert, tr("accounts_failed").replace("{reason}", error)).mt(px(20.))
                        .child(div().flex_shrink_0().child(retry)))
                    .into_any_element();
            }
            _ => {}
        }
        let compact = appearance::get().accounts_compact;
        let mut page = page.child(top(Some(self.accounts_tools(compact, cx))));
        if let Some((text, error)) = &accounts.outcome {
            let (color, icon) = if *error { (theme::danger(), IconName::CircleAlert) } else { (theme::success(), IconName::CircleCheck) };
            page = page.child(banner(color, icon, text.clone()).mt(px(16.)));
        }
        // Arquivo dos modelos ilegível não é "nenhum modelo": pode estar escondendo modelos de verdade.
        let engines_problem = match &accounts.engines.value {
            Some(Ok(Engines { broken_file: Some(path), .. })) => Some(tr("accounts_engines_broken").replace("{path}", path)),
            Some(Err(error)) => Some(tr("accounts_engines_failed").replace("{reason}", error)),
            // Releitura que falhou com os modelos anteriores na tela.
            _ => accounts.engines_notice.as_ref().map(|error| tr("accounts_refresh_failed").replace("{reason}", error)),
        };
        page = page.child(self.accounts_summary(&build_summary(&accounts.sections))).child(self.accounts_tabs(cx));
        let mut order = 0;
        for section in &accounts.sections {
            let problem = match section.group {
                Group::Subscriptions => accounts.notice.as_ref().map(|notice| tr("accounts_refresh_failed").replace("{reason}", notice)),
                Group::Models => engines_problem.clone(),
                Group::Others => None,
            };
            page = page.child(self.render_accounts_section(section, problem, wide, compact, &mut order, window, cx));
        }
        page.child(div().mt(px(14.)).flex().items_center().gap(px(8.)).text_size(px(12.5)).text_color(theme::muted())
            .child(chrome::small_icon(IconName::Ellipsis, 14., theme::faint())).child(tr("accounts_menu_note")))
            .into_any_element()
    }

    /// Atualizar com a idade da leitura, Completa × Compacta e Adicionar conta, no topo da página.
    fn accounts_tools(&self, compact: bool, cx: &mut Context<Self>) -> Div {
        let density = segments("accounts-density", &[tr("accounts_full"), tr("accounts_compact")], compact as usize, 2, false, String::new(),
            |this: &mut Hangar, index, _: &mut Window, cx| {
                let mut next = appearance::get();
                next.accounts_compact = index == 1;
                this.apply_appearance(next, true, cx);
            }, cx);
        let loading = self.accounts.list.loading;
        let label = if loading { tr("accounts_refreshing") }
            else { self.accounts.read_at.map(|at| tr("accounts_read_ago").replace("{n}", &age(at.elapsed().as_secs_f64()))).unwrap_or_else(|| tr("accounts_refresh")) };
        let refresh = Button::new("accounts-refresh").ghost().small().disabled(loading)
            .map(|el| if loading {
                el.child(div().flex().items_center().gap(px(6.))
                    .child(chrome::Spinner::new("accounts-refresh-spin", IconName::RefreshCw, px(14.), theme::muted())).child(label))
            } else { el.icon(IconName::RefreshCw).label(label) })
            .tooltip(tr("accounts_refresh")).accessibility_label(tr("accounts_refresh"))
            .on_click(cx.listener(|this, _, _, cx| this.refresh_accounts(cx)));
        let add = Button::new("accounts-add-account").primary().small().icon(IconName::Plus).label(tr("accounts_add_account"))
            .disabled(self.accounts_busy()).on_click(cx.listener(|this, _, window, cx| this.open_add_account(window, cx)));
        div().flex().items_center().gap(px(8.))
            .child(self.mark(div().child(refresh), "accounts_refresh"))
            .child(self.mark(div().child(density), "accounts_density"))
            .child(add)
    }

    /// Quatro números tirados da própria lista: o que está em uso, o que esgotou a semana, o login que vence primeiro e
    /// as redefinições guardadas do Codex.
    fn accounts_summary(&self, summary: &Summary) -> Div {
        let tile = |key: &str| div().flex_1().min_w_0().p(px(14.)).flex().flex_col().gap(px(6.)).rounded(px(12.)).border_1()
            .border_color(theme::border()).bg(theme::boxed()).child(div().text_size(px(12.)).text_color(theme::muted()).child(tr(key)));
        let big = |text: String, color: Hsla| div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(color).truncate().child(text);
        let small = |text: String| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text);
        let in_use = if summary.in_use.is_empty() { tile("accounts_summary_in_use").child(small(tr("accounts_summary_in_use_none"))) } else {
            tile("accounts_summary_in_use").children(summary.in_use.iter().map(|(provider, name)| {
                let (color, letter) = theme::provider(provider);
                let logo = div().size(px(20.)).flex_shrink_0().rounded(px(6.)).bg(color.opacity(0.16)).flex().items_center().justify_center()
                    .text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(color);
                div().flex().items_center().gap(px(8.)).min_w_0().text_size(px(13.5))
                    .child(match chrome::provider_logo(provider, 12., color) { Some(svg) => logo.child(svg), None => logo.child(letter) })
                    .child(div().min_w_0().truncate().child(name.clone()))
            }))
        };
        let accounts = |n: usize| if n == 1 { tr("accounts_summary_one_account") } else { tr("accounts_summary_accounts").replace("{n}", &n.to_string()) };
        let week = match summary.week_full.first() {
            None => tile("accounts_summary_week_full").child(big(tr("accounts_summary_none"), theme::text())).child(small(tr("accounts_summary_week_free"))),
            Some((name, reset)) => tile("accounts_summary_week_full").child(big(accounts(summary.week_full.len()), theme::danger()))
                .child(small(if reset.is_empty() { name.clone() } else { tr("accounts_summary_week_back").replace("{name}", name).replace("{n}", reset) }))
                .when(summary.week_stale, |el| el.child(div().flex().items_center().gap(px(5.)).text_size(px(12.)).text_color(theme::warning())
                    .child(chrome::small_icon(IconName::Clock, 12., theme::warning())).child(tr("accounts_summary_week_stale")))),
        };
        let login = match &summary.login {
            None => tile("accounts_summary_login").child(big("—".into(), theme::muted())).child(small(tr("accounts_summary_login_none"))),
            Some((days, names)) => {
                let (text, color) = match *days {
                    ..=0 => (tr("accounts_summary_login_expired"), theme::danger()),
                    1 => (tr("accounts_summary_one_day"), theme::warning()),
                    n => (tr("accounts_summary_days").replace("{n}", &n.to_string()), if n <= RENEW_DAYS { theme::warning() } else { theme::text() }),
                };
                tile("accounts_summary_login").child(big(text, color)).child(small(names.join(" · ")))
            }
        };
        let resets = tile("accounts_summary_resets")
            .child(big(match (summary.resets, summary.resets_unread) { (0, true) => "—".into(), (0, false) => tr("accounts_summary_none"),
                (n, _) => n.to_string() }, if summary.resets == 0 && summary.resets_unread { theme::muted() } else { theme::text() }))
            .child(small(tr(match (summary.resets, summary.resets_unread) { (_, true) => "accounts_summary_resets_unread",
                (0, false) => "accounts_summary_resets_none", _ => "accounts_summary_resets_rule" })));
        div().mt(px(20.)).flex().gap(px(12.)).child(in_use).child(week).child(login).child(resets)
    }

    /// Atalhos para as três seções, com quantas linhas cada uma tem: com muitas assinaturas, os modelos ficam lá embaixo.
    fn accounts_tabs(&self, cx: &mut Context<Self>) -> Div {
        let current = self.jumped();
        div().mt(px(16.)).flex().child(div().flex().gap(px(4.)).p(px(3.)).rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
            .children(self.accounts.sections.iter().map(|section| {
                let key = section.group.text().0;
                let on = current == Some(key);
                Button::new(SharedString::from(format!("accounts-jump-{key}")))
                    .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
                        .foreground(if on { theme::accent_text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
                    .small().h(px(28.)).px(px(11.)).rounded(px(7.)).when(on, |el| el.bg(theme::accent_dim()))
                    .child(div().flex().items_center().gap(px(7.)).child(tr(key))
                        .child(div().px(px(6.)).rounded(px(5.)).bg(theme::hover()).text_size(px(11.5))
                            .text_color(if on { theme::accent_text() } else { theme::faint() }).child(section.rows.len().to_string())))
                    .on_click(cx.listener(move |this, _, _, cx| { this.jump_to(key); cx.notify(); }))
            })))
    }

    /// Cartão de uma seção: cabeça com ícone e o que ela guarda, avisos dela, e as linhas. As assinaturas se separam por
    /// provider; na página larga, cada bloco nomeia as colunas de cota.
    #[allow(clippy::too_many_arguments)]
    fn render_accounts_section(&self, section: &Section, problem: Option<String>, wide: bool, compact: bool, order: &mut usize,
        window: &mut Window, cx: &mut Context<Self>) -> Div {
        let (title, empty, lead, icon) = section.group.text();
        let extra = match section.group {
            Group::Subscriptions if !section.labels.is_empty() => Some(legend()),
            Group::Models => Some(div().child(Button::new("accounts-add-model").outline().small().icon(IconName::Plus).label(tr("accounts_add_model"))
                .disabled(self.accounts_busy())
                .on_click(cx.listener(|this, _, window, cx| this.open_add_account_at(AddStep::Catalog(true), window, cx))))),
            _ => None,
        };
        let head = section_head(icon, tr(title), Some(tr(lead)), extra.map(|el| el.flex_shrink_0().into_any_element()), px(16.));
        let mut card = settings_box().mt(px(16.)).child(self.mark(head, title))
            .children(problem.map(|text| div().px_4().pb(px(12.)).child(banner(theme::danger(), IconName::TriangleAlert, text))));
        if section.rows.is_empty() {
            return card.child(div().mx_4().mb(px(16.)).py(px(24.)).px(px(20.)).rounded(px(12.)).border_1().border_dashed().border_color(theme::border_strong())
                .flex().flex_col().items_center().gap(px(10.))
                .child(div().size(px(36.)).rounded(px(10.)).bg(theme::inset()).flex().items_center().justify_center()
                    .child(chrome::small_icon(icon, 18., theme::faint())))
                .child(div().max_w(px(460.)).text_center().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr(empty))));
        }
        let mut blocks: Vec<(&str, Vec<&Row>)> = Vec::new();
        for row in &section.rows {
            let provider = if section.group == Group::Subscriptions { row.glyph.0 } else { "" };
            match blocks.iter_mut().find(|(p, _)| *p == provider) {
                Some((_, rows)) => rows.push(row),
                None => blocks.push((provider, vec![row])),
            }
        }
        for (provider, rows) in blocks {
            let title = match provider { "claude" => Some("Claude"), "codex" => Some("Codex"), _ => None };
            if wide && (title.is_some() || !section.labels.is_empty()) { card = card.child(column_head(title, &rows, &section.labels)); }
            for row in rows {
                card = card.child(self.render_account_row(row, &section.labels, wide, compact, *order, window, cx));
                *order += 1;
            }
        }
        card
    }

    #[allow(clippy::too_many_arguments)]
    fn render_account_row(&self, row: &Row, labels: &[String], wide: bool, compact: bool, order: usize, window: &mut Window,
        cx: &mut Context<Self>) -> Div {
        let (color, _) = theme::provider(row.glyph.0);
        let size = if compact { 28. } else { 36. };
        let avatar = div().size(px(size)).flex_shrink_0().rounded(px(if compact { 8. } else { 10. })).bg(color.opacity(0.16)).flex().items_center().justify_center()
            .text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(color);
        let avatar = match chrome::provider_logo(row.glyph.0, size * 0.55, color) {
            Some(logo) => avatar.child(logo),
            None => avatar.child(row.glyph.1.clone()),
        };
        let renaming = self.accounts.rename.as_ref().filter(|r| r.id == row.id);
        let name_line = match renaming {
            Some(rename) => self.render_rename(rename, &row.name, cx),
            None => div().flex().items_center().gap(px(8.)).min_w_0()
                .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(row.name.clone()))
                .children(row.plan.clone().filter(|_| !compact).map(|plan| div().flex_shrink_0().h(px(18.)).px(px(6.)).flex().items_center()
                    .rounded(px(5.)).border_1().border_color(theme::border_strong()).text_size(px(11.)).text_color(theme::muted()).child(plan)))
                .when(row.active, |el| el.child(div().flex_shrink_0().h(px(20.)).px(px(8.)).flex().items_center().gap(px(5.)).rounded_full()
                    .bg(theme::success().alpha(0.12)).text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(theme::success())
                    .child(div().size(px(6.)).rounded_full().bg(theme::success())).child(tr("accounts_in_use")))),
        };
        // Um texto só, com a cor de aviso por trecho: corta no fim com reticências, sem pedaço solto.
        let (text, marks) = &row.subtitle;
        let subtitle = div().min_w_0().text_size(px(13.)).text_color(theme::muted()).truncate()
            .child(StyledText::new(text.clone()).with_highlights(marks.iter().map(|(range, tone)| (range.clone(),
                HighlightStyle { color: Some(if *tone == Tone::Danger { theme::danger() } else { theme::warning() }), ..Default::default() }))));
        // Sem espaço, a última ficha encolhe com reticências em vez de sair cortada na borda.
        let chips = div().flex().min_w_0().overflow_hidden().gap(px(5.)).children(row.chips.iter().enumerate().map(|(n, chip)| div()
            .map(|el| if n + 1 == row.chips.len() { el.min_w_0().truncate() } else { el.flex_shrink_0() }).px(px(5.)).rounded(px(4.)).border_1()
            .border_color(theme::border_strong()).font_family(theme::MONO).text_size(px(11.5)).text_color(theme::muted()).child(chip.clone())));
        let identity = div().flex_1().min_w_0().flex().flex_col().gap(px(3.)).child(name_line)
            .when(renaming.is_none() && !compact, |el| el.when(!row.subtitle.0.is_empty(),|el| el.child(subtitle))
                .when(!row.chips.is_empty(), |el| el.child(chips)));
        let quota = div().w(px(if wide { columns_width(labels.len()) } else { 206. })).flex_shrink_0().flex().flex_col()
            .gap(px(if compact || wide { 4. } else { 8. }))
            .map(|el| match &row.quota {
                QuotaView::Bars { bars, stale } => el.when(stale.is_some(), |el| el.opacity(0.6))
                    .map(|el| if wide {
                        el.child(div().flex().gap(px(COLUMN_GAP)).children(labels.iter().enumerate().map(|(n, label)| {
                            let key = SharedString::from(format!("accounts-bar-{}-{label}", row.id));
                            let grow = motion::enter(key, motion::FADE_IN.after(120 + 35 * (order.min(12) + n) as u64), window, cx);
                            quota_column(bars.iter().find(|b| &b.label == label), compact, grow)
                        })))
                    } else if compact { el.child(mini_quota(bars, labels)) } else { el.children(bars.iter().map(quota_bar)) })
                    .children(stale.clone().map(|text| div().flex().items_center().gap(px(5.)).text_size(px(11.5)).text_color(theme::muted())
                        .child(chrome::small_icon(IconName::Clock, 12., theme::muted())).child(text))),
                QuotaView::Note(text) => el.child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(text.clone())),
                QuotaView::Nothing => el,
            });
        let this = cx.entity().downgrade();
        let (id, sign_out, remove, cookie_set) = (row.id.clone(), row.sign_out.is_some(), row.remove.is_some(), row.cookie == Some(true));
        // Uma escrita em voo (nome, saída, remoção, login) segura as outras em toda linha.
        let busy = self.accounts_busy();
        let menu_title = row.name.clone();
        let menu = Button::new(SharedString::from(format!("accounts-menu-{}", row.id))).ghost().small().icon(IconName::Ellipsis)
            .accessibility_label(tr("accounts_more").replace("{name}", &row.name))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let item = |key: &str, action: fn(&mut Hangar, String, &mut Window, &mut Context<Hangar>)| {
                    let (this, id) = (this.clone(), id.clone());
                    PopupMenuItem::new(tr(key)).disabled(busy).on_click(move |_, window, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.root_focus.focus(window, cx);
                            action(this, id.clone(), window, cx);
                        });
                    })
                };
                sidebar::menu_style(menu).min_w(px(220.)).label(menu_title.clone()).item(item("accounts_rename", |this, id, window, cx| this.start_rename(id, window, cx)))
                    .when(sign_out || cookie_set || remove, |m| m.separator())
                    .when(sign_out, |m| m.item(item("accounts_sign_out", |this, id, window, cx| this.confirm_change(id, ChangeKind::SignOut, window, cx))))
                    .when(cookie_set, |m| m.item(item("accounts_cookie_clear", |this, id, window, cx| this.confirm_clear_cookie(id, window, cx))))
                    .when(remove, |m| m.item(item("accounts_remove", |this, id, window, cx| this.confirm_change(id, ChangeKind::Remove, window, cx))))
            });
        let changing = self.accounts.change.as_ref().filter(|c| c.id == row.id)
            .map(|c| tr(if c.kind == ChangeKind::SignOut { "accounts_signing_out" } else { "accounts_removing" }))
            .or_else(|| (self.accounts.cookie_clearing.as_deref() == Some(row.id.as_str())).then(|| tr("accounts_cookie_clearing")));
        let edit = row.edit.filter(|_| changing.is_none()).map(|ready| {
            let id = row.id.clone();
            Button::new(SharedString::from(format!("accounts-edit-{}", row.id))).outline().small().label(tr("accounts_edit")).disabled(busy || !ready)
                .when(!ready, |el| el.tooltip(tr("accounts_edit_needs_models")))
                .on_click(cx.listener(move |this, _, window, cx| this.open_engine_form(id.clone(), window, cx)))
        });
        let cookie_open = self.accounts.cookie.as_ref().is_some_and(|c| c.id == row.id);
        let cookie = row.cookie.filter(|_| changing.is_none() && !cookie_open).map(|_| {
            let id = row.id.clone();
            Button::new(SharedString::from(format!("accounts-cookie-{}", row.id))).outline().small().label(tr("accounts_cookie")).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| this.start_cookie(id.clone(), window, cx)))
        });
        let sign_in = row.sign_in.clone().filter(|_| changing.is_none()).map(|label| {
            let id = row.id.clone();
            Button::new(SharedString::from(format!("accounts-sign-in-{}", row.id))).outline().small().label(label).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| this.start_sign_in(id.clone(), window, cx)))
        });
        // "Herdar" curto na linha para não cobrir a cota; o nome inteiro fica na dica e na leitura de tela.
        let codex = row.codex.clone().filter(|_| changing.is_none()).map(|(account, inherit)| {
            let label = tr(if inherit { "accounts_codex_inherit_short" } else { "accounts_sign_in" });
            Button::new(SharedString::from(format!("accounts-codex-{}", row.id))).outline().small().label(label).disabled(busy)
                .when(inherit, |el| el.tooltip(tr("accounts_inherit")).accessibility_label(tr("accounts_inherit")))
                .on_click(cx.listener(move |this, _, window, cx| this.start_codex(Some(account.clone()), inherit, window, cx)))
        });
        let actions = div().w(px(ACTIONS)).flex_shrink_0().flex().items_center().justify_end().gap(px(6.))
            .children(changing.map(|text| div().text_size(px(12.5)).text_color(theme::muted()).child(text)))
            .children(sign_in)
            .children(codex)
            .children(edit)
            .children(cookie)
            .child(menu);
        let line = div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_center().gap(px(12.)).px_4()
            .py(px(if compact { 8. } else { 14. }))
            .when(row.active, |el| el.bg(theme::success().alpha(0.035)))
            .hover(|el| el.bg(theme::hover()))
            .child(avatar).child(identity).child(quota).child(actions);
        let line = match renaming.and_then(|r| r.error.clone()) {
            // A cor mora num filho: a caixa pinta o texto de cinza por cima.
            Some(error) => div().child(line).child(div().px_4().pb(px(12.)).pl(px(16. + size + 12.)).text_size(px(13.))
                .child(div().text_color(theme::danger()).child(error))),
            None => line,
        };
        let line = match (&row.reset, self.accounts.reset.as_ref().is_some_and(|r| r.id == row.id)) {
            (None, false) => line,
            (offer, _) => div().child(line).child(self.render_reset(row, offer.as_ref(), size, cx)),
        };
        let block = match self.accounts.cookie.as_ref().filter(|c| c.id == row.id) {
            Some(form) => div().child(line).child(self.render_cookie(form, size, cx)),
            None => line,
        };
        // As linhas chegam uma depois da outra ao abrir a página; depois disso o relógio de meio minuto não as refaz.
        let shown = motion::enter(SharedString::from(format!("accounts-row-{}", row.id)), motion::FADE_IN.after(35 * order.min(12) as u64), window, cx);
        motion::fade_in(block, shown)
    }

    /// Campo do apelido no lugar do nome: Enter salva, Esc desiste.
    fn render_rename(&self, rename: &Rename, name: &str, cx: &mut Context<Self>) -> Div {
        let saving = rename.saving;
        div().flex().flex_col().gap(px(8.))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| { cx.stop_propagation(); this.cancel_rename(window, cx); }))
            .child(Input::new(&rename.input).small().disabled(saving).aria_label(tr("accounts_rename_of").replace("{name}", name)))
            .child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(tr("accounts_rename_hint")))
            .child(div().flex().items_center().gap(px(6.))
                .child(Button::new("accounts-rename-save").primary().small().label(tr(if saving { "accounts_saving" } else { "accounts_save" }))
                    .disabled(saving).on_click(cx.listener(|this, _, window, cx| this.save_rename(window, cx))))
                .child(Button::new("accounts-rename-cancel").ghost().small().label(tr("cancel")).disabled(saving)
                    .on_click(cx.listener(|this, _, window, cx| this.cancel_rename(window, cx)))))
    }
}

/// Página larga: largura de cada coluna de cota, o vão entre elas e a coluna das ações.
const COLUMN: f32 = 140.;
const COLUMN_GAP: f32 = 16.;
const ACTIONS: f32 = 160.;

/// Seção sem janela nenhuma ainda guarda o espaço de duas: a nota da cota não espreme o nome.
fn columns_width(count: usize) -> f32 {
    let count = count.max(2) as f32;
    count * COLUMN + (count - 1.) * COLUMN_GAP
}

impl Group {
    /// Título, texto de vazio, o que a seção guarda e o ícone dela.
    fn text(self) -> (&'static str, &'static str, &'static str, IconName) {
        match self {
            Group::Subscriptions => ("accounts_subscriptions", "accounts_subscriptions_empty", "accounts_subscriptions_desc", IconName::Users),
            Group::Models => ("accounts_models", "accounts_models_empty", "accounts_models_desc", IconName::Cpu),
            Group::Others => ("accounts_others", "accounts_others_empty", "accounts_others_desc", IconName::Key),
        }
    }
}

/// Aviso numa faixa tingida pela cor do tipo (falha, feito).
fn banner(color: Hsla, icon: IconName, text: String) -> Div {
    div().w_full().px(px(12.)).py(px(10.)).flex().items_center().gap(px(10.)).rounded(px(10.)).border_1()
        .border_color(color.alpha(0.3)).bg(color.alpha(0.08)).text_size(px(13.))
        .child(chrome::small_icon(icon, 15., color))
        .child(div().flex_1().min_w_0().whitespace_normal().child(text))
}

/// O que cada cor da barra quer dizer.
fn legend() -> Div {
    let item = |color: Hsla, key: &str| div().flex().items_center().gap(px(6.))
        .child(div().w(px(10.)).h(px(4.)).rounded(px(2.)).bg(color)).child(tr(key));
    div().flex().items_center().gap(px(14.)).text_size(px(12.)).text_color(theme::muted())
        .child(item(theme::accent(), "accounts_legend_ok")).child(item(theme::warning(), "accounts_legend_warn"))
        .child(item(theme::danger(), "accounts_legend_danger"))
}

/// Cabeça de um bloco na página larga: provider e quantas contas, e o nome de cada coluna que alguma linha do bloco usa.
fn column_head(title: Option<&str>, rows: &[&Row], labels: &[String]) -> Div {
    let used = |label: &String| rows.iter().any(|r| matches!(&r.quota, QuotaView::Bars { bars, .. } if bars.iter().any(|b| &b.label == label)));
    div().mt(px(-1.)).border_t_1().border_color(theme::border()).bg(theme::inset()).flex().items_center().gap(px(12.)).px_4().py(px(8.))
        .text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::faint())
        .child(div().flex_1().min_w_0().flex().items_center().gap(px(8.))
            .children(title.map(|t| div().child(t.to_owned())))
            .when(title.is_some(), |el| el.child(div().font_weight(FontWeight::NORMAL).child(rows.len().to_string()))))
        .child(div().w(px(columns_width(labels.len()))).flex_shrink_0().flex().gap(px(COLUMN_GAP))
            .children(labels.iter().map(|label| div().w(px(COLUMN)).flex_shrink_0().truncate()
                .child(if used(label) { window_label(label) } else { String::new() }))))
        .child(div().w(px(ACTIONS)).flex_shrink_0())
}

/// Uma janela na coluna dela: o %, a barra que cresce ao abrir (`grow` de 0 a 1) e quando reinicia. Na Compacta, só o %.
fn quota_column(bar: Option<&Bar>, compact: bool, grow: f32) -> Div {
    let cell = div().w(px(COLUMN)).flex_shrink_0().flex().flex_col().gap(px(5.));
    let Some(bar) = bar else { return cell.child(div().text_size(px(13.)).text_color(theme::faint()).child("—")) };
    cell.child(div().text_size(px(if compact { 13. } else { 15. })).font_weight(FontWeight::SEMIBOLD)
            .text_color(if bar.pct > 80. { level(bar.pct) } else { theme::text() }).child(format!("{}%", bar.pct.round())))
        .when(!compact, |el| el
            .child(div().h(px(5.)).w_full().rounded_full().bg(theme::raised())
                .child(div().h_full().rounded_full().bg(level(bar.pct)).w(relative(grow * (bar.pct.clamp(0., 100.) / 100.) as f32))))
            .child(div().min_w_0().truncate().text_size(px(11.5)).text_color(theme::faint())
                .child(if bar.reset.is_empty() { String::new() } else { tr("accounts_resets").replace("{n}", &bar.reset) })))
}

/// Janela de cota na linha completa: nome, reinício e % acima da barra.
fn quota_bar(bar: &Bar) -> Div {
    div().flex().flex_col().gap(px(4.))
        .child(div().flex().items_center().gap(px(6.)).text_size(px(11.5))
            .child(div().flex_shrink_0().text_color(theme::muted()).child(window_label(&bar.label)))
            .child(div().flex_1().min_w_0().truncate().text_color(theme::faint())
                .child(if bar.reset.is_empty() { String::new() } else { tr("accounts_resets").replace("{n}", &bar.reset) }))
            .child(div().flex_shrink_0().text_color(if bar.pct > 80. { level(bar.pct) } else { theme::muted() }).child(format!("{}%", bar.pct.round()))))
        .child(div().h(px(4.)).w_full().rounded_full().bg(theme::raised())
            .child(div().h_full().rounded_full().bg(level(bar.pct)).w(relative((bar.pct.clamp(0., 100.) / 100.) as f32))))
}

/// Janelas na linha compacta: rótulo e %, uma coluna por janela do grupo.
fn mini_quota(bars: &[Bar], labels: &[String]) -> Div {
    div().flex().gap(px(10.)).children(labels.iter().map(|label| {
        let bar = bars.iter().find(|b| &b.label == label);
        div().flex_1().flex().items_center().gap(px(5.)).text_size(px(12.))
            .child(div().text_color(theme::muted()).child(window_label(label)))
            .children(bar.map(|b| div().font_weight(FontWeight::SEMIBOLD).text_color(if b.pct > 80. { level(b.pct) } else { theme::text() })
                .child(format!("{}%", b.pct.round()))))
    }))
}

#[cfg(test)]
mod tests {
    use super::{Credential, Engine, Group, QuotaView, Tone, age, build_row, build_sections, build_summary, group_of, model_names, reset_text};
    use crate::i18n::tr;
    use serde_json::{Value, json};
    use std::collections::HashMap;

    fn credential(value: Value) -> Credential { serde_json::from_value(value).expect("synthetic credential") }

    #[test]
    fn proxy_targets_use_exact_credential_ids_and_never_unlisted_accounts() {
        let session = crate::api::dto::SessionInfo { provider: "claude".into(), engine: Some("proxy".into()),
            engine_account: Some("default".into()), conta: Some("codex:/first".into()), ..Default::default() };
        let engine: Engine = serde_json::from_value(json!({"cliproxy_accounts":[
            {"account":"default", "credential_id":"codex:/first", "email":"first@example.com", "label":"First"},
            {"account":"other", "credential_id":"codex:/second", "email":"second@example.com", "label":"Second"}
        ]})).unwrap();
        let list = [
            credential(json!({"id":"codex:/first", "tipo":"codex", "nome":"First", "codex_account":"default", "login":{"estado":"ok", "loggedIn":true}})),
            credential(json!({"id":"codex:/second", "tipo":"codex", "nome":"Second", "codex_account":"other", "login":{"estado":"ok", "loggedIn":true}})),
            credential(json!({"id":"codex:/unlisted", "tipo":"codex", "nome":"Unlisted", "codex_account":"unlisted", "login":{"estado":"ok", "loggedIn":true}})),
            credential(json!({"id":"claude:/storage", "tipo":"claude", "nome":"Storage"}))
        ];
        let targets = super::proxy_account_targets(&session, &list, &engine);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].engine_account.as_deref(), Some("other"));
        assert_eq!(targets[0].label, "Second");
        let invalid = Engine { cliproxy_error: Some("invalid discovery".into()), ..engine };
        assert!(super::proxy_account_targets(&session, &list, &invalid).is_empty());
    }

    #[test]
    fn summary_counts_full_weeks_nearest_login_and_saved_resets() {
        let now = 1_000_000.;
        let claude = |name: &str, week: f64, reset: f64, days: f64, active: bool| credential(json!({"id": format!("claude:/{name}"), "tipo": "claude",
            "nome": name, "ativa": active, "login": {"estado": "ok", "loggedIn": true, "plano": "max", "refreshExpiresAt": now + days * 86_400.},
            "cota": {"estado": "lida", "janelas": [{"rotulo": "7d", "pct": week, "reset_ts": now + reset}]}}));
        let codex = credential(json!({"id": "codex:/c", "tipo": "codex", "nome": "c", "codex_account": "c", "auth_method": "oauth",
            "cota": {"estado": "lida", "janelas": [{"rotulo": "7d", "pct": 60}], "reset_credits": {"available_count": 2}}}));
        let list = [claude("a", 100., 7200., 14., false), claude("b", 100., 600., 20., true), claude("c", 40., 600., 14., false), codex];
        let summary = build_summary(&build_sections(&list, &HashMap::new(), now));
        assert_eq!(summary.week_full.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(summary.login, Some((14, vec!["a".to_owned(), "c".to_owned()])));
        assert_eq!(summary.in_use, [("claude", "b".to_owned())]);
        assert!(summary.resets == 2 && !summary.resets_unread && !summary.week_stale);
        let unread = credential(json!({"id": "codex:/d", "tipo": "codex", "nome": "d", "codex_account": "d", "auth_method": "oauth"}));
        assert_eq!(build_row(&unread, &HashMap::new(), false, now).resets, None);
        assert_eq!(build_row(&list[0], &HashMap::new(), false, now).plan.as_deref(), Some("Max"));
    }

    #[test]
    fn groups_follow_the_web() {
        let engines: HashMap<String, Engine> = serde_json::from_value(json!({"kimi-coding": {"base_url": "https://api.kimi.com/coding", "model": "k3"}})).unwrap();
        let claude = credential(json!({"id": "claude:/h/.claude", "tipo": "claude", "nome": "jefferson"}));
        let codex_none = credential(json!({"id": "codex:/h/.codex-b", "tipo": "codex", "nome": "b", "auth_method": "none"}));
        let model = credential(json!({"id": "chave:kimi-coding", "tipo": "chave", "nome": "kimi", "auth_method": "api_key"}));
        let copy = credential(json!({"id": "kimi:kimi-coding", "tipo": "chave", "nome": "kimi", "auth_method": "api_key"}));
        let other = credential(json!({"id": "chave:solta", "tipo": "chave", "nome": "solta", "auth_method": "api_key"}));
        let list = [claude, codex_none, model, copy, other];
        let models = model_names(&list, &engines);
        let groups: Vec<_> = list.iter().map(|c| group_of(c, &models)).collect();
        assert!(groups == [Some(Group::Subscriptions), Some(Group::Subscriptions), Some(Group::Models), None, Some(Group::Others)]);
    }

    #[test]
    fn a_model_stays_a_model_when_the_engines_read_fails() {
        let model = credential(json!({"id": "chave:deepseek", "tipo": "chave", "nome": "deepseek", "auth_method": "api_key", "usos": ["claude_code"]}));
        let (list, none) = ([model], HashMap::new());
        let models = model_names(&list, &none);
        assert!(group_of(&list[0], &models) == Some(Group::Models));
        // Editar continua na linha, desligado: sem os detalhes do disco o formulário sairia em branco.
        assert_eq!(build_row(&list[0], &HashMap::new(), false, 0.).edit, Some(false));
    }

    #[test]
    fn short_windows_count_down_and_long_ones_name_the_day() {
        assert_eq!(reset_text(Some(1000. + 80. * 60.), 1000.), "1h20");
        assert_eq!(reset_text(Some(1000. + 35. * 60.), 1000.), "35m");
        assert_eq!(reset_text(Some(1000. + 7200.), 1000.), "2h");
        assert_eq!(reset_text(Some(900.), 1000.), "");
        assert!(reset_text(Some(1000. + 3. * 86_400.), 1000.).ends_with('h'));
        assert_eq!(age(10.), "1 min");
        assert_eq!(age(7300.), "2 h");
    }

    #[test]
    fn login_close_to_expiry_offers_renewal_and_expired_quota_offers_sign_in() {
        let now = 1_000_000.;
        let renew = credential(json!({"id": "claude:/a", "tipo": "claude", "nome": "a",
            "login": {"estado": "ok", "loggedIn": true, "refreshExpiresAt": now + 2. * 86_400.}}));
        let row = build_row(&renew, &HashMap::new(), false, now);
        assert_eq!(row.sign_in, Some(tr("accounts_renew")));
        assert!(row.codex.is_none() && row.reset.is_none());
        let (line, marks) = &row.subtitle;
        assert!(marks.iter().any(|(range, tone)| *tone == Tone::Warn && line[range.clone()] == tr("accounts_login_expires").replace("{n}", "2")));
        let expired = credential(json!({"id": "claude:/b", "tipo": "claude", "nome": "b",
            "login": {"estado": "ok", "loggedIn": true}, "cota": {"estado": "expirada", "motivo": "sessao-viva"}}));
        let row = build_row(&expired, &HashMap::new(), false, now);
        assert_eq!(row.sign_in, Some(tr("accounts_sign_in")));
        assert!(matches!(row.quota, QuotaView::Note(ref t) if *t == tr("accounts_quota_live_session")));
        assert!(row.sign_out.is_none());
    }

    #[test]
    fn codex_signs_out_through_its_own_route_only_when_logged_in() {
        let now = 1_000_000.;
        let logged = credential(json!({"id": "codex:/c", "tipo": "codex", "nome": "c", "codex_account": "c", "auth_method": "oauth",
            "login": {"estado": "ok", "loggedIn": true}}));
        let row = build_row(&logged, &HashMap::new(), false, now);
        assert_eq!(row.sign_out, Some(vec!["codex-contas".to_owned(), "c".to_owned(), "logout".to_owned()]));
        let revoked = credential(json!({"id": "codex:/c", "tipo": "codex", "nome": "c", "codex_account": "c", "auth_method": "none",
            "login": {"estado": "ok", "loggedIn": false}}));
        let row = build_row(&revoked, &HashMap::new(), false, now);
        assert!(row.sign_out.is_none());
        assert!(row.codex.is_some());
    }

    #[test]
    fn reset_shows_with_credit_and_unlocks_only_at_a_full_week() {
        let codex = |pct: f64, credits: u64| serde_json::from_str::<Credential>(&format!(r#"{{"id": "codex:/c", "tipo": "codex", "nome": "c", "codex_account": "c",
            "auth_method": "oauth", "cota": {{"estado": "lida", "janelas": [{{"rotulo": "7d", "pct": {pct}}}],
            "reset_credits": {{"available_count": {credits}, "credits": [{{"id": "k1", "expires_at": null, "status": "available"}}]}}}}}}"#))
            .expect("synthetic credential");
        let row = |pct, credits| build_row(&codex(pct, credits), &HashMap::new(), false, 0.);
        assert!(row(100., 0).reset.is_none());
        let blocked = row(99.4, 1).reset.expect("offer with credit");
        assert_eq!(blocked.blocked, Some(tr("accounts_reset_weekly_remaining").replace("{pct}", "99")));
        let free = row(100., 1).reset.expect("offer with credit");
        assert!(free.blocked.is_none() && free.credit.as_deref() == Some("k1"));
    }

    #[test]
    fn reset_credits_accept_null_missing_and_filled_like_the_web() {
        let codex = |credits: &str| serde_json::from_str::<Credential>(&format!(r#"{{"id": "codex:/c", "tipo": "codex", "nome": "c", "codex_account": "c",
            "auth_method": "oauth", "cota": {{"estado": "lida", "janelas": [{{"rotulo": "7d", "pct": 100}}],
            "reset_credits": {{"available_count": 1{credits}}}}}}}"#)).expect("credits null, missing or filled parse");
        for credits in [r#", "credits": null"#, ""] {
            let offer = build_row(&codex(credits), &HashMap::new(), false, 0.).reset.expect("offer from available_count");
            assert!(offer.blocked.is_none() && offer.credit.is_none() && offer.expires.is_none());
        }
        let filled = build_row(&codex(r#", "credits": [{"id": "k1", "expires_at": null, "status": "available"}]"#), &HashMap::new(), false, 0.);
        assert_eq!(filled.reset.and_then(|o| o.credit).as_deref(), Some("k1"));
    }

    #[test]
    fn codex_row_offers_sign_in_or_inherit_like_the_web() {
        let codex = |auth: &str, sync: &str, logged: bool| credential(json!({"id": "codex:/h/.codex-b", "tipo": "codex", "nome": "b", "codex_account": "b",
            "auth_method": auth, "codex_sync": sync, "login": {"estado": "ok", "loggedIn": logged}}));
        assert_eq!(build_row(&codex("none", "idle", false), &HashMap::new(), false, 0.).codex, Some(("b".into(), false)));
        assert_eq!(build_row(&codex("oauth", "idle", true), &HashMap::new(), false, 0.).codex, Some(("b".into(), true)));
        assert_eq!(build_row(&codex("oauth", "ready", true), &HashMap::new(), false, 0.).codex, None);
    }
}
