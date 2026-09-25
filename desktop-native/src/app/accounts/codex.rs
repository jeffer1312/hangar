//! Contas do Codex (`CodexContaLogin` e a redefinição de `ContasSettings` do web): entrar pelo código do aparelho,
//! importar do Claude ou herdar da padrão depois do login, abrir a sessão onde o próprio Codex pede a aprovação dos
//! hooks, e gastar uma redefinição guardada da cota. Cada resposta traz o número da vez: a de outra vez não mexe nesta.
use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(in crate::app) enum Import { Claude, Inherit }

#[derive(Clone, Default, Deserialize)]
struct Issue { code: String, #[serde(default)] params: HashMap<String, String> }

#[derive(Clone, Deserialize)]
struct Attempt { attempt_id: String, status: String, user_code: Option<String>, verification_url: Option<String>, error: Option<Issue> }

#[derive(Deserialize)]
struct Preparation {
    status: String,
    #[serde(default)] trust_pending: bool,
    #[serde(default)] issues: Vec<Issue>,
    etapa: Option<String>,
    herdado: Option<HashMap<String, u64>>,
}

#[derive(Deserialize)]
struct Status { status: String }

#[derive(Deserialize)]
struct CodexAccount { id: String, #[serde(default)] is_default: bool, auth: Status, sync: Status, #[serde(default)] has_settings: bool }

/// Aviso de um item que ficou de fora de propósito (a variável de execução aponta para a pasta da outra conta): não é erro.
const DELIBERATE: [&str; 2] = ["codex_account_mcp_runtime_excluded", "codex_account_mcp_auth_excluded"];

/// O resultado da importação, já em texto: o que veio, os avisos e se falta aprovar hooks no Codex.
pub(in crate::app) struct Imported { ok: bool, inherited: Vec<String>, issues: Vec<(String, bool)>, trust_pending: bool }

/// O painel do Codex, no lugar da lista enquanto existe (como o login do Claude).
pub(super) struct CodexFlow {
    account: Option<String>,
    name: Entity<InputState>,
    slug: String,
    attempt: Option<Attempt>,
    /// Montados na chegada da tentativa: o motivo dela e o endereço que se pode abrir (só https, sem usuário).
    attempt_error: Option<String>,
    url: Option<String>,
    question: Option<Import>,
    importing: bool,
    stage: String,
    result: Option<Imported>,
    opening: bool,
    busy: bool,
    error: Option<String>,
    turn: u64,
    task: Option<JoinHandle<()>>,
    copied: bool,
    _subscription: Subscription,
}

impl Drop for CodexFlow {
    // Tarefa do tokio não morre com o handle: sem isto a leitura seguiria depois do painel fechado.
    fn drop(&mut self) { if let Some(task) = self.task.take() { task.abort(); } }
}

/// Redefinição sendo confirmada: a chave nasce ao abrir a pergunta e fica até dar certo, para repetir ser a mesma tentativa.
/// `pending`: um pedido saiu e a resposta se perdeu. Daí em diante ela não se descarta, aparece na linha da conta mesmo
/// que a oferta suma (o crédito pode ter sido gasto) e nenhuma outra redefinição abre até um resultado definitivo.
pub(super) struct ResetTry { pub(super) id: String, account: String, credit: Option<String>, key: String, pub(super) consuming: bool,
    pending: bool, error: Option<String> }

impl ResetTry {
    /// Segura as outras redefinições: uma em voo ou uma com resultado desconhecido.
    fn holds(&self) -> bool { self.consuming || self.pending }
}

/// O que a linha de uma conta Codex com redefinição guardada mostra.
pub(super) struct ResetOffer { pub(super) account: String, pub(super) credit: Option<String>, pub(super) count: String, pub(super) expires: Option<String>,
    /// Por que não dá para usar agora (semanal desconhecida ou abaixo de 100%); `None` = liberada.
    pub(super) blocked: Option<String> }

pub(in crate::app) enum CodexReply {
    /// Lista das contas Codex, para entrar na padrão quando ela está sem login.
    Default(u64, Result<Value, Failure>),
    Created(u64, String),
    /// Tentativa lida, começada ou cancelada; `true` = acabou de começar, a leitura periódica começa daqui.
    Attempt(u64, Result<Value, Failure>, bool),
    After(u64, Option<Import>),
    Stage(u64, String),
    Imported(u64, Result<Imported, String>),
    Session(u64, Result<SessionInfo, String>),
    Reset(String, Result<Value, Failure>),
}

/// Mensagem de um código do servidor, como o `codexAccountMessage` do web.
fn issue_text(issue: &Issue) -> String {
    crate::i18n::tr_web(&issue.code, &issue.params).or_else(|| crate::i18n::tr_web("codex_account_error_unknown", &HashMap::new())).unwrap_or_else(|| issue.code.clone())
}

fn preparation_text(stage: Option<&str>) -> String {
    tr(match stage {
        Some("principal") => "accounts_codex_stage_main", Some("configuracoes") => "accounts_codex_stage_settings",
        Some("recursos") => "accounts_codex_stage_resources", Some("plugins") => "accounts_codex_stage_plugins",
        _ => "accounts_codex_preparing_account",
    })
}

/// Etapa da importação do Claude: o código traduzido pelo texto do web, senão o texto que o servidor mandou.
fn integration_text(stage: Option<&Value>) -> String {
    match stage {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(fields)) => {
            let params: HashMap<String, String> = fields.get("params").and_then(Value::as_object).map(|p| p.iter()
                .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_owned))).collect()).unwrap_or_default();
            fields.get("codigo").and_then(Value::as_str).and_then(|code| crate::i18n::tr_web(&format!("harness_codex_m_{code}"), &params))
                .or_else(|| fields.get("texto").and_then(Value::as_str).map(str::to_owned)).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn imported(p: Preparation) -> Imported {
    let inherited = ["skills", "hooks", "agents", "plugins", "mcps"].iter()
        .filter_map(|kind| p.herdado.as_ref()?.get(*kind).filter(|n| **n > 0).map(|n| tr(&format!("accounts_codex_inherited_{kind}")).replace("{n}", &n.to_string())))
        .collect();
    let issues = p.issues.iter().map(|issue| {
        let deliberate = DELIBERATE.contains(&issue.code.as_str());
        let mut text = issue_text(issue);
        if deliberate && let Some(server) = issue.params.get("server") {
            text = format!("{text} {}", tr("accounts_codex_mcp_detail").replace("{server}", server)
                .replace("{variable}", issue.params.get("variable").map_or("—", String::as_str)));
        }
        (text, deliberate)
    }).collect();
    Imported { ok: p.status == "ready", inherited, issues, trust_pending: p.trust_pending }
}

/// Chave idempotente no formato UUID v4 (o servidor exige UUID). Aleatória o bastante para não repetir entre tentativas.
fn idempotency_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    let half = |salt: u64| {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        hasher.write_u64(salt);
        hasher.finish()
    };
    let mut b = ((u128::from(half(1)) << 64) | u128::from(half(2))).to_be_bytes();
    (b[6], b[8]) = ((b[6] & 0x0f) | 0x40, (b[8] & 0x3f) | 0x80);
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// A oferta de redefinição de uma credencial Codex: aparece sempre que há crédito; abaixo de 100% na semana, desligada com o motivo.
pub(super) fn reset_offer(c: &Credential, now: f64) -> Option<ResetOffer> {
    let account = c.codex_account.clone().filter(|_| c.kind == "codex")?;
    let credits = c.quota.as_ref()?.reset_credits.as_ref().filter(|r| r.available_count > 0)?;
    let available = || credits.credits.iter().flatten().filter(|credit| credit.status == "available");
    let weekly = c.quota.as_ref().and_then(|q| q.windows.iter().find(|w| w.label == "7d")).map(|w| w.pct);
    let expires = available().filter_map(|credit| credit.expires_at).reduce(f64::min).map(|at| reset_text(Some(at), now)).filter(|t| !t.is_empty())
        .map(|text| tr("accounts_reset_expires").replace("{n}", &text));
    Some(ResetOffer {
        account, credit: available().next().map(|credit| credit.id.clone()), expires,
        count: if credits.available_count == 1 { tr("accounts_reset_one") } else { tr("accounts_reset_many").replace("{n}", &credits.available_count.to_string()) },
        blocked: match weekly {
            None => Some(tr("accounts_reset_weekly_unavailable")),
            Some(pct) if pct < 100. => Some(tr("accounts_reset_weekly_remaining").replace("{pct}", &format!("{}", pct.round()))),
            _ => None,
        },
    })
}

impl Hangar {
    fn codex_send(&self) -> impl Fn(CodexReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let done = self.accounts_send_later();
        move |reply| done(AccountsReply::Codex(reply))
    }

    /// Nova vez do painel: a tarefa da vez anterior para, e só respostas desta vez valem.
    fn codex_turn(&mut self) -> Option<(u64, Api)> {
        let api = self.api.clone()?;
        self.accounts.keys_seq += 1;
        let turn = self.accounts.keys_seq;
        let flow = self.accounts.codex.as_mut()?;
        if let Some(task) = flow.task.take() { task.abort(); }
        flow.turn = turn;
        Some((turn, api))
    }

    fn codex_flow(&mut self, turn: u64) -> Option<&mut CodexFlow> { self.accounts.codex.as_mut().filter(|f| f.turn == turn) }

    /// Abre o painel. Com `account`: Entrar ou Herdar da linha. Sem: o "Adicionar", que entra na padrão quando ela está
    /// sem login ou cria uma conta com nome.
    pub(super) fn start_codex(&mut self, account: Option<String>, inherit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() || self.api.is_none() { return; }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(tr("accounts_codex_name_placeholder")));
        let subscription = cx.subscribe_in(&name, window, |this: &mut Hangar, input, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let slug = actions::account_slug(&input.read(cx).value());
                if let Some(flow) = this.accounts.codex.as_mut() { flow.slug = slug; }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.codex_start(cx),
            _ => {}
        });
        if account.is_none() { name.update(cx, |input, cx| input.focus(window, cx)); }
        self.accounts.outcome = None;
        self.accounts.codex = Some(CodexFlow { account: account.clone(), name, slug: String::new(), attempt: None, attempt_error: None, url: None,
            question: None, importing: false, stage: String::new(), result: None, opening: false, busy: false, error: None, turn: 0, task: None,
            copied: false, _subscription: subscription });
        match account {
            Some(_) if inherit => self.codex_import(Import::Inherit, cx),
            Some(_) => self.codex_poll(cx),
            None => {
                let Some((turn, api)) = self.codex_turn() else { return };
                let done = self.codex_send();
                let task = self.runtime.spawn(async move { done(CodexReply::Default(turn, api.server_read(&["codex-contas"], &[], 8).await)).await });
                if let Some(flow) = self.accounts.codex.as_mut() { flow.task = Some(task); }
            }
        }
        cx.notify();
    }

    /// Lê a tentativa agora e a cada segundo enquanto ela espera a confirmação no navegador.
    fn codex_poll(&mut self, cx: &mut Context<Self>) {
        let Some((turn, api)) = self.codex_turn() else { return };
        let done = self.codex_send();
        let Some(flow) = self.accounts.codex.as_mut() else { return };
        let Some(id) = flow.account.clone() else { return };
        flow.task = Some(self.runtime.spawn(async move {
            loop {
                let result = api.server_read(&["codex-contas", &id, "login"], &[], 8).await;
                let waiting = matches!(&result, Ok(value) if value.get("status").and_then(Value::as_str) == Some("waiting"));
                done(CodexReply::Attempt(turn, result, false)).await;
                if !waiting { break; }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }));
        cx.notify();
    }

    /// Entrar: cria a conta com o nome (se ainda não há) e pede o código do aparelho.
    fn codex_start(&mut self, cx: &mut Context<Self>) {
        if self.accounts.codex.as_ref().is_none_or(|f| f.busy || (f.account.is_none() && f.slug.is_empty())) { return; }
        let Some((turn, api)) = self.codex_turn() else { return };
        let done = self.codex_send();
        let Some(flow) = self.accounts.codex.as_mut() else { return };
        (flow.busy, flow.error) = (true, None);
        let (account, slug) = (flow.account.clone(), flow.slug.clone());
        flow.task = Some(self.runtime.spawn(async move {
            let id = match account {
                Some(id) => id,
                None => match api.server_send(reqwest::Method::POST, &["codex-contas"], Some(json!({"name": slug})), 30).await {
                    Ok(created) => {
                        let id = created.get("id").and_then(Value::as_str).unwrap_or(&slug).to_owned();
                        done(CodexReply::Created(turn, id.clone())).await;
                        id
                    }
                    Err(error) => return done(CodexReply::Attempt(turn, Err(error), false)).await,
                },
            };
            done(CodexReply::Attempt(turn, api.server_post(&["codex-contas", &id, "login"], 30).await, true)).await
        }));
        cx.notify();
    }

    fn codex_cancel(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = self.accounts.codex.as_ref().filter(|f| !f.busy) else { return };
        let (Some(id), Some(attempt)) = (flow.account.clone(), flow.attempt.as_ref().map(|a| a.attempt_id.clone())) else { return };
        let Some((turn, api)) = self.codex_turn() else { return };
        let done = self.codex_send();
        let Some(flow) = self.accounts.codex.as_mut() else { return };
        (flow.busy, flow.error) = (true, None);
        flow.task = Some(self.runtime.spawn(async move {
            done(CodexReply::Attempt(turn, api.server_delete(&["codex-contas", &id, "login"], &[("attempt_id", &attempt)], 15).await, false)).await
        }));
        cx.notify();
    }

    /// Logou. A padrão oferece importar do Claude (se nunca importou); a adicional, herdar da padrão (se ela tem o que
    /// herdar e esta nunca herdou). Nada a oferecer, ou a lista indisponível: o painel fecha, como no web.
    fn codex_after_login(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.accounts.codex.as_ref().and_then(|f| f.account.clone()) else { return };
        let Some((turn, api)) = self.codex_turn() else { return };
        let done = self.codex_send();
        let task = self.runtime.spawn(async move {
            let offer = async {
                let list: Vec<CodexAccount> = serde_json::from_value(api.server_read(&["codex-contas"], &[], 8).await.ok()?).ok()?;
                let account = list.iter().find(|a| a.id == id)?;
                if account.is_default {
                    let state = api.server_read(&["harness", "codex", "integracao"], &[], 8).await.ok()?;
                    return state.get("ultima_execucao").is_none_or(Value::is_null).then_some(Import::Claude);
                }
                let has = list.iter().find(|a| a.is_default).is_some_and(|a| a.has_settings);
                (account.sync.status == "idle" && has).then_some(Import::Inherit)
            }.await;
            done(CodexReply::After(turn, offer)).await
        });
        if let Some(flow) = self.accounts.codex.as_mut() { flow.task = Some(task); }
        cx.notify();
    }

    /// Importar do Claude ou herdar da padrão: começa no servidor e lê a etapa a cada segundo até acabar.
    fn codex_import(&mut self, kind: Import, cx: &mut Context<Self>) {
        let Some(id) = self.accounts.codex.as_ref().filter(|f| !f.importing).and_then(|f| f.account.clone()) else { return };
        let Some((turn, api)) = self.codex_turn() else { return };
        let done = self.codex_send();
        let Some(flow) = self.accounts.codex.as_mut() else { return };
        (flow.question, flow.importing, flow.error, flow.stage, flow.result) = (None, true, None, String::new(), None);
        flow.task = Some(self.runtime.spawn(async move {
            let fail = |error: Failure| Hangar::setting_failure(&error);
            let out = match kind {
                Import::Inherit => {
                    let mut reply = api.server_post(&["codex-contas", &id, "prepare"], 30).await;
                    loop {
                        let prep = match reply.map_err(fail).and_then(|v| serde_json::from_value::<Preparation>(v).map_err(|_| tr("invalid_response"))) {
                            Ok(prep) => prep,
                            Err(error) => break Err(error),
                        };
                        if prep.status != "running" { break Ok(imported(prep)); }
                        done(CodexReply::Stage(turn, preparation_text(prep.etapa.as_deref()))).await;
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        reply = api.server_read(&["codex-contas", &id, "prepare"], &[], 8).await;
                    }
                }
                Import::Claude => {
                    let mut reply = api.server_post(&["harness", "codex", "integracao"], 30).await;
                    loop {
                        let state = match reply { Ok(state) => state, Err(error) => break Err(fail(error)) };
                        let now = state.get("estado").and_then(Value::as_str).unwrap_or_default();
                        if now != "executando" {
                            break Ok(Imported { ok: now == "ok", inherited: Vec::new(), issues: Vec::new(), trust_pending: false });
                        }
                        done(CodexReply::Stage(turn, integration_text(state.get("etapa")))).await;
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        reply = api.server_read(&["harness", "codex", "integracao"], &[], 8).await;
                    }
                }
            };
            done(CodexReply::Imported(turn, out)).await
        }));
        cx.notify();
    }

    /// Quem aprova hook é o Codex, nunca o app: cria (ou reaproveita) a sessão `codex-<conta>` na primeira pasta
    /// liberada e abre a conversa dela, onde o Codex pergunta.
    fn codex_open_session(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.accounts.codex.as_ref().filter(|f| !f.opening).and_then(|f| f.account.clone()) else { return };
        let Some((turn, api)) = self.codex_turn() else { return };
        let done = self.codex_send();
        let Some(flow) = self.accounts.codex.as_mut() else { return };
        (flow.opening, flow.error) = (true, None);
        flow.task = Some(self.runtime.spawn(async move {
            let opened = async {
                let roots = api.server_read(&["fs", "roots"], &[], 8).await.map_err(|e| Hangar::failure(&e))?;
                let cwd = roots.as_array().and_then(|r| r.first()).and_then(|r| r.get("path")).and_then(Value::as_str)
                    .ok_or_else(|| tr("accounts_codex_no_folder"))?.to_owned();
                let name = format!("codex-{id}");
                let body = json!({"name": name, "cwd": cwd, "provider": "codex", "codex_account": id});
                match api.server_send(reqwest::Method::POST, &["sessions"], Some(body), 60).await {
                    // Já existe (409), ou a resposta se perdeu: a lista das sessões diz se ela está lá.
                    Ok(_) => {}
                    Err(error) if error.status == Some(409) || error.uncertain => {}
                    Err(error) => return Err(Hangar::failure(&error)),
                }
                let sessions = api.sessions().await.map_err(|e| Hangar::failure(&e))?;
                sessions.into_iter().find(|s| s.name == name).ok_or_else(|| tr("accounts_codex_session_failed"))
            }.await;
            done(CodexReply::Session(turn, opened)).await
        }));
        cx.notify();
    }

    /// Fechar ou concluir: o painel sai e a lista relida mostra a conta como o servidor a vê.
    fn codex_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts.codex.as_ref().is_some_and(|f| f.busy || f.opening) { return; }
        self.accounts.codex = None;
        self.load_accounts(false, cx);
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn open_reset(&mut self, id: String, cx: &mut Context<Self>) {
        if self.accounts_busy() || self.accounts.reset.as_ref().is_some_and(ResetTry::holds) { return; }
        let Some(offer) = self.find_row(&id).and_then(|r| r.reset.as_ref()).filter(|o| o.blocked.is_none()) else { return };
        let (account, credit) = (offer.account.clone(), offer.credit.clone());
        self.accounts.outcome = None;
        self.accounts.reset = Some(ResetTry { id, account, credit, key: idempotency_key(), consuming: false, pending: false, error: None });
        cx.notify();
    }

    fn consume_reset(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let done = self.codex_send();
        let Some(r) = self.accounts.reset.as_mut().filter(|r| !r.consuming) else { return };
        (r.consuming, r.error) = (true, None);
        let (id, account, body) = (r.id.clone(), r.account.clone(), json!({"credit_id": r.credit, "idempotency_key": r.key}));
        self.runtime.spawn(async move {
            done(CodexReply::Reset(id, api.server_send(reqwest::Method::POST, &["codex-contas", &account, "rate-limit-reset"], Some(body), 90).await)).await
        });
        cx.notify();
    }

    pub(super) fn receive_codex(&mut self, reply: CodexReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            CodexReply::Default(turn, result) => {
                // Lista indisponível: segue criando a conta com nome, como no web.
                let default = result.ok().and_then(|v| serde_json::from_value::<Vec<CodexAccount>>(v).ok())
                    .and_then(|list| list.into_iter().find(|a| a.is_default && a.auth.status == "disconnected"));
                let Some(flow) = self.codex_flow(turn) else { return };
                flow.task = None;
                if let Some(account) = default {
                    flow.account = Some(account.id);
                    self.codex_poll(cx);
                }
            }
            CodexReply::Created(turn, id) => if let Some(flow) = self.codex_flow(turn) { flow.account = Some(id); },
            CodexReply::Attempt(turn, result, started) => {
                let Some(flow) = self.codex_flow(turn) else { return };
                flow.busy = false;
                let attempt = match result.map(|v| (!v.is_null()).then(|| serde_json::from_value::<Attempt>(v))) {
                    Ok(None) => None,
                    Ok(Some(Ok(attempt))) => Some(attempt),
                    Ok(Some(Err(_))) => { flow.error = Some(tr("invalid_response")); return; }
                    Err(error) => {
                        flow.error = Some(match &error {
                            e if e.uncertain && started => tr("accounts_codex_start_uncertain"),
                            e if e.status.is_none() && !e.uncertain => tr("accounts_codex_read_failed"),
                            e => Hangar::setting_failure(e),
                        });
                        return;
                    }
                };
                flow.error = None;
                flow.attempt_error = attempt.as_ref().and_then(|a| a.error.as_ref()).map(issue_text);
                flow.url = attempt.as_ref().and_then(|a| a.verification_url.as_deref()).and_then(|u| url::Url::parse(u).ok())
                    .filter(|u| u.scheme() == "https" && u.username().is_empty() && u.password().is_none()).map(|u| u.to_string());
                let status = attempt.as_ref().map(|a| a.status.clone());
                flow.attempt = attempt;
                match status.as_deref() {
                    Some("completed") => self.codex_after_login(cx),
                    Some("waiting") if started => self.codex_poll(cx),
                    _ => {}
                }
            }
            CodexReply::After(turn, offer) => {
                let Some(flow) = self.codex_flow(turn) else { return };
                flow.task = None;
                match offer {
                    Some(kind) => flow.question = Some(kind),
                    None => self.codex_close(window, cx),
                }
            }
            CodexReply::Stage(turn, text) => if let Some(flow) = self.codex_flow(turn) { flow.stage = text; },
            CodexReply::Imported(turn, result) => {
                let Some(flow) = self.codex_flow(turn) else { return };
                (flow.importing, flow.stage, flow.task) = (false, String::new(), None);
                match result {
                    Ok(done) if done.ok && done.issues.is_empty() && !done.trust_pending => self.codex_close(window, cx),
                    Ok(done) => flow.result = Some(done),
                    Err(error) => {
                        flow.error = Some(error);
                        flow.result = Some(Imported { ok: false, inherited: Vec::new(), issues: Vec::new(), trust_pending: false });
                    }
                }
            }
            CodexReply::Session(turn, result) => {
                let Some(flow) = self.codex_flow(turn) else { return };
                (flow.opening, flow.task) = (false, None);
                match result {
                    Ok(session) => {
                        self.accounts.codex = None;
                        self.load_accounts(false, cx);
                        self.close_settings(window, cx);
                        self.select(session, window, cx);
                    }
                    Err(error) => flow.error = Some(error),
                }
            }
            CodexReply::Reset(id, result) => {
                let Some(r) = self.accounts.reset.as_mut().filter(|r| r.id == id) else {
                    // A pergunta saiu com a página: a lista relida mostra se a cota mudou.
                    self.load_accounts(true, cx);
                    return;
                };
                r.consuming = false;
                match result {
                    Ok(value) => {
                        let outcome = value.get("outcome").and_then(Value::as_str).unwrap_or_default();
                        let applied = matches!(outcome, "reset" | "alreadyRedeemed");
                        let text = tr(match outcome {
                            "reset" => "accounts_reset_success", "alreadyRedeemed" => "accounts_reset_already",
                            "nothingToReset" => "accounts_reset_nothing", _ => "accounts_reset_no_credit",
                        });
                        self.accounts.reset = None;
                        self.accounts.outcome = Some((text, !applied));
                        self.load_accounts(true, cx);
                        self.accounts.reset_refresh = Some((self.accounts.list.seq, applied));
                    }
                    // A pergunta e a chave ficam: repetir é a mesma tentativa, que o servidor não gasta duas vezes. Uma recusa
                    // depois de uma perda não apaga a perda: o resultado dela continua desconhecido.
                    Err(error) => {
                        r.pending |= error.uncertain;
                        r.error = Some(if error.uncertain { tr("accounts_reset_uncertain") } else { Hangar::failure(&error) });
                    }
                }
            }
        }
    }

    /// O painel no lugar da lista: os passos do web, um por vez.
    pub(super) fn render_codex(&self, cx: &mut Context<Self>) -> Option<Div> {
        let f = self.accounts.codex.as_ref()?;
        let (color, _) = theme::provider("codex");
        let avatar = div().size(px(36.)).flex_shrink_0().rounded(px(10.)).bg(color.opacity(0.16)).flex().items_center().justify_center()
            .text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child("X");
        let title = match &f.account { Some(id) => tr("accounts_codex_title_of").replace("{name}", id), None => tr("accounts_add_codex") };
        let head = div().flex().items_center().gap(px(12.)).child(avatar)
            .child(div().flex().flex_col().gap(px(2.)).min_w_0()
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("accounts_login_server").replace("{server}", &self.server_label(cx)))));
        let line = |text: String, color: Hsla| div().text_size(px(13.)).whitespace_normal().child(div().text_color(color).child(text));
        let strong = |text: String| div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(text);
        let footer = || div().mt(px(4.)).flex().justify_end().gap(px(8.));
        let close = |label: &str, primary: bool, cx: &mut Context<Self>| {
            let button = Button::new(SharedString::from(format!("accounts-codex-close-{label}"))).small().label(tr(label)).disabled(f.busy || f.opening);
            if primary { button.primary() } else { button.outline() }.on_click(cx.listener(|this, _, window, cx| this.codex_close(window, cx)))
        };
        let mut panel = settings_box().mt(px(24.)).p(px(20.)).gap(px(16.)).child(head);
        let waiting = f.attempt.as_ref().filter(|a| a.status == "waiting");
        if let Some(attempt) = waiting {
            let number = |n: &'static str| div().size(px(22.)).flex_shrink_0().rounded_full().border_1().border_color(theme::border_strong())
                .flex().items_center().justify_center().text_size(px(12.)).text_color(theme::muted()).child(n);
            let step = |n: &'static str, title: &'static str, body: AnyElement| div().flex().gap(px(12.)).child(number(n))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(8.)).child(div().font_weight(FontWeight::MEDIUM).child(tr(title))).child(body));
            let link = match f.url.clone() {
                Some(url) => {
                    let open = url.clone();
                    div().flex().flex_col().gap(px(6.))
                        .child(div().flex().child(Button::new("accounts-codex-open").outline().small().icon(IconName::ExternalLink)
                            .label(tr("accounts_login_open")).on_click(move |_, _, cx| cx.open_url(&open))))
                        .child(div().min_w_0().truncate().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::faint()).child(url))
                }
                None => div(),
            };
            panel = panel.child(line(tr("accounts_codex_waiting"), theme::muted()))
                .child(step("1", "accounts_codex_step_open", link.into_any_element()));
            if let Some(code) = attempt.user_code.clone() {
                let copy = code.clone();
                let body = div().flex().items_center().gap(px(12.)).flex_wrap()
                    .child(div().font_family(theme::MONO).text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::accent()).child(code))
                    .child(Button::new("accounts-codex-copy").outline().small().icon(IconName::Copy)
                        .label(tr(if f.copied { "accounts_login_copied" } else { "accounts_codex_copy_code" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            if let Some(flow) = this.accounts.codex.as_mut() { flow.copied = true; }
                            cx.notify();
                        })));
                panel = panel.child(step("2", "accounts_codex_step_code", body.into_any_element()));
            }
            return Some(panel.children(f.error.clone().map(|e| line(e, theme::danger()))).child(footer()
                .when(f.error.is_some(), |el| el.child(Button::new("accounts-codex-reread").outline().small().label(tr("accounts_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.codex_poll(cx)))))
                .child(Button::new("accounts-codex-cancel").outline().small().label(tr(if f.busy { "accounts_codex_cancelling" } else { "accounts_codex_cancel" }))
                    .disabled(f.busy).on_click(cx.listener(|this, _, _, cx| this.codex_cancel(cx))))));
        }
        if let Some(kind) = f.question {
            let (question, desc, now) = match kind {
                Import::Claude => ("accounts_codex_import_question", "accounts_codex_import_desc", "accounts_codex_import_now"),
                Import::Inherit => ("accounts_codex_inherit_question", "accounts_codex_inherit_desc", "accounts_codex_inherit_now"),
            };
            return Some(panel.child(line(tr("accounts_codex_done"), theme::success())).child(strong(tr(question))).child(line(tr(desc), theme::muted()))
                .child(footer().child(close("accounts_codex_later", false, cx))
                    .child(Button::new("accounts-codex-import").primary().small().label(tr(now))
                        .on_click(cx.listener(move |this, _, _, cx| this.codex_import(kind, cx))))));
        }
        // Como no web, importando não há saída pelo painel: fechar esconderia o resultado (avisos, hooks a aprovar).
        if f.importing {
            return Some(panel.child(line(tr("accounts_codex_importing"), theme::muted()))
                .when(!f.stage.is_empty(), |el| el.child(line(f.stage.clone(), theme::muted()))));
        }
        if let Some(r) = &f.result {
            panel = panel.child(match (r.ok, &f.error) {
                (true, _) => line(tr("accounts_codex_imported"), theme::success()),
                (false, Some(error)) => line(error.clone(), theme::danger()),
                (false, None) => line(tr("accounts_codex_import_failed"), theme::danger()),
            });
            if !r.inherited.is_empty() { panel = panel.child(line(r.inherited.join(" · "), theme::muted())); }
            panel = panel.children(r.issues.iter().map(|(text, deliberate)| line(text.clone(), if *deliberate { theme::warning() } else { theme::danger() })));
            if r.trust_pending {
                return Some(panel.child(strong(tr("accounts_codex_trust"))).child(line(tr("accounts_codex_trust_how"), theme::muted()))
                    .children(f.error.clone().filter(|_| r.ok).map(|e| line(e, theme::danger())))
                    .child(footer().child(close("accounts_codex_later", false, cx))
                        .child(Button::new("accounts-codex-session").primary().small()
                            .label(tr(if f.opening { "accounts_codex_opening" } else { "accounts_codex_open_session" })).disabled(f.opening)
                            .on_click(cx.listener(|this, _, _, cx| this.codex_open_session(cx))))));
            }
            return Some(panel.child(footer().child(close("accounts_codex_finish", true, cx))));
        }
        if f.attempt.as_ref().is_some_and(|a| a.status == "completed") {
            return Some(panel.child(line(tr("accounts_codex_done"), theme::success())));
        }
        panel = panel.child(line(tr("accounts_codex_intro"), theme::muted()));
        if f.account.is_none() {
            let hint = tr("accounts_codex_name_hint").replace("{id}", if f.slug.is_empty() { "…" } else { &f.slug });
            panel = panel.child(div().flex().flex_col().gap(px(6.)).max_w(px(420.))
                .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(tr("accounts_codex_name")))
                .child(Input::new(&f.name).disabled(f.busy).aria_label(tr("accounts_codex_name")))
                .child(line(hint, theme::muted())));
        }
        let can_start = f.account.is_some() || !f.slug.is_empty();
        Some(panel.children(f.attempt_error.clone().map(|e| line(e, theme::danger())))
            .when(f.attempt.as_ref().is_some_and(|a| a.status == "cancelled"), |el| el.child(line(tr("accounts_codex_cancelled"), theme::muted())))
            .children(f.error.clone().map(|e| line(e, theme::danger())))
            .child(footer().child(close("accounts_engine_close", false, cx))
                .child(Button::new("accounts-codex-start").primary().small()
                    .label(tr(match (f.busy, f.account.is_some()) { (false, _) => "accounts_sign_in", (true, true) => "accounts_codex_requesting",
                        (true, false) => "accounts_codex_creating" }))
                    .disabled(f.busy || !can_start).on_click(cx.listener(|this, _, _, cx| this.codex_start(cx))))))
    }

    /// Abaixo da linha Codex: quantas redefinições há, o botão (desligado com o motivo) e a pergunta antes de gastar.
    /// `offer` some quando o crédito acaba; a tentativa desta linha continua aparecendo sem ela.
    pub(super) fn render_reset(&self, row: &Row, offer: Option<&ResetOffer>, size: f32, cx: &mut Context<Self>) -> Div {
        let asking = self.accounts.reset.as_ref().filter(|r| r.id == row.id);
        let held = self.accounts_busy() || self.accounts.reset.as_ref().is_some_and(ResetTry::holds);
        let small = |text: String| div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(text);
        let mut block = div().px_4().pb(px(14.)).pl(px(16. + size + 12.)).flex().flex_col().gap(px(8.));
        if let Some(offer) = offer {
            let id = row.id.clone();
            let use_button = Button::new(SharedString::from(format!("accounts-reset-{}", row.id))).outline().small().label(tr("accounts_reset_use"))
                .disabled(held || offer.blocked.is_some() || asking.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.open_reset(id.clone(), cx)));
            block = block.child(div().flex().items_center().gap(px(12.)).flex_wrap()
                .child(div().flex().flex_col().gap(px(2.)).child(div().text_size(px(13.)).child(offer.count.clone())).children(offer.expires.clone().map(small)))
                .child(use_button)
                .children(offer.blocked.clone().map(small)));
        }
        if let Some(r) = asking {
            // Com o resultado desconhecido não há Cancelar: o que resta é repetir com a mesma chave até o servidor responder.
            let ok = if r.consuming { "accounts_reset_consuming" } else if r.pending { "accounts_reset_retry" } else { "accounts_reset_confirm_ok" };
            block = block.child(div().max_w(px(560.)).p(px(12.)).rounded(px(8.)).border_1().border_color(theme::border_strong()).flex().flex_col().gap(px(10.))
                .child(div().text_size(px(13.)).whitespace_normal().child(tr("accounts_reset_confirm")))
                .children(r.error.clone().map(|e| div().text_size(px(12.5)).whitespace_normal().child(div().text_color(theme::danger()).child(e))))
                .child(div().flex().gap(px(8.))
                    .child(Button::new("accounts-reset-ok").danger().small().label(tr(ok))
                        .disabled(r.consuming).on_click(cx.listener(|this, _, _, cx| this.consume_reset(cx))))
                    .when(!r.pending, |el| el.child(Button::new("accounts-reset-cancel").ghost().small().label(tr("cancel")).disabled(r.consuming)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.accounts.reset.as_ref().is_some_and(|r| !r.holds()) { this.accounts.reset = None; }
                            cx.notify();
                        }))))));
        }
        block
    }
}

#[cfg(test)]
mod tests {
    use super::{Preparation, idempotency_key, imported};
    use serde_json::json;

    #[test]
    fn keys_are_distinct_uuid_v4() {
        let (a, b) = (idempotency_key(), idempotency_key());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn deliberate_issue_is_a_warning_and_counts_skip_zero() {
        let prep: Preparation = serde_json::from_value(json!({"status": "partial", "trust_pending": true,
            "issues": [{"code": "codex_account_mcp_runtime_excluded", "params": {"server": "s", "variable": "V"}}, {"code": "codex_account_conflict"}],
            "herdado": {"skills": 2, "hooks": 0}})).unwrap();
        let out = imported(prep);
        assert!(!out.ok && out.trust_pending);
        assert_eq!(out.inherited.len(), 1);
        assert!(out.issues[0].1 && !out.issues[1].1);
    }
}
