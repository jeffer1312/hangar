//! Página Status: o retrato do servidor selecionado (`GET /api/status/events`, só do dono), ao vivo
//! enquanto a página está aberta e a janela à vista. Fechar a página, esconder a janela na bandeja ou
//! trocar de servidor encerra a conexão; voltar à vista abre outra.
mod agent_fix;
mod agent_view;
mod down;
mod model;
mod view;

use hangar_api::status::{AttentionTarget, StatusSnapshot};

use super::*;
use super::device::{DeviceReply, Remote};
use super::settings::Page;

/// O que a conexão manda à página.
pub(super) enum Feed {
    Snapshot(Box<StatusSnapshot>),
    Ping,
    /// A conexão caiu depois de aberta; a tarefa tenta de novo sozinha.
    Lost,
    /// Não abriu (ou mandou um retrato que este app não lê). Recusa que não muda sozinha (rota ausente, sem
    /// permissão) encerra a tarefa; o resto ela tenta de novo.
    Failed(Failure),
    /// Do servidor desta máquina parado (`down.rs`), com a geração da leitura dele: não depende da conexão do retrato.
    Local(u64, down::LocalReply),
}

#[derive(Default)]
pub(super) struct StatusPage {
    view: Remote<Vec<model::Block>>,
    /// O que "Copiar detalhe" leva quando a leitura falhou.
    detail: Option<String>,
    /// O retrato na tela veio da conexão aberta agora.
    live: bool,
    by_process: bool,
    /// Bloco que acabou de receber o pulo de um ponto de atenção.
    flash: Option<AttentionTarget>,
    flash_task: Option<Task<()>>,
    generation: u64,
    stream: Option<JoinHandle<()>>,
    /// A última notícia da conexão aberta foi a queda ou a recusa: o servidor de outra máquina não responde.
    stream_failed: bool,
    /// O servidor selecionado fora do ar; com ele nesta máquina, a leitura do serviço e as ações.
    down: Option<down::Down>,
    /// Os agentes no PATH e a conta de cada um; `None` antes da primeira leitura.
    agents: Option<Vec<agent_fix::Choice>>,
    detecting: bool,
    agent_pick: usize,
    /// O "Consertar com agente" em curso ou acabado; sobrevive à volta do servidor até "Ver Status".
    fix: Option<agent_view::AgentRun>,
}

impl Drop for StatusPage {
    fn drop(&mut self) { if let Some(task) = self.stream.take() { task.abort(); } }
}

impl Hangar {
    /// Página aberta pela navegação ou pela busca: começa do carregando, sem o retrato de uma visita anterior.
    pub(super) fn status_opened(&mut self, _cx: &mut Context<Self>) {
        self.status_left();
        self.device.status.view.reset();
        self.device.status.detail = None;
        self.status_connect();
    }

    /// Fora da página (outra página, fechada ou janela na bandeja): a conexão fecha e o que chegar dela é descartado.
    pub(super) fn status_left(&mut self) {
        let page = &mut self.device.status;
        page.generation = next_generation();
        if let Some(task) = page.stream.take() { task.abort(); }
        (page.live, page.stream_failed) = (false, false);
    }

    /// Abre a conexão. O retrato que já está na tela fica até o próximo chegar.
    fn status_connect(&mut self) {
        let Some(api) = self.api.clone() else { return };
        // O convite leva 403 na rota; escondida na bandeja, ninguém lê a tela.
        if self.active_invite() || self.window_tray.hidden { return; }
        let page = &mut self.device.status;
        if let Some(task) = page.stream.take() { task.abort(); }
        page.generation = next_generation();
        page.stream_failed = false;
        if page.view.value.is_none() { page.view.loading = true; }
        let generation = page.generation;
        let done = self.device_send_later();
        self.device.status.stream = Some(self.runtime.spawn(stream(api, generation, done)));
    }

    /// No desenho: a janela voltou da bandeja ou a conexão trocou de servidor. Conexão que parou por recusa fica parada.
    fn status_ensure(&mut self) {
        if self.device.status.stream.is_none() { self.status_connect(); }
    }

    fn status_retry(&mut self, cx: &mut Context<Self>) {
        self.status_left();
        self.device.status.view.reset();
        self.device.status.detail = None;
        self.status_connect();
        cx.notify();
    }

    /// Clique num ponto de atenção: rola até o bloco e o contorna por um instante.
    fn status_jump(&mut self, target: AttentionTarget, cx: &mut Context<Self>) {
        self.jump_to(target_key(target));
        self.device.status.flash = Some(target);
        self.device.status.flash_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1200)).await;
            let _ = this.update(cx, |this, cx| { this.device.status.flash = None; cx.notify(); });
        }));
        cx.notify();
    }

    pub(super) fn receive_status(&mut self, generation: u64, feed: Feed, cx: &mut Context<Self>) {
        // O ponto da navegação também lê o servidor parado: vale com a página fechada.
        let feed = match feed {
            Feed::Local(generation, reply) => return self.receive_local(generation, reply, cx),
            feed => feed,
        };
        if generation != self.device.status.generation { return; }
        // Fechada ou na bandeja: a conexão só volta com a página à vista (o `ping` de 10 s traz a notícia).
        if self.settings != Some(Page::Status) || self.window_tray.hidden { self.status_left(); return; }
        match feed {
            Feed::Snapshot(snapshot) => {
                let blocks = model::view_model(&snapshot);
                let page = &mut self.device.status;
                page.view.set(Ok(blocks));
                (page.detail, page.live, page.stream_failed) = (None, true, false);
            }
            // Só prova que a conexão vive: nada a redesenhar, salvo o painel de outra máquina parada.
            // A leitura local já saiu acima.
            Feed::Ping => {
                if !std::mem::take(&mut self.device.status.stream_failed) { return; }
            }
            Feed::Local(..) => return,
            Feed::Lost => {
                // Caiu antes do primeiro retrato: sem o erro, a página ficaria no esqueleto a cada reconexão.
                let failure = self.device.status.view.ok().is_none().then(|| failure_text(&self.server_label(cx), &Failure::local("stream_lost")));
                let page = &mut self.device.status;
                (page.live, page.stream_failed) = (false, true);
                if let Some((text, detail)) = failure {
                    page.view.set(Err(text));
                    page.detail = Some(detail);
                }
            }
            Feed::Failed(error) => {
                let (text, detail) = failure_text(&self.server_label(cx), &error);
                let page = &mut self.device.status;
                // Só falha de rede (ou o proxy dizendo que não alcança) prova o servidor fora: um 404 ou 403 é
                // resposta dele, e o painel de parado esconderia a rota ausente. Retrato ilegível também é resposta.
                let unreachable = error.detail != "invalid_response" && error.status.is_none_or(|code| matches!(code, 502..=504));
                (page.live, page.stream_failed) = (false, unreachable);
                // Com um retrato na tela, a falha só apaga o "ao vivo": o servidor parado aparece no ponto da navegação.
                if page.view.ok().is_none() {
                    page.view.set(Err(text));
                    page.detail = Some(detail);
                }
            }
        }
        cx.notify();
    }

    /// O ponto na linha "Status" da navegação: só o estado do servidor selecionado. Vermelho quando ele não responde,
    /// âmbar subindo, nada de pé; nesta máquina quem diz é a leitura do serviço.
    pub(super) fn status_nav_dot(&mut self) -> Option<(Hsla, String)> {
        self.status_down_sync();
        self.device.status.down.as_ref()?;
        self.local_dot().unwrap_or_else(|| Some((theme::danger(), tr("status_nav_down"))))
    }
}

/// Geração nova da conexão do retrato ou da leitura local. Um contador só, que nunca volta: a troca de servidor recria
/// a página, e um número que recomeçasse do zero deixaria passar evento da conexão anterior.
fn next_generation() -> u64 {
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// A linha marcada de cada bloco: o pulo da lista de atenção e a busca das Configurações rolam até ela.
fn target_key(target: AttentionTarget) -> &'static str {
    match target {
        AttentionTarget::Server => "status_memory",
        AttentionTarget::Exits => "status_days_title",
        AttentionTarget::Machines => "status_machines_title",
        AttentionTarget::Sessions => "status_sessions_title",
        AttentionTarget::Slow => "status_slow_title",
        AttentionTarget::Host => "status_host_title",
        AttentionTarget::Outside => "status_outside_title",
    }
}

/// A frase da falha e o detalhe técnico que vai para a área de transferência.
fn failure_text(server: &str, error: &Failure) -> (String, String) {
    let text = match error.status {
        Some(code) => tr("status_error_http").replace("{server}", server).replace("{code}", &code.to_string()),
        None if error.detail == "invalid_response" => tr("status_error_invalid").replace("{server}", server),
        None => tr("status_error_network").replace("{server}", server),
    };
    let code = error.status.map_or_else(|| "—".to_owned(), |code| code.to_string());
    (text, format!("GET /api/status/events · {code} · {}", error.detail))
}

/// Lê os eventos até a conexão cair e reabre com espera crescente; recusa que não muda sozinha encerra.
async fn stream<F>(api: Api, generation: u64, done: F)
where F: Fn(DeviceReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static {
    use eventsource_stream::Eventsource;
    use futures::StreamExt;
    let mut delay = 1u64;
    loop {
        api::route::ensure(&api).await;
        match api.server_events(&["status", "events"]).await {
            Ok(response) => {
                // O servidor manda `ping` a cada 10 s: sem bytes por 25 s, a conexão está morta.
                let bytes = futures::stream::unfold(Box::pin(response.bytes_stream()), |mut stream| async move {
                    match tokio::time::timeout(Duration::from_secs(25), stream.next()).await {
                        Ok(Some(chunk)) => Some((chunk, stream)),
                        _ => None,
                    }
                });
                let mut events = Box::pin(bytes.eventsource());
                while let Some(Ok(item)) = events.next().await {
                    let feed = match item.event.as_str() {
                        "status" => match serde_json::from_str::<StatusSnapshot>(&item.data) {
                            Ok(snapshot) => Feed::Snapshot(Box::new(snapshot)),
                            Err(_) => Feed::Failed(Failure::local("invalid_response")),
                        },
                        "ping" => Feed::Ping,
                        _ => continue,
                    };
                    // A espera curta volta só com um evento válido: um 200 que fecha na hora reconectaria a cada segundo.
                    if !matches!(feed, Feed::Failed(_)) { delay = 1; }
                    done(DeviceReply::Status(generation, feed)).await;
                }
                api::route::forget(&api);
                done(DeviceReply::Status(generation, Feed::Lost)).await;
            }
            Err(error) => {
                let lasting = matches!(error.status, Some(code) if (400..500).contains(&code) && code != 408 && code != 429);
                done(DeviceReply::Status(generation, Feed::Failed(error))).await;
                if lasting { return; }
            }
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}
