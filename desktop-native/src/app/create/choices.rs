//! As escolhas finas do diálogo: modelo, esforço e permissão pelo catálogo global, motor, subagentes e Jev, a cota de cada
//! conta, criar e apagar conta Claude e o contexto estendido do Codex (`CreateSessionSheet.svelte`, `CodexContextControl.svelte`).
//! Toda escolha gravada saiu de uma leitura do servidor; leitura que falhou deixa o campo no padrão, nunca num valor inventado.
use super::*;
use gpui_kit::component::switch::Switch;

/// Os níveis de esforço fechados de cada provider, os do backend (`model_args.py`). O Kimi não tem nível; o Codex vem por modelo.
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const PI_EFFORTS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const PERMISSIONS: [&str; 6] = ["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"];
const CODEX_PERMISSIONS: [&str; 3] = ["Ask for approval", "Approve for me", "Full Access"];
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
}

impl ModelOption {
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

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Motor { label: Option<String>, #[serde(default)] model: String }

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

/// "2h10", "35m", "3d4h": quanto falta para a janela voltar (`faltaPara` do web).
fn until(reset: Option<f64>, now: f64) -> String {
    let Some(s) = reset.filter(|r| r.is_finite()).map(|r| r - now).filter(|s| *s > 0.) else { return String::new() };
    let min = (s / 60.).floor() as i64;
    if min < 60 { return format!("{min}m"); }
    let h = min / 60;
    if h < 24 { return if min % 60 != 0 { format!("{h}h{:02}", min % 60) } else { format!("{h}h") }; }
    if h % 24 != 0 { format!("{}d{}h", h / 24, h % 24) } else { format!("{}d", h / 24) }
}

/// Uma resposta de conta: criar (o POST e a lista relida) ou apagar (nome, caminho, o DELETE e a lista relida).
pub(in crate::app) enum AccountDone {
    Added(Result<Value, Failure>, Option<Result<Value, Failure>>),
    Deleted(String, String, Result<Value, Failure>, Option<Result<Value, Failure>>),
}

fn configs_of(result: Result<Value, Failure>) -> Result<Vec<ConfigDir>, String> {
    result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response")))
}

impl NewSession {
    pub(super) fn load_extras(&mut self, cx: &mut Context<Self>) {
        let (engines, config, quotas) = (self.engines.start(), self.jev.start(), self.quotas.start());
        self.request(cx, move |api, send| Box::pin(async move {
            let (e, c, q) = tokio::join!(api.server_read(&["engines"], &[], 15), api.server_read(&["config"], &[], 8),
                api.server_read(&["cotas"], &[], 15));
            send(CreateReply::Engines(engines, e)).await;
            send(CreateReply::Config(config, c)).await;
            send(CreateReply::Quotas(quotas, q)).await;
        }));
    }

    /// A chave da memória do último modelo: servidor, provider e a conta do Codex ou o motor (`chaveMemoria` do web).
    pub(super) fn memory_key(&self) -> String {
        let who = if self.provider == "codex" { self.codex_account.clone() } else { Some(self.engine.clone()).filter(|e| !e.is_empty()).unwrap_or("-".into()) };
        format!("cp_last_model:{}:{}:{who}", self.link.api.identity(), self.provider)
    }

    /// O catálogo da conta escolhida. Pedir de novo zera modelo, esforço e subagente: o que valia para outra conta não vale aqui.
    pub(super) fn load_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.models.start();
        (self.model, self.effort, self.subagent) = (String::new(), String::new(), String::new());
        self.build_model_picks(window, cx);
        if self.provider == "codex" && self.codex_account.is_empty() {
            self.models.finish(seq, Ok(Catalog { models: Vec::new(), reduced: false }));
            return;
        }
        let mut query = vec![("provider".to_owned(), self.provider.to_owned())];
        if self.provider == "claude" && !self.engine.is_empty() { query.push(("engine".into(), self.engine.clone())); }
        if let Some(config) = self.config.clone() { query.push(("config_dir".into(), config)); }
        if self.provider == "codex" { query.push(("codex_account".into(), self.codex_account.clone())); }
        let key = self.memory_key();
        self.request(cx, move |api, send| Box::pin(async move {
            let remembered = tokio::task::spawn_blocking(move || crate::appearance::last_model(&key)).await.unwrap_or_default();
            let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            send(CreateReply::Models(seq, api.server_read(&["model-options"], &query, 30).await, remembered)).await
        }));
    }

    pub(super) fn receive_models(&mut self, seq: u64, result: Result<Value, Failure>, remembered: (String, String), window: &mut Window,
        cx: &mut Context<Self>) {
        let catalog = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| {
            let models = serde_json::from_value(v.get("models").cloned().unwrap_or_default()).map_err(|_| tr("invalid_response"))?;
            Ok(Catalog { models, reduced: v.get("reduced").and_then(Value::as_bool).unwrap_or(false) })
        });
        if !self.models.finish(seq, catalog) { return; }
        let (model, effort) = remembered;
        // O lembrado só volta com a lista lida, se ainda estiver nela, e o esforço só se couber no modelo que ficou.
        if self.models.ok().is_some() {
            if self.catalog().iter().any(|m| m.value() == model) { self.model = model; }
            if self.levels().contains(&effort) { self.effort = effort; }
        }
        self.build_model_picks(window, cx);
    }

    fn catalog(&self) -> &[ModelOption] { self.models.ok().map(|c| c.models.as_slice()).unwrap_or_default() }

    /// Os níveis do modelo escolhido: fechados por provider, e os do próprio modelo no Codex.
    pub(super) fn levels(&self) -> Vec<String> {
        match self.provider {
            "codex" => self.catalog().iter().find(|m| m.id == self.model).map(|m| m.efforts.clone()).unwrap_or_default(),
            "claude" => CLAUDE_EFFORTS.map(String::from).to_vec(),
            "pi" | "omp" => PI_EFFORTS.map(String::from).to_vec(),
            _ => Vec::new(),
        }
    }

    /// A permissão existe para o Claude e para o Codex sem terminal, cada um com a própria lista.
    pub(super) fn permissions(&self) -> Option<&'static [&'static str]> {
        match (self.provider, self.headless) { ("claude", _) => Some(&PERMISSIONS), ("codex", true) => Some(&CODEX_PERMISSIONS), _ => None }
    }

    fn pick_at(choices: &[ModelChoice], value: &str) -> Option<usize> { choices.iter().position(|c| c.id == value).or(Some(0)) }

    /// Os seletores do trio e do subagente, montados de novo quando a lista ou o valor muda por fora do clique.
    pub(super) fn build_model_picks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default = || ModelChoice { id: String::new(), label: tr("create_default"), hint: String::new() };
        let models: Vec<ModelChoice> = std::iter::once(default()).chain(self.catalog().iter().filter(|m| m.id != "default")
            .map(|m| ModelChoice { id: m.value(), label: m.label(), hint: m.hint() })).collect();
        let at = Self::pick_at(&models, &self.model);
        self.model_pick = Some(picker(models, at, |this, id, window, cx| {
            this.model = id;
            // Trocar de modelo pode tirar o nível escolhido da lista (só o Codex tem níveis por modelo).
            if !this.levels().contains(&this.effort) { this.effort.clear(); }
            this.build_effort_pick(window, cx);
        }, window, cx));
        let subagents: Vec<ModelChoice> = std::iter::once(ModelChoice { id: String::new(), label: tr("create_subagent_default"), hint: String::new() })
            .chain(self.catalog().iter().filter(|m| m.id != "default").map(|m| ModelChoice { id: m.value(), label: m.label(), hint: String::new() })).collect();
        let at = Self::pick_at(&subagents, &self.subagent);
        self.subagent_pick = Some(picker(subagents, at, |this, id, _, _| this.subagent = id, window, cx));
        self.build_effort_pick(window, cx);
        self.build_permission_pick(window, cx);
    }

    fn build_effort_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choices: Vec<ModelChoice> = std::iter::once(String::new()).chain(self.levels())
            .map(|n| ModelChoice { label: if n.is_empty() { tr("create_default") } else { n.clone() }, id: n, hint: String::new() }).collect();
        let at = Self::pick_at(&choices, &self.effort);
        self.effort_pick = Some(picker(choices, at, |this, id, _, _| this.effort = id, window, cx));
    }

    pub(super) fn build_permission_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(modes) = self.permissions() else { self.permission_pick = None; return };
        let choices: Vec<ModelChoice> = std::iter::once(ModelChoice { id: String::new(), label: tr("create_permission_default"), hint: String::new() })
            .chain(modes.iter().map(|m| ModelChoice { id: (*m).into(), label: (*m).into(), hint: String::new() })).collect();
        let at = Self::pick_at(&choices, &self.permission);
        self.permission_pick = Some(picker(choices, at, |this, id, _, _| this.permission = id, window, cx));
    }

    /// A conta Claude com a cota de cada uma na dica ("atual · 5h 42% · 7d 18%").
    pub(super) fn build_config_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(list) = self.configs.ok() else { self.config_pick = None; return };
        let choices: Vec<ModelChoice> = list.iter().map(|c| {
            let quota = self.quota_of(&format!("claude:{}", c.path)).map(QuotaLine::summary).unwrap_or_default();
            let hint = [c.active.then(|| tr("create_current")), Some(quota).filter(|q| !q.is_empty())].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            ModelChoice { id: c.path.clone(), label: c.label.clone(), hint }
        }).collect();
        let at = choices.iter().position(|c| Some(&c.id) == self.config.as_ref());
        self.config_pick = Some(picker(choices, at, |this, path, window, cx| {
            this.config = Some(path);
            this.load_models(window, cx);
        }, window, cx));
    }

    pub(super) fn build_engine_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(list) = self.engines.ok().filter(|l| !l.is_empty()) else { self.engine_pick = None; return };
        let choices: Vec<ModelChoice> = std::iter::once(ModelChoice { id: String::new(), label: tr("create_own_account"), hint: String::new() })
            .chain(list.iter().map(|(name, m)| ModelChoice { id: name.clone(), label: m.label.clone().unwrap_or_else(|| name.clone()), hint: m.model.clone() }))
            .collect();
        let at = Self::pick_at(&choices, &self.engine);
        self.engine_pick = Some(picker(choices, at, |this, name, window, cx| {
            this.engine = name;
            this.load_models(window, cx);
        }, window, cx));
    }

    fn quota_of(&self, id: &str) -> Option<&QuotaLine> { self.quotas.ok()?.iter().find(|q| q.id == id) }

    pub(super) fn receive_extra(&mut self, reply: CreateReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            CreateReply::Engines(seq, result) => {
                let list = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| {
                    let map: HashMap<String, Motor> = serde_json::from_value(v.get("motores").cloned().unwrap_or_default()).map_err(|_| tr("invalid_response"))?;
                    let mut list: Vec<(String, Motor)> = map.into_iter().collect();
                    list.sort_by(|a, b| a.0.cmp(&b.0));
                    Ok(list)
                });
                if self.engines.finish(seq, list) { self.build_engine_pick(window, cx); }
            }
            CreateReply::Config(seq, result) => {
                let jev = result.map_err(|e| Hangar::fetch_failure(&e)).map(|v| Jev {
                    key: v.pointer("/campos/jev_api_key/definido").and_then(Value::as_bool).unwrap_or(false),
                    default: v.pointer("/campos/jev_padrao/valor").and_then(Value::as_bool).unwrap_or(false),
                });
                if self.jev.finish(seq, jev) { self.jev_on = self.jev.ok().is_some_and(|j| j.default); }
            }
            CreateReply::Quotas(seq, result) => {
                let list = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response")));
                if self.quotas.finish(seq, list) { self.build_config_pick(window, cx); }
            }
            CreateReply::Models(seq, result, remembered) => self.receive_models(seq, result, remembered, window, cx),
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
        basename(&selected.path).strip_prefix(".claude-").filter(|n| !n.is_empty()).map(str::to_owned)
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
        self.request(cx, move |api, send| Box::pin(async move {
            let deleted = api.server_send(reqwest::Method::DELETE, &["claude-configs", &name], None, 120).await;
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
        let list = self.configs.ok()?;
        list.iter().find(|c| c.active).or(list.first()).map(|c| c.path.clone())
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
                            self.build_config_pick(window, cx);
                            self.load_models(window, cx);
                        }
                        None => (self.created_path, self.notice) = (None, Some((tr("create_account_missing"), false))),
                    }
                }
                _ => self.notice = Some((tr("create_account_list_failed"), false)),
            },
            AccountDone::Deleted(name, path, Ok(_), list) => {
                self.confirming = false;
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
                self.notice = Some((tr(key).replace("{nome}", &name), false));
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

    pub(super) fn render_codex_quota(&self, credential: Option<&str>) -> Option<Stateful<Div>> {
        let quota = self.quota_of(credential?)?;
        Some(self.render_quota("create-codex-quota".into(), quota))
    }

    pub(super) fn render_claude_account(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.creating || self.account_busy;
        let deletable = self.deletable().filter(|_| !self.asking && !self.confirming);
        let small = |id: &'static str, text: String| Button::new(id).outline().small().flex_shrink_0().label(text);
        let picker = match (&self.config_pick, self.configs.value.as_ref()) {
            // A falha vem antes do seletor: a leitura que falhou também deixa um seletor vazio.
            (_, Some(Err(error))) if !self.configs.loading => alert("create-configs-error", error.clone()).into_any_element(),
            (Some((pick, _)), _) if !self.configs.loading => Select::new(pick).small().disabled(busy).accessibility_label(tr("create_claude_account")).into_any_element(),
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
                    .on_click(cx.listener(|this, _, _, cx| { (this.confirming, this.notice) = (true, None); cx.notify(); })))))
            .when_some(selected_quota, |el, q| el.child(self.render_quota("create-claude-quota".into(), q)))
            .when_some(self.deletable().filter(|_| self.confirming), |el, name| el.child(div().flex().items_center().gap(px(8.))
                .child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
                    .child(format!("{} ", tr("create_delete_start"))).child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(name))
                    .child(format!(" {}", tr("create_delete_end"))).flex().flex_wrap().gap_x(px(0.)))
                .child(Button::new("create-account-delete-yes").outline().small().text_color(theme::danger()).border_color(theme::danger())
                    .label(if self.account_busy { "…".into() } else { tr("create_delete") }).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.delete_account(cx))))
                .child(small("create-account-delete-no", tr("create_cancel")).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| { this.confirming = false; cx.notify(); })))))
            .when(self.asking, |el| {
                let ready = !self.account_name.read(cx).value().trim().is_empty();
                el.child(div().id("create-account-line").flex().items_center().gap(px(8.)).on_action(esc)
                    .child(div().flex_1().min_w_0().child(Input::new(&self.account_name).small().disabled(busy).aria_label(tr("create_account_new_aria"))))
                    .child(small("create-account-new-ok", tr("create_account_create")).disabled(busy || !ready)
                        .on_click(cx.listener(|this, _, _, cx| this.add_account(cx))))
                    .child(small("create-account-new-no", tr("create_cancel")).disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| { this.asking = false; cx.notify(); }))))
            })
            .when_some(notice, |el, (text, error)| el.child(if error { alert("create-account-notice", text) }
                else { div().id("create-account-notice").role(Role::Status).text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(text) }))
    }

    /// Modelo, esforço e permissão lado a lado; cada um some quando não se aplica.
    pub(super) fn render_trio(&self) -> Option<Div> {
        let busy = self.creating;
        let field = |title: String, pick: &Option<Picker>| pick.as_ref().map(|(p, _)| div().flex_1().min_w(px(150.)).flex().flex_col().gap(px(4.))
            .child(label(title.clone())).child(Select::new(p).small().disabled(busy).accessibility_label(title)));
        let model = field(tr("create_model"), &self.model_pick).map(|el| el
            .when(self.models.ok().is_some_and(|c| c.reduced), |el| el.child(div().id("create-models-reduced").role(Role::Status).child(muted(tr("create_models_reduced")))))
            .when_some(self.models.value.as_ref().and_then(|v| v.as_ref().err()), |el, error| el.child(alert("create-models-error",
                tr("create_models_default").replace("{erro}", &format!("{}: {error}", tr("create_models_failed")))))));
        let effort_title = tr(if matches!(self.provider, "pi" | "omp") { "create_reasoning" } else { "create_effort" });
        let effort = (!self.levels().is_empty()).then(|| field(effort_title, &self.effort_pick)).flatten();
        let permission = self.permissions().and(field(tr("create_permission"), &self.permission_pick));
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

    /// "Mais opções": motor, modelo dos subagentes e Jev, recolhidos, com o valor de cada um no resumo.
    pub(super) fn render_more(&self, cx: &mut Context<Self>) -> Option<Div> {
        let engine = (self.provider == "claude").then_some(self.engine_pick.as_ref()).flatten();
        // O bastão não leva subagente nem Jev: a rota dele não recebe os dois.
        let subagent = (self.target().is_none() && self.baton.is_none() && self.provider == "claude" && self.engine.is_empty() && !self.catalog().is_empty())
            .then_some(self.subagent_pick.as_ref()).flatten();
        // O retomar não leva o Jev: com uma conversa escolhida, o interruptor seria um controle sem efeito.
        let jev = self.jev_choice().is_some() && self.target().is_none() && self.baton.is_none();
        if engine.is_none() && subagent.is_none() && !jev { return None; }
        let engine_label = self.engines.ok().and_then(|l| l.iter().find(|(n, _)| *n == self.engine))
            .map(|(n, m)| m.label.clone().unwrap_or_else(|| n.clone())).unwrap_or_else(|| tr("create_own_account"));
        let subagent_label = self.catalog().iter().find(|m| m.value() == self.subagent).map(ModelOption::label)
            .unwrap_or_else(|| if self.subagent.is_empty() { tr("create_subagent_default") } else { self.subagent.clone() });
        let pill = |text: String| div().px(px(8.)).py(px(1.)).rounded_full().border_1().border_color(theme::border()).text_size(px(11.5)).text_color(theme::muted()).child(text);
        let this = cx.entity().downgrade();
        let busy = self.creating;
        Some(div().flex().flex_col().gap(px(10.)).p(px(10.)).rounded(px(8.)).border_1().border_color(theme::border())
            .child(div().flex().items_center().gap(px(8.)).flex_wrap()
                .child(Disclosure::new("create-more", self.more, tr("create_more"), false)
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.more = open; cx.notify(); }); }))
                .when(!self.more, |el| el
                    .when(engine.is_some(), |el| el.child(pill(format!("{} {engine_label}", tr("create_engine")))))
                    .when(subagent.is_some(), |el| el.child(pill(format!("{} {subagent_label}", tr("create_more_subagents")))))
                    .when(jev && self.jev_on, |el| el.child(pill(tr("create_jev"))))))
            .when(self.more, |el| el
                .when_some(engine, |el, (p, _)| el.child(div().flex().flex_col().gap(px(4.)).child(label(tr("create_engine")))
                    .child(Select::new(p).small().disabled(busy).accessibility_label(tr("create_engine")))))
                .when_some(subagent, |el, (p, _)| el.child(div().flex().flex_col().gap(px(4.)).child(label(tr("create_subagent")))
                    .child(Select::new(p).small().disabled(busy).accessibility_label(tr("create_subagent"))).child(muted(tr("create_subagent_help")))))
                .when(jev, |el| el.child(div().flex().flex_col().gap(px(4.))
                    .child(Checkbox::new("create-jev").label(tr("create_jev")).checked(self.jev_on).disabled(busy)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| { this.jev_on = *checked; cx.notify(); })))
                    .child(muted(tr("create_jev_help")))))))
    }

    pub(super) fn render_omp(&self) -> Div {
        div().flex().flex_col().gap(px(4.)).child(label(tr("create_omp_profile")))
            .child(Input::new(&self.omp).small().font_family(theme::MONO).disabled(self.creating).aria_label(tr("create_omp_profile")))
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelOption, QuotaLine, until};

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
}
