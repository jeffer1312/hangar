//! Páginas Geral, Diário de uso e Sobre. Idioma e moeda são deste computador (mesmo arquivo da Aparência);
//! cotação, diário e atualização vêm do servidor conectado, em pedidos numerados: resposta de pedido anterior
//! ao último é descartada. O que a tela mostra é preparado na chegada da resposta, nunca no desenho.
use super::*;
use super::settings::{Page, segments, settings_box};
use crate::appearance::{self, Currency, Language};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::switch::Switch;

/// Um pedido ao servidor e o último resultado: carregando, erro, vazio e com dados saem daqui.
pub(super) struct Remote<T> { pub(super) value: Option<Result<T, String>>, pub(super) loading: bool, pub(super) seq: u64 }

impl<T> Default for Remote<T> {
    fn default() -> Self { Self { value: None, loading: false, seq: 0 } }
}

impl<T> Remote<T> {
    pub(super) fn start(&mut self) -> u64 { self.seq += 1; self.loading = true; self.seq }
    /// Guarda o resultado só se ele for do último pedido.
    pub(super) fn finish(&mut self, seq: u64, value: Result<T, String>) -> bool {
        if seq != self.seq { return false; }
        (self.loading, self.value) = (false, Some(value));
        true
    }
    pub(super) fn ok(&self) -> Option<&T> { self.value.as_ref()?.as_ref().ok() }
    /// Valor que chegou por outro caminho e é mais novo que qualquer pedido em voo: esse pedido passa a ser descartado.
    pub(super) fn set(&mut self, value: Result<T, String>) { self.seq += 1; (self.loading, self.value) = (false, Some(value)); }
    /// Volta ao vazio sem reusar o número: a resposta do pedido em voo passa a ser descartada.
    pub(super) fn reset(&mut self) { self.seq += 1; (self.loading, self.value) = (false, None); }
}

#[derive(serde::Deserialize)]
struct Channel { branch: String, checkout_branch: String, last_branch: String }

fn record_channel_404(path: &std::path::Path, saved: bool) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "[{}] update_channel.failed route=/api/update-channel method={} status=404",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), if saved { "PUT" } else { "GET" })
}

fn record_channel_404_or_report(path: &std::path::Path, saved: bool, report: impl FnOnce(std::io::Error)) {
    if let Err(error) = record_channel_404(path, saved) { report(error); }
}

#[derive(Default)]
struct ChannelEditor {
    request: Remote<()>,
    current: Option<Channel>,
    enabled: bool,
    draft: String,
    applied: bool,
    unsupported: bool,
}

impl ChannelEditor {
    fn receive_unsupported(&mut self, seq: u64, saved: bool, message: String) -> bool {
        if saved { return self.receive(seq, true, Err(message)); }
        if !self.request.finish(seq, Ok(())) { return false; }
        self.unsupported = true;
        self.current = None;
        self.applied = false;
        true
    }
    fn after_update(&mut self) { self.applied = false; self.request.reset(); }
    fn target(&self) -> &str { if self.enabled { self.draft.trim() } else { "" } }
    fn is_dirty(&self) -> bool {
        self.current.as_ref().is_some_and(|current| {
            let enabled = !current.branch.is_empty();
            self.enabled != enabled || current.branch != self.target()
        })
    }
    fn blocks_update(&self) -> bool {
        self.request.loading || self.is_dirty()
    }
    fn receive(&mut self, seq: u64, saved: bool, result: Result<Channel, String>) -> bool {
        let preserve_draft = !saved && self.is_dirty();
        if !self.request.finish(seq, result.as_ref().map(|_| ()).map_err(Clone::clone)) { return false; }
        if let Ok(channel) = result {
            self.unsupported = false;
            if !preserve_draft {
                self.enabled = !channel.branch.is_empty();
                self.draft = if self.enabled { channel.branch.clone() } else { channel.last_branch.clone() };
            }
            self.current = Some(channel);
            self.applied = saved;
        }
        true
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Level { Ok, Warn, Error }

struct DiaryLine { when: String, event: String, context: String, level: Level }

/// `missing`: servidor anterior à rota do diário; não é falha, só não há o que mostrar.
pub(super) struct Diary { days: u64, bytes: u64, keep: u64, lines: Vec<DiaryLine>, missing: bool }

pub(super) struct ServerVersion { version: String, local: bool, ts: Option<String>, channel: String }

/// Onde está a atualização que este app mandou (ou achou rodando).
#[derive(Clone, PartialEq)]
enum Run { Starting, Uncertain, Running { step: u64, total: u64, text: String }, Restarting, Done { ok: bool, text: String } }

#[derive(Default)]
pub(super) struct Device {
    rate: Remote<Option<f64>>,
    diary: Remote<Diary>,
    /// Caminho gravado (ou o erro). `saving`: o arquivo já tem destino e está vindo do servidor.
    download: Remote<String>,
    saving: bool,
    about: Remote<ServerVersion>,
    channel: ChannelEditor,
    channel_input: Option<Entity<InputState>>,
    _channel_subscription: Option<Subscription>,
    /// Resultado da última procura: quantas mudanças há (0 = em dia).
    search: Option<Result<usize, String>>,
    searching: bool,
    /// Lendo avisos e novidades antes do "Atualizar?" (Sobre e Servidores).
    pub(super) notes_loading: bool,
    run: Option<Run>,
    run_seq: u64,
    run_task: Option<JoinHandle<()>>,
    /// A atualização em curso já apareceu como "rodando": o próximo "pronto" é o desfecho dela.
    saw_running: bool,
    /// Marca do estado lido antes do clique: "pronto" com a mesma marca é o desfecho de uma atualização anterior.
    baseline_ts: Option<String>,
    diary_scroll: ScrollHandle,
    pub(super) migration: Remote<Option<super::migration::MigrationView>>,
    pub(super) migration_poll: Option<Task<()>>,
}

impl Drop for Device {
    fn drop(&mut self) { if let Some(task) = self.run_task.take() { task.abort(); } }
}

impl Device {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("settings_channel_branch_placeholder")));
        let sub = cx.subscribe_in(&input, window, |this: &mut Hangar, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if this.device.channel.draft != value {
                    this.device.channel.draft = value;
                    this.device.channel.applied = false;
                    this.sync_channel_update_guard(cx);
                    cx.notify();
                }
            }
        });
        let mut device = Self::default();
        device.channel_input = Some(input);
        device._channel_subscription = Some(sub);
        device
    }
    fn rate_value(&self) -> Option<f64> { self.rate.ok().copied().flatten() }
    fn run_active(&self) -> bool { self.run.as_ref().is_some_and(|run| !matches!(run, Run::Done { .. })) }
}

pub(super) enum DeviceReply {
    Rate(u64, Result<Value, Failure>),
    Diary(u64, Result<Value, Failure>),
    Downloaded(u64, Result<PathBuf, String>),
    About(u64, bool, Result<Value, Failure>),
    Channel(u64, bool, Result<Value, Failure>),
    Started(u64, Result<Value, Failure>),
    Tick(u64, Result<Value, Failure>),
    Migration(u64, Result<Value, Failure>),
}

/// Valor em dólar na moeda escolhida. Real sem cotação fica em dólar.
pub(super) fn money(usd: f64, currency: Currency, rate: Option<f64>) -> String {
    let (symbol, value) = match (currency, rate) { (Currency::Brl, Some(rate)) => ("R$", usd * rate), _ => ("US$", usd) };
    // ponytail: sem separador de milhar; custo por sessão raramente passa de mil.
    format!("{symbol} {}", format!("{value:.2}").replace('.', &tr("decimal")))
}

pub(super) fn size_text(bytes: u64) -> String {
    let (value, unit) = match bytes { b if b >= 1 << 20 => (b as f64 / (1 << 20) as f64, "MB"), b if b >= 1 << 10 => (b as f64 / 1024., "KB"), b => (b as f64, "B") };
    let text = if unit == "B" { format!("{value:.0}") } else { format!("{value:.1}").replace('.', &tr("decimal")) };
    format!("{text} {unit}")
}

fn text_of(value: &Value) -> Option<String> {
    match value { Value::String(s) if !s.is_empty() => Some(s.clone()), Value::Number(n) => Some(n.to_string()), _ => None }
}

/// "2026-09-24T14:37:12-03:00" → "24/09 14:37:12", no relógio de quem gravou. Fora do formato, traços.
fn when(ts: &str) -> String {
    let (Some(date), Some(time)) = (ts.get(..10), ts.get(11..19)) else { return "--/-- --:--:--".into() };
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 || time.len() != 8 { return "--/-- --:--:--".into(); }
    format!("{}/{} {time}", parts[2], parts[1])
}

/// O contexto de uma linha, como no web: o código HTTP só quando não deu certo.
fn diary_line(value: &Value) -> DiaryLine {
    let field = |name: &str| value.get(name).and_then(text_of);
    let level = match field("nivel").as_deref() { Some("aviso") => Level::Warn, Some("erro") => Level::Error, _ => Level::Ok };
    let platform = field("so").map(|so| [Some(so), field("navegador"), field("vista"), field("tela_px")].into_iter().flatten().collect::<Vec<_>>().join(" · "));
    let code = field("codigo").filter(|_| level != Level::Ok).map(|c| format!("#{c}"));
    let context = [field("detalhe"), platform, field("sessao"), field("tela"), code, field("ms").map(|ms| format!("{ms}ms"))]
        .into_iter().flatten().collect::<Vec<_>>().join(" · ");
    DiaryLine { when: when(&field("ts").unwrap_or_default()), event: field("evento").unwrap_or_else(|| "?".into()), context, level }
}

fn parse_diary(value: &Value) -> Diary {
    let number = |name: &str| value.get(name).and_then(Value::as_u64).unwrap_or(0);
    let lines = value.get("ultimas").and_then(Value::as_array).map(|lines| lines.iter().map(diary_line).collect()).unwrap_or_default();
    Diary { days: number("dias"), bytes: number("bytes"), keep: value.get("dias_guardados").and_then(Value::as_u64).unwrap_or(7), lines, missing: false }
}

/// Versão legível do web: `-dirty` vira aviso, e do `git describe` antigo fica só o hash.
fn legible(raw: &str) -> (String, bool) {
    let local = raw.ends_with("-dirty");
    let clean = raw.strip_suffix("-dirty").unwrap_or(raw);
    let hash = clean.rsplit_once("-g").map(|(_, h)| h).filter(|h| h.len() >= 7 && h.chars().all(|c| c.is_ascii_hexdigit()));
    (hash.unwrap_or(clean).to_owned(), local)
}

fn parse_version(value: &Value) -> ServerVersion {
    let raw = value.pointer("/versao_legivel/backend").and_then(Value::as_str)
        .or_else(|| value.pointer("/versoes/backend").and_then(Value::as_str)).unwrap_or("?");
    let (version, local) = legible(raw);
    ServerVersion { version, local, ts: value.pointer("/estado/ts").and_then(Value::as_str).map(str::to_owned), channel: crate::update::alvo(value) }
}

/// Mudanças à espera: commits novos, ou o servidor rodando código diferente do que está no disco.
fn pending(value: &Value) -> usize {
    let behind = value.get("atras").and_then(Value::as_u64).unwrap_or(0) as usize;
    let available = value.get("atualizacao_disponivel").and_then(Value::as_bool).unwrap_or(false);
    let stale = value.pointer("/versoes/repo") != value.pointer("/versoes/backend");
    if available || stale { behind.max(1) } else { 0 }
}

impl Hangar {
    pub(super) fn money(&self, usd: f64) -> String { money(usd, appearance::get().currency, self.device.rate_value()) }

    /// Nova conexão: nada do servidor anterior fica na tela, e a cotação é pedida para os custos.
    pub(super) fn reset_device(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.device = Device::new(window, cx);
        self.sync_channel_update_guard(cx);
        self.load_rate(cx);
        // A troca levou a leitura periódica junto; com a página aberta, ela recomeça no servidor novo.
        if self.settings == Some(Page::Migration) { self.migration_opened(cx); }
    }

    /// Página aberta: pede o que ela mostra do servidor.
    pub(super) fn settings_opened(&mut self, page: Page, cx: &mut Context<Self>) {
        // Outra página: a tentativa de login perde a tela (a navegação e a busca passam por aqui).
        if page != Page::Accounts { self.accounts_page_left(); }
        if page != Page::Sync { self.sync_page_left(); }
        if page != Page::Connect { self.connect_page_left(); }
        if page != Page::Devices { self.devices_page_left(); }
        match page {
            Page::General if !self.device.rate.loading && self.device.rate_value().is_none() => self.load_rate(cx),
            Page::Diary => self.load_diary(cx),
            Page::About => {
                if !self.device.about.loading { self.load_about(false, cx); }
                if !self.device.channel.request.loading && !self.device.channel.is_dirty() { self.load_channel(cx); }
            }
            Page::Accounts => self.accounts_opened(cx),
            Page::Orchestration => self.orchestration_opened(cx),
            Page::Worktrees => self.worktree_settings_opened(cx),
            Page::Shortcuts => self.shortcuts_opened(cx),
            Page::Harnesses => self.harness_opened(cx),
            Page::Voice | Page::Jev | Page::Notifications | Page::Attachments | Page::Advanced => self.server_config_opened(page, cx),
            // As origens do terminal moram no Avançado do detalhe e usam o rascunho do servidor.
            Page::Servers => { self.machines_opened(cx); self.server_config_opened(page, cx); }
            Page::Windows => self.computer_opened(cx),
            Page::Sync => self.sync_opened(cx),
            Page::Connect => self.connect_opened(cx),
            Page::Devices => self.devices_opened(cx),
            Page::SharedConfig => self.shared_config_opened(cx),
            Page::Migration => self.migration_opened(cx),
            _ => {}
        }
    }

    fn load_rate(&mut self, cx: &mut Context<Self>) {
        // `cotacao` é rota do servidor inteiro: o convite a recusa com 403.
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        let seq = self.device.rate.start();
        let done = self.device_send_later();
        self.runtime.spawn(async move { done(DeviceReply::Rate(seq, api.server_read(&["cotacao"], &[], 15).await)).await });
        cx.notify();
    }

    fn load_diary(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.device.diary.start();
        // O "Salvo em…" da baixa anterior sai: a linha volta a descrever o diário que está chegando.
        if !self.device.download.loading { self.device.download.value = None; }
        let done = self.device_send_later();
        self.runtime.spawn(async move { done(DeviceReply::Diary(seq, api.server_read(&["diag"], &[], 20).await)).await });
        cx.notify();
    }

    fn load_about(&mut self, search: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.device.about.start();
        self.device.searching = search;
        let done = self.device_send_later();
        self.runtime.spawn(async move {
            // Procurar vai à rede antes de comparar (`git fetch`, até 120 s no servidor).
            let query: &[(&str, &str)] = if search { &[("procurar", "1")] } else { &[] };
            done(DeviceReply::About(seq, search, api.server_read(&["atualizacao"], query, if search { 150 } else { 20 }).await)).await
        });
        cx.notify();
    }

    fn load_channel(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        if self.device.channel.request.loading { return; }
        let seq = self.device.channel.request.start();
        self.sync_channel_update_guard(cx);
        let done = self.device_send_later();
        self.runtime.spawn(async move { done(DeviceReply::Channel(seq, false, api.server_read(&["update-channel"], &[], 30).await)).await });
        cx.notify();
    }

    fn apply_channel(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        let channel = &mut self.device.channel;
        if channel.request.loading || channel.current.is_none() || !channel.is_dirty() || (channel.enabled && channel.target().is_empty()) { return; }
        if self.device.run_active() || self.device.about.loading || self.channel_updater_busy(cx) { return; }
        let branch = self.device.channel.target().to_owned();
        let seq = self.device.channel.request.start();
        self.device.channel.applied = false;
        self.sync_channel_update_guard(cx);
        let done = self.device_send_later();
        self.runtime.spawn(async move {
            done(DeviceReply::Channel(seq, true, api.server_send(reqwest::Method::PUT, &["update-channel"], Some(json!({"branch": branch})), 45).await)).await
        });
        cx.notify();
    }

    fn sync_channel_update_guard(&self, cx: &mut Context<Self>) {
        let Some(updater) = cx.try_global::<crate::update::Handle>().map(|handle| handle.0.clone()) else { return };
        let address = self.api.as_ref().filter(|_| self.device.channel.blocks_update()).map(Api::identity);
        updater.update(cx, |updater, cx| updater.set_channel_blocked(address, cx));
    }

    fn channel_updater_busy(&self, cx: &App) -> bool {
        self.api.as_ref().is_some_and(Api::is_loopback) && cx.try_global::<crate::update::Handle>().is_some_and(|handle| handle.0.read(cx).is_busy())
    }

    /// Envio de resposta para depois, amarrado à conexão de agora.
    pub(super) fn device_send_later(&self) -> impl Fn(DeviceReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Device(reply) }).await; })
        }
    }

    /// Escolhe onde salvar primeiro; só então pede o arquivo ao servidor.
    fn download_diary(&mut self, cx: &mut Context<Self>) {
        if self.device.download.loading || self.api.is_none() { return; }
        let seq = self.device.download.start();
        self.device.saving = false;
        let prompt = cx.prompt_for_new_path(&downloads_folder(), Some("hangar-uso.jsonl"));
        cx.spawn(async move |this, cx| {
            let chosen = prompt.await;
            let _ = this.update(cx, |this, cx| {
                match chosen {
                    Ok(Ok(Some(path))) => return this.fetch_diary(seq, path, cx),
                    // Cancelado: volta ao que estava, sem mensagem.
                    Ok(Ok(None)) => if this.device.download.seq == seq { this.device.download.loading = false; },
                    _ => { this.device.download.finish(seq, Err(tr("save_dialog_failed"))); }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn fetch_diary(&mut self, seq: u64, path: PathBuf, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| self.device.download.seq == seq) else { return };
        self.device.saving = true;
        let done = self.device_send_later();
        self.runtime.spawn(async move {
            let result = match api.server_bytes(&["diag", "arquivo"], 60).await {
                Err(error) => Err(Self::failure(&error)),
                Ok(bytes) => tokio::task::spawn_blocking(move || std::fs::write(&path, bytes).map(|()| path)).await
                    .map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| format!("{}: {e}", tr("save_failed")))),
            };
            done(DeviceReply::Downloaded(seq, result)).await
        });
        cx.notify();
    }

    fn open_update_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_server_update(Self::start_update, window, cx);
    }

    /// "Atualizar {server}?" do servidor ativo, com os avisos e as novidades lidos antes de abrir; `start` roda ao confirmar.
    pub(super) fn confirm_server_update(&mut self, start: fn(&mut Self, &mut Context<Self>), window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.device.notes_loading { return; }
        self.device.notes_loading = true;
        cx.notify();
        crate::update::confirm::load_then_open(api, &self.runtime, window, cx, move |this, _, notes, window, cx| {
            this.device.notes_loading = false;
            cx.notify();
            let weak = cx.entity().downgrade();
            let server = this.server_label(cx);
            crate::update::confirm::open(window, cx, tr("update_confirm_title").replace("{server}", &server), tr("update_confirm_desc"),
                notes, None, move |_, cx| { let _ = weak.update(cx, |this, cx| start(this, cx)); });
        });
    }

    fn start_update(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.device.run_active() || self.device.channel.blocks_update() { return; }
        self.device.baseline_ts = self.device.about.ok().and_then(|v| v.ts.clone());
        self.device.run = Some(Run::Starting);
        self.device.run_seq += 1;
        let seq = self.device.run_seq;
        let done = self.device_send_later();
        self.runtime.spawn(async move { done(DeviceReply::Started(seq, api.server_post(&["atualizacao", "iniciar"], 30).await)).await });
        cx.notify();
    }

    /// Lê o estado a cada 2 s até o desfecho; a queda durante o reinício é esperada. Dez minutos sem desfecho encerram.
    fn follow_update(&mut self) {
        let Some(api) = self.api.clone() else { return };
        if let Some(task) = self.device.run_task.take() { task.abort(); }
        self.device.run_seq += 1;
        let seq = self.device.run_seq;
        let done = self.device_send_later();
        self.device.run_task = Some(self.runtime.spawn(async move {
            for _ in 0..300 {
                done(DeviceReply::Tick(seq, api.server_read(&["atualizacao"], &[], 10).await)).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            done(DeviceReply::Tick(seq, Err(Failure::local("update_silent")))).await;
        }));
    }

    fn finish_update(&mut self, ok: bool, text: String, cx: &mut Context<Self>) {
        self.device.run = Some(Run::Done { ok, text });
        if let Some(task) = self.device.run_task.take() { task.abort(); }
        self.device.search = None;
        self.device.channel.after_update();
        self.load_channel(cx);
    }

    pub(super) fn receive_device(&mut self, reply: DeviceReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            DeviceReply::Channel(seq, saved, result) => {
                if result.as_ref().is_err_and(|error| error.status == Some(404)) {
                    record_channel_404_or_report(&crate::log_dir().join("native.log"), saved, |error| {
                        window.push_notification(Notification::error(tr("settings_channel_log_failed")), cx);
                        eprintln!("não consegui registrar o 404 do canal de testes: {error}");
                    });
                    let message = Self::setting_failure(result.as_ref().unwrap_err());
                    if self.device.channel.receive_unsupported(seq, saved, message) { self.sync_channel_update_guard(cx); cx.notify(); }
                    return;
                }
                let value = result.map_err(|e| Self::setting_failure(&e)).and_then(|value|
                    serde_json::from_value::<Channel>(value).map_err(|_| tr("settings_channel_invalid_response")));
                let succeeded = value.is_ok();
                if !self.device.channel.receive(seq, saved, value) { return; }
                self.sync_channel_update_guard(cx);
                if succeeded {
                    if let Some(input) = self.device.channel_input.clone() {
                        let draft = self.device.channel.draft.clone();
                        input.update(cx, |input, cx| input.set_value(draft, window, cx));
                    }
                    if saved {
                        self.device.search = None;
                        self.load_about(false, cx);
                        // O app segue o canal do servidor desta máquina: a procura da release dele refaz já.
                        if let Some(updater) = cx.try_global::<crate::update::Handle>().map(|handle| handle.0.clone()) {
                            updater.update(cx, |updater, cx| updater.refresh(cx));
                        }
                    }
                }
            }
            DeviceReply::Migration(seq, result) => {
                self.device.migration.finish(seq, result.map(|v| super::migration::parse(&v)).map_err(|e| Self::failure(&e)));
            }
            DeviceReply::Rate(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e))
                    .map(|v| v.get("usd_brl").and_then(Value::as_f64).filter(|r| r.is_finite() && *r > 0.));
                self.device.rate.finish(seq, parsed);
            }
            DeviceReply::Diary(seq, result) => {
                // Servidor sem a rota do diário: diz isso, não é falha.
                let parsed = match result {
                    Ok(v) => Ok(parse_diary(&v)),
                    Err(e) if e.status == Some(404) => Ok(Diary { days: 0, bytes: 0, keep: 7, lines: Vec::new(), missing: true }),
                    Err(e) => Err(Self::failure(&e)),
                };
                self.device.diary.finish(seq, parsed);
            }
            DeviceReply::Downloaded(seq, result) => {
                if self.device.download.finish(seq, result.map(|path| tr("saved").replace("{path}", &path.display().to_string()))) {
                    self.device.saving = false;
                }
            }
            DeviceReply::About(seq, search, result) => {
                let pending_count = result.as_ref().ok().map(pending);
                let running = result.as_ref().ok().and_then(|v| v.pointer("/estado/fase")).and_then(Value::as_str) == Some("rodando");
                let failure = result.as_ref().err().map(Self::failure);
                if !self.device.about.finish(seq, result.map(|v| parse_version(&v)).map_err(|e| Self::failure(&e))) { return; }
                if search {
                    self.device.searching = false;
                    self.device.search = Some(pending_count.ok_or_else(|| failure.unwrap_or_default()));
                }
                // Atualização em curso, mandada daqui antes ou por outra tela: acompanha.
                if running && !self.device.run_active() {
                    (self.device.run, self.device.saw_running) = (Some(Run::Running { step: 0, total: 0, text: String::new() }), true);
                    self.follow_update();
                }
            }
            DeviceReply::Started(seq, result) => {
                if seq != self.device.run_seq { return; }
                match result {
                    // O servidor grava "rodando" antes de responder: o próximo "pronto" é o desfecho.
                    Ok(value) if crate::update::update_process_id(&value).is_some() => {
                        (self.device.run, self.device.saw_running) = (Some(Run::Running { step: 0, total: 0, text: String::new() }), true);
                        self.follow_update();
                    }
                    Ok(_) => self.finish_update(false, tr("invalid_response"), cx),
                    Err(error) if error.uncertain => { (self.device.run, self.device.saw_running) = (Some(Run::Uncertain), false); self.follow_update(); }
                    Err(error) => self.finish_update(false, tr("update_refused").replace("{reason}", &Self::failure(&error)), cx),
                }
            }
            DeviceReply::Tick(seq, result) => {
                if seq != self.device.run_seq { return; }
                match result {
                    Err(error) if error.status.is_none() && error.detail == "update_silent" => self.finish_update(false, tr("update_silent"), cx),
                    Err(error) if matches!(error.status, Some(401 | 403)) => self.finish_update(false, tr("auth_error"), cx),
                    // Reiniciando: sem resposta por alguns segundos.
                    Err(_) => if self.device.saw_running { self.device.run = Some(Run::Restarting); },
                    Ok(value) => {
                        self.device.about.set(Ok(parse_version(&value)));
                        self.device.searching = false;
                        let state = value.get("estado").cloned().unwrap_or(Value::Null);
                        let text = |name: &str| state.get(name).and_then(Value::as_str).unwrap_or("").to_owned();
                        let number = |name: &str| state.get(name).and_then(Value::as_u64).unwrap_or(0);
                        match text("fase").as_str() {
                            "rodando" => {
                                self.device.saw_running = true;
                                self.device.run = Some(Run::Running { step: number("passo"), total: number("total"), text: text("texto") });
                            }
                            "pronto" if !self.device.saw_running && state.get("ts").and_then(Value::as_str) == self.device.baseline_ts.as_deref() =>
                                self.finish_update(false, tr("update_not_started"), cx),
                            "pronto" => {
                                let ok = state.get("ok").and_then(Value::as_bool) == Some(true);
                                let message = if !ok {
                                    // O modelo da frase já fecha com ponto.
                                    let reason = Some(text("erro").trim_end_matches('.').to_owned()).filter(|e| !e.is_empty()).unwrap_or_else(|| "?".into());
                                    let back = state.get("voltou").and_then(Value::as_bool) == Some(true);
                                    format!("{}{}", tr("update_failed").replace("{reason}", &reason), if back { format!(" {}", tr("update_rolled_back")) } else { String::new() })
                                } else if state.get("reiniciar_manual").and_then(Value::as_bool) == Some(true) {
                                    tr("update_done_manual")
                                } else {
                                    tr("update_done").replace("{version}", &parse_version(&value).version)
                                };
                                self.finish_update(ok, message, cx);
                                if ok { crate::update::confirm::show_pending(false, window, cx); }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    pub(super) fn set_language(&mut self, language: Language, window: &mut Window, cx: &mut Context<Self>) {
        let mut next = appearance::get();
        next.language = language;
        crate::i18n::set_language(language);
        self.apply_appearance(next, true, cx);
        // Textos guardados dentro dos campos; o resto é lido a cada desenho.
        for (input, key) in [(self.address.clone(), "server"), (self.token.clone(), "token"), (self.command_search.clone(), "commands_search")] {
            input.update(cx, |input, cx| input.set_placeholder(tr(key), window, cx));
        }
        for input in self.ask_form.inputs.clone() {
            match input {
                AnyInputState::Input(input) => input.update(cx, |input, cx| input.set_placeholder(tr("ask_placeholder"), window, cx)),
                AnyInputState::Textarea(input) => input.update(cx, |input, cx| input.set_placeholder(tr("ask_placeholder"), window, cx)),
                _ => unreachable!("a resposta é um campo simples ou multilinha"),
            }
        }
        if let Some(input) = self.device.channel_input.clone() {
            input.update(cx, |input, cx| input.set_placeholder(tr("settings_channel_branch_placeholder"), window, cx));
        }
        if let Some(icon) = &self.window_tray.icon { icon.refresh(); }
        self.relabel_settings(window, cx);
        self.rebuild_accounts();
        // O texto preparado das mensagens de aviso e o separador decimal das tabelas dependem do idioma.
        self.chat.invalidate();
        self.sync_rows(cx);
        self.list_state.remeasure();
        cx.refresh_windows();
    }

    pub(super) fn page_top(&self, title: &'static str, lead: String) -> Div {
        div().flex().flex_col()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr(title)))
            .child(div().mt(px(6.)).text_color(theme::muted()).child(lead))
    }

    pub(super) fn heading(&self, key: &'static str) -> Div {
        self.mark(div().mt(px(28.)).mb(px(10.)).rounded(px(6.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr(key)), key)
    }

    pub(super) fn render_general(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let a = appearance::get();
        const LANGUAGES: [Language; 3] = [Language::System, Language::Pt, Language::En];
        // Português e English com o próprio nome: quem caiu no idioma errado acha a saída sem ler o idioma da tela.
        let language = segments("language", &[tr("settings_language_system"), "Português".into(), "English".into()],
            LANGUAGES.iter().position(|l| *l == a.language).unwrap_or(0), 3, false, String::new(),
            |this: &mut Hangar, index, window: &mut Window, cx| this.set_language(LANGUAGES[index], window, cx), cx);
        let rate = self.device.rate_value();
        let rate_note = match (&self.api, &self.device.rate.value, self.device.rate.loading) {
            (None, ..) => tr("settings_currency_offline"),
            (_, _, true) => tr("settings_currency_loading"),
            (_, Some(Ok(Some(rate))), _) => tr("settings_currency_rate").replace("{rate}", &format!("{rate:.2}").replace('.', &tr("decimal"))),
            (_, Some(Ok(None)), _) => tr("settings_currency_none"),
            (_, Some(Err(error)), _) => tr("settings_currency_failed").replace("{reason}", error),
            (_, None, false) => tr("settings_currency_none"),
        };
        // Real sem cotação fica travado: a escolha mostrada é a que vale nos custos.
        let shown = if a.currency == Currency::Brl && rate.is_some() { 1 } else { 0 };
        let currency = segments("currency", &["US$".into(), "R$".into()], shown, if rate.is_some() { 2 } else { 1 }, false,
            tr("settings_currency_no_rate"),
            |this: &mut Hangar, index, _: &mut Window, cx| {
                let mut next = appearance::get();
                next.currency = if index == 1 { Currency::Brl } else { Currency::Usd };
                this.apply_appearance(next, true, cx);
            }, cx);
        let tray = crate::tray::SUPPORTED.then(|| {
            let note = match (&self.window_tray.error, &self.window_tray.icon) {
                (Some(reason), _) if a.keep_in_tray => tr("settings_tray_failed").replace("{reason}", reason),
                (_, Some(icon)) if a.keep_in_tray && !icon.online() => tr("settings_tray_no_host"),
                _ => tr("settings_tray_desc"),
            };
            let toggle = Switch::new("keep-in-tray").checked(a.keep_in_tray).accessibility_label(tr("settings_tray"))
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    let mut next = appearance::get();
                    next.keep_in_tray = *on;
                    this.apply_appearance(next, true, cx);
                }));
            settings_box().child(self.row(IconName::ArrowDownToLine, "settings_tray", Some(note), true, toggle.into_any_element()))
        });
        // Senhas do Chrome só se leem no Linux; nos outros sistemas a opção não existe.
        let autofill = cfg!(target_os = "linux").then(|| {
            let toggle = Switch::new("chrome-autofill").checked(a.chrome_autofill).accessibility_label(tr("settings_chrome_autofill"))
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    let mut next = appearance::get();
                    next.chrome_autofill = *on;
                    this.apply_appearance(next, true, cx);
                }));
            let full = Switch::new("full-chrome").checked(a.full_chrome).accessibility_label(tr("settings_full_chrome"))
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    let mut next = appearance::get();
                    next.full_chrome = *on;
                    this.apply_appearance(next, true, cx);
                }));
            settings_box().child(self.row(IconName::Key, "settings_chrome_autofill", Some(tr("settings_chrome_autofill_desc")), true, toggle.into_any_element()))
                .child(self.row(IconName::Globe, "settings_full_chrome", Some(tr("settings_full_chrome_desc")), true, full.into_any_element()))
        });
        let lead = tr(if crate::tray::SUPPORTED { "settings_general_lead_window" } else { "settings_general_lead" });
        div().flex().flex_col()
            .child(self.page_top("settings_page_general", lead))
            .child(self.heading("settings_general_group"))
            .child(settings_box()
                .child(self.row(IconName::Languages, "settings_language", Some(tr("settings_language_desc")), true, language))
                .child(self.row(IconName::Banknote, "settings_currency", Some(rate_note), true, currency)))
            .when_some(tray, |el, tray| el.child(self.heading("settings_window_group")).child(tray))
            .when_some(autofill, |el, autofill| el.child(self.heading("settings_browser_group")).child(autofill))
            .into_any_element()
    }

    pub(super) fn render_diary(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let diary = &self.device.diary;
        let keep = diary.ok().map(|d| d.keep).unwrap_or(7);
        let rule = |icon: IconName, text: String| div().mt(px(-1.)).border_t_1().border_color(theme::border()).px_4().py(px(12.))
            .flex().items_center().gap(px(12.)).child(chrome::small_icon(icon, 16., theme::muted())).child(div().flex_1().text_size(px(13.5)).child(text));
        let rules = settings_box()
            .child(rule(IconName::HardDrive, tr("settings_diary_rule_local")))
            .child(rule(IconName::EyeOff, tr("settings_diary_rule_private")))
            .child(rule(IconName::Clock, tr("settings_diary_rule_keep").replace("{days}", &keep.to_string())));

        let download = &self.device.download;
        let status = if self.api.is_none() { tr("settings_offline") }
            else if download.loading { tr(if self.device.saving { "settings_diary_downloading" } else { "settings_diary_choosing" }) }
            else if let Some(result) = &download.value { match result { Ok(saved) => saved.clone(), Err(error) => error.clone() } }
            else { match (&diary.value, diary.loading) {
                (Some(Ok(d)), _) if d.days > 0 => tr("settings_diary_has").replace("{days}", &d.days.to_string()).replace("{size}", &size_text(d.bytes)),
                (Some(Ok(d)), _) => tr(if d.missing { "diary_missing" } else { "settings_diary_empty" }),
                (_, true) => tr("settings_diary_loading"),
                // A falha já aparece na lista logo abaixo.
                (Some(Err(_)), _) | (None, false) => String::new(),
            } };
        let failed = matches!(download.value, Some(Err(_))) && !download.loading;
        let has_data = diary.ok().is_some_and(|d| d.days > 0);
        let button = Button::new("diary-download").primary().small().icon(IconName::Download).label(tr("settings_diary_download"))
            .disabled(!has_data || download.loading || self.api.is_none())
            .on_click(cx.listener(|this, _, _, cx| this.download_diary(cx)));
        let file_box = settings_box().child(self.row_with(IconName::FileText, "settings_diary_download",
            // A cor mora num filho: a linha pinta a própria descrição de cinza por cima.
            div().child(div().when(failed, |el| el.text_color(theme::danger())).child(status)), true, button.into_any_element()));

        let reload = Button::new("diary-reload").outline().small().icon(IconName::RefreshCw)
            .label(tr(if diary.loading { "settings_diary_reloading" } else { "settings_diary_reload" }))
            .disabled(diary.loading || self.api.is_none())
            .on_click(cx.listener(|this, _, _, cx| this.load_diary(cx)));
        let note = |text: String, color: Hsla| div().px_4().py(px(18.)).text_size(px(13.)).text_color(color).child(text);
        let list = match (&diary.value, diary.loading) {
            (_, _) if self.api.is_none() => note(tr("settings_offline"), theme::muted()).into_any_element(),
            (Some(Ok(d)), _) if !d.lines.is_empty() => div().id("diary-lines").max_h(px(340.)).overflow_y_scroll().track_scroll(&self.device.diary_scroll)
                .py(px(4.)).font_family(theme::MONO).text_size(px(12.))
                .children(d.lines.iter().enumerate().map(|(n, line)| div().mt(px(if n == 0 { 0. } else { -1. })).border_t_1().border_color(theme::border())
                    .px_3().py(px(5.)).flex().gap(px(10.)).items_start()
                    .child(div().flex_shrink_0().text_color(theme::muted()).child(line.when.clone()))
                    .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD)
                        .text_color(match line.level { Level::Warn => theme::warning(), Level::Error => theme::danger(), Level::Ok => theme::text() })
                        .child(line.event.clone()))
                    .child(div().flex_1().min_w_0().whitespace_normal().text_color(theme::muted()).child(line.context.clone()))))
                .into_any_element(),
            (Some(Ok(d)), _) => note(tr(if d.missing { "diary_missing" } else { "settings_diary_empty" }), theme::muted()).into_any_element(),
            (Some(Err(error)), false) => note(error.clone(), theme::danger()).into_any_element(),
            _ => note(tr("settings_diary_loading"), theme::muted()).into_any_element(),
        };
        let server = self.server_label(cx);
        div().flex().flex_col()
            .child(self.page_top("settings_page_diary", tr("settings_diary_lead").replace("{server}", &server)))
            .child(self.heading("settings_diary_rules")).child(rules)
            .child(self.heading("settings_diary_file")).child(file_box)
            .child(div().flex().items_end().child(div().flex_1().child(self.heading("settings_diary_recent"))).child(div().mb(px(6.)).child(reload)))
            .child(settings_box().child(list))
            .into_any_element()
    }

    pub(super) fn render_about(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (app_version, app_local) = legible(env!("HANGAR_NATIVE_VERSION"));
        let mono = |text: String| div().font_family(theme::MONO).text_size(px(12.5)).child(text);
        let with_local = |text: String, local: bool| div().flex().flex_col().gap(px(2.)).child(mono(text))
            .when(local, |el| el.child(div().child(tr("settings_about_local"))));
        let channel = |text: String| div().flex().items_center().gap(px(6.)).text_color(theme::warning())
            .child(chrome::small_icon(IconName::GitBranch, 13., theme::warning())).child(text);
        let updater = cx.try_global::<crate::update::Handle>().map(|handle| handle.0.clone());
        let app_channel = updater.as_ref().map(|u| u.read(cx).channel_lines()).unwrap_or_default();
        let (check, updating) = updater.as_ref().map(|u| (u.read(cx).app_check(), u.read(cx).is_busy())).unwrap_or((crate::update::AppCheck::Never, false));
        let (check_text, check_color) = match &check {
            crate::update::AppCheck::Never => (None, theme::muted()),
            crate::update::AppCheck::Checking => (Some(tr("settings_about_searching")), theme::muted()),
            crate::update::AppCheck::UpToDate => (Some(tr("settings_about_app_up_to_date")), theme::muted()),
            crate::update::AppCheck::Available(version) => (Some(tr("app_update_available").replace("{version}", version)), theme::text()),
            crate::update::AppCheck::Failed(reason) => (Some(tr("settings_about_failed").replace("{reason}", reason)), theme::danger()),
            // A procura respondeu, só não há app da branch: não é falha de conexão, e o app atual continua.
            crate::update::AppCheck::NoRelease(text) => (Some(text.clone()), theme::danger()),
        };
        let app_control = updater.map(|updater| match &check {
            crate::update::AppCheck::Available(_) => Button::new("app-update-start").primary().small().label(tr("app_update_now")).disabled(updating)
                .on_click(move |_, window, cx| updater.update(cx, |u, cx| u.start_update(window, cx))),
            _ => Button::new("app-update-search").outline().small().icon(IconName::RefreshCw)
                .label(tr(if matches!(check, crate::update::AppCheck::Checking) { "settings_about_searching_short" } else { "settings_about_search" }))
                .disabled(matches!(check, crate::update::AppCheck::Checking) || updating)
                .on_click(move |_, _, cx| updater.update(cx, |u, cx| u.check_app(cx))),
        }.into_any_element()).unwrap_or_else(|| div().into_any_element());
        let app_row = self.row_with(IconName::Monitor, "settings_about_app",
            with_local(format!("{} ({app_version}) · {}", env!("HANGAR_NATIVE_RELEASE"), tr("settings_about_built").replace("{date}", env!("HANGAR_NATIVE_BUILD_DATE"))), app_local)
                .children(app_channel.into_iter().map(channel))
                .when_some(check_text, |el, text| el.child(div().text_color(check_color).whitespace_normal().child(text))),
            true, app_control);

        let about = &self.device.about;
        let server_desc = match (&self.api, &about.value, about.loading) {
            (None, ..) => div().child(tr("settings_offline")),
            (_, Some(Ok(v)), _) => with_local(v.version.clone(), v.local).children(crate::update::test_channel(&v.channel).map(channel)),
            (_, _, true) => div().child(tr("settings_about_reading")),
            (_, Some(Err(error)), _) => div().child(div().text_color(theme::danger()).child(tr("settings_about_failed").replace("{reason}", error))),
            (_, None, false) => div(),
        };
        let server_row = self.row_with(IconName::Server, "settings_about_server", server_desc, self.api.is_some(), div().into_any_element());

        let run = self.device.run.clone();
        let active = self.device.run_active();
        let searching = self.device.searching;
        let (search_text, search_error) = match (&self.api, searching, &self.device.search) {
            (None, ..) => (tr("settings_offline"), false),
            (_, true, _) => (tr("settings_about_searching"), false),
            (_, _, Some(Ok(0))) => (tr("settings_about_up_to_date"), false),
            (_, _, Some(Ok(n))) => (tr("settings_about_available").replace("{n}", &n.to_string()), false),
            (_, _, Some(Err(error))) => (tr("settings_about_failed").replace("{reason}", error), true),
            (_, _, None) => (tr("settings_about_update_desc"), false),
        };
        let available = matches!(self.device.search, Some(Ok(n)) if n > 0) && !searching;
        let control = if available {
            Button::new("update-start").primary().small().label(tr("settings_about_update_now")).loading(self.device.notes_loading)
                .disabled(active || self.device.channel.blocks_update())
                .on_click(cx.listener(|this, _, window, cx| this.open_update_confirm(window, cx)))
        } else {
            Button::new("update-search").outline().small().icon(IconName::RefreshCw)
                .label(tr(if searching { "settings_about_searching_short" } else { "settings_about_search" }))
                .disabled(searching || active || self.api.is_none() || self.device.channel.blocks_update())
                .on_click(cx.listener(|this, _, _, cx| this.load_about(true, cx)))
        };
        let update_row = self.row_with(IconName::RefreshCw, "settings_about_update",
            div().child(div().when(search_error, |el| el.text_color(theme::danger())).child(search_text)), self.api.is_some(), control.into_any_element());

        // Progresso e desfecho logo abaixo da linha que disparou.
        let progress = run.map(|run| {
            let (text, value, color) = match &run {
                Run::Starting => (tr("update_starting"), None, theme::muted()),
                Run::Uncertain => (tr("update_uncertain"), None, theme::warning()),
                Run::Restarting => (tr("update_restarting"), None, theme::muted()),
                Run::Running { step, total, text } if *total > 0 => (tr("update_step").replace("{step}", &step.to_string())
                    .replace("{total}", &total.to_string()).replace("{text}", text), Some(*step as f32 * 100. / *total as f32), theme::muted()),
                Run::Running { .. } => (tr("update_running"), None, theme::muted()),
                Run::Done { ok: true, text } => (text.clone(), Some(100.), theme::success()),
                Run::Done { ok: false, text } => (text.clone(), None, theme::danger()),
            };
            let done = matches!(run, Run::Done { .. });
            div().mt(px(-1.)).border_t_1().border_color(theme::border()).px_4().py(px(14.)).flex().flex_col().gap(px(10.))
                .when(!done, |el| el.child(Progress::new("update-progress").accessibility_label(tr("settings_about_update"))
                    .map(|bar| match value { Some(v) => bar.value(v), None => bar.loading(true) })))
                .child(div().text_size(px(13.)).text_color(color).whitespace_normal().child(text))
        });
        let server = self.server_label(cx);
        div().flex().flex_col()
            .child(self.page_top("settings_page_about", tr("settings_about_lead").replace("{server}", &server)))
            .child(self.heading("settings_about_app_group")).child(settings_box().child(app_row))
            .child(self.heading("settings_about_server_group"))
            .child(settings_box().child(server_row).child(update_row).children(progress))
            .when(!self.active_invite() && self.api.is_some(), |el| el.child(div().mt(px(12.)).child(Button::new("migration-open").outline().small()
                .icon(IconName::Activity).label(tr_shared("migration_open", &[]))
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(Page::Migration, window, cx))))))
            .when(!self.active_invite() && !self.device.channel.unsupported, |el| el.child(self.heading("settings_channel_title")).child(self.render_channel(cx)))
            .into_any_element()
    }

    fn render_channel(&self, cx: &mut Context<Self>) -> AnyElement {
        let channel = &self.device.channel;
        let busy = channel.request.loading || self.device.run_active() || self.device.about.loading || self.device.searching || self.channel_updater_busy(cx);
        let unavailable = self.api.is_none() || channel.current.is_none();
        let (status, failed) = match (&channel.request.value, channel.request.loading) {
            _ if self.api.is_none() => (tr("settings_offline"), false),
            (_, true) => (tr("settings_channel_loading"), false),
            (Some(Err(error)), false) => (error.clone(), true),
            _ if channel.applied && !channel.is_dirty() => (tr("settings_channel_applied"), false),
            _ if channel.is_dirty() => (tr("settings_channel_pending"), false),
            _ => (String::new(), false),
        };
        let toggle = Switch::new("update-channel-toggle").checked(channel.enabled).accessibility_label(tr("settings_channel_title"))
            .disabled(unavailable || busy).on_click(cx.listener(|this, enabled: &bool, _, cx| {
                if this.device.channel.request.loading || this.device.run_active() || this.channel_updater_busy(cx) { return; }
                this.device.channel.enabled = *enabled;
                this.device.channel.applied = false;
                this.sync_channel_update_guard(cx);
                cx.notify();
            }));
        let controls = div().flex().items_center().gap_2()
            .child(Button::new("update-channel-apply").small().label(tr("settings_channel_apply"))
                .disabled(unavailable || busy || !channel.is_dirty() || (channel.enabled && channel.target().is_empty()))
                .on_click(cx.listener(|this, _, _, cx| this.apply_channel(cx))))
            .when(failed, |el| el.child(Button::new("update-channel-retry").outline().small().label(tr("settings_channel_reload"))
                .disabled(busy || self.api.is_none()).on_click(cx.listener(|this, _, _, cx| this.load_channel(cx)))))
            .when(channel.applied && !channel.blocks_update(), |el| el.child(Button::new("update-channel-update").primary().small()
                .loading(self.device.notes_loading)
                .label(tr("settings_about_update_now")).disabled(busy || self.api.is_none())
                .on_click(cx.listener(|this, _, window, cx| this.open_update_confirm(window, cx)))));
        let content = div().p_4().flex().flex_col().gap_3()
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("settings_channel_help")))
            .when_some(channel.current.as_ref(), |el, current| el.child(div().text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr("settings_channel_current").replace("{branch}", if current.branch.is_empty() { "main" } else { &current.branch })
                    .replace("{checkout}", &current.checkout_branch))))
            .child(div().text_sm().child(tr("settings_channel_branch")))
            .children(self.device.channel_input.as_ref().map(|input| Input::new(input).small()
                .aria_label(tr("settings_channel_branch")).disabled(unavailable || busy || !channel.enabled)))
            .child(controls)
            .when(!status.is_empty(), |el| el.child(div().id("update-channel-status").role(Role::Status).text_sm().whitespace_normal()
                .text_color(if failed { theme::danger() } else if channel.applied { theme::success() } else { theme::muted() }).child(status)));
        settings_box().child(self.row_with(IconName::GitBranch, "settings_channel_title", div(), !unavailable, toggle.into_any_element()))
            .child(content).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Currency, Level, diary_line, legible, money, pending};

    #[test]
    fn update_channel_remembers_last_branch_and_requires_apply() {
        let mut editor = super::ChannelEditor::default();
        let seq = editor.request.start();
        assert!(editor.receive(seq, false, Ok(super::Channel { branch: String::new(), checkout_branch: "main".into(), last_branch: "test/channel".into() })));
        assert_eq!(editor.draft, "test/channel");
        assert!(!editor.enabled);
        editor.enabled = true;
        assert_eq!(editor.target(), "test/channel");
        assert!(editor.is_dirty());
        let seq = editor.request.start();
        assert!(editor.receive(seq, true, Ok(super::Channel { branch: "test/channel".into(), checkout_branch: "main".into(), last_branch: "test/channel".into() })));
        assert!(editor.applied);
        assert!(!editor.is_dirty());
        editor.enabled = false;
        assert_eq!(editor.target(), "");
        assert!(editor.is_dirty());
        editor.enabled = true;
        assert_eq!(editor.draft, "test/channel");
    }

    #[test]
    fn update_channel_failure_keeps_draft_and_old_reply_is_ignored() {
        let mut editor = super::ChannelEditor::default();
        let old = editor.request.start();
        let seq = editor.request.start();
        assert!(editor.receive(seq, false, Ok(super::Channel { branch: String::new(), checkout_branch: "main".into(), last_branch: String::new() })));
        editor.enabled = true;
        editor.draft = "my/tests".into();
        assert!(!editor.receive(old, false, Ok(super::Channel { branch: "obsolete".into(), checkout_branch: "main".into(), last_branch: "obsolete".into() })));
        assert_eq!(editor.target(), "my/tests");
        let seq = editor.request.start();
        editor.receive(seq, true, Err("origin offline".into()));
        assert_eq!(editor.draft, "my/tests");
        assert!(editor.is_dirty());
        assert!(!editor.applied);
        assert!(!editor.request.loading);
        assert_eq!(editor.current.as_ref().unwrap().branch, "");
    }

    #[test]
    fn update_channel_enabled_empty_branch_blocks_update() {
        let mut editor = super::ChannelEditor::default();
        let seq = editor.request.start();
        editor.receive(seq, false, Ok(super::Channel { branch: String::new(), checkout_branch: "main".into(), last_branch: String::new() }));
        editor.enabled = true;
        assert!(editor.is_dirty());
        assert!(editor.blocks_update());
    }

    #[test]
    fn update_channel_refresh_after_update_clears_offer_and_preserves_draft() {
        let mut editor = super::ChannelEditor::default();
        let old = editor.request.start();
        editor.receive(old, true, Ok(super::Channel { branch: "test/channel".into(), checkout_branch: "main".into(), last_branch: "test/channel".into() }));
        editor.draft = "test/next".into();
        editor.after_update();
        assert!(!editor.applied);
        let seq = editor.request.start();
        editor.receive(seq, false, Ok(super::Channel { branch: "test/channel".into(), checkout_branch: "test/channel".into(), last_branch: "test/channel".into() }));
        assert_eq!(editor.current.as_ref().unwrap().checkout_branch, "test/channel");
        assert_eq!(editor.draft, "test/next");
        assert!(editor.is_dirty());
    }

    #[test]
    fn update_channel_read_error_does_not_block_applied_channel() {
        let mut editor = super::ChannelEditor::default();
        let seq = editor.request.start();
        editor.receive(seq, false, Ok(super::Channel { branch: "test/channel".into(), checkout_branch: "test/channel".into(), last_branch: "test/channel".into() }));
        let seq = editor.request.start();
        assert!(editor.blocks_update());
        editor.receive(seq, false, Err("network error".into()));
        assert!(!editor.is_dirty());
        assert!(!editor.blocks_update());
        editor.draft = "test/next".into();
        assert!(editor.blocks_update());
    }

    #[test]
    fn update_channel_missing_route_hides_only_for_current_reply() {
        let mut editor = super::ChannelEditor::default();
        let old = editor.request.start();
        let seq = editor.request.start();
        assert!(!editor.receive_unsupported(old, false, "get-404".into()));
        assert!(!editor.unsupported);
        assert!(editor.receive_unsupported(seq, false, "get-404".into()));
        assert!(editor.unsupported);
        assert!(!editor.blocks_update());
        assert!(editor.request.ok().is_some());
        let seq = editor.request.start();
        editor.receive(seq, false, Ok(super::Channel { branch: String::new(), checkout_branch: "main".into(), last_branch: String::new() }));
        assert!(!editor.unsupported);
    }

    #[test]
    fn update_channel_put_404_keeps_section_draft_and_error() {
        let mut editor = super::ChannelEditor::default();
        let seq = editor.request.start();
        editor.receive(seq, false, Ok(super::Channel { branch: String::new(), checkout_branch: "main".into(), last_branch: "test/channel".into() }));
        editor.enabled = true;
        let seq = editor.request.start();
        assert!(editor.receive_unsupported(seq, true, "save-404".into()));
        assert!(!editor.unsupported);
        assert_eq!(editor.current.as_ref().unwrap().branch, "");
        assert_eq!(editor.draft, "test/channel");
        assert_eq!(editor.request.value, Some(Err("save-404".into())));
        assert!(!editor.applied);
        assert!(editor.is_dirty());
    }

    #[test]
    fn update_channel_404_is_recorded_for_get_and_put() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("hangar-channel-log-{}-{stamp}", std::process::id()));
        let path = dir.join("native.log");
        super::record_channel_404(&path, false).unwrap();
        super::record_channel_404(&path, true).unwrap();
        let log = std::fs::read_to_string(&path).unwrap();
        assert!(log.contains("method=GET status=404"));
        assert!(log.contains("method=PUT status=404"));
        assert_eq!(log.lines().count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_channel_log_failure_reaches_feedback_channel() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let blocker = std::env::temp_dir().join(format!("hangar-log-blocker-{}-{stamp}", std::process::id()));
        std::fs::write(&blocker, "blocked").unwrap();
        let mut failures = 0;
        super::record_channel_404_or_report(&blocker.join("native.log"), false, |_| failures += 1);
        super::record_channel_404_or_report(&blocker.join("native.log"), true, |_| failures += 1);
        assert_eq!(failures, 2);
        std::fs::remove_file(blocker).unwrap();
    }

    #[test]
    fn money_converts_only_with_a_rate() {
        assert!(money(1.5, Currency::Usd, Some(5.)).starts_with("US$ 1"));
        assert!(money(1.5, Currency::Brl, None).starts_with("US$ 1"));
        assert!(money(1.5, Currency::Brl, Some(5.)).starts_with("R$ 7"));
    }

    #[test]
    fn diary_line_keeps_web_context_and_levels() {
        let line = diary_line(&serde_json::json!({"ts": "2026-09-24T14:37:12.5-03:00", "evento": "http", "nivel": "erro",
            "detalhe": "GET /api/x", "codigo": 500, "ms": 12}));
        assert_eq!((line.when.as_str(), line.event.as_str(), line.context.as_str()), ("24/09 14:37:12", "http", "GET /api/x · #500 · 12ms"));
        assert!(line.level == Level::Error);
        let ok = diary_line(&serde_json::json!({"ts": "x", "evento": "abriu", "codigo": 200}));
        assert_eq!((ok.when.as_str(), ok.context.as_str()), ("--/-- --:--:--", ""));
    }

    #[test]
    fn versions_read_like_the_web() {
        assert_eq!(legible("2026.09.14-ae8a7bf-dirty"), ("2026.09.14-ae8a7bf".into(), true));
        assert_eq!(legible("v0.1.2-130-g787a0cf8"), ("787a0cf8".into(), false));
        let v = serde_json::json!({"versoes": {"repo": "a", "backend": "a"}, "atras": 0, "atualizacao_disponivel": false});
        assert_eq!(pending(&v), 0);
        let stale = serde_json::json!({"versoes": {"repo": "b", "backend": "a"}, "atras": 0});
        assert_eq!(pending(&stale), 1);
    }
}
