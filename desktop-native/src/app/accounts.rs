//! Contas e modelos: a lista única de credenciais do servidor (`GET /api/credenciais`) em três grupos,
//! com a cota de cada uma, e os modelos do Claude Code (`GET /api/engines`). O que a tela mostra é
//! montado na chegada da resposta e a cada meio minuto (o "há X" e os prazos envelhecem); o desenho só lê.
//! Renomear escreve daqui; entrar, sair, remover e adicionar conta Claude moram em `actions`. Editar modelo, Codex e
//! o cookie do OpenCode aparecem desligados até chegarem.
mod actions;

use super::*;
use super::device::Remote;
use actions::{ActionReply, AddAccount, Change, ChangeKind, SignIn};
use super::settings::{Page, segments, settings_box};
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
}

#[derive(Clone, Deserialize)]
struct ResetCredits { available_count: u64 }

/// O que a lista usa de um modelo do Claude Code (`engines.json`).
#[derive(Clone, Deserialize)]
struct Engine {
    #[serde(default)] model: String,
    subagent_model: Option<String>,
    context_window: Option<u64>,
    adaptive_thinking: Option<bool>,
}

pub(super) struct Engines { map: HashMap<String, Engine>, broken_file: Option<String> }

#[derive(Clone, Copy, PartialEq)]
enum Group { Subscriptions, Models, Others }

#[derive(Clone, Copy, PartialEq)]
enum Tone { Muted, Warn, Danger }

struct Bar { label: String, reset: String, pct: f64 }

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
    quota: QuotaView,
    /// Ações que chegam na próxima versão, mostradas desligadas na linha.
    later: Vec<String>,
    /// Entrar ou Renovar login, quando a conta Claude precisa.
    sign_in: Option<String>,
    can_sign_out: bool,
    /// Rota do DELETE conforme o tipo, o prazo dele e o texto da confirmação.
    remove: Option<(Vec<String>, u64, &'static str)>,
}

struct Section { group: Group, rows: Vec<Row>, labels: Vec<String> }

struct Rename { id: String, input: Entity<InputState>, saving: bool, error: Option<String>, seq: u64, _subscription: Subscription }

#[derive(Default)]
pub(super) struct Accounts {
    list: Remote<Vec<Credential>>,
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
}

pub(super) enum AccountsReply {
    List(u64, Result<Value, Failure>),
    Engines(u64, Result<Value, Failure>),
    Renamed(u64, Result<Value, Failure>),
    Action(ActionReply),
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
fn reset_text(at: Option<f64>, now: f64) -> String {
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
    let mut later = Vec::new();
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
                subtitle.extend(login.plan.clone().map(muted));
                subtitle.extend(login.email.clone().map(muted));
            } else if logged == Some(false) {
                subtitle.push(muted(tr("accounts_not_connected")));
            }
            if !c.active && let Some(sync) = c.codex_sync.as_deref() {
                subtitle.push(muted(tr(match sync { "ready" => "accounts_codex_inherited", "running" => "accounts_codex_preparing",
                    "idle" => "accounts_codex_not_inherited", _ => "accounts_codex_inherit_failed" })));
            }
            if c.codex_account.is_some() && c.auth() == "none" { later.push(tr("accounts_sign_in")); }
            else if c.codex_account.is_some() && c.codex_sync.as_deref() == Some("idle") && logged == Some(true) { later.push(tr("accounts_inherit")); }
            let weekly_full = c.read_windows().is_some_and(|w| w.iter().any(|w| w.label == "7d" && w.pct >= 100.));
            let credits = c.quota.as_ref().and_then(|q| q.reset_credits.as_ref()).map_or(0, |r| r.available_count);
            if c.codex_account.is_some() && credits > 0 && weekly_full { later.push(tr("accounts_use_reset")); }
        }
        _ => match engine {
            // Como no mock: só o que foge do padrão vira ficha; o endereço e o resto ficam no Editar.
            Some(engine) => {
                if has_kimi_copy { subtitle.push(muted(tr("accounts_kimi_copy"))); }
                if !engine.model.is_empty() { chips.push(engine.model.clone()); }
                chips.extend(engine.context_window.map(|n| format!("{}k", (n as f64 / 1000.).round())));
                chips.extend(engine.subagent_model.as_deref().filter(|m| !m.is_empty()).map(|m| tr("accounts_chip_subagent").replace("{id}", m)));
                if engine.adaptive_thinking != Some(false) { chips.push(tr("accounts_chip_thinking_on")); }
                later.push(tr("accounts_edit"));
            }
            None => {
                subtitle.push(muted(tr("accounts_api_key")));
                if let Some(url) = &c.base_url { subtitle.push(muted(host(url).to_owned())); }
                subtitle.extend(c.masked_key.clone().map(muted));
                if c.cookie_set { subtitle.push(muted(tr("accounts_cookie_set"))); }
                if c.managed == Some(false) { subtitle.push(muted(tr("accounts_agent_key"))); }
                // Modelo do Claude Code cujos detalhes não chegaram: continua sendo editável.
                if c.engine_name().is_some() && c.uses.iter().any(|u| u == "claude_code") { later.push(tr("accounts_edit")); }
            }
        },
    }
    if c.accepts_cookie { later.push(tr("accounts_cookie")); }
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
            let mut bars: Vec<Bar> = windows.iter().map(|w| Bar { label: w.label.clone(), reset: reset_text(w.reset_at, now), pct: w.pct }).collect();
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
    Row {
        id: c.id.clone(), name: c.name.clone(), label: c.natural.clone(), natural: c.natural.clone(), alias: c.alias.clone().unwrap_or_default(),
        active: c.active, glyph, subtitle, chips, quota, later, sign_in,
        can_sign_out: c.kind == "claude" && c.managed != Some(false) && logged == Some(true) && !c.expired(),
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

fn parse_engines(value: &Value) -> Result<Engines, String> {
    let map = value.get("motores").cloned().map(serde_json::from_value::<HashMap<String, Engine>>).transpose()
        .map_err(|_| tr("invalid_response"))?.unwrap_or_default();
    let broken = value.get("arquivo_corrompido").and_then(Value::as_bool) == Some(true);
    Ok(Engines { map, broken_file: broken.then(|| value.get("arquivo_caminho").and_then(Value::as_str).unwrap_or("engines.json").to_owned()) })
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
            AccountsReply::List(seq, result) => match result.map(serde_json::from_value::<Vec<Credential>>) {
                // `finish` só aceita o último pedido: resposta atrasada não apaga a falha de uma releitura mais nova.
                Ok(Ok(list)) => if accounts.list.finish(seq, Ok(list)) { (accounts.read_at, accounts.notice) = (Some(Instant::now()), None); },
                // Falhou com a lista anterior na tela: ela fica, e a falha aparece acima dela.
                failed if accounts.list.ok().is_some() => if seq == accounts.list.seq {
                    accounts.list.loading = false;
                    accounts.notice = Some(match failed { Err(e) => Self::failure(&e), Ok(_) => tr("invalid_response") });
                },
                Err(e) => { accounts.list.finish(seq, Err(Self::failure(&e))); }
                Ok(Err(_)) => { accounts.list.finish(seq, Err(tr("invalid_response"))); }
            },
            AccountsReply::Engines(seq, result) => match result.map_err(|e| Self::failure(&e)).and_then(|v| parse_engines(&v)) {
                // Releitura que falhou com os modelos já na tela: eles ficam, e a falha aparece como a da lista.
                Err(error) if accounts.engines.ok().is_some() => if seq == accounts.engines.seq {
                    accounts.engines.loading = false;
                    accounts.engines_notice = Some(error);
                },
                parsed => if accounts.engines.finish(seq, parsed) { accounts.engines_notice = None; },
            },
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
        }
        self.rebuild_accounts();
        cx.notify();
    }

    pub(super) fn render_accounts(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let server = self.server_label(cx);
        let chip = div().mt(px(12.)).flex().child(div().h(px(24.)).px(px(9.)).flex().items_center().gap(px(6.)).rounded_full().border_1()
            .border_color(theme::border_strong()).text_size(px(12.5)).text_color(theme::muted())
            .child(chrome::small_icon(IconName::Server, 13., theme::muted())).child(tr("accounts_this_server").replace("{server}", &server)));
        let page = div().flex().flex_col().child(self.page_top("settings_page_accounts", tr("accounts_lead"))).child(chip);
        let note = |text: String, color: Hsla| div().px_4().py(px(18.)).text_size(px(13.)).text_color(color).whitespace_normal().child(text);
        let accounts = &self.accounts;
        if self.api.is_none() {
            return page.child(self.heading("accounts_subscriptions")).child(settings_box().child(note(tr("settings_offline"), theme::muted()))).into_any_element();
        }
        if let Some(panel) = self.render_sign_in(cx) { return page.child(panel).into_any_element(); }
        match (&accounts.list.value, accounts.list.loading) {
            (None, _) => return page.child(self.heading("accounts_subscriptions"))
                .child(settings_box().child(note(tr("accounts_loading"), theme::muted()))).into_any_element(),
            (Some(Err(error)), loading) => {
                let retry = Button::new("accounts-retry").outline().small().icon(IconName::RefreshCw)
                    .label(tr(if loading { "accounts_loading_short" } else { "accounts_retry" })).disabled(loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load_accounts(false, cx)));
                return page.child(self.heading("accounts_subscriptions"))
                    .child(settings_box().child(note(tr("accounts_failed").replace("{reason}", error), theme::danger()))
                        .child(div().px_4().pb(px(16.)).flex().child(retry)))
                    .into_any_element();
            }
            _ => {}
        }
        let mut page = page;
        if let Some((text, error)) = &accounts.outcome {
            page = page.child(div().mt(px(16.)).text_size(px(13.)).whitespace_normal()
                .child(div().text_color(if *error { theme::danger() } else { theme::success() }).child(text.clone())));
        }
        let compact = appearance::get().accounts_compact;
        // Arquivo dos modelos ilegível não é "nenhum modelo": pode estar escondendo modelos de verdade.
        let engines_problem = match &accounts.engines.value {
            Some(Ok(Engines { broken_file: Some(path), .. })) => Some(tr("accounts_engines_broken").replace("{path}", path)),
            Some(Err(error)) => Some(tr("accounts_engines_failed").replace("{reason}", error)),
            // Releitura que falhou com os modelos anteriores na tela.
            _ => accounts.engines_notice.as_ref().map(|error| tr("accounts_refresh_failed").replace("{reason}", error)),
        };
        for section in &accounts.sections {
            let (title, empty) = match section.group {
                Group::Subscriptions => ("accounts_subscriptions", "accounts_subscriptions_empty"),
                Group::Models => ("accounts_models", "accounts_models_empty"),
                Group::Others => ("accounts_others", "accounts_others_empty"),
            };
            let tools = match section.group {
                Group::Subscriptions => div().flex().items_center().gap(px(8.)).child(self.accounts_tools(compact, cx))
                    .child(Button::new("accounts-add-account").outline().small().icon(IconName::Plus).label(tr("accounts_add_account"))
                        .disabled(self.accounts_busy()).on_click(cx.listener(|this, _, window, cx| this.open_add_account(window, cx))))
                    .into_any_element(),
                Group::Models => soon_button("accounts-add-model", tr("accounts_add_model")).icon(IconName::Plus).into_any_element(),
                Group::Others => div().into_any_element(),
            };
            page = page.child(div().flex().items_end().gap(px(12.)).child(div().flex_1().child(self.heading(title))).child(div().mb(px(8.)).child(tools)));
            if section.group == Group::Subscriptions && let Some(notice) = &accounts.notice {
                page = page.child(div().mb(px(10.)).text_size(px(13.)).text_color(theme::danger()).child(tr("accounts_refresh_failed").replace("{reason}", notice)));
            }
            if section.group == Group::Models && let Some(problem) = &engines_problem {
                page = page.child(div().mb(px(10.)).text_size(px(13.)).text_color(theme::danger()).whitespace_normal().child(problem.clone()));
            }
            let body = if section.rows.is_empty() { settings_box().child(note(tr(empty), theme::muted())) }
                else { settings_box().children(section.rows.iter().map(|row| self.render_account_row(row, &section.labels, compact, cx))) };
            page = page.child(body);
        }
        page.child(div().mt(px(14.)).text_size(px(13.)).text_color(theme::muted()).child(tr("accounts_menu_note"))).into_any_element()
    }

    /// Completa × Compacta e o Atualizar com a idade da leitura.
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
        let refresh = Button::new("accounts-refresh").ghost().small().icon(IconName::RefreshCw).label(label).disabled(loading)
            .tooltip(tr("accounts_refresh")).accessibility_label(tr("accounts_refresh"))
            .on_click(cx.listener(|this, _, _, cx| this.refresh_accounts(cx)));
        div().flex().items_center().gap(px(8.))
            .child(self.mark(div().child(density), "accounts_density"))
            .child(self.mark(div().child(refresh), "accounts_refresh"))
    }

    fn render_account_row(&self, row: &Row, labels: &[String], compact: bool, cx: &mut Context<Self>) -> Div {
        let (color, _) = theme::provider(row.glyph.0);
        let size = if compact { 28. } else { 36. };
        let avatar = div().size(px(size)).flex_shrink_0().rounded(px(if compact { 8. } else { 10. })).bg(color.opacity(0.16)).flex().items_center().justify_center()
            .text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(row.glyph.1.clone());
        let renaming = self.accounts.rename.as_ref().filter(|r| r.id == row.id);
        let name_line = match renaming {
            Some(rename) => self.render_rename(rename, &row.name, cx),
            None => div().flex().items_center().gap(px(8.)).min_w_0()
                .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(row.name.clone()))
                .when(row.active, |el| el.child(div().flex_shrink_0().flex().items_center().gap(px(5.)).text_size(px(12.)).text_color(theme::muted())
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
        let quota = div().w(px(206.)).flex_shrink_0().flex().flex_col().gap(px(if compact { 2. } else { 8. }))
            .map(|el| match &row.quota {
                QuotaView::Bars { bars, stale } => el.when(stale.is_some(), |el| el.opacity(0.6))
                    .map(|el| if compact { el.child(mini_quota(bars, labels)) } else { el.children(bars.iter().map(quota_bar)) })
                    .children(stale.clone().map(|text| div().text_size(px(11.5)).text_color(theme::muted()).child(text))),
                QuotaView::Note(text) => el.child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(text.clone())),
                QuotaView::Nothing => el,
            });
        let this = cx.entity().downgrade();
        let (id, sign_out, remove) = (row.id.clone(), row.can_sign_out, row.remove.is_some());
        // Uma escrita em voo (nome, saída, remoção, login) segura as outras em toda linha.
        let busy = self.accounts_busy();
        let menu = Button::new(SharedString::from(format!("accounts-menu-{}", row.id))).ghost().small().icon(IconName::Ellipsis)
            .accessibility_label(tr("accounts_more").replace("{name}", &row.name))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let item = |key: &str, action: fn(&mut Hangar, String, &mut Window, &mut Context<Hangar>)| {
                    let (this, id) = (this.clone(), id.clone());
                    PopupMenuItem::new(tr(key)).disabled(busy).on_click(move |_, window, cx| {
                        let _ = this.update(cx, |this, cx| action(this, id.clone(), window, cx));
                    })
                };
                menu.item(item("accounts_rename", |this, id, window, cx| this.start_rename(id, window, cx)))
                    .when(sign_out, |m| m.item(item("accounts_sign_out", |this, id, window, cx| this.confirm_change(id, ChangeKind::SignOut, window, cx))))
                    .when(remove, |m| m.item(item("accounts_remove", |this, id, window, cx| this.confirm_change(id, ChangeKind::Remove, window, cx))))
            });
        let changing = self.accounts.change.as_ref().filter(|c| c.id == row.id)
            .map(|c| tr(if c.kind == ChangeKind::SignOut { "accounts_signing_out" } else { "accounts_removing" }));
        let sign_in = row.sign_in.clone().filter(|_| changing.is_none()).map(|label| {
            let id = row.id.clone();
            Button::new(SharedString::from(format!("accounts-sign-in-{}", row.id))).outline().small().label(label).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| this.start_sign_in(id.clone(), window, cx)))
        });
        let actions = div().w(px(160.)).flex_shrink_0().flex().items_center().justify_end().gap(px(6.))
            .children(changing.map(|text| div().text_size(px(12.5)).text_color(theme::muted()).child(text)))
            .children(sign_in)
            .children(row.later.iter().enumerate().map(|(n, label)| soon_button(SharedString::from(format!("accounts-later-{}-{n}", row.id)), label.clone())))
            .child(menu);
        let line = div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_center().gap(px(12.)).px_4()
            .py(px(if compact { 8. } else { 14. }))
            .child(avatar).child(identity).child(quota).child(actions);
        match renaming.and_then(|r| r.error.clone()) {
            // A cor mora num filho: a caixa pinta o texto de cinza por cima.
            Some(error) => div().child(line).child(div().px_4().pb(px(12.)).pl(px(16. + size + 12.)).text_size(px(13.))
                .child(div().text_color(theme::danger()).child(error))),
            None => line,
        }
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

/// Ação que chega na próxima versão: aparece, desligada, dizendo isso.
fn soon_button(id: impl Into<ElementId>, label: String) -> Button {
    Button::new(id).outline().small().label(label).disabled(true).tooltip(tr("settings_next_version"))
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
    use super::{Credential, Engine, Group, QuotaView, Tone, age, build_row, group_of, model_names, reset_text};
    use crate::i18n::tr;
    use serde_json::{Value, json};
    use std::collections::HashMap;

    fn credential(value: Value) -> Credential { serde_json::from_value(value).expect("synthetic credential") }

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
        assert_eq!(build_row(&list[0], &HashMap::new(), false, 0.).later, vec![tr("accounts_edit")]);
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
        assert!(row.later.is_empty());
        let (line, marks) = &row.subtitle;
        assert!(marks.iter().any(|(range, tone)| *tone == Tone::Warn && line[range.clone()] == tr("accounts_login_expires").replace("{n}", "2")));
        let expired = credential(json!({"id": "claude:/b", "tipo": "claude", "nome": "b",
            "login": {"estado": "ok", "loggedIn": true}, "cota": {"estado": "expirada", "motivo": "sessao-viva"}}));
        let row = build_row(&expired, &HashMap::new(), false, now);
        assert_eq!(row.sign_in, Some(tr("accounts_sign_in")));
        assert!(matches!(row.quota, QuotaView::Note(ref t) if *t == tr("accounts_quota_live_session")));
        assert!(!row.can_sign_out);
    }

    #[test]
    fn reset_offer_needs_the_weekly_window_full() {
        let codex = |pct: f64| serde_json::from_str::<Credential>(&format!(r#"{{"id": "codex:/c", "tipo": "codex", "nome": "c", "codex_account": "c",
            "auth_method": "oauth", "cota": {{"estado": "lida", "janelas": [{{"rotulo": "7d", "pct": {pct}}}], "reset_credits": {{"available_count": 1}}}}}}"#))
            .expect("synthetic credential");
        assert!(build_row(&codex(99.), &HashMap::new(), false, 0.).later.is_empty());
        assert_eq!(build_row(&codex(100.), &HashMap::new(), false, 0.).later, vec![tr("accounts_use_reset")]);
    }
}
