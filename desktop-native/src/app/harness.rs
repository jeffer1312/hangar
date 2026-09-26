//! Saúde dos harnesses: uma linha por CLI, o que o app instalou nele e o botão do conserto que o servidor já tem.
//! Os textos dos itens são as tabelas de códigos do web (`harness_*`), lidas por `tr_web`.
use super::*;
use super::chrome::Skeleton;
use super::server_config::chip;
use super::settings::{Disclosure, Page, settings_box};
use gpui_kit::component::{progress::Progress, switch::Switch};
use serde::Deserialize;
use serde_json::Map;

/// Opções do Claude no `/api/config`, na ordem do card: (chave, rótulo, ajuda), textos do web.
const CLAUDE_OPTIONS: [(&str, &str, &str); 2] = [
    ("claude_statusline_update", "harness_claude_statusline_atualizar", "harness_claude_statusline_ajuda"),
    ("claude_function_hooks", "harness_claude_function_hooks", "harness_claude_function_hooks_ajuda"),
];

#[derive(Clone, Deserialize)]
struct Item {
    id: String,
    ok: Option<bool>,
    codigo: String,
    #[serde(default)]
    params: HashMap<String, String>,
    conserto: Option<String>,
    #[serde(default)]
    info: bool,
}

#[derive(Clone, Deserialize)]
struct Cli { id: String, nome: String, instalado: bool, versao: Option<String>, itens: Vec<Item> }

#[derive(Deserialize)]
struct Repaired { feito: String, harnesses: Vec<Cli> }

/// Conserto em curso ou o desfecho dele, preso ao item (CLI, item) onde o botão estava.
struct Repair { cli: String, item: String, started: Instant, outcome: Option<Result<String, String>> }

/// `/api/harness/instalar`, lido por polling: a instalação vive no servidor, sair da página não a perde.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Install {
    fase: String,
    harness: Option<String>,
    etapa: Option<String>,
    passo: u32,
    total: u32,
    log: Vec<String>,
    avisos: Vec<String>,
    ok: Option<bool>,
    erro: Option<String>,
    comandos: HashMap<String, String>,
    manual: HashMap<String, String>,
}

impl Install {
    fn running(&self) -> bool { self.fase == "rodando" }
}

/// `/api/harness/codex/integracao`: a cópia do Claude Code para o Codex, com os dois interruptores do `/api/config` junto.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Integration {
    estado: String,
    etapa: Option<Value>,
    ultima_execucao: Option<String>,
    proxima_atualizacao: Option<String>,
    plugins: Vec<Plugin>,
    avisos: Vec<Value>,
    erros: Vec<Value>,
    confianca_pendente: bool,
    progresso: Option<Steps>,
    etapa_segundos: Option<u64>,
    skills: Option<Skills>,
    automatica: bool,
    memoria: bool,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Plugin { id: String, versao: String, origem: String }

#[derive(Clone, Deserialize)]
struct Steps { passo: u32, total: u32, sub: Option<Sub> }

#[derive(Clone, Deserialize)]
struct Sub { atual: u32, total: u32 }

#[derive(Clone, Deserialize)]
struct Skills { ponte: u64, nativas: u64 }

impl Steps {
    /// Etapas concluídas mais a fração que o sub-andamento mede; sem ele a etapa em curso conta zero, como no web.
    fn percent(&self) -> f32 {
        let inside = self.sub.as_ref().filter(|s| s.total > 0).map_or(0., |s| (s.atual as f32 - 1.) / s.total as f32);
        if self.total == 0 { return 0.; }
        ((self.passo as f32 - 1. + inside) / self.total as f32).clamp(0., 1.) * 100.
    }
}

/// Os interruptores da integração: (chave do `/api/config`, rótulo, ajuda), textos do web.
const CODEX_SWITCHES: [(&str, &str, &str); 2] = [
    ("codex_sync", "harness_codex_automatica", "harness_codex_automatica_ajuda"),
    ("codex_memory_import", "harness_codex_memoria", "harness_codex_memoria_ajuda"),
];

#[derive(Default)]
pub(in crate::app) struct Harnesses {
    /// A última lista lida; uma releitura que falha não a apaga (o erro aparece em cima dela).
    list: Option<Vec<Cli>>,
    loading: bool,
    load_seq: u64,
    error: Option<String>,
    repair: Option<Repair>,
    repair_seq: u64,
    why: Vec<(String, String)>,
    _clock: Option<Task<()>>,
    install: Option<Install>,
    install_seq: u64,
    /// O CLI cujo pedido de instalar ainda não voltou: trava o botão contra o clique duplo.
    starting: Option<String>,
    /// Erro de instalar e o CLI dono dele; sem dono na lista, vai ao pé da página.
    install_error: Option<(Option<String>, String)>,
    install_log: ScrollHandle,
    _install_poll: Option<Task<()>>,
    /// `campos` da última leitura ou gravação boa do `/api/config`; o interruptor mostra isso, nunca o clique.
    options: Option<Map<String, Value>>,
    /// Leituras e gravações numa fila só: um ↻ que responde depois da gravação não repõe o valor velho.
    options_seq: u64,
    options_error: Option<String>,
    /// Chaves cuja gravação ainda não voltou: o interruptor delas fica travado contra o clique duplo.
    toggling: Vec<&'static str>,
    /// A última leitura boa da integração do Codex; uma que falha não a apaga.
    integration: Option<Integration>,
    integration_seq: u64,
    /// O pedido de reconciliar ainda não voltou.
    reconciling: bool,
    integration_error: Option<String>,
    /// Interruptores da integração cuja gravação não voltou.
    integration_toggling: Vec<&'static str>,
    _integration_poll: Option<Task<()>>,
}

pub(super) enum HarnessReply {
    Loaded(u64, Result<Value, Failure>),
    Repaired(u64, Result<Value, Failure>),
    Install(u64, Option<String>, Result<Value, Failure>),
    /// Número na fila, a chave gravada (nenhuma = leitura) e a resposta.
    Options(u64, Option<&'static str>, Result<Value, Failure>),
    /// Número na fila da integração e a resposta (leitura ou reconciliar).
    Integration(u64, Result<Value, Failure>),
    /// Interruptor da integração gravado no `/api/config`.
    IntegrationSwitch(&'static str, Result<Value, Failure>),
}

/// Código do servidor → frase do web; código que o app não conhece aparece cru em vez de sumir.
fn item_text(item: &Item) -> String {
    let key = match item.codigo.as_str() {
        "skills_ok" if item.params.contains_key("origem") => "skills_origem".to_owned(),
        "extensoes_outra_fonte" if item.params.contains_key("faltam") => "extensoes_outra_fonte_e_faltam".to_owned(),
        code => code.to_owned(),
    };
    crate::i18n::tr_web(&format!("harness_{key}"), &item.params).unwrap_or_else(|| item.codigo.clone())
}

fn item_label(id: &str) -> String {
    let key = match id {
        "bloco" => "tmux_bloco", "default_terminal" => "tmux_term", "truecolor" => "tmux_truecolor",
        "titulo" => "tmux_titulo", "mouse" => "tmux_mouse", "persistencia" => "tmux_persist", other => other,
    };
    crate::i18n::tr_web(&format!("harness_item_{key}"), &HashMap::new()).unwrap_or_else(|| id.to_owned())
}

/// "por quê?" declarado por CLI: o mesmo id em outro card fala de outra coisa (os hooks do Codex são os do usuário).
fn explained(item: &Item, cli: &str) -> bool {
    match item.id.as_str() {
        "credenciais" => ["codex", "pi", "omp", "kimi"].contains(&cli) && item.conserto.as_deref().is_some_and(|c| c.starts_with("sync:")),
        "extensoes" => ["pi", "omp"].contains(&cli),
        "skills" => ["pi", "kimi"].contains(&cli),
        "hooks" => ["claude", "kimi"].contains(&cli),
        "contas" => cli == "claude",
        _ => false,
    }
}

fn web(key: &str) -> String { crate::i18n::tr_web(key, &HashMap::new()).unwrap_or_else(|| key.to_owned()) }

fn web_with(key: &str, params: &[(&str, String)]) -> String {
    let params = params.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
    crate::i18n::tr_web(key, &params).unwrap_or_else(|| key.to_owned())
}

/// Etapa que o app não conhece aparece pela chave crua, como no web.
fn install_step(key: Option<&str>) -> String {
    let key = key.unwrap_or_default();
    crate::i18n::tr_web(&format!("harness_inst_etapa_{key}"), &HashMap::new()).unwrap_or_else(|| key.to_owned())
}

/// Mensagem da integração: código do servidor → frase do web (`harness_codex_m_<código>`); código desconhecido mostra o
/// `texto` que veio junto em vez de sumir.
fn codex_text(message: Option<&Value>) -> String {
    match message {
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

/// Data do servidor na hora local, no formato do `toLocaleString` do web; nenhuma é "nenhuma", ilegível vai crua.
fn codex_date(value: Option<&str>) -> String {
    let Some(value) = value else { return web("harness_codex_nunca") };
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(value) else { return value.to_owned() };
    let at = at.with_timezone(&chrono::Local);
    at.format(if crate::i18n::english() { "%-m/%-d/%Y, %-I:%M:%S %p" } else { "%d/%m/%Y, %H:%M:%S" }).to_string()
}

/// A caixa só acompanha a última linha de quem já está no fim: quem rolou para cima está lendo.
fn near_bottom(handle: &ScrollHandle) -> bool { handle.max_offset().y + handle.offset().y < px(40.) }

impl Hangar {
    fn harness_send_later(&self) -> impl Fn(HarnessReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Harness(reply) }).await; })
        }
    }

    /// Conserto em curso sobrevive a sair e voltar: a resposta dele ainda é desta conexão.
    pub(super) fn harness_opened(&mut self, cx: &mut Context<Self>) {
        let h = &mut self.harness;
        (h.error, h.why, h.install_error, h.options_error) = (None, Vec::new(), None, None);
        if h.repair.as_ref().is_some_and(|r| r.outcome.is_some()) { h.repair = None; }
        self.load_harness(cx);
        self.poll_install(None, cx);
        self.load_options(cx);
        if !self.harness.reconciling { self.read_integration(false, true, cx); }
    }

    /// Lê a integração, ou (`reconcile`) pede uma rodada agora. `clear`: gesto novo, o erro anterior sai.
    fn read_integration(&mut self, reconcile: bool, clear: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let h = &mut self.harness;
        h.integration_seq += 1;
        let seq = h.integration_seq;
        if reconcile { h.reconciling = true; }
        if clear { h.integration_error = None; }
        let done = self.harness_send_later();
        self.runtime.spawn(async move {
            let result = if reconcile { api.server_post(&["harness", "codex", "integracao"], 30).await }
                else { api.server_read(&["harness", "codex", "integracao"], &[], 15).await };
            done(HarnessReply::Integration(seq, result)).await
        });
        cx.notify();
    }

    /// Ocupada: reconciliando, ou rodando sem erro. Erro quer dizer que ninguém sabe mais se roda, e aí o botão destrava.
    fn integration_busy(&self) -> bool {
        let h = &self.harness;
        h.reconciling || (h.integration_error.is_none() && h.integration.as_ref().is_some_and(|i| i.estado == "executando"))
    }

    fn reconcile_integration(&mut self, cx: &mut Context<Self>) {
        if !self.integration_busy() { self.read_integration(true, true, cx); }
    }

    /// O interruptor mostra o servidor: grava a chave e é a releitura da integração que muda a tela.
    fn toggle_integration(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.harness.integration_toggling.contains(&key) { return; }
        self.harness.integration_error = None;
        self.harness.integration_toggling.push(key);
        let done = self.harness_send_later();
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::POST, &["config"], Some(serde_json::json!({ key: on })), 8).await;
            done(HarnessReply::IntegrationSwitch(key, result)).await
        });
        cx.notify();
    }

    /// Sem `write`, lê o `/api/config`; com ele, grava a chave. Os dois devolvem `campos`, e só o mais novo da fila os aplica.
    fn load_options(&mut self, cx: &mut Context<Self>) { self.send_options(None, cx); }

    fn toggle_option(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>) {
        if self.api.is_none() || self.harness.toggling.contains(&key) { return; }
        self.harness.options_error = None;
        self.harness.toggling.push(key);
        self.send_options(Some((key, on)), cx);
    }

    fn send_options(&mut self, write: Option<(&'static str, bool)>, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.harness.options_seq += 1;
        let seq = self.harness.options_seq;
        let done = self.harness_send_later();
        self.runtime.spawn(async move {
            let result = match write {
                Some((key, on)) => api.server_send(reqwest::Method::POST, &["config"], Some(serde_json::json!({ key: on })), 8).await,
                None => api.config().await,
            };
            done(HarnessReply::Options(seq, write.map(|(key, _)| key), result)).await
        });
        cx.notify();
    }

    /// Sem `cli`, lê o estado; com ele, pede a instalação. Só a resposta do pedido mais novo vale.
    fn poll_install(&mut self, cli: Option<String>, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let h = &mut self.harness;
        h.install_seq += 1;
        let seq = h.install_seq;
        if cli.is_some() { (h.install_error, h.starting) = (None, cli.clone()); }
        let done = self.harness_send_later();
        self.runtime.spawn(async move {
            let result = match &cli {
                Some(cli) => api.server_post(&["harness", "instalar", cli], 30).await,
                None => api.server_read(&["harness", "instalar"], &[], 15).await,
            };
            done(HarnessReply::Install(seq, cli, result)).await
        });
        cx.notify();
    }

    fn schedule_install_poll(&mut self, cx: &mut Context<Self>) {
        self.harness._install_poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1200)).await;
            // Página fechada não consulta; reabrir relê e retoma o ciclo.
            let _ = this.update(cx, |this, cx| if this.settings == Some(Page::Harnesses) { this.poll_install(None, cx) });
        }));
    }

    /// Relê a rodada em curso daqui a 1,5 s, se nada mais novo saiu e a página segue aberta.
    fn schedule_integration_poll(&mut self, seq: u64, cx: &mut Context<Self>) {
        self.harness._integration_poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| if this.settings == Some(Page::Harnesses) && this.harness.integration_seq == seq {
                this.read_integration(false, false, cx)
            });
        }));
    }

    fn confirm_install(&mut self, cli: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self.harness.install.as_ref().and_then(|i| i.comandos.get(&cli).cloned()) else { return };
        let this = cx.entity().downgrade();
        let body = format!("{}\n\n{command}\n\n{}", web("harness_inst_conf_corpo"), web("harness_inst_conf_depois"));
        chrome::confirm_alert(window, cx, web_with("harness_inst_conf_titulo", &[("nome", name)]), body, web("harness_inst_botao"),
            ButtonVariant::Primary, move |_, cx| { let _ = this.update(cx, |this, cx| this.start_install(cli.clone(), cx)); true });
    }

    /// Outra instalação pode ter começado (noutro aparelho) com a confirmação aberta: diz o porquê em vez de não fazer nada.
    fn start_install(&mut self, cli: String, cx: &mut Context<Self>) {
        if self.harness.install.as_ref().is_some_and(Install::running) || self.harness.starting.is_some() {
            self.harness.install_error = Some((Some(cli), web("harness_inst_ocupado")));
            cx.notify();
            return;
        }
        self.harness._install_poll = None;
        self.poll_install(Some(cli), cx);
    }

    fn load_harness(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.harness.load_seq += 1;
        let seq = self.harness.load_seq;
        self.harness.loading = true;
        let done = self.harness_send_later();
        // `--version` de cinco CLIs em série no servidor.
        self.runtime.spawn(async move { done(HarnessReply::Loaded(seq, api.server_read(&["harness"], &[], 30).await)).await });
        cx.notify();
    }

    fn repair_harness(&mut self, id: String, cli: String, item: String, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none()) { return; }
        self.harness.repair_seq += 1;
        let seq = self.harness.repair_seq;
        (self.harness.repair, self.harness.error) = (Some(Repair { cli, item, started: Instant::now(), outcome: None }), None);
        self.harness_clock(cx);
        let done = self.harness_send_later();
        self.runtime.spawn(async move {
            done(HarnessReply::Repaired(seq, api.server_post(&["harness", "conserto", &id], 120).await)).await
        });
        cx.notify();
    }

    fn harness_clock(&mut self, cx: &mut Context<Self>) {
        self.harness._clock = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let running = this.update(cx, |this, cx| {
                cx.notify();
                this.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none())
            });
            if !running.unwrap_or(false) { break; }
        }));
    }

    pub(super) fn receive_harness(&mut self, reply: HarnessReply, cx: &mut Context<Self>) {
        match reply {
            HarnessReply::Loaded(seq, result) => {
                if seq != self.harness.load_seq { return; }
                self.harness.loading = false;
                match result.map_err(|error| Self::fetch_failure(&error))
                    .and_then(|value| serde_json::from_value::<Vec<Cli>>(value).map_err(|_| tr("invalid_response"))) {
                    Ok(list) => (self.harness.list, self.harness.error) = (Some(list), None),
                    Err(error) => self.harness.error = Some(error),
                }
            }
            HarnessReply::Repaired(seq, result) => {
                if seq != self.harness.repair_seq { return; }
                let parsed = result.map_err(|error| Self::fetch_failure(&error))
                    .and_then(|value| serde_json::from_value::<Repaired>(value).map_err(|_| tr("invalid_response")));
                let outcome = match parsed {
                    Ok(done) => {
                        // A lista do conserto é mais nova que qualquer leitura em voo.
                        self.harness.load_seq += 1;
                        (self.harness.list, self.harness.loading) = (Some(done.harnesses), false);
                        Ok(done.feito)
                    }
                    Err(error) => {
                        // O conserto pode ter rodado antes da falha: a lista relida mostra o estado real.
                        self.load_harness(cx);
                        Err(error)
                    }
                };
                if let Some(repair) = &mut self.harness.repair { repair.outcome = Some(outcome); }
            }
            HarnessReply::Install(seq, cli, result) => {
                let h = &mut self.harness;
                if seq != h.install_seq { return; }
                h.starting = None;
                let parsed = result.map_err(|error| match error.status {
                    Some(409) => web("harness_inst_ocupado"),
                    Some(400) => web_with("erro_harness_sem_instalador", &[("cli", cli.clone().unwrap_or_default())]),
                    _ => Self::fetch_failure(&error),
                }).and_then(|value| serde_json::from_value::<Install>(value).map_err(|_| tr("invalid_response")));
                match parsed {
                    Ok(state) => {
                        // Só uma instalação andando apaga o erro: o que a impediu de começar fica à vista.
                        if state.running() { h.install_error = None; }
                        let before = h.install.as_ref();
                        let finished = before.is_some_and(Install::running) && state.fase == "pronto";
                        if before.is_none_or(|b| b.log != state.log) && near_bottom(&h.install_log) { h.install_log.scroll_to_bottom(); }
                        let running = state.running();
                        h.install = Some(state);
                        // Quem diz se instalou é o disco relido, não o fim do comando.
                        if running { self.schedule_install_poll(cx); } else if finished { self.load_harness(cx); }
                    }
                    Err(error) => {
                        let owner = cli.clone().or_else(|| h.install.as_ref().and_then(|i| i.harness.clone()));
                        h.install_error = Some((owner, error));
                        // O trabalho vive no servidor: resposta perdida não congela a tela, a próxima leitura desempata.
                        if h.install.as_ref().is_some_and(Install::running) || cli.is_some() { self.schedule_install_poll(cx); }
                    }
                }
            }
            HarnessReply::Options(seq, key, result) => {
                let h = &mut self.harness;
                if let Some(key) = key { h.toggling.retain(|k| *k != key); }
                let parsed = result.map_err(|error| Self::fetch_failure(&error))
                    .and_then(|value| match value.get("campos") { Some(Value::Object(campos)) => Ok(campos.clone()), _ => Err(tr("invalid_response")) });
                match parsed {
                    Ok(campos) => if seq == h.options_seq { h.options = Some(campos); }
                        // Gravação boa passada por uma leitura mais nova, que pode ter lido antes dela: relê.
                        else if key.is_some() { self.load_options(cx); },
                    // Gravar que falhou fala mesmo atrasada; a leitura atrasada cala. A última leitura boa fica à vista.
                    Err(error) => if key.is_some() || seq == h.options_seq {
                        h.options_error = Some(error);
                        // A gravação pode ter pegado antes da falha: o servidor relido desempata.
                        if key.is_some() { self.load_options(cx); }
                    },
                }
            }
            HarnessReply::Integration(seq, result) => {
                let h = &mut self.harness;
                if seq != h.integration_seq { return; }
                h.reconciling = false;
                match result.map_err(|error| Self::fetch_failure(&error))
                    .and_then(|value| serde_json::from_value::<Integration>(value).map_err(|_| tr("invalid_response"))) {
                    Ok(state) => {
                        let running = state.estado == "executando";
                        h.integration = Some(state);
                        if running { self.schedule_integration_poll(seq, cx); }
                    }
                    // A última leitura boa fica; sem nova leitura agendada, o botão destrava pelo erro.
                    Err(error) => h.integration_error = Some(error),
                }
            }
            HarnessReply::IntegrationSwitch(key, result) => {
                self.harness.integration_toggling.retain(|k| *k != key);
                // A gravação pode ter pegado antes da falha: a releitura mostra o valor real, e o erro fica à vista.
                if let Err(error) = result { self.harness.integration_error = Some(Self::setting_failure(&error)); }
                self.read_integration(false, false, cx);
            }
        }
        cx.notify();
    }

    /// Interruptores do card do Claude (`HarnessSettings.svelte`): o valor é o do servidor, e o clique grava na hora.
    fn claude_options(&self, cx: &mut Context<Self>) -> Div {
        let h = &self.harness;
        let rows = h.options.iter().flat_map(|campos| CLAUDE_OPTIONS.iter().filter(|(key, ..)| campos.contains_key(*key)).map(|&(key, label, help)| {
            let on = campos.get(key).and_then(|f| f.get("valor")) == Some(&Value::Bool(true));
            div().id(SharedString::from(format!("harness-claude-{key}"))).flex().items_center().gap_3().py(px(6.))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(web(label)))
                    .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(web(help))))
                .child(Switch::new(SharedString::from(format!("harness-claude-{key}-switch"))).checked(on)
                    .accessibility_label(web(label)).disabled(h.toggling.contains(&key))
                    .on_click(cx.listener(move |this, on: &bool, _, cx| this.toggle_option(key, *on, cx))))
        })).collect::<Vec<_>>();
        // Servidor que não conhece a chave diz o porquê: calado, a opção sumiria sem explicação.
        let old = h.options.as_ref().is_some_and(|campos| !campos.contains_key(CLAUDE_OPTIONS[0].0));
        div().flex().flex_col().when(!rows.is_empty() || old || h.options_error.is_some(), |el| el.mt(px(6.)).pt(px(4.))
                .border_t_1().border_color(theme::border()))
            .children(rows)
            .when(old, |el| el.child(div().id("harness-claude-options-old").role(Role::Status).py(px(4.)).text_size(px(12.5))
                .text_color(theme::muted()).whitespace_normal().child(web("harness_opcoes_indisponiveis"))))
            .children(h.options_error.clone().map(|error| div().id("harness-claude-options-error").role(Role::Alert).py(px(4.))
                .text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error)))
    }

    /// Integração do Codex (`HarnessSettings.svelte`): reconciliar, os dois interruptores e o andamento da última rodada.
    fn codex_integration(&self, cx: &mut Context<Self>) -> Div {
        let h = &self.harness;
        let busy = self.integration_busy();
        let header = div().flex().items_center().gap_3().py(px(6.))
            .child(div().flex_1().min_w_0().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text())
                .whitespace_normal().child(web("harness_codex_integracao")))
            .child(Button::new("harness-codex-reconcile").outline().small().flex_shrink_0()
                .label(web(if busy { "harness_codex_executando" } else { "harness_codex_reconciliar" })).loading(busy).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.reconcile_integration(cx))));
        let mut block = div().flex().flex_col().mt(px(6.)).pt(px(4.)).border_t_1().border_color(theme::border()).child(header)
            .child(self.codex_why("reconcile", "harness_codex_reconciliar", "harness_codex_reconciliar_vered", "harness_codex_reconciliar_porque", cx));
        if let Some(state) = &h.integration {
            block = block.children(CODEX_SWITCHES.iter().map(|&(key, label, help)| {
                let on = if key == "codex_sync" { state.automatica } else { state.memoria };
                div().id(SharedString::from(format!("harness-{key}"))).flex().items_center().gap_3().py(px(6.))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(web(label)))
                        .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(web(help))))
                    .child(Switch::new(SharedString::from(format!("harness-{key}-switch"))).checked(on)
                        .accessibility_label(web(label)).disabled(h.integration_toggling.contains(&key))
                        .on_click(cx.listener(move |this, on: &bool, _, cx| this.toggle_integration(key, *on, cx))))
            }))
                // O prazo é lido antes de ligar: quem liga achando que já vale é o engano que ele evita.
                .child(self.codex_why("memory", "harness_codex_memoria", "harness_codex_memoria_vered", "harness_codex_memoria_prazo", cx))
                .child(codex_status(state));
        }
        block.children(h.integration_error.clone().map(|error| div().id("harness-codex-error").role(Role::Alert).py(px(4.))
            .text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error)))
    }

    /// Veredito curto e o "por quê?" que abre a explicação, como o `cfg-porque` do web.
    fn codex_why(&self, topic: &'static str, name: &str, verdict: &str, reason: &str, cx: &mut Context<Self>) -> Div {
        let key = ("codex".to_owned(), format!("integration-{topic}"));
        let open = self.harness.why.contains(&key);
        let this = cx.entity().downgrade();
        div().pb(px(4.)).flex().flex_col().gap(px(4.))
            .child(div().flex().items_center().gap(px(6.)).text_size(px(12.5))
                .child(div().text_color(theme::muted()).child(web(verdict)))
                .child(Disclosure::new(format!("harness-codex-{topic}-why"), open, tr("accounts_engine_why"), true)
                    .name(tr("accounts_engine_why_of").replace("{name}", &web(name)))
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| {
                        this.harness.why.retain(|k| *k != key);
                        if open { this.harness.why.push(key.clone()); }
                        cx.notify();
                    }); })))
            .when(open, |el| el.child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(web(reason))))
    }

    fn render_harness_item(&self, cli: &str, item: &Item, cx: &mut Context<Self>) -> Div {
        let running = self.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none());
        let here = self.harness.repair.as_ref().filter(|r| r.cli == cli && r.item == item.id);
        let (glyph, color) = match (item.info, item.ok) {
            (true, _) => ("·", theme::muted()),
            (false, Some(true)) => ("✓", theme::success()),
            (false, Some(false)) => ("✕", theme::danger()),
            (false, None) => ("?", theme::muted()),
        };
        let label = item_label(&item.id);
        let row = div().flex().items_start().gap_2().py(px(3.))
            .child(div().w(px(14.)).flex_shrink_0().text_sm().font_weight(FontWeight::BOLD).text_color(color).child(glyph))
            // Um texto só, rótulo em negrito: quebra por palavra dentro da coluna, como o `<b>` do web.
            .child(div().flex_1().min_w_0().text_sm().whitespace_normal().text_color(theme::muted())
                .child(StyledText::new(format!("{label}  {}", item_text(item))).with_highlights([(0..label.len(),
                    HighlightStyle { color: Some(theme::text()), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() })])))
            .when_some(item.conserto.clone(), |el, id| {
                let text = if id.starts_with("sync:") { "harness_sync" } else if item.ok == Some(false) { "harness_fix" } else { "harness_redo" };
                let (cli, item_id) = (cli.to_owned(), item.id.clone());
                el.child(Button::new(SharedString::from(format!("harness-{cli}-{item_id}-fix"))).outline().small().flex_shrink_0()
                    .label(tr(text)).loading(here.is_some_and(|r| r.outcome.is_none())).disabled(running)
                    .on_click(cx.listener(move |this, _, _, cx| this.repair_harness(id.clone(), cli.clone(), item_id.clone(), cx))))
            });
        let key = (cli.to_owned(), item.id.clone());
        let open = self.harness.why.contains(&key);
        let this = cx.entity().downgrade();
        let why = explained(item, cli).then(|| div().pl(px(22.)).flex().flex_col().gap(px(4.))
            .child(div().flex().items_center().gap(px(6.)).text_size(px(12.5))
                .child(div().text_color(theme::muted()).child(web(&format!("harness_item_{}_vered", item.id))))
                .child(Disclosure::new(format!("harness-{cli}-{}-why", item.id), open, tr("accounts_engine_why"), true)
                    .name(tr("accounts_engine_why_of").replace("{name}", &label))
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| {
                        this.harness.why.retain(|k| *k != key);
                        if open { this.harness.why.push(key.clone()); }
                        cx.notify();
                    }); })))
            .when(open, |el| el.child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
                .child(web(&format!("harness_item_{}_porque", item.id))))));
        div().flex().flex_col().child(row).children(why).children(here.map(|r| repair_line(r, &label)))
    }

    pub(super) fn render_harness(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let running = self.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none());
        let title = div().flex().items_center().gap_2()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Harnesses.title()))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()))
            .child(div().flex_1())
            .when(self.api.is_some(), |el| el.child(Button::new("harness-reload").ghost().small().icon(IconName::RefreshCw)
                .label(tr("reload")).loading(self.harness.loading).disabled(self.harness.loading || running)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.harness.options_error = None;
                    this.load_harness(cx);
                    this.poll_install(None, cx);
                    this.load_options(cx);
                    if !this.harness.reconciling { this.read_integration(false, true, cx); }
                }))));
        let mut page = div().flex().flex_col().gap_4().child(title)
            .child(self.mark(div().rounded(px(6.)).text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr("harness_legend")), "harness_legend"));
        if self.api.is_none() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).into_any_element();
        }
        if let Some(error) = &self.harness.error {
            page = page.child(div().id("harness-load-error").role(Role::Alert).text_sm().text_color(theme::danger())
                .whitespace_normal().child(error.clone()));
        }
        let Some(list) = &self.harness.list else {
            if self.harness.loading {
                return page.child(div().id("harness-loading").role(Role::Status).aria_label(tr("loading")).flex().flex_col().gap_2()
                    .children((0..4usize).map(|i| settings_box().id(("harness-skeleton", i)).p_3().gap_2()
                        .child(div().flex().items_center().gap_2().child(Skeleton::new(("harness-glyph", i)).size(px(22.)))
                            .child(Skeleton::new(("harness-name", i)).w(px(120.)).h(px(12.))))
                        .child(Skeleton::new(("harness-item", i)).w(px(320.)).h(px(10.))))))
                    .into_any_element();
            }
            return page.child(Button::new("harness-retry").outline().small().label(tr("server_retry"))
                .on_click(cx.listener(|this, _, _, cx| this.load_harness(cx)))).into_any_element();
        };
        if list.is_empty() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("harness_empty"))).into_any_element();
        }
        let cards: Vec<Div> = list.iter().map(|h| {
            let bad = h.instalado && h.itens.iter().any(|i| i.ok == Some(false));
            let dot = if !h.instalado { theme::faint() } else if bad { theme::danger() } else { theme::success() };
            let glyph = if h.id == "tmux" {
                div().size(px(22.)).flex_shrink_0().rounded(px(4.)).bg(theme::raised()).flex().items_center().justify_center()
                    .text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(theme::muted()).child("⌗")
            } else { chrome::provider_glyph(&h.id, 22.) };
            let version = if h.instalado { h.versao.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| tr("harness_installed")) }
                else { tr("harness_not_installed") };
            // Desfecho de um item que sumiu depois do conserto (deixou de se aplicar): fica no card.
            let orphan = self.harness.repair.as_ref().filter(|r| r.cli == h.id && r.outcome.is_some() && !h.itens.iter().any(|i| i.id == r.item));
            settings_box().p_3().gap_1().when(!h.instalado, |el| el.opacity(0.7))
                .child(div().flex().items_center().gap_2().pb(px(4.))
                    .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(dot))
                    .child(glyph)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(h.nome.clone()))
                    .child(div().min_w_0().truncate().text_sm().text_color(theme::muted()).font_family(theme::MONO).child(version)))
                .children(h.itens.iter().map(|item| self.render_harness_item(&h.id, item, cx)))
                .children(orphan.map(|r| repair_line(r, &item_label(&r.item))))
                .when(h.id == "claude", |el| el.child(self.claude_options(cx)))
                .when(h.id == "codex", |el| el.child(self.codex_integration(cx)))
                .children((!h.instalado).then(|| self.install_offer(h, cx)).flatten())
                .children(self.harness.install_error.as_ref().filter(|(owner, _)| owner.as_deref() == Some(h.id.as_str()))
                    .map(|(_, error)| install_alert(error)))
                .children(self.install_progress(&h.id))
        }).collect();
        let loose = self.harness.install_error.as_ref()
            .filter(|(owner, _)| !owner.as_ref().is_some_and(|o| list.iter().any(|h| h.id == *o))).map(|(_, error)| install_alert(error));
        page.child(div().flex().flex_col().gap_3().children(cards)).children(loose).into_any_element()
    }

    /// CLI ausente: botão quando o servidor tem comando conferido para este sistema, senão o endereço do fornecedor.
    fn install_offer(&self, h: &Cli, cx: &mut Context<Self>) -> Option<Div> {
        let install = self.harness.install.as_ref()?;
        let text = |key: &str| div().text_sm().whitespace_normal().text_color(theme::muted()).child(web(key));
        let row = div().flex().items_start().gap_2().py(px(3.))
            .child(div().w(px(14.)).flex_shrink_0().text_sm().font_weight(FontWeight::BOLD).text_color(theme::muted()).child("·"));
        if install.comandos.contains_key(&h.id) {
            let busy = install.running() || self.harness.starting.is_some();
            let (cli, name) = (h.id.clone(), h.nome.clone());
            return Some(row.child(text("harness_inst_disponivel").flex_1().min_w_0())
                .child(Button::new(SharedString::from(format!("harness-{}-install", h.id))).outline().small().flex_shrink_0()
                    .label(web("harness_inst_botao")).loading(self.harness.starting.as_deref() == Some(h.id.as_str())).disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.confirm_install(cli.clone(), name.clone(), window, cx)))));
        }
        // Só http(s): o endereço vem do servidor e vira clique que abre o navegador.
        let url = install.manual.get(&h.id).filter(|u| u.starts_with("https://") || u.starts_with("http://")).cloned();
        Some(row.child(div().flex_1().min_w_0().flex().flex_col().items_start().gap(px(2.)).child(text("harness_inst_manual"))
            .children(url.map(|url| {
                let open = url.clone();
                Button::new(SharedString::from(format!("harness-{}-manual", h.id))).link().small().label(url)
                    .on_click(move |_, _, cx| cx.open_url(&open))
            }))))
    }

    /// Andamento e desfecho da instalação deste CLI, com a saída do instalador.
    fn install_progress(&self, cli: &str) -> Option<Div> {
        let install = self.harness.install.as_ref().filter(|i| i.harness.as_deref() == Some(cli) && (i.running() || i.fase == "pronto"))?;
        let step = install_step(install.etapa.as_deref());
        let (headline, color) = if install.running() {
            (web_with("harness_inst_andamento", &[("passo", install.passo.to_string()), ("total", install.total.to_string()), ("etapa", step)]),
                theme::muted())
        } else if install.ok == Some(true) {
            (web("harness_inst_pronto"), theme::success())
        } else {
            (web_with("harness_inst_falhou", &[("etapa", step)]), theme::danger())
        };
        let line = || div().text_size(px(12.5)).whitespace_normal();
        Some(div().pl(px(22.)).pt(px(4.)).flex().flex_col().gap_2()
            .child(line().id("harness-install-status").role(Role::Status).text_color(color).child(headline))
            .when(install.running(), |el| el.child(Progress::new("harness-install-progress").accessibility_label(web("harness_inst_progresso"))
                .map(|bar| if install.total > 0 { bar.value(install.passo as f32 * 100. / install.total as f32) } else { bar.loading(true) })))
            .children(install.erro.clone().map(|error| line().id("harness-install-error").role(Role::Alert).text_color(theme::danger()).child(error)))
            // Etapa pulada não vive só no log: a manchete promete "ligado ao app".
            .when(!install.avisos.is_empty(), |el| el.child(line().id("harness-install-warnings").role(Role::Status).text_color(theme::warning())
                .flex().flex_col().gap_1().children(install.avisos.iter().map(|aviso| div().child(aviso.clone())))))
            .when(!install.log.is_empty(), |el| el.child(div().rounded(px(6.)).bg(theme::raised()).p_2()
                .child(scrolled("harness-install-log", &self.harness.install_log, 220., div().id("harness-install-log-text")
                    .aria_label(web("harness_inst_log")).font_family(theme::MONO).text_size(px(11.5)).text_color(theme::muted())
                    .whitespace_normal().children(install.log.iter().map(|l| div().child(l.clone()))))))))
    }
}

#[cfg(test)]
mod tests {
    use super::{Install, Integration};

    #[test]
    fn integration_reads_backend_shape_and_measures_progress() {
        let running: Integration = serde_json::from_value(serde_json::json!({"estado": "executando",
            "etapa": {"codigo": "etapa_inventariando", "params": {}, "texto": "Inventariando configuração"},
            "ultima_execucao": null, "proxima_atualizacao": null, "plugins": [{"id": "p@m", "versao": "1.0", "origem": "claude"}],
            "erros": [], "avisos": [], "confianca_pendente": false, "progresso": {"passo": 3, "total": 5, "sub": {"atual": 2, "total": 4}},
            "etapa_segundos": 7, "skills": {"ponte": 2, "nativas": 1}, "automatica": true, "memoria": false})).unwrap();
        let steps = running.progresso.as_ref().unwrap();
        // (3 - 1 + (2 - 1) / 4) / 5 = 45%: a etapa em curso conta só o que o sub-andamento mediu.
        assert!((steps.percent() - 45.).abs() < 0.01 && running.automatica && !running.memoria && running.plugins.len() == 1);
        let old: Integration = serde_json::from_value(serde_json::json!({"estado": "ocioso"})).unwrap();
        assert!(old.progresso.is_none() && old.plugins.is_empty() && old.skills.is_none() && !old.automatica);
    }

    #[test]
    fn install_state_reads_backend_shape_and_tolerates_missing_fields() {
        let full: Install = serde_json::from_value(serde_json::json!({"fase": "rodando", "harness": "kimi", "etapa": "comando",
            "passo": 1, "total": 4, "log": ["$ curl"], "avisos": [], "ok": null, "erro": null,
            "comandos": {"kimi": "curl -fsSL x | bash"}, "manual": {"omp": "https://example.com"}})).unwrap();
        assert!(full.running() && full.comandos.contains_key("kimi") && full.log.len() == 1);
        let old: Install = serde_json::from_value(serde_json::json!({"fase": "ocioso"})).unwrap();
        assert!(!old.running() && old.log.is_empty() && old.comandos.is_empty());
    }
}

fn install_alert(error: &str) -> Stateful<Div> {
    div().id("harness-install-failure").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.to_owned())
}

/// "Consertando X… Ns" enquanto roda; depois, o que o servidor fez ou o motivo da falha.
fn repair_line(repair: &Repair, label: &str) -> Stateful<Div> {
    let line = div().pl(px(22.)).pb(px(2.)).text_size(px(12.5)).whitespace_normal();
    match &repair.outcome {
        None => line.id("harness-repairing").role(Role::Status).text_color(theme::muted())
            .child(tr("harness_fixing").replace("{item}", label).replace("{s}", &repair.started.elapsed().as_secs().to_string())),
        Some(Ok(done)) => line.id("harness-repaired").role(Role::Status).text_color(theme::success()).child(done.clone()),
        Some(Err(error)) => line.id("harness-repair-error").role(Role::Alert).text_color(theme::danger()).child(error.clone()),
    }
}

/// Estado da última rodada da integração: manchete, andamento, datas, o que ela gerencia, avisos e erros.
fn codex_status(state: &Integration) -> Div {
    let line = || div().text_size(px(12.5)).whitespace_normal().text_color(theme::muted());
    let name = match state.estado.as_str() {
        known @ ("ocioso" | "executando" | "ok" | "parcial" | "erro" | "indisponivel") => web(&format!("harness_codex_{known}")),
        other => other.to_owned(),
    };
    let stage = codex_text(state.etapa.as_ref());
    let headline = if stage.is_empty() { name } else { format!("{name} · {stage}") };
    let progress = state.progresso.as_ref().filter(|_| state.estado == "executando").map(|steps| {
        let mut text = web_with("harness_codex_progresso", &[("passo", steps.passo.to_string()), ("total", steps.total.to_string())]);
        if let Some(sub) = &steps.sub {
            text += &format!(" · {}", web_with("harness_codex_progresso_sub", &[("atual", sub.atual.to_string()), ("total", sub.total.to_string())]));
        }
        if let Some(seconds) = state.etapa_segundos { text += &format!(" · {}", web_with("harness_codex_etapa_tempo", &[("s", seconds.to_string())])); }
        div().flex().flex_col().gap(px(4.)).child(line().child(text))
            .child(Progress::new("harness-codex-progress").accessibility_label(web("harness_codex_integracao")).value(steps.percent()))
    });
    div().flex().flex_col().gap(px(3.)).pt(px(4.)).pb(px(6.))
        .child(line().id("harness-codex-status").role(Role::Status).text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text())
            .child(headline))
        .children(progress)
        .child(line().child(web_with("harness_codex_ultima", &[("data", codex_date(state.ultima_execucao.as_deref()))])))
        .children(state.proxima_atualizacao.as_deref().map(|at| line().child(web_with("harness_codex_proxima", &[("data", codex_date(Some(at)))]))))
        .child(line().child(web_with("harness_codex_plugins", &[("n", state.plugins.len().to_string())])))
        .children(state.skills.as_ref().map(|s| line().child(web_with("harness_codex_skills",
            &[("ponte", s.ponte.to_string()), ("nativas", s.nativas.to_string())]))))
        .when(!state.plugins.is_empty(), |el| el.child(div().pl(px(12.)).flex().flex_col().gap(px(2.))
            .children(state.plugins.iter().map(|p| line().child(StyledText::new(format!("{} · {} · {}", p.id, p.versao, p.origem))
                .with_highlights([(0..p.id.len(), HighlightStyle { color: Some(theme::text()), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() })]))))))
        .when(state.confianca_pendente, |el| el.child(line().id("harness-codex-trust").role(Role::Status).child(web("harness_codex_confianca"))))
        .children(state.avisos.iter().map(|aviso| line().child(codex_text(Some(aviso)))))
        .children(state.erros.iter().enumerate().map(|(i, falha)| line().id(("harness-codex-failure", i)).role(Role::Alert)
            .text_color(theme::danger()).child(codex_text(Some(falha)))))
}
