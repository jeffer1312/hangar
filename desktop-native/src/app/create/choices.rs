//! As escolhas finas do diálogo: modelo, esforço e permissão pelo catálogo global, motor, subagentes e Jev, a cota de cada
//! conta, criar e apagar conta Claude e o contexto estendido do Codex (`CreateSessionSheet.svelte`, `CodexContextControl.svelte`).
//! Toda escolha gravada saiu de uma leitura do servidor; leitura que falhou deixa o campo no padrão, nunca num valor inventado.
use super::*;
use gpui_kit::component::{checkbox::Checkbox, switch::Switch};

/// Os níveis de esforço fechados de cada provider, os do backend (`model_args.py`). O Kimi não tem nível; o Codex vem por modelo.
pub(in crate::app) const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub(in crate::app) const PI_EFFORTS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
pub(in crate::app) const PERMISSIONS: [&str; 6] = ["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"];
pub(in crate::app) const CODEX_PERMISSIONS: [&str; 3] = ["Ask for approval", "Approve for me", "Full Access"];
/// Faixas da cota, as da `QuotaStrip` do web: âmbar acima de 80%, vermelho acima de 90%.
const QUOTA_WARN: f64 = 80.;
const QUOTA_FULL: f64 = 90.;

#[derive(Clone, Debug, Deserialize)]
pub(super) struct ModelOption {
    id: String,
    name: Option<String>,
    provider: Option<String>,
    context_length: Option<f64>,
    context: Option<String>,
    vision: Option<bool>,
    images: Option<bool>,
    #[serde(default)] efforts: Vec<String>,
    #[serde(default)] service_tiers: Vec<Value>,
    #[serde(default, rename = "supports_fast")] fast: bool,
}

impl ModelOption {
    fn supports_fast(&self) -> bool {
        self.service_tiers.iter().any(|tier| tier.get("id").and_then(Value::as_str) == Some("priority")
            && tier.get("hidden").and_then(Value::as_bool) != Some(true))
    }
    /// `provider/id` quando há provider: o catálogo do Pi repete ids entre providers (`valorModelo` do web).
    fn value(&self) -> String { self.provider.as_ref().map(|p| format!("{p}/{}", self.id)).unwrap_or_else(|| self.id.clone()) }
    fn label(&self) -> String { self.name.clone().unwrap_or_else(|| self.id.clone()) }
    fn hint(&self) -> String {
        let context = self.context.clone().or_else(|| self.context_length.map(|n| format!("{}K", (n / 1000.).round())));
        [self.name.as_ref().filter(|n| **n != self.id).map(|_| self.id.clone()), self.provider.clone(), context,
            (self.vision.or(self.images) == Some(true)).then(|| "👁".to_owned())].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }
}

pub(super) struct Catalog { models: Vec<ModelOption>, reduced: bool }

fn matched_model<'a>(catalog: &'a [ModelOption], provider: &str, local_proxy: bool, model: &str) -> Option<&'a ModelOption> {
    catalog.iter().find(|m| m.value() == model).or_else(|| {
        let base = model.strip_suffix("[1m]")?;
        catalog.iter().find(|m| provider == "claude" && local_proxy && m.fast && m.value() == base)
    })
}

fn chosen_model(catalog: &[ModelOption], provider: &str, local_proxy: bool, current: &str, next: &str) -> String {
    if provider == "claude" && current.ends_with("[1m]")
        && fast_model_available(catalog, provider, local_proxy, current)
        && fast_model_available(catalog, provider, local_proxy, next) {
        controls::context_model(next, true)
    } else { next.to_owned() }
}

fn remembered_choice(catalog: &[ModelOption], provider: &str, local_proxy: bool, remembered: (String, String)) -> (String, String) {
    let (value, effort) = remembered;
    let model = matched_model(catalog, provider, local_proxy, &value);
    let value = if model.is_some() { value } else { String::new() };
    let levels = match provider {
        "codex" => model.map(|m| m.efforts.clone()).unwrap_or_default(),
        "claude" => CLAUDE_EFFORTS.map(String::from).to_vec(),
        "pi" | "omp" => PI_EFFORTS.map(String::from).to_vec(),
        _ => Vec::new(),
    };
    let effort = if levels.contains(&effort) { effort } else { String::new() };
    (value, effort)
}

fn selected_model_valid(catalog: &[ModelOption], provider: &str, local_proxy: bool, model: &str) -> bool {
    !model.is_empty() && matched_model(catalog, provider, local_proxy, model).is_some()
}

fn fast_model_available(catalog: &[ModelOption], provider: &str, local_proxy: bool, model: &str) -> bool {
    matched_model(catalog, provider, local_proxy, model).is_some_and(|m| match provider {
        "codex" => m.supports_fast(),
        "claude" => local_proxy && m.fast,
        _ => false,
    })
}

fn reloaded_choice(catalog: &[ModelOption], provider: &str, local_proxy: bool,
    current: (String, String), remembered: (String, String)) -> (String, String) {
    let context = (provider == "claude" && local_proxy && !current.0.is_empty()).then_some(current.0.ends_with("[1m]"));
    let choice = if provider == "claude" && fast_model_available(catalog, provider, local_proxy, &current.0) { current } else { remembered };
    let (model, effort) = remembered_choice(catalog, provider, local_proxy, choice);
    let model = match context {
        Some(on) if fast_model_available(catalog, provider, local_proxy, &model) => controls::context_model(&model, on),
        _ => model,
    };
    (model, effort)
}

fn creation_tier(tier: Option<&str>, available: bool, fresh: bool) -> Option<&str> {
    tier.filter(|tier| available && fresh && matches!(*tier, "default" | "priority"))
}

fn model_memory_key(server: &str, provider: &str, account: &str, engine: &str, engine_account: &str) -> String {
    let who = if provider == "codex" { account } else if engine.is_empty() { "-" } else { engine };
    let key = format!("cp_last_model:{server}:{provider}:{who}");
    if provider == "claude" && !engine_account.is_empty() { format!("{key}:account:{engine_account}") } else { key }
}

/// O que a tela de criação traria sem toque, sem motor e na conta padrão: padrão marcado do harness, senão o último
/// modelo lembrado; a permissão só vem do padrão marcado. Bloqueante (lê disco).
pub(in crate::app) fn creation_defaults(server: &str, provider: &str) -> ((String, String), Option<String>) {
    let saved = crate::appearance::harness_default(&format!("{server}:{provider}:"));
    let account = if provider == "codex" { "default" } else { "" };
    let remembered = saved.clone().map(|(m, e, _)| (m, e))
        .unwrap_or_else(|| crate::appearance::last_model(&model_memory_key(server, provider, account, "", "")));
    (remembered, saved.map(|s| s.2))
}

/// O lembrado só vale se ainda estiver no catálogo (`/api/model-options`), como na tela; `None` = catálogo ilegível.
pub(in crate::app) fn checked_choice(catalog: &Value, provider: &str, remembered: (String, String)) -> Option<(String, String)> {
    let models: Vec<ModelOption> = serde_json::from_value(catalog.get("models").cloned().unwrap_or_default()).ok()?;
    Some(reloaded_choice(&models, provider, false, (String::new(), String::new()), remembered))
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Motor {
    label: Option<String>,
    #[serde(default)] model: String,
    cliproxy_accounts: Option<Vec<crate::api::dto::CliProxyAccount>>,
    cliproxy_error: Option<String>,
}

fn eligible_engine_account(account: &crate::api::dto::CliProxyAccount) -> bool {
    !account.account.is_empty() && account.credential_id.strip_prefix("codex:").is_some_and(|id| !id.is_empty())
}

fn choose_engine_account<'a>(accounts: &'a [crate::api::dto::CliProxyAccount], selected: &str) -> Option<&'a str> {
    accounts.iter().find(|a| a.account == selected && eligible_engine_account(a))
        .or_else(|| accounts.iter().find(|a| eligible_engine_account(a))).map(|a| a.account.as_str())
}

fn engine_account_ready(motor: &Motor, account: &str) -> bool {
    motor.cliproxy_error.is_none() && motor.cliproxy_accounts.as_ref().is_none_or(|list|
        list.iter().any(|a| a.account == account && eligible_engine_account(a)))
}

/// O Jev só existe com a chave guardada no servidor; o padrão é o `jev_padrao` lido de lá.
pub(super) struct Jev { key: bool, default: bool }

#[derive(Clone, Debug, Deserialize)]
pub(super) struct QuotaLine {
    id: String,
    #[serde(rename = "estado", default)] state: String,
    #[serde(rename = "janelas", default)] windows: Vec<QuotaWindow>,
    #[serde(rename = "motivo")] reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct QuotaWindow { #[serde(rename = "rotulo")] label: String, pct: Option<f64>, reset_ts: Option<f64> }

impl QuotaLine {
    fn windows(&self) -> impl Iterator<Item = (&QuotaWindow, f64)> { self.windows.iter().filter_map(|w| w.pct.filter(|p| p.is_finite()).map(|p| (w, p))) }
    /// "5h 42% · 7d 18%", vazio sem leitura (`resumoCota`).
    fn summary(&self) -> String {
        if self.state != "lida" { return String::new(); }
        self.windows().map(|(w, p)| format!("{} {}%", w.label, p.round())).collect::<Vec<_>>().join(" · ")
    }
}

/// Cota lida com a janela geral (sessão `5h` ou semana `7d`) cheia e ainda não renovada: `Some` com a volta, a da última
/// janela cheia (com duas, só volta quando as duas voltarem). Janela por modelo, leitura que não é `lida` ou janela cuja
/// volta já passou não bloqueiam: a leitura é de antes da renovação.
pub(super) fn exhausted(quota: &QuotaLine, now: f64) -> Option<Option<f64>> {
    if quota.state != "lida" { return None; }
    let full: Vec<&QuotaWindow> = quota.windows()
        .filter(|(w, pct)| matches!(w.label.as_str(), "5h" | "7d") && *pct >= 100. && w.reset_ts.is_none_or(|r| r > now))
        .map(|(w, _)| w).collect();
    if full.is_empty() { return None; }
    Some(full.iter().filter_map(|w| w.reset_ts).reduce(f64::max))
}

/// A conta para onde sair quando a escolhida (`selected`) tem a janela geral esgotada: a de mais folga (100 menos a maior
/// janela, a regra do `sugerir_claude` do backend) entre as de cota lida e não esgotada, empate com a ativa. `None` quando a
/// escolhida não está esgotada, não tem cota conhecida ou nenhuma outra serve.
fn quota_switch(selected: Option<&str>, accounts: &[(&str, bool, Option<&QuotaLine>)], now: f64) -> Option<String> {
    exhausted(accounts.iter().find(|a| Some(a.0) == selected)?.2?, now)?;
    accounts.iter().filter_map(|&(path, active, quota)| {
        let quota = quota.filter(|q| exhausted(q, now).is_none() && q.state == "lida")?;
        Some((100. - quota.windows().map(|(_, p)| p).reduce(f64::max)?, active, path))
    }).max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1))).map(|(_, _, path)| path.to_owned())
}

/// "2h10", "35m", "3d4h": quanto falta para a janela voltar (`faltaPara` do web).
fn until(reset: Option<f64>, now: f64) -> String {
    let Some(s) = reset.filter(|r| r.is_finite()).map(|r| r - now).filter(|s| *s > 0.) else { return String::new() };
    let min = (s / 60.).floor() as i64;
    if min < 60 { return format!("{min}m"); }
    let h = min / 60;
    if h < 24 { return if min % 60 != 0 { format!("{h}h{:02}", min % 60) } else { format!("{h}h") }; }
    if h % 24 != 0 { format!("{}d{}h", h / 24, h % 24) } else { format!("{}d", h / 24) }
}

/// Conta Claude no seletor do diálogo: a esgotada aparece apagada e não se escolhe.
#[derive(Clone)]
pub(in crate::app) struct AccountChoice { choice: ModelChoice, exhausted: bool }

impl SearchableListItem for AccountChoice {
    type Value = String;
    fn title(&self) -> SharedString { self.choice.title() }
    fn value(&self) -> &String { self.choice.value() }
    fn render(&self, window: &mut Window, cx: &mut App) -> impl IntoElement { self.choice.render(window, cx) }
    fn disabled(&self) -> bool { self.exhausted }
}

/// Uma resposta de conta: criar (o POST e a lista relida) ou apagar (nome, caminho, o DELETE e a lista relida).
pub(in crate::app) enum AccountDone {
    Added(Result<Value, Failure>, Option<Result<Value, Failure>>),
    Deleted(String, String, Result<Value, Failure>, Option<Result<Value, Failure>>),
}

fn configs_of(result: Result<Value, Failure>) -> Result<Vec<ConfigDir>, String> {
    result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response")))
}

async fn owner_config(api: &Api) -> (bool, Result<Option<Value>, Failure>) {
    match api.server_read(&["me"], &[], 8).await {
        Ok(me) if me.get("role").and_then(Value::as_str) == Some("guest") => return (false, Ok(None)),
        Ok(me) if me.get("role").and_then(Value::as_str) == Some("owner") => {},
        Err(error) if error.status == Some(404) => {},
        Err(error) => return (false, Err(error)),
        _ => return (false, Err(Failure::local(tr("invalid_response")))),
    }
    (true, api.server_read(&["config"], &[], 8).await.map(Some))
}

impl NewSession {
    pub(super) fn load_extras(&mut self, cx: &mut Context<Self>) {
        let (engines, config, quotas) = (self.engines.start(), self.jev.start(), self.quotas.start());
        self.request(cx, move |api, send| Box::pin(async move {
            let (e, c, q) = tokio::join!(api.server_read(&["engines"], &[], 15), owner_config(&api),
                api.server_read(&["cotas"], &[], 15));
            send(CreateReply::Engines(engines, e)).await;
            send(CreateReply::Config(config, c)).await;
            send(CreateReply::Quotas(quotas, q)).await;
        }));
    }

    /// A tela compacta também lê o modo padrão do servidor.
    pub(in crate::app) fn load_quotas(&mut self, cx: &mut Context<Self>) {
        let (seq, config) = (self.quotas.start(), self.jev.start());
        self.request(cx, move |api, send| Box::pin(async move {
            let (q, c) = tokio::join!(api.server_read(&["cotas"], &[], 15), owner_config(&api));
            send(CreateReply::Quotas(seq, q)).await;
            send(CreateReply::Config(config, c)).await;
        }));
    }

    /// As contas Claude oferecidas. O `/api/cotas` só traz conta de verdade (carimbada pelo app) e a ativa, o mesmo corte do
    /// `/api/credenciais` da página Contas: pasta de backup (`~/.claude-x.bak-…`) fica fora. Sem a cota lida, vão todas.
    pub(super) fn accounts(&self) -> impl Iterator<Item = &ConfigDir> {
        let quotas = self.quotas.ok();
        self.configs.ok().into_iter().flatten()
            .filter(move |c| c.active || quotas.is_none_or(|list| list.iter().any(|q| q.id.strip_prefix("claude:") == Some(c.path.as_str()))))
    }

    /// A chave da memória do último modelo: servidor, provider e a conta do Codex ou o motor (`chaveMemoria` do web).
    pub(super) fn memory_key(&self) -> String {
        model_memory_key(&self.link.api.identity(), self.provider, &self.codex_account, &self.engine, &self.engine_account)
    }

    fn selected_engine(&self) -> Option<&Motor> {
        (self.provider == "claude").then(|| self.engines.ok()?.iter().find(|(name, _)| *name == self.engine).map(|(_, motor)| motor)).flatten()
    }

    pub(super) fn proxy_accounts(&self) -> Option<&[crate::api::dto::CliProxyAccount]> {
        self.selected_engine()?.cliproxy_accounts.as_deref()
    }

    pub(super) fn proxy_account(&self) -> Option<&crate::api::dto::CliProxyAccount> {
        self.proxy_accounts()?.iter().find(|a| a.account == self.engine_account)
    }

    pub(super) fn engine_ready(&self) -> bool {
        if self.provider != "claude" || self.engine.is_empty() { return true; }
        !self.engines.loading && self.selected_engine().is_some_and(|m| engine_account_ready(m, &self.engine_account))
            && (self.proxy_accounts().is_none() || (!self.models.loading && self.models.ok().is_some_and(|c| !c.models.is_empty())
                && self.config.as_ref().is_some_and(|path| self.configs.ok().is_some_and(|list| list.iter().any(|c| &c.path == path)))))
    }

    pub(super) fn resume_ready(&self) -> bool {
        self.engine_ready() && (self.proxy_accounts().is_none() || selected_model_valid(self.catalog(), self.provider, true, &self.model))
    }

    pub(super) fn proxy_note(&self) -> Option<String> {
        if self.provider != "claude" || self.engine.is_empty() { return None; }
        if self.engines.loading { return Some(tr("loading")); }
        if let Some(Err(error)) = &self.engines.value { return Some(error.clone()); }
        let Some(motor) = self.selected_engine() else { return Some(tr("create_engine_missing").replace("{name}", &self.engine)); };
        if let Some(error) = &motor.cliproxy_error { return Some(tr("create_proxy_error").replace("{reason}", error)); }
        let accounts = motor.cliproxy_accounts.as_ref()?;
        if accounts.is_empty() { return Some(tr("create_proxy_no_accounts")); }
        if self.proxy_account().is_none() { return Some(tr("create_proxy_choose_account")); }
        if self.configs.loading { return Some(tr("loading")); }
        if let Some(Err(error)) = &self.configs.value { return Some(error.clone()); }
        if self.config.is_none() { return Some(tr("new_chat_no_accounts")); }
        if !self.models.loading && self.models.ok().is_some_and(|c| c.models.is_empty()) { return Some(tr("create_proxy_no_models")); }
        None
    }

    pub(super) fn build_engine_account_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(accounts) = self.proxy_accounts() else {
            self.engine_account.clear();
            self.engine_account_pick = None;
            return;
        };
        let selected = choose_engine_account(accounts, &self.engine_account).unwrap_or_default().to_owned();
        self.engine_account = selected;
        let choices: Vec<ModelChoice> = self.proxy_accounts().unwrap_or_default().iter().filter(|a| eligible_engine_account(a)).map(|a| ModelChoice {
            id: a.account.clone(), label: if a.label.is_empty() { a.email.clone() } else { a.label.clone() },
            hint: [Some(a.email.clone()), self.quota_of(&a.credential_id).map(QuotaLine::summary)]
                .into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "),
        }).collect();
        let at = choices.iter().position(|a| a.id == self.engine_account);
        self.engine_account_pick = Some(picker(choices, at, false, |this, account, window, cx| {
            if this.creating || this.engine_account == account { return; }
            this.engine_account = account;
            this.load_models(window, cx);
        }, window, cx));
    }

    /// O catálogo da conta escolhida reconfirma a escolha do proxy antes de deixá-la criar.
    pub(super) fn load_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_customizations(window, cx);
        let seq = self.models.start();
        if self.proxy_accounts().is_none() { self.model.clear(); self.effort.clear(); }
        self.subagent.clear();
        self.service_tier = None;
        self.build_model_picks(window, cx);
        if (self.provider == "codex" && self.codex_account.is_empty())
            || (self.provider == "claude" && !self.engine.is_empty()
                && !self.selected_engine().is_some_and(|m| engine_account_ready(m, &self.engine_account))) {
            self.models.finish(seq, Ok(Catalog { models: Vec::new(), reduced: false }));
            return;
        }
        let mut query = vec![("provider".to_owned(), self.provider.to_owned())];
        if self.provider == "claude" && !self.engine.is_empty() {
            query.push(("engine".into(), self.engine.clone()));
            if self.proxy_accounts().is_some() { query.push(("engine_account".into(), self.engine_account.clone())); }
        }
        if let Some(config) = self.config.clone() { query.push(("config_dir".into(), config)); }
        if self.provider == "codex" { query.push(("codex_account".into(), self.codex_account.clone())); }
        let (key, default_key) = (self.memory_key(), self.default_key());
        self.request(cx, move |api, send| Box::pin(async move {
            // O padrão marcado do harness vence a última escolha lembrada.
            let (remembered, saved) = tokio::task::spawn_blocking(move || {
                let saved = crate::appearance::harness_default(&default_key);
                (saved.clone().map(|(m, e, _)| (m, e)).unwrap_or_else(|| crate::appearance::last_model(&key)), saved)
            }).await.unwrap_or_default();
            let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            send(CreateReply::Models(seq, api.server_read(&["model-options"], &query, 30).await, remembered, saved)).await
        }));
    }

    /// A conta do proxy também separa o padrão: seu catálogo pode ser diferente.
    fn default_key(&self) -> String {
        let key = format!("{}:{}:{}", self.link.api.identity(), self.provider, if self.provider == "claude" { self.engine.as_str() } else { "" });
        if self.proxy_accounts().is_some() { format!("{key}:account:{}", self.engine_account) } else { key }
    }

    fn current_choice(&self) -> (String, String, String) {
        let permission = if self.is_transfer() { self.saved_default.as_ref().map(|s| s.2.clone()).unwrap_or_default() }
            else if self.permissions().is_some() { self.permission.clone() } else { String::new() };
        (self.model.clone(), self.effort.clone(), permission)
    }

    /// Marcar grava modelo, esforço e permissão de agora como padrão do harness; desmarcar apaga. Marcado = a escolha de agora é o padrão.
    pub(super) fn render_default_check(&self, cx: &mut Context<Self>) -> Option<Div> {
        self.models.ok()?;
        if self.proxy_accounts().is_some() && !self.engine_ready() { return None; }
        let checked = self.saved_default.as_ref() == Some(&self.current_choice());
        let label = tr("create_default_for_harness").replace("{harness}", provider_name(self.provider));
        Some(div().px(px(4.)).child(Checkbox::new("create-default-harness").label(label).checked(checked).disabled(self.creating)
            .on_change(cx.listener(|this, on: &bool, _, cx| {
                let value = on.then(|| this.current_choice());
                // Na hora, num arquivo pequeno: o check só fica marcado se gravou, e duas gravações seguidas não se cruzam.
                match crate::appearance::set_harness_default(&this.default_key(), value.clone()) {
                    Ok(()) => this.saved_default = value,
                    Err(error) => this.error = Some(tr("create_default_failed").replace("{erro}", &error)),
                }
                cx.notify();
            }))))
    }

    pub(super) fn receive_models(&mut self, seq: u64, result: Result<Value, Failure>, remembered: (String, String),
        saved: Option<(String, String, String)>, window: &mut Window, cx: &mut Context<Self>) {
        let catalog = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| {
            let mut models: Vec<ModelOption> = serde_json::from_value(v.get("models").cloned().unwrap_or_default()).map_err(|_| tr("invalid_response"))?;
            // O picker do Claude dá o mesmo id (`opus`) às versões antigas: escolher "Opus 4.6" abriria o Opus atual.
            // Linha que não dá para escolher de verdade não aparece.
            let mut seen = std::collections::HashSet::new();
            models.retain(|m| seen.insert(m.value().to_owned()));
            Ok(Catalog { models, reduced: v.get("reduced").and_then(Value::as_bool).unwrap_or(false) })
        });
        if !self.models.finish(seq, catalog) { return; }
        // Permissão escolhida à mão nesta tela fica; "" é a opção "Padrão" do seletor.
        if let Some(permission) = saved.as_ref().map(|s| s.2.clone()).filter(|p| !self.permission_touched
            && self.permissions().is_some_and(|list| p.is_empty() || list.contains(&p.as_str()))) {
            self.permission = permission;
            self.build_permission_pick(window, cx);
        }
        self.saved_default = saved;
        // O lembrado só volta com a lista lida, se ainda estiver nela, e o esforço só se couber no modelo que ficou.
        if self.models.ok().is_some() {
            (self.model, self.effort) = reloaded_choice(self.catalog(), self.provider, self.proxy_accounts().is_some(),
                (self.model.clone(), self.effort.clone()), remembered);
        }
        self.build_model_picks(window, cx);
    }

    fn catalog(&self) -> &[ModelOption] { self.models.ok().map(|c| c.models.as_slice()).unwrap_or_default() }

    pub(super) fn has_transfer_models(&self) -> bool { self.models.ok().is_some_and(|c| !c.models.is_empty()) }

    pub(super) fn transfer_quota_pct(&self) -> Option<f64> {
        let credential = self.codex.ok()?.iter().find(|a| a.id == self.codex_account)?.credential_id.as_deref()?;
        let quota = self.quota_of(credential)?;
        if quota.state != "lida" { return None; }
        let now = chrono::Local::now().timestamp() as f64;
        quota.windows().filter(|(w, _)| matches!(w.label.as_str(), "5h" | "7d") && w.reset_ts.is_none_or(|r| r > now))
            .map(|(_, pct)| pct).reduce(f64::max)
    }

    /// Os níveis do modelo escolhido: fechados por provider, e os do próprio modelo no Codex.
    pub(super) fn levels(&self) -> Vec<String> {
        match self.provider {
            "codex" => self.catalog().iter().find(|m| m.id == self.model).map(|m| m.efforts.clone()).unwrap_or_default(),
            "claude" => CLAUDE_EFFORTS.map(String::from).to_vec(),
            "pi" | "omp" => PI_EFFORTS.map(String::from).to_vec(),
            _ => Vec::new(),
        }
    }

    pub(super) fn fast_available(&self) -> bool {
        fast_model_available(self.catalog(), self.provider, self.proxy_accounts().is_some(), &self.model)
    }

    pub(super) fn service_tier_for_creation(&self) -> Option<&str> {
        creation_tier(self.service_tier.as_deref(), self.fast_available(),
            !self.is_transfer() && self.baton.is_none() && !self.want_resume)
    }

    pub(super) fn render_fast_choice(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !(self.provider == "codex" || (self.provider == "claude" && self.proxy_accounts().is_some()))
            || self.is_transfer() || self.baton.is_some() || self.want_resume { return None; }
        let available = self.fast_available();
        let on = self.service_tier_for_creation() == Some("priority");
        let hint = if !available { "ctl_fast_unavailable" }
            else if self.service_tier.is_none() { "create_fast_default_hint" } else { "ctl_fast_hint" };
        Some(div().flex().items_center().gap_2().px_2().py_1()
            .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                .child(div().text_sm().child(tr("ctl_fast")))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr(hint))))
            .child(Switch::new("new-chat-fast").small().checked(on).accessibility_label(tr("ctl_fast"))
                .disabled(self.creating || !available)
                .on_change(cx.listener(|this, on: &bool, _, cx| {
                    if !this.fast_available() { return; }
                    this.service_tier = Some(if *on { "priority" } else { "default" }.into());
                    cx.notify();
                }))))
    }

    pub(super) fn render_engine_context(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.provider != "claude" || !self.fast_available() { return None; }
        Some(div().flex().items_center().gap_2().px_2().py_1()
            .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                .child(div().text_sm().child(tr("create_context_title")))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("create_engine_context_help"))))
            .child(Switch::new("new-chat-engine-context").small().checked(self.model.ends_with("[1m]"))
                .accessibility_label(tr("create_context_title")).disabled(self.creating || !self.engine_ready())
                .on_change(cx.listener(|this, on: &bool, window, cx| {
                    if this.creating || !this.engine_ready() || this.provider != "claude" || !this.fast_available() { return; }
                    this.model = controls::context_model(&this.model, *on);
                    this.model_choice_touched = true;
                    this.build_model_picks(window, cx);
                    cx.notify();
                }))))
    }

    /// A permissão existe para o Claude e para o Codex sem terminal, cada um com a própria lista.
    pub(super) fn permissions(&self) -> Option<&'static [&'static str]> {
        if self.is_transfer() { return None; }
        match (self.provider, self.headless && !self.headless_inherited()) { ("claude", _) => Some(&PERMISSIONS), ("codex", true) => Some(&CODEX_PERMISSIONS), _ => None }
    }

    fn pick_at(choices: &[ModelChoice], value: &str) -> Option<usize> { choices.iter().position(|c| c.id == value).or(Some(0)) }

    /// Os seletores do trio e do subagente, montados de novo quando a lista ou o valor muda por fora do clique.
    pub(super) fn build_model_picks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default = || ModelChoice { id: String::new(), label: tr("create_default"), hint: String::new() };
        let models: Vec<ModelChoice> = std::iter::once(default()).chain(self.catalog().iter().filter(|m| m.id != "default")
            .map(|m| ModelChoice { id: m.value(), label: m.label(), hint: m.hint() })).collect();
        let selected = matched_model(self.catalog(), self.provider, self.proxy_accounts().is_some(), &self.model)
            .map(ModelOption::value).unwrap_or_else(|| self.model.clone());
        let at = Self::pick_at(&models, &selected);
        self.model_pick = Some(picker(models, at, true, |this, id, window, cx| {
            this.model_choice_touched = true;
            this.model = chosen_model(this.catalog(), this.provider, this.proxy_accounts().is_some(), &this.model, &id);
            if !this.fast_available() { this.service_tier = None; }
            // Trocar de modelo pode tirar o nível escolhido da lista (só o Codex tem níveis por modelo).
            if !this.levels().contains(&this.effort) { this.effort.clear(); }
            this.build_effort_pick(window, cx);
        }, window, cx));
        let subagents: Vec<ModelChoice> = std::iter::once(ModelChoice { id: String::new(), label: tr("create_subagent_default"), hint: String::new() })
            .chain(self.catalog().iter().filter(|m| m.id != "default").map(|m| ModelChoice { id: m.value(), label: m.label(), hint: String::new() })).collect();
        let at = Self::pick_at(&subagents, &self.subagent);
        self.subagent_pick = Some(picker(subagents, at, true, |this, id, _, _| { this.model_choice_touched = true; this.subagent = id; }, window, cx));
        self.build_effort_pick(window, cx);
        self.build_permission_pick(window, cx);
    }

    fn build_effort_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choices: Vec<ModelChoice> = std::iter::once(String::new()).chain(self.levels())
            .map(|n| ModelChoice { label: if n.is_empty() { tr("create_default") } else { n.clone() }, id: n, hint: String::new() }).collect();
        let at = Self::pick_at(&choices, &self.effort);
        self.effort_pick = Some(picker(choices, at, false, |this, id, _, _| { this.model_choice_touched = true; this.effort = id; }, window, cx));
    }

    pub(super) fn build_permission_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(modes) = self.permissions() else { self.permission_pick = None; return };
        let choices: Vec<ModelChoice> = std::iter::once(ModelChoice { id: String::new(), label: tr("create_permission_default"), hint: String::new() })
            .chain(modes.iter().map(|m| ModelChoice { id: (*m).into(), label: (*m).into(), hint: String::new() })).collect();
        let at = Self::pick_at(&choices, &self.permission);
        self.permission_pick = Some(picker(choices, at, false, |this, id, _, _| (this.permission, this.permission_touched) = (id, true), window, cx));
    }

    /// A conta Claude com a cota de cada uma na dica ("atual · 5h 42% · 7d 18%").
    /// A dica da conta Claude: "atual · 5h 42% · 7d 18%".
    pub(super) fn config_hint(&self, c: &ConfigDir) -> String {
        let quota = self.quota_of(&format!("claude:{}", c.path)).map(QuotaLine::summary).unwrap_or_default();
        [c.active.then(|| tr("create_current")), Some(quota).filter(|q| !q.is_empty())].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }

    /// A pílula de modelo da tela sem sessão, dentro do compositor: o provider, o modelo e o esforço escolhidos.
    pub(super) fn render_model_pill(&self, cx: &mut Context<Self>) -> Div {
        let id = Menu::Model.anchor();
        let model = matched_model(self.catalog(), self.provider, self.proxy_accounts().is_some(), &self.model).map(ModelOption::label)
            .unwrap_or_else(|| provider_name(self.provider).to_owned());
        popup::anchor(div(), id).child(chrome::pill_button(id, cx).gap(px(6.)).selected(self.menu.get() == Some(Menu::Model)).disabled(self.creating)
            .accessibility_label(format!("{}: {model}", tr("create_model")))
            .child(chrome::provider_glyph(self.provider, 16.))
            .child(div().max_w(px(160.)).truncate().text_xs().font_weight(FontWeight::SEMIBOLD).child(model))
            .when(!self.effort.is_empty(), |el| el.child(div().text_xs().text_color(theme::muted()).child(self.effort.clone())))
            .when(self.service_tier_for_creation() == Some("priority"), |el|
                el.child(div().id("new-chat-fast-active").text_xs().text_color(theme::muted()).child(tr("ctl_fast"))))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_menu(Menu::Model, window, cx))))
    }

    pub(super) fn render_transfer_model(&self, cx: &mut Context<Self>) -> Div {
        use gpui_kit::component::popover::Popover;
        let title = matched_model(self.catalog(), self.provider, self.proxy_accounts().is_some(), &self.model).map(ModelOption::label).unwrap_or_else(|| tr("create_default"));
        let view = cx.entity().downgrade();
        let opening = view.clone();
        div().flex().flex_col().gap_2().child(label(tr("create_model")))
            .child(Popover::new("transfer-model-menu").open(self.menu.get() == Some(Menu::Model))
                .trigger(Button::new("transfer-model-trigger").outline().label(title).icon(IconName::ChevronDown)
                    .disabled(self.creating || self.models.loading || !self.has_transfer_models()).accessibility_label(tr("create_model")))
                .content(move |_, window, cx| view.update(cx, |view, cx| view.render_model_menu(cx)
                    .w((window.rem_size() * 24.).min(window.viewport_size().width - window.rem_size() * 2.))
                    .into_any_element()).unwrap_or_else(|_| div().into_any_element()))
                .on_open_change(move |open, window, cx| { let _ = opening.update(cx, |view, cx| {
                    view.menu.set(open.then_some(Menu::Model));
                    if *open { view.menu_query.update(cx, |input, cx| { input.set_value("", window, cx); input.focus(window, cx); }); }
                    cx.notify();
                }); }))
            .when(!self.effort.is_empty(), |el| el.child(muted(format!("{}: {}", tr("create_effort"), self.effort))))
    }

    /// O menu da pílula de modelo, no desenho do Zeron: o provider em abas, a busca, a lista e o esforço no rodapé.
    pub(super) fn render_model_menu(&self, cx: &mut Context<Self>) -> Div {
        let query = self.menu_filter(cx);
        let tabs = div().id("new-chat-providers").role(Role::Group).aria_label(tr("create_provider_aria")).flex().items_center().gap(px(2.))
            .px(px(4.)).pb(px(4.)).children(PROVIDERS.iter().map(|&p| {
                let available = self.providers.ok().and_then(|m| m.get(p)).is_none_or(|probe| probe.disponivel);
                Button::new(SharedString::from(format!("new-chat-provider-{p}"))).ghost().small().selected(self.provider == p)
                    .disabled(!available || self.creating).tooltip(provider_name(p)).accessibility_label(provider_name(p))
                    .child(chrome::provider_glyph(p, 18.))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_provider(p, window, cx)))
            }));
        let list = match &self.models.value {
            _ if self.models.loading || self.models.value.is_none() => popup::skeleton("new-chat-models", 5).into_any_element(),
            Some(Err(error)) => Self::menu_failure("new-chat-models-error", format!("{}: {error}", tr("create_models_failed")),
                |this, window, cx| this.load_models(window, cx), cx),
            _ => {
                let rows = std::iter::once((String::new(), tr("create_default"), String::new()))
                    .chain(self.catalog().iter().filter(|m| m.id != "default").map(|m| (m.value(), m.label(), m.hint())))
                    .filter(|(_, label, hint)| wanted(&query, label, hint))
                    .map(|(id, label, hint)| {
                        let on = self.model == id || matched_model(self.catalog(), self.provider, self.proxy_accounts().is_some(), &self.model)
                            .is_some_and(|m| m.value() == id);
                        menu_row(SharedString::from(format!("new-chat-model-{id}")), on, label, hint)
                            .disabled(self.creating || (self.proxy_accounts().is_some() && !self.engine_ready()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // O menu fica aberto: o esforço, logo abaixo, costuma ser a escolha seguinte.
                                this.model_choice_touched = true;
                                this.model = chosen_model(this.catalog(), this.provider, this.proxy_accounts().is_some(), &this.model, &id);
                                if !this.fast_available() { this.service_tier = None; }
                                if !this.levels().contains(&this.effort) { this.effort.clear(); }
                                this.build_effort_pick(window, cx);
                                cx.notify();
                            }))
                            .into_any_element()
                    }).collect();
                Self::menu_list("new-chat-model-list", rows)
            }
        };
        let levels = self.levels();
        // "Padrão" mais até 4 níveis numa linha; mais que isso quebra linha em vez de cortar o rótulo.
        let many = levels.len() > 4;
        let effort = (!levels.is_empty()).then(|| div().flex().flex_col().gap(px(2.))
            .child(popup::separator())
            .child(popup::title(tr("new_chat_reasoning"), None))
            .child(div().id("new-chat-efforts").role(Role::Group).aria_label(tr("new_chat_reasoning")).px(px(4.)).pb(px(2.)).flex()
                .when(many, |el| el.flex_wrap())
                .gap(px(2.)).children(std::iter::once(String::new()).chain(levels).map(|level| {
                    let label = if level.is_empty() { tr("create_default") } else { level.clone() };
                    Button::new(SharedString::from(format!("new-chat-effort-{level}"))).ghost().xsmall().when(!many, |b| b.flex_1().min_w_0())
                        .selected(self.effort == level).label(label).disabled(self.creating)
                        .on_click(cx.listener(move |this, _, window, cx| { this.model_choice_touched = true; this.effort = level.clone(); this.build_effort_pick(window, cx); cx.notify(); }))
                }))));
        let default = self.render_default_check(cx).map(|check| div().flex().flex_col().gap(px(4.)).child(popup::separator()).child(check.py(px(4.))));
        let engine = (self.compact && self.provider == "claude").then(|| {
            let body = if self.engines.loading { popup::skeleton("new-chat-engines", 1).into_any_element() }
                else if let Some(Err(error)) = &self.engines.value {
                    Self::menu_failure("new-chat-engines-error", error.clone(), |this, _, cx| this.load_extras(cx), cx)
                } else { div().when_some(self.engine_pick.as_ref(), |el, (pick, _)| el
                    .child(label(tr("create_engine")))
                    .child(Select::new(pick).id("new-chat-engine").disabled(self.creating).accessibility_label(tr("create_engine"))))
                    .into_any_element() };
            div().px_1().py_1().flex().flex_col().gap_2().child(body)
                .when(self.proxy_accounts().is_some(), |el| el.child(self.render_engine_account()))
        });
        div().p(px(popup::INSET)).flex().flex_col().gap(px(2.)).when(!self.is_transfer(), |el| el.child(tabs))
            .children(engine).child(self.menu_search()).child(list).children(effort).children(self.render_fast_choice(cx))
            .children(self.render_engine_context(cx)).children(default)
    }

    pub(super) fn build_config_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.configs.ok().is_none() { self.config_pick = None; return; }
        // A esgotada fica na lista, apagada; a já escolhida continua clicável (sem outra que sirva, ela fica).
        let choices: Vec<AccountChoice> = self.accounts().map(|c| AccountChoice {
            choice: ModelChoice { id: c.path.clone(), label: c.label.clone(), hint: self.config_hint(c) },
            exhausted: Some(&c.path) != self.config.as_ref() && self.account_exhausted(&c.path),
        }).collect();
        let at = choices.iter().position(|c| Some(&c.choice.id) == self.config.as_ref());
        self.config_pick = Some(picker(choices, at, false, |this, path, window, cx| {
            (this.config, this.account_touched) = (Some(path), true);
            this.load_models(window, cx);
        }, window, cx));
    }

    /// Cota lida com a janela de sessão ou de semana cheia e ainda não renovada.
    pub(super) fn account_exhausted(&self, path: &str) -> bool {
        self.quota_of(&format!("claude:{path}")).is_some_and(|q| exhausted(q, chrono::Local::now().timestamp() as f64).is_some())
    }

    pub(super) fn build_engine_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.engines.ok().map(Vec::as_slice).unwrap_or_default();
        if list.is_empty() && self.engine.is_empty() { self.engine_pick = None; return; }
        let choices: Vec<ModelChoice> = std::iter::once(ModelChoice { id: String::new(), label: tr("create_own_account"), hint: String::new() })
            .chain(list.iter().map(|(name, m)| ModelChoice { id: name.clone(), label: m.label.clone().unwrap_or_else(|| name.clone()), hint: m.model.clone() }))
            .collect();
        let at = choices.iter().position(|c| c.id == self.engine);
        self.engine_pick = Some(picker(choices, at, false, |this, name, window, cx| {
            if this.creating || this.engine == name { return; }
            this.engine = name;
            this.model.clear();
            this.effort.clear();
            this.asking = false;
            this.confirming = false;
            this.build_engine_account_pick(window, cx);
            this.load_models(window, cx);
        }, window, cx));
    }

    /// A conta Claude escolhida com a cota esgotada troca sozinha pela de mais folga, salvo escolha à mão no menu.
    /// A conversa fechada não passa por aqui: lá o envio fica bloqueado. `true` quando trocou.
    pub(super) fn leave_exhausted_account(&mut self) -> bool {
        // Com o menu de conta aberto, a lista não muda debaixo do clique: fechar ou a próxima cota reavalia.
        if self.account_touched || self.menu.get() == Some(Menu::Account) || self.reopen_config.is_some() || self.creating || self.account_busy
            || self.provider != "claude" || !self.engine.is_empty() || self.target().is_some() { return false; }
        let accounts: Vec<_> = self.accounts().map(|c| (c.path.as_str(), c.active, self.quota_of(&format!("claude:{}", c.path)))).collect();
        let Some(path) = quota_switch(self.config.as_deref(), &accounts, chrono::Local::now().timestamp() as f64) else { return false };
        self.config = Some(path);
        true
    }

    pub(super) fn quota_of(&self, id: &str) -> Option<&QuotaLine> { self.quotas.ok()?.iter().find(|q| q.id == id) }

    pub(super) fn receive_extra(&mut self, reply: CreateReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            CreateReply::Engines(seq, result) => {
                let list = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| {
                    let map: HashMap<String, Motor> = serde_json::from_value(v.get("motores").cloned().unwrap_or_default()).map_err(|_| tr("invalid_response"))?;
                    let mut list: Vec<(String, Motor)> = map.into_iter().collect();
                    list.sort_by(|a, b| a.0.cmp(&b.0));
                    Ok(list)
                });
                if self.engines.finish(seq, list) {
                    self.build_engine_pick(window, cx);
                    self.build_engine_account_pick(window, cx);
                    if self.provider == "claude" && !self.engine.is_empty() { self.load_models(window, cx); }
                }
            }
            CreateReply::Config(seq, (owner, result)) => {
                if seq != self.jev.seq { return; }
                self.headless_owner = Some(owner);
                if !self.headless_touched && !self.creating {
                    if let Ok(Some(value)) = &result {
                        self.headless = value.pointer("/campos/headless_default/valor").and_then(Value::as_bool).unwrap_or(true);
                        self.build_permission_pick(window, cx);
                    } else if let Err(error) = &result { self.error = Some(Hangar::fetch_failure(error)); }
                }
                let jev = result.map_err(|e| Hangar::fetch_failure(&e)).map(|v| Jev {
                    key: v.as_ref().and_then(|v| v.pointer("/campos/jev_api_key/definido")).and_then(Value::as_bool).unwrap_or(false),
                    default: v.as_ref().and_then(|v| v.pointer("/campos/jev_padrao/valor")).and_then(Value::as_bool).unwrap_or(false),
                });
                if self.jev.finish(seq, jev) { self.jev_on = self.jev.ok().is_some_and(|j| j.default); }
            }
            CreateReply::Quotas(seq, result) => {
                let list = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response")));
                if !self.quotas.finish(seq, list) { return; }
                if self.is_transfer() { return; }
                // A escolhida antes da cota chegar pode ser uma pasta que não é conta.
                let stale = self.config.as_ref().is_some_and(|path| !self.accounts().any(|c| &c.path == path));
                if stale { self.config = self.fallback_config(); }
                if self.leave_exhausted_account() || stale { self.load_models(window, cx); }
                self.build_config_pick(window, cx);
                self.build_engine_account_pick(window, cx);
            }
            CreateReply::Models(seq, result, remembered, saved) => self.receive_models(seq, result, remembered, saved, window, cx),
            CreateReply::Context(seq, result) => {
                if seq != self.context_seq { return; }
                self.context_busy = false;
                self.context_want = None;
                match result.map(|v| v.get("contexto_estendido").and_then(Value::as_bool)) {
                    Ok(Some(on)) => (self.context_on, self.context_error) = (Some(on), None),
                    Ok(None) => self.context_error = Some(tr("invalid_response")),
                    Err(error) => self.context_error = Some(Hangar::fetch_failure(&error)),
                }
            }
            CreateReply::Account(seq, done) => self.account_done(seq, done, window, cx),
            _ => {}
        }
    }

    /// Com o Jev na tela, a escolha dele vai para a criação; o que mudou vira o padrão do servidor (`salvarPadraoJev`).
    pub(super) fn jev_choice(&self) -> Option<(bool, bool)> { self.jev.ok().filter(|j| j.key).map(|j| (self.jev_on, self.jev_on != j.default)) }

    // Contexto estendido do Codex: a leitura e a gravação travam o Criar, como o `contextBusy` do web.
    pub(super) fn load_context(&mut self, cx: &mut Context<Self>) {
        self.context_seq += 1;
        let seq = self.context_seq;
        (self.context_busy, self.context_error) = (true, None);
        self.request(cx, move |api, send| Box::pin(async move {
            send(CreateReply::Context(seq, api.server_read(&["harness", "codex", "opcoes"], &[], 8).await)).await
        }));
    }

    /// Saiu do Codex: a leitura em voo não trava mais nada.
    pub(super) fn drop_context(&mut self) { self.context_seq += 1; self.context_busy = false; self.context_want = None; }

    /// Saiu do Codex: a lista de contas em voo cai (senão ela relê catálogo e arquivo do provider novo) e o Codex volta ao
    /// estado de antes de entrar, como a limpeza do efeito do web.
    pub(super) fn drop_codex(&mut self) {
        let seq = self.codex.seq + 1;
        self.codex = Remote::default();
        self.codex.seq = seq;
        (self.codex_account, self.codex_pick) = (String::new(), None);
    }

    fn toggle_context(&mut self, cx: &mut Context<Self>) {
        let Some(on) = self.context_on.filter(|_| !self.context_busy) else { return };
        self.context_seq += 1;
        let seq = self.context_seq;
        (self.context_busy, self.context_want, self.context_error) = (true, Some(!on), None);
        self.request(cx, move |api, send| Box::pin(async move {
            let body = json!({"contexto_estendido": !on});
            send(CreateReply::Context(seq, api.server_send(reqwest::Method::POST, &["harness", "codex", "opcoes"], Some(body), 8).await)).await
        }));
        cx.notify();
    }

    // Conta Claude: "+ conta" e "Apagar". A operação trava a linha inteira; a lista é relida depois, numa fase própria.
    fn deletable(&self) -> Option<String> {
        let selected = self.configs.ok()?.iter().find(|c| Some(&c.path) == self.config.as_ref())?;
        if selected.active { return None; }
        crate::app::accounts::claude_folder_name(&selected.path).map(str::to_owned)
    }

    fn open_account_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        (self.asking, self.confirming, self.notice, self.created_path) = (true, false, None, None);
        self.account_name.update(cx, |input, cx| { input.set_value("", window, cx); input.focus(window, cx); });
        cx.notify();
    }

    pub(super) fn add_account(&mut self, cx: &mut Context<Self>) {
        if self.account_busy { return; }
        let name = self.account_name.read(cx).value().trim().to_owned();
        if name.is_empty() { (self.notice, self.created_path) = (None, None); cx.notify(); return; }
        self.account_seq += 1;
        let seq = self.account_seq;
        (self.account_busy, self.notice) = (true, None);
        self.request(cx, move |api, send| Box::pin(async move {
            let created = api.server_send(reqwest::Method::POST, &["claude-configs"], Some(json!({"nome": name})), 30).await;
            let list = if created.is_ok() { Some(api.server_read(&["claude-configs"], &[], 15).await) } else { None };
            send(CreateReply::Account(seq, AccountDone::Added(created, list))).await
        }));
        cx.notify();
    }

    fn delete_account(&mut self, cx: &mut Context<Self>) {
        let (Some(name), Some(path)) = (self.deletable(), self.config.clone()) else { return };
        if self.account_busy { return; }
        self.account_seq += 1;
        let seq = self.account_seq;
        (self.account_busy, self.notice, self.created_path) = (true, None, None);
        let keep = if self.keep_transcripts { "1" } else { "0" };
        self.request(cx, move |api, send| Box::pin(async move {
            let deleted = api.server_send_query(reqwest::Method::DELETE, &["claude-configs", &name], &[("keep_transcripts", keep)], None, 120).await;
            let list = if deleted.is_ok() { Some(api.server_read(&["claude-configs"], &[], 15).await) } else { None };
            send(CreateReply::Account(seq, AccountDone::Deleted(name, path, deleted, list))).await
        }));
        cx.notify();
    }

    /// A lista relida entra como leitura nova: a de um pedido anterior ainda em voo cai.
    fn replace_configs(&mut self, list: Vec<ConfigDir>) {
        let seq = self.configs.start();
        self.configs.finish(seq, Ok(list));
    }

    pub(super) fn fallback_config(&self) -> Option<String> {
        self.accounts().find(|c| c.active).or_else(|| self.accounts().next()).map(|c| c.path.clone())
    }

    fn account_done(&mut self, seq: u64, done: AccountDone, window: &mut Window, cx: &mut Context<Self>) {
        if seq != self.account_seq { return; }
        self.account_busy = false;
        match done {
            AccountDone::Added(Err(error), _) | AccountDone::Deleted(_, _, Err(error), _) => self.notice = Some((Hangar::fetch_failure(&error), true)),
            AccountDone::Added(Ok(created), list) => match list.map(configs_of) {
                Some(Ok(list)) => {
                    let path = created.get("path").and_then(Value::as_str).map(str::to_owned);
                    let found = path.filter(|p| list.iter().any(|c| &c.path == p));
                    self.replace_configs(list);
                    match found {
                        Some(path) => {
                            self.asking = false;
                            self.account_name.update(cx, |input, cx| input.set_value("", window, cx));
                            (self.config, self.created_path, self.notice) = (Some(path.clone()), Some(path), Some((tr("create_account_logged_out"), false)));
                            // Conta recém-criada é escolha dela: a cota que chegar depois não a tira daqui.
                            self.account_touched = true;
                            self.build_config_pick(window, cx);
                            self.load_models(window, cx);
                        }
                        None => (self.created_path, self.notice) = (None, Some((tr("create_account_missing"), false))),
                    }
                }
                _ => self.notice = Some((tr("create_account_list_failed"), false)),
            },
            AccountDone::Deleted(name, path, Ok(deleted), list) => {
                self.confirming = false;
                // A caixa fica travada enquanto o DELETE corre: o valor dela ainda é o que foi pedido.
                let kept = crate::app::accounts::kept_transcripts_notice(&name, self.keep_transcripts, &deleted);
                let key = match list.map(configs_of) {
                    Some(Ok(list)) => { self.replace_configs(list); "create_account_deleted" }
                    // O DELETE deu certo: a pasta não existe mais, então ela sai da lista local mesmo sem a releitura.
                    _ => {
                        let list = self.configs.ok().map(|l| l.iter().filter(|c| c.path != path).cloned().collect()).unwrap_or_default();
                        self.replace_configs(list);
                        "create_account_deleted_list"
                    }
                };
                self.config = self.fallback_config();
                // O que o DELETE confirmou sobre as conversas fica; a falha da releitura vem junto.
                self.notice = Some((match kept {
                    Some(kept) if key == "create_account_deleted" => kept,
                    Some(kept) => format!("{kept} {}", crate::i18n::tr_shared("criar_conta_lista_falhou", &[])),
                    None => tr(key).replace("{nome}", &name),
                }, false));
                self.build_config_pick(window, cx);
                self.load_models(window, cx);
            }
        }
        cx.notify();
    }
}

impl NewSession {
    /// A cota da conta escolhida, uma janela por trecho, com a cor da faixa e o número escrito.
    fn render_quota(&self, id: String, quota: &QuotaLine) -> Stateful<Div> {
        let now = chrono::Local::now().timestamp() as f64;
        let row = div().id(SharedString::from(id)).flex().flex_wrap().gap_x(px(6.)).text_size(px(12.)).text_color(theme::muted());
        match quota.state.as_str() {
            "lida" if quota.windows().next().is_some() => row.children(quota.windows().enumerate().map(|(n, (w, pct))| {
                let color = if pct > QUOTA_FULL { theme::danger() } else if pct > QUOTA_WARN { theme::warning() } else { theme::muted() };
                let reset = until(w.reset_ts, now);
                div().flex().gap(px(6.))
                    .when(n > 0, |el| el.child(div().text_color(theme::faint()).child("·")))
                    .child(div().text_color(color).child(format!("{} {}%", w.label, pct.round())))
                    .when(!reset.is_empty(), |el| el.child(div().text_color(theme::faint()).child(tr("create_quota_resets").replace("{quando}", &reset))))
            })),
            "expirada" | "sem_credencial" => row.child(format!("{} {}", tr("create_quota_none"), tr("create_quota_sign_in"))),
            _ => row.child(format!("{} {}", tr("create_quota_none"),
                if quota.reason.as_deref() == Some("renovacao-falhou") { tr("create_quota_stopped") } else { String::new() }).trim().to_owned()),
        }
    }

    /// A linha da cota de `credential` nos menus da tela sem sessão, só sessão (5h) e semana (7d): as janelas por modelo
    /// ficam no diálogo, onde há largura. Nada enquanto não há leitura.
    pub(super) fn quota_line(&self, id: String, credential: &str) -> Option<AnyElement> {
        let mut quota = self.quota_of(credential)?.clone();
        quota.windows.retain(|w| matches!(w.label.as_str(), "5h" | "7d"));
        Some(self.render_quota(id, &quota).into_any_element())
    }

    /// A conta Claude escolhida para retomar a conversa fechada, quando a cota lida dela tem a janela geral (sessão ou semana)
    /// esgotada: o aviso, com a volta se o servidor a deu. Sem leitura, ou leitura vencida, não bloqueia.
    pub(in crate::app) fn reopen_quota_block(&self) -> Option<String> {
        // Sem caminho (`None`) é a conta do próprio servidor fora da lista: sem cota conhecida, não bloqueia.
        let path = self.reopen_config.clone().flatten()?;
        let now = chrono::Local::now().timestamp() as f64;
        let reset = exhausted(self.quota_of(&format!("claude:{path}"))?, now)?;
        let account = self.configs.ok().and_then(|l| l.iter().find(|c| c.path == path)).map(|c| c.label.clone()).unwrap_or(path);
        let when = until(reset, now);
        use super::super::costs::web_with;
        Some(if when.is_empty() { web_with("conversa_conta_sem_cota", &[("conta", account)]) }
            else { web_with("conversa_conta_sem_cota_volta", &[("conta", account), ("quando", when)]) })
    }

    pub(super) fn render_codex_quota(&self, credential: Option<&str>) -> Option<Stateful<Div>> {
        let quota = self.quota_of(credential?)?;
        Some(self.render_quota("create-codex-quota".into(), quota))
    }

    fn render_engine_account(&self) -> Div {
        let invalid = self.selected_engine().is_some_and(|m| m.cliproxy_error.is_some());
        let pick = self.engine_account_pick.as_ref().map(|(pick, _)| Select::new(pick).id("create-engine-account")
            .placeholder(tr("create_proxy_choose_account")).accessibility_label(tr("create_chatgpt_account"))
            .disabled(self.creating || self.engines.loading || invalid || self.proxy_accounts().is_none_or(|a| a.is_empty())));
        let quota = self.proxy_account().and_then(|a| self.quota_of(&a.credential_id));
        div().flex().flex_col().gap_2()
            .child(label(tr("create_chatgpt_account")))
            .children(pick)
            .when_some(quota, |el, q| el.child(self.render_quota("create-engine-quota".into(), q)))
            .when(quota.is_none() && self.proxy_account().is_some(), |el| el.child(muted(tr(if self.quotas.loading { "loading" } else { "create_quota_none" }))))
            .when_some(self.quotas.value.as_ref().and_then(|q| q.as_ref().err()), |el, error| el.child(muted(tr("accounts_failed").replace("{reason}", error))))
            .when_some(self.proxy_note(), |el, note| el.child(if invalid { alert("create-engine-account-error", note).into_any_element() } else { muted(note).into_any_element() }))
    }

    pub(super) fn render_claude_account(&self, cx: &mut Context<Self>) -> Div {
        if self.proxy_accounts().is_some() { return self.render_engine_account(); }
        let busy = self.creating || self.account_busy;
        let deletable = self.deletable().filter(|_| !self.asking && !self.confirming);
        let small = |id: &'static str, text: String| Button::new(id).outline().flex_shrink_0().label(text);
        let picker = match (&self.config_pick, self.configs.value.as_ref()) {
            // A falha vem antes do seletor: a leitura que falhou também deixa um seletor vazio.
            (_, Some(Err(error))) if !self.configs.loading => alert("create-configs-error", error.clone()).into_any_element(),
            (Some((pick, _)), _) if !self.configs.loading => Select::new(pick).disabled(busy).accessibility_label(tr("create_claude_account")).into_any_element(),
            _ => muted(tr("loading")).into_any_element(),
        };
        let selected_quota = self.config.as_ref().and_then(|p| self.quota_of(&format!("claude:{p}")));
        let this = cx.entity().downgrade();
        let esc = move |_: &gpui_kit::component::input::Escape, _: &mut Window, cx: &mut App| {
            let _ = this.update(cx, |this, cx| { this.asking = false; cx.notify(); });
        };
        let notice = self.notice.clone().filter(|_| self.created_path.is_none() || self.created_path == self.config);
        div().flex().flex_col().gap(px(6.))
            .child(label(tr("create_claude_account")))
            .child(div().flex().items_center().gap(px(8.))
                .child(div().flex_1().min_w_0().child(picker))
                .child(small("create-account-add", if self.account_busy { "…".into() } else { tr("create_add_account") }).disabled(busy)
                    .accessibility_label(tr(if self.account_busy { "create_adding_account_aria" } else { "create_add_account_aria" }))
                    .on_click(cx.listener(|this, _, window, cx| this.open_account_line(window, cx))))
                .when_some(deletable, |el, name| el.child(small("create-account-delete", tr("create_delete")).disabled(busy)
                    .accessibility_label(tr("create_delete_account_aria").replace("{nome}", &name))
                    .on_click(cx.listener(|this, _, _, cx| { (this.confirming, this.keep_transcripts, this.notice) = (true, true, None); cx.notify(); })))))
            .when_some(selected_quota, |el, q| el.child(self.render_quota("create-claude-quota".into(), q)))
            .when_some(self.deletable().filter(|_| self.confirming), |el, name| el.child(div().flex().items_center().gap(px(8.))
                .child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
                    .child(format!("{} ", tr("create_delete_start"))).child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(name))
                    .child(if self.keep_transcripts { "?".to_owned() } else { format!(" {}", tr("create_delete_end")) })
                    .flex().flex_wrap().gap_x(px(0.)))
                .child(Button::new("create-account-delete-yes").outline().text_color(theme::danger()).border_color(theme::danger())
                    .label(if self.account_busy { "…".into() } else { tr("create_delete") }).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.delete_account(cx))))
                .child(small("create-account-delete-no", tr("create_cancel")).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| { this.confirming = false; cx.notify(); })))))
            .when(self.confirming && self.deletable().is_some(), |el| el.child(
                Checkbox::new("create-account-keep").small().label(crate::i18n::tr_shared("contas_juntar_conversas", &[]))
                    .checked(self.keep_transcripts).disabled(busy)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| { this.keep_transcripts = *checked; cx.notify(); }))))
            .when(self.asking, |el| {
                let ready = !self.account_name.read(cx).value().trim().is_empty();
                el.child(div().id("create-account-line").flex().items_center().gap(px(8.)).on_action(esc)
                    .child(div().flex_1().min_w_0().child(Input::new(&self.account_name).disabled(busy).aria_label(tr("create_account_new_aria"))))
                    .child(small("create-account-new-ok", tr("create_account_create")).disabled(busy || !ready)
                        .on_click(cx.listener(|this, _, _, cx| this.add_account(cx))))
                    .child(small("create-account-new-no", tr("create_cancel")).disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| { this.asking = false; cx.notify(); }))))
            })
            .when_some(notice, |el, (text, error)| el.child(if error { alert("create-account-notice", text) }
                else { div().id("create-account-notice").role(Role::Status).text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(text) }))
    }

    /// Modelo, esforço e permissão lado a lado; cada um some quando não se aplica.
    pub(super) fn render_trio(&self, model_only: bool) -> Option<Div> {
        let busy = self.creating || (self.proxy_accounts().is_some() && !self.engine_ready());
        let field = |id: &'static str, title: String, pick: &Option<Picker>| pick.as_ref().map(|(p, _)| div().flex_1().min_w(px(150.)).flex().flex_col().gap(px(6.))
            .child(label(title.clone())).child(Select::new(p).id(id).disabled(busy).accessibility_label(title)));
        let model = field("create-pick-model", tr("create_model"), &self.model_pick).map(|el| el
            .when(self.models.ok().is_some_and(|c| c.reduced), |el| el.child(div().id("create-models-reduced").role(Role::Status).child(muted(tr("create_models_reduced")))))
            .when_some(self.models.value.as_ref().and_then(|v| v.as_ref().err()), |el, error| el.child(alert("create-models-error",
                if self.proxy_accounts().is_some() { format!("{}: {error}", tr("create_models_failed")) }
                else { tr("create_models_default").replace("{erro}", &format!("{}: {error}", tr("create_models_failed"))) }))));
        let effort_title = tr(if matches!(self.provider, "pi" | "omp") { "create_reasoning" } else { "create_effort" });
        let effort = (!model_only && !self.levels().is_empty()).then(|| field("create-pick-effort", effort_title, &self.effort_pick)).flatten();
        let permission = (!model_only).then(|| self.permissions().and(field("create-pick-permission", tr("create_permission"), &self.permission_pick))).flatten();
        let fields: Vec<Div> = [model, effort, permission].into_iter().flatten().collect();
        (!fields.is_empty()).then(|| div().flex().flex_wrap().items_start().gap(px(12.)).children(fields))
    }

    pub(super) fn render_context(&self, cx: &mut Context<Self>) -> Div {
        let checked = self.context_want.or(self.context_on).unwrap_or(false);
        let state = if self.context_busy { Some(tr(if self.context_want.is_some() { "create_context_saving" } else { "loading" })) } else { None };
        div().flex().flex_col().gap(px(6.))
            .child(div().flex().items_center().gap(px(10.))
                .child(Switch::new("create-context").checked(checked).disabled(self.context_busy || self.context_on.is_none() || self.creating)
                    .label(tr("create_context_title")).on_click(cx.listener(|this, _, _, cx| this.toggle_context(cx))))
                .when_some(state, |el, text| el.child(div().id("create-context-state").role(Role::Status).text_size(px(12.)).text_color(theme::muted()).child(text))))
            .child(muted(tr("create_context_help")))
            .when_some(self.context_error.clone(), |el, error| el.child(alert("create-context-error", error))
                .child(div().child(Button::new("create-context-retry").outline().small().label(tr("create_try_again")).disabled(self.context_busy)
                    .on_click(cx.listener(|this, _, _, cx| { this.load_context(cx); cx.notify(); })))))
    }

    /// Motor logo abaixo do provedor, como no seletor da conversa: a conta e a lista de modelos dependem dele.
    pub(super) fn render_engine(&self) -> Option<Div> {
        if self.provider != "claude" { return None; }
        let loading = self.engines.loading;
        let problem = self.engines.value.as_ref().and_then(|v| v.as_ref().err()).cloned()
            .or_else(|| self.selected_engine().and_then(|m| m.cliproxy_error.as_ref()).map(|e| tr("create_proxy_error").replace("{reason}", e)));
        let pick = self.engine_pick.as_ref();
        if pick.is_none() && !loading && problem.is_none() { return None; }
        Some(div().flex().flex_col().gap(px(6.))
            .child(label(tr("create_engine")))
            .when_some(pick, |el, (p, _)| el.child(Select::new(p).id("create-pick-engine").disabled(self.creating || loading)
                .accessibility_label(tr("create_engine"))))
            .when(loading && pick.is_none(), |el| el.child(muted(tr("loading"))))
            .when_some(problem, |el, problem| el.child(alert("create-engine-error", problem))))
    }

    /// "Mais opções": modelo dos subagentes e Jev, recolhidos, com o valor de cada um no resumo.
    pub(super) fn render_more(&self, cx: &mut Context<Self>) -> Option<Div> {
        // O bastão não leva subagente nem Jev: a rota dele não recebe os dois.
        let subagent = (self.target().is_none() && self.baton.is_none() && self.provider == "claude" && self.engine.is_empty() && !self.catalog().is_empty())
            .then_some(self.subagent_pick.as_ref()).flatten();
        // O retomar não leva o Jev: com uma conversa escolhida, o interruptor seria um controle sem efeito.
        let jev = self.jev_choice().is_some() && self.target().is_none() && self.baton.is_none();
        if subagent.is_none() && !jev { return None; }
        let subagent_label = self.catalog().iter().find(|m| m.value() == self.subagent).map(ModelOption::label)
            .unwrap_or_else(|| if self.subagent.is_empty() { tr("create_subagent_default") } else { self.subagent.clone() });
        let pill = |text: String| div().px(px(8.)).py(px(1.)).rounded_full().border_1().border_color(theme::border()).text_size(px(11.5)).text_color(theme::muted()).child(text);
        let this = cx.entity().downgrade();
        let busy = self.creating || self.engines.loading;
        Some(div().flex().flex_col().gap(px(12.)).px(px(12.)).py(px(10.)).rounded(px(10.)).border_1().border_color(theme::border())
            .child(div().flex().items_center().gap(px(8.)).flex_wrap()
                .child(Disclosure::new("create-more", self.more, tr("create_more"), false)
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.more = open; cx.notify(); }); }))
                .when(!self.more, |el| el
                    .when(subagent.is_some(), |el| el.child(pill(format!("{} {subagent_label}", tr("create_more_subagents")))))
                    .when(jev && self.jev_on, |el| el.child(pill(tr("create_jev"))))))
            .when(self.more, |el| el
                .when_some(subagent, |el, (p, _)| el.child(div().flex().flex_col().gap(px(4.)).child(label(tr("create_subagent")))
                    .child(Select::new(p).disabled(busy).accessibility_label(tr("create_subagent"))).child(muted(tr("create_subagent_help")))))
                .when(jev, |el| el.child(div().flex().flex_col().gap(px(4.))
                    .child(Checkbox::new("create-jev").small().label(tr("create_jev")).checked(self.jev_on).disabled(busy)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| { this.jev_on = *checked; cx.notify(); })))
                    .child(muted(tr("create_jev_help")))))))
    }

    pub(super) fn render_omp(&self) -> Div {
        div().flex().flex_col().gap(px(4.)).child(label(tr("create_omp_profile")))
            .child(Input::new(&self.omp).font_family(theme::MONO).disabled(self.creating).aria_label(tr("create_omp_profile")))
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelOption, QuotaLine, exhausted, quota_switch, until};

    #[test]
    fn fast_creation_requires_an_advertised_priority_tier() {
        let models: Vec<ModelOption> = serde_json::from_value(serde_json::json!([
            {"id":"supported", "service_tiers":[{"id":"priority"}]},
            {"id":"legacy", "additional_speed_tiers":["fast"]},
            {"id":"hidden-tier", "service_tiers":[{"id":"priority", "hidden":true}]},
            {"id":"standard"}
        ])).unwrap();
        assert_eq!(models.iter().map(ModelOption::supports_fast).collect::<Vec<_>>(), [true,false,false,false]);
    }

    #[test]
    fn fast_creation_never_leaks_between_providers_engines_or_models() {
        let models: Vec<ModelOption> = serde_json::from_value(serde_json::json!([
            {"id":"gpt", "supports_fast":true}, {"id":"other", "supports_fast":false},
            {"id":"codex", "service_tiers":[{"id":"priority"}]}
        ])).unwrap();
        let available = |provider, local, model| super::fast_model_available(&models, provider, local, model);
        assert!(available("claude", true, "gpt"));
        assert!(!available("claude", false, "gpt"));
        assert!(!available("claude", true, "other"));
        assert!(!available("claude", true, "account/gpt"));
        assert!(!available("claude", true, "codex"));
        assert!(!available("claude", true, ""));
        assert!(!available("codex", false, "gpt"));
        assert!(available("codex", false, "codex"));
        for provider in ["pi", "omp", "kimi"] { assert!(!available(provider, true, "gpt")); }
        for tier in ["priority", "default"] {
            assert_eq!(super::creation_tier(Some(tier), true, true), Some(tier));
            assert_eq!(super::creation_tier(Some(tier), false, true), None);
            assert_eq!(super::creation_tier(Some(tier), true, false), None);
        }
        assert_eq!(super::creation_tier(None, true, true), None);
        assert_eq!(super::creation_tier(Some("fast"), true, true), None);
    }

    #[test]
    fn proxy_context_matches_base_and_preserves_memory_fast_and_model_changes() {
        let models: Vec<ModelOption> = serde_json::from_value(serde_json::json!([
            {"id":"gpt-one", "supports_fast":true}, {"id":"gpt-two", "supports_fast":true},
            {"id":"other", "supports_fast":false}, {"id":"opus"}, {"id":"opus[1m]"}
        ])).unwrap();
        let model = "gpt-one[1m]";
        assert!(super::selected_model_valid(&models, "claude", true, model));
        assert!(super::fast_model_available(&models, "claude", true, model));
        assert_eq!(super::matched_model(&models, "claude", true, model).unwrap().value(), "gpt-one");
        assert_eq!(super::remembered_choice(&models, "claude", true, (model.into(), "high".into())), (model.into(), "high".into()));
        for (provider, local) in [("claude", false), ("codex", true), ("pi", true)] {
            assert!(!super::selected_model_valid(&models, provider, local, model));
            assert!(!super::fast_model_available(&models, provider, local, model));
        }
        for invalid in ["other[1m]", "account/gpt-one[1m]", "gpt-one[1m][1m]"] {
            assert!(!super::selected_model_valid(&models, "claude", true, invalid));
        }
        let choose = |provider, local, next| super::chosen_model(&models, provider, local, model, next);
        assert_eq!(choose("claude", true, "gpt-two"), "gpt-two[1m]");
        assert_eq!(choose("claude", true, "other"), "other");
        assert_eq!(choose("claude", true, ""), "");
        assert_eq!(choose("claude", false, "gpt-two"), "gpt-two");
        assert_eq!(choose("codex", true, "gpt-two"), "gpt-two");
        assert_eq!(super::matched_model(&models, "claude", false, "opus[1m]").unwrap().value(), "opus[1m]");
        assert_eq!(super::chosen_model(&models, "claude", false, "opus[1m]", "opus"), "opus");
        assert_eq!(super::matched_model(&models[..4], "claude", false, "opus[1m]").map(ModelOption::value), None);
    }

    #[test]
    fn proxy_account_reload_reconfirms_context_and_preserves_explicit_off() {
        let models: Vec<ModelOption> = serde_json::from_value(serde_json::json!([
            {"id":"gpt-one", "supports_fast":true}, {"id":"gpt-two", "supports_fast":true}
        ])).unwrap();
        let reload = |provider, local, current: &str, remembered: &str| super::reloaded_choice(&models, provider, local,
            (current.into(), "high".into()), (remembered.into(), "medium".into()));
        assert_eq!(reload("claude", true, "gpt-one[1m]", "gpt-two"), ("gpt-one[1m]".into(), "high".into()));
        assert_eq!(reload("claude", true, "gpt-one", "gpt-two[1m]"), ("gpt-one".into(), "high".into()));
        assert_eq!(reload("claude", true, "removed[1m]", "gpt-two"), ("gpt-two[1m]".into(), "medium".into()));
        assert_eq!(reload("claude", true, "removed", "gpt-two[1m]"), ("gpt-two".into(), "medium".into()));
        assert_eq!(reload("claude", true, "", "gpt-two[1m]"), ("gpt-two[1m]".into(), "medium".into()));
        assert_eq!(reload("claude", false, "gpt-one[1m]", "gpt-two"), ("gpt-two".into(), "medium".into()));
        assert_eq!(reload("codex", true, "gpt-one[1m]", "gpt-two"), ("gpt-two".into(), String::new()));
        assert_eq!(super::creation_tier(Some("priority"), super::fast_model_available(&models, "claude", true, "gpt-one[1m]"), true), Some("priority"));
    }

    #[test]
    fn proxy_account_fallback_skips_invalid_entries_and_preserves_valid_selection() {
        let accounts = serde_json::from_value::<Vec<crate::api::dto::CliProxyAccount>>(serde_json::json!([
            {"account":"", "credential_id":"codex:x", "email":"", "label":"empty"},
            {"account":"broken", "credential_id":"", "email":"", "label":"broken"},
            {"account":"empty-id", "credential_id":"codex:", "email":"", "label":"empty-id"},
            {"account":"wrong", "credential_id":"claude:x", "email":"", "label":"wrong"},
            {"account":"first", "credential_id":"codex:first", "email":"", "label":"first"},
            {"account":"second", "credential_id":"codex:second", "email":"", "label":"second"}
        ])).unwrap();
        assert_eq!(super::choose_engine_account(&accounts, ""), Some("first"));
        assert_eq!(super::choose_engine_account(&accounts, "broken"), Some("first"));
        assert_eq!(super::choose_engine_account(&accounts, "removed"), Some("first"));
        assert_eq!(super::choose_engine_account(&accounts, "second"), Some("second"));
        assert_eq!(super::choose_engine_account(&accounts[..4], "first"), None);
        assert_eq!(super::choose_engine_account(&[], "first"), None);
    }

    #[test]
    fn proxy_choices_require_an_exact_account_and_memory_is_scoped_to_it() {
        let motor: super::Motor = serde_json::from_value(serde_json::json!({"model":"gpt-6.1", "cliproxy_accounts":[
            {"account":"default", "credential_id":"codex:/home/.codex", "email":"one@example.com", "label":"One"},
            {"account":"other", "credential_id":"codex:/home/.codex-other", "email":"two@example.com", "label":"Two", "prefix":"other"}
        ]})).unwrap();
        assert!(!super::engine_account_ready(&motor, ""));
        assert!(!super::engine_account_ready(&motor, "missing"));
        assert!(super::engine_account_ready(&motor, "default"));
        assert!(super::engine_account_ready(&motor, "other"));
        let empty: super::Motor = serde_json::from_value(serde_json::json!({"cliproxy_accounts":[]})).unwrap();
        assert!(!super::engine_account_ready(&empty, "default"));
        let invalid: super::Motor = serde_json::from_value(serde_json::json!({"cliproxy_accounts":[
            {"account":"other", "credential_id":"codex:/home/.codex-other", "email":"two@example.com", "label":"Two"}],
            "cliproxy_error":"duplicate account"})).unwrap();
        assert!(!super::engine_account_ready(&invalid, "other"));
        let ordinary: super::Motor = serde_json::from_value(serde_json::json!({"model":"k3"})).unwrap();
        assert!(ordinary.cliproxy_accounts.is_none());
        assert!(super::engine_account_ready(&ordinary, ""));
        let key = super::model_memory_key("server", "claude", "", "proxy", "default");
        assert_ne!(key, super::model_memory_key("server", "claude", "", "proxy", "other"));
        assert_ne!(key, super::model_memory_key("another", "claude", "", "proxy", "default"));
        assert_ne!(key, super::model_memory_key("server", "claude", "", "proxy", ""));
        let catalog: Vec<ModelOption> = serde_json::from_value(serde_json::json!([{"id":"gpt-base"}])).unwrap();
        assert!(super::selected_model_valid(&catalog, "claude", true, "gpt-base"));
        assert!(!super::selected_model_valid(&catalog, "claude", true, ""));
        assert!(!super::selected_model_valid(&catalog, "claude", true, "other-prefix/gpt-base"));
        assert!(!super::selected_model_valid(&catalog, "claude", true, "removed-model"));
    }

    #[test]
    fn remembered_codex_choices_are_scoped_and_never_invent_a_model_or_effort() {
        let models: Vec<ModelOption> = serde_json::from_value(serde_json::json!([
            {"id":"model-a", "efforts":["low","high"]}, {"id":"model-b", "efforts":["medium"]}
        ])).unwrap();
        let remembered = |model: &str, effort: &str| super::remembered_choice(&models, "codex", false, (model.into(), effort.into()));
        assert_eq!(remembered("model-a", "high"), ("model-a".into(), "high".into()));
        assert_eq!(remembered("model-b", "high"), ("model-b".into(), String::new()));
        assert_eq!(remembered("claude-model", "high"), (String::new(), String::new()));
        assert_eq!(remembered("", "high"), (String::new(), String::new()));
        assert_eq!(remembered("model-b", "medium"), ("model-b".into(), "medium".into()));
        let key = super::model_memory_key("server-a", "codex", "account-a", "", "");
        assert_ne!(key, super::model_memory_key("server-b", "codex", "account-a", "", ""));
        assert_ne!(key, super::model_memory_key("server-a", "claude", "account-a", "", ""));
        assert_ne!(key, super::model_memory_key("server-a", "codex", "account-b", "", ""));
        let mut remote = super::Remote::<super::Catalog>::default();
        let old = remote.start();
        let current = remote.start();
        assert!(remote.finish(current, Ok(super::Catalog { models, reduced: false })));
        assert!(!remote.finish(old, Ok(super::Catalog { models: Vec::new(), reduced: false })));
        assert_eq!(remote.ok().unwrap().models.len(), 2);
    }

    #[tokio::test]
    async fn config_is_only_requested_for_owner_or_legacy_backend() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, role, owner) in [(200, "guest", false), (200, "owner", true), (404, "", true)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = super::Api::new(&format!("http://{}", listener.local_addr().unwrap()), "test").unwrap();
            let server = tokio::spawn(async move {
                let mut paths = Vec::new();
                let mut replies = vec![(status, serde_json::json!({"role": role}).to_string())];
                if owner { replies.push((200, "{\"campos\":{}}".into())); }
                for (status, body) in replies {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut bytes = vec![0; 4096];
                    let count = stream.read(&mut bytes).await.unwrap();
                    paths.push(String::from_utf8_lossy(&bytes[..count]).lines().next().unwrap().to_owned());
                    let response = format!("HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
                paths
            });
            let (is_owner, config) = super::owner_config(&api).await;
            assert_eq!(is_owner, owner);
            assert_eq!(config.unwrap().is_some(), owner);
            let paths = tokio::time::timeout(std::time::Duration::from_secs(2), server).await.unwrap().unwrap();
            assert!(paths[0].starts_with("GET /api/me "));
            assert_eq!(paths.len(), if owner { 2 } else { 1 });
            if owner { assert!(paths[1].starts_with("GET /api/config ")); }
        }
    }

    #[test]
    fn quota_and_models_read_like_the_web() {
        assert_eq!(until(Some(100. + 35. * 60.), 100.), "35m");
        assert_eq!(until(Some(100. + 130. * 60.), 100.), "2h10");
        assert_eq!(until(Some(100. + 3. * 3600.), 100.), "3h");
        assert_eq!(until(Some(100. + 28. * 3600.), 100.), "1d4h");
        assert_eq!(until(Some(50.), 100.), "");
        let q: QuotaLine = serde_json::from_value(serde_json::json!({"id": "claude:/x", "estado": "lida",
            "janelas": [{"rotulo": "5h", "pct": 42.4}, {"rotulo": "7d", "pct": 18.0}, {"rotulo": "x", "pct": null}]})).unwrap();
        assert_eq!(q.summary(), "5h 42% · 7d 18%");
        let m: ModelOption = serde_json::from_value(serde_json::json!({"id": "k3", "name": "Kimi K3", "provider": "kimi-coding",
            "context_length": 256000, "images": true})).unwrap();
        assert_eq!((m.value(), m.hint()), ("kimi-coding/k3".to_owned(), "k3 · kimi-coding · 256K · 👁".to_owned()));
    }

    #[test]
    fn exhausted_account_blocks_only_on_a_current_general_window() {
        let quota = |state: &str, windows: serde_json::Value| -> QuotaLine {
            serde_json::from_value(serde_json::json!({"id": "claude:/x", "estado": state, "janelas": windows})).unwrap()
        };
        let now = 1000.;
        // Janela geral cheia bloqueia, com a volta dela.
        assert_eq!(exhausted(&quota("lida", serde_json::json!([{"rotulo": "5h", "pct": 100.0, "reset_ts": 2000.0}])), now), Some(Some(2000.)));
        // Sem volta conhecida ainda bloqueia, sem hora.
        assert_eq!(exhausted(&quota("lida", serde_json::json!([{"rotulo": "7d", "pct": 120.0}])), now), Some(None));
        // Janela por modelo cheia não bloqueia.
        assert_eq!(exhausted(&quota("lida", serde_json::json!([{"rotulo": "7d opus", "pct": 100.0, "reset_ts": 2000.0},
            {"rotulo": "5h", "pct": 40.0}])), now), None);
        // Leitura que não é `lida` não bloqueia.
        assert_eq!(exhausted(&quota("expirada", serde_json::json!([{"rotulo": "5h", "pct": 100.0, "reset_ts": 2000.0}])), now), None);
        // Janela cuja volta já passou não bloqueia.
        assert_eq!(exhausted(&quota("lida", serde_json::json!([{"rotulo": "5h", "pct": 100.0, "reset_ts": 900.0}])), now), None);
        // Duas cheias: volta quando a última voltar; uma vencida e outra cheia: a cheia decide.
        assert_eq!(exhausted(&quota("lida", serde_json::json!([{"rotulo": "5h", "pct": 100.0, "reset_ts": 2000.0},
            {"rotulo": "7d", "pct": 100.0, "reset_ts": 9000.0}])), now), Some(Some(9000.)));
        assert_eq!(exhausted(&quota("lida", serde_json::json!([{"rotulo": "5h", "pct": 100.0, "reset_ts": 500.0},
            {"rotulo": "7d", "pct": 100.0, "reset_ts": 9000.0}])), now), Some(Some(9000.)));
        assert_eq!(exhausted(&quota("lida", serde_json::json!([{"rotulo": "5h", "pct": 99.0, "reset_ts": 2000.0},
            {"rotulo": "7d", "pct": 50.0, "reset_ts": 9000.0}])), now), None);
    }

    #[test]
    fn exhausted_account_switches_to_the_roomiest() {
        let quota = |windows: serde_json::Value| -> QuotaLine {
            serde_json::from_value(serde_json::json!({"id": "claude:/x", "estado": "lida", "janelas": windows})).unwrap()
        };
        let now = 1000.;
        let full = quota(serde_json::json!([{"rotulo": "5h", "pct": 100.0, "reset_ts": 2000.0}]));
        let busy = quota(serde_json::json!([{"rotulo": "5h", "pct": 70.0}, {"rotulo": "7d", "pct": 10.0}]));
        let free = quota(serde_json::json!([{"rotulo": "5h", "pct": 20.0}, {"rotulo": "7d opus", "pct": 40.0}]));
        let model_full = quota(serde_json::json!([{"rotulo": "7d opus", "pct": 100.0}, {"rotulo": "5h", "pct": 10.0}]));
        // Esgotada: vai para a de mais folga, contando todas as janelas; a sem leitura não entra.
        assert_eq!(quota_switch(Some("/a"), &[("/a", true, Some(&full)), ("/b", false, Some(&busy)), ("/c", false, Some(&free)), ("/d", false, None)], now),
            Some("/c".into()));
        // Janela por modelo cheia não é esgotada: fica onde está.
        assert_eq!(quota_switch(Some("/a"), &[("/a", true, Some(&model_full)), ("/c", false, Some(&free))], now), None);
        // Nenhuma serve: fica.
        assert_eq!(quota_switch(Some("/a"), &[("/a", true, Some(&full)), ("/b", false, Some(&full)), ("/d", false, None)], now), None);
        // Sem conta escolhida conhecida: fica.
        assert_eq!(quota_switch(None, &[("/a", true, Some(&full)), ("/c", false, Some(&free))], now), None);
        // Empate fica com a ativa, em qualquer ordem.
        assert_eq!(quota_switch(Some("/a"), &[("/a", false, Some(&full)), ("/c", true, Some(&free)), ("/e", false, Some(&free))], now), Some("/c".into()));
        assert_eq!(quota_switch(Some("/a"), &[("/a", false, Some(&full)), ("/e", false, Some(&free)), ("/c", true, Some(&free))], now), Some("/c".into()));
    }
}
