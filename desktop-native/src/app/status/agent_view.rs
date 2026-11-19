//! Os quatro estados do "Consertar com agente" (mock aprovado): o botão no diagnóstico, o agente trabalhando, consertou e
//! não consertou. A execução vive na página, fora da queda: o servidor que volta fecha a queda, e o "Consertou" fica
//! até a pessoa voltar ao Status. Trocar de servidor, descartar a execução ou fechar o app encerra o agente.
use super::*;
use super::agent_fix::{self, Action, Choice, Progress, StepState};
use super::down::{Attempt, Fix, big_icon, down_model, spinner};
use super::model::fill;
use super::super::setup::agent::{self as setup_agent, Agent};
use hangar_local_server::{Host, LocalState, Os, STARTING_LIMIT, ServiceState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum RunPhase {
    Starting,
    Running,
    /// O agente saiu (ou foi parado): o app lê o servidor de novo.
    Checking,
    Fixed { version: Option<String> },
    /// `error`: o agente nem subiu.
    Failed { error: Option<String> },
}

pub(in crate::app) struct AgentRun {
    agent: Agent,
    id: u64,
    started: Instant,
    /// Quando o agente saiu: o relógio para aí.
    ended: Option<Duration>,
    progress: Progress,
    phase: RunPhase,
    /// O processo, também enquanto ele sobe; sai do `Handle` no `Exit`.
    handle: agent_fix::Handle,
    stop_requested: bool,
    stop_failed: bool,
    /// O teto passou com um Parar ainda em curso: se esse falhar, o teto tenta de novo.
    cap_pending: bool,
    steps_open: bool,
    diagnosis: String,
    tasks: Vec<Task<()>>,
    _quit: Option<Subscription>,
}

impl Drop for AgentRun {
    // Descartada com o agente vivo (troca de servidor, outra execução): ele não segue sozinho.
    fn drop(&mut self) {
        if let Some(owner) = agent_fix::abandon(&self.handle) { std::thread::spawn(move || { agent_fix::stop(&owner); }); }
    }
}

impl AgentRun {
    fn elapsed(&self) -> Duration { self.ended.unwrap_or_else(|| self.started.elapsed()) }
    fn active(&self) -> bool { matches!(self.phase, RunPhase::Starting | RunPhase::Running | RunPhase::Checking) }
    fn stoppable(&self) -> bool { can_stop(&self.phase, self.stop_requested, self.stop_failed) }
}

/// Parar vale enquanto o agente sobe ou roda; depois de um Parar que não alcançou o processo, o teto e um novo clique
/// tentam de novo.
fn can_stop(phase: &RunPhase, requested: bool, failed: bool) -> bool {
    matches!(phase, RunPhase::Starting | RunPhase::Running) && (!requested || failed)
}

fn short_name(agent: Agent) -> &'static str { match agent { Agent::Claude => "Claude", Agent::Codex => "Codex" } }

fn count_text(n: usize, one: &str, many: &str) -> String {
    if n == 1 { tr(one) } else { fill(tr(many), &[("n", &n.to_string())]) }
}

fn step_title(action: &Action) -> String {
    match action {
        Action::Command { description: Some(text), .. } => text.clone(),
        Action::Command { description: None, .. } => tr("status_agent_step_command"),
        Action::Read(path) => fill(tr("status_agent_step_read"), &[("path", path)]),
        Action::Edit(paths) => fill(tr("status_agent_step_edit"), &[("path", &paths.join(", "))]),
        Action::Search(pattern) => fill(tr("status_agent_step_search"), &[("pattern", pattern)]),
    }
}

/// O texto do "Copiar relatório", com a mesma limpeza do relatório do instalador: um passo que leu o `.env` traz o
/// `CP_AUTH_TOKEN`, e o texto vai para a área de transferência.
fn report(run: &AgentRun) -> String {
    use super::super::setup::{Secrets, scrub};
    scrub(&raw_report(run), &Secrets::here(Vec::new()))
}

/// O diagnóstico, cada passo com o comando e a saída, e o fim.
fn raw_report(run: &AgentRun) -> String {
    let mut out = vec![fill(tr("status_agent_report_title"), &[("agent", run.agent.name())]), String::new(), run.diagnosis.clone(), String::new(),
        tr("status_agent_report_steps")];
    for step in &run.progress.steps {
        let mark = match step.state { StepState::Done => "ok", StepState::Failed => "x", StepState::Running => "…" };
        out.push(format!("[{mark}] {}", step_title(&step.action)));
        if let Action::Command { command, .. } = &step.action { out.push(format!("$ {command}")); }
        out.extend(step.output.iter().map(|line| format!("  {line}")));
    }
    out.push(String::new());
    out.push(outcome_text(run));
    if !run.progress.raw.is_empty() { out.push(String::new()); out.extend(run.progress.raw.iter().cloned()); }
    out.join("\n")
}

/// O fim contado pelo agente, ou por que não há o que contar. Consertado, vale o resumo; sem conserto, o erro dele vem antes.
fn outcome_text(run: &AgentRun) -> String {
    let start_error = match &run.phase { RunPhase::Failed { error } => error.as_deref(), _ => None };
    // Um Parar que não alcançou o processo não explica a saída: vale o código dela.
    match run.progress.ending(start_error, run.stop_requested && !run.stop_failed) {
        agent_fix::Ending::StartFailed(error) => fill(tr("status_agent_start_failed"), &[("detail", &error)]),
        agent_fix::Ending::Stopped => tr("status_agent_stopped_by_you"),
        agent_fix::Ending::Error(error) => fill(tr("status_agent_error"), &[("detail", &error)]),
        agent_fix::Ending::Exit { ended, tail } => {
            let text = match ended {
                setup_agent::Ended::Code(code) => fill(tr("status_agent_exit"), &[("code", &code.to_string())]),
                setup_agent::Ended::Signal(signal) => fill(tr("status_agent_signal"), &[("signal", &signal.to_string())]),
                setup_agent::Ended::Unknown(detail) => fill(tr("status_agent_exit_unknown"), &[("detail", &detail)]),
            };
            if tail.is_empty() { text } else { format!("{text}\n{}", tail.join("\n")) }
        }
        agent_fix::Ending::Summary(summary) => summary,
        agent_fix::Ending::Silent => tr("status_agent_no_summary"),
    }
}

/// Fora da janela: lê o servidor desta máquina até sair do "subindo" (o agente pode ter acabado de pedir a subida). De pé,
/// a versão é a de quem respondeu, não a do `current`: trocado com a versão antiga de pé, seriam outras.
fn check(home: std::path::PathBuf) -> (LocalState, Option<String>) {
    let (server, _) = super::down::local_server(&home);
    let deadline = Instant::now() + STARTING_LIMIT;
    loop {
        let state = server.probe();
        if state == LocalState::Running { return (state, answered_version(&server)); }
        let rising = state == LocalState::Starting
            || matches!(state, LocalState::Stopped { .. }) && restarting(server.service().state, server.os);
        if !rising || Instant::now() >= deadline { return (state, None); }
        std::thread::sleep(super::down::STARTING_POLL);
    }
}

/// O gerenciador ainda vai religar: a espera do `systemd`/`launchd`, ou a tarefa do Windows de pé com o lançador
/// esperando para subir o `hangar-server` (a leitura crua diz `Running`). A leitura da tela chama isso de falha.
fn restarting(raw: ServiceState, os: Os) -> bool {
    raw == ServiceState::Restarting || (os == Os::Windows && raw == ServiceState::Running)
}

/// O `package_version` da saúde; ausente fora de pacote ou se a resposta mudou.
fn answered_version(server: &hangar_local_server::LocalServer) -> Option<String> {
    let reply = server.host.get(server.port, "/__hangar_server/health").filter(|reply| reply.status == 200)?;
    serde_json::from_str::<serde_json::Value>(&reply.body).ok()?.get("package_version")?.as_str().map(str::to_owned)
}

impl Hangar {
    pub(super) fn agent_choices(&self) -> &[Choice] { self.device.status.agents.as_deref().unwrap_or(&[]) }

    /// Uma vez por página: quais CLIs estão no PATH e com qual conta. Fora da janela.
    pub(super) fn agent_detect(&mut self, cx: &mut Context<Self>) {
        let page = &mut self.device.status;
        if page.agents.is_some() || page.detecting { return; }
        page.detecting = true;
        let (done, found) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || { let _ = done.send(agent_fix::detect()); });
        cx.spawn(async move |this, cx| {
            let found = found.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                let page = &mut this.device.status;
                (page.agents, page.detecting) = (Some(found), false);
                cx.notify();
            });
        }).detach();
    }

    fn agent_start(&mut self, agent: Agent, cx: &mut Context<Self>) {
        if self.device.status.fix.as_ref().is_some_and(AgentRun::active) { return; }
        let Some(home) = std::env::home_dir() else { return };
        let Some(probe) = self.local_probe().cloned() else { return };
        let os = Os::current();
        let Some(view) = down_model(&probe, os, Attempt::Done) else { return };
        let facts = agent_fix::Facts { title: &view.title, detail: view.detail.as_deref(), state: &probe.state, versions: &probe.versions,
            port: probe.port, port_issue: probe.port_issue.as_deref(), output: &view.output };
        let diagnosis = agent_fix::diagnosis(&facts);
        let prompt = agent_fix::prompt(&diagnosis, &home, probe.port, os, crate::i18n::english());
        let id = super::next_generation();
        let handle = agent_fix::Handle::default();
        // Antes de subir: fechar o app enquanto o agente ainda sobe também o encerra (quem sobe confere a marca).
        let on_quit = handle.clone();
        // O `taskkill` do Windows roda fora da janela: o fechar espera no máximo o prazo do GPUI para os
        // `on_app_quit`, e o `taskkill` já lançado segue sozinho se o app sair antes.
        let quit = cx.on_app_quit(move |_, _| {
            let (done, killed) = tokio::sync::oneshot::channel::<()>();
            match agent_fix::abandon(&on_quit) {
                Some(owner) => { std::thread::spawn(move || { agent_fix::kill_now(&owner); let _ = done.send(()); }); }
                None => drop(done),
            }
            async move { let _ = killed.await; }
        });
        // A anterior (já acabada) sai daqui: só uma execução por página.
        self.device.status.fix = Some(AgentRun { agent, id, started: Instant::now(), ended: None, progress: Progress::default(),
            phase: RunPhase::Starting, handle: handle.clone(), stop_requested: false, stop_failed: false, cap_pending: false,
            steps_open: false, diagnosis, tasks: Vec::new(), _quit: Some(quit) });
        let (done, started) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || { let _ = done.send(agent_fix::start(agent, prompt, &home, &handle)); });
        let reader = cx.spawn(async move |this, cx| {
            let started = started.await.unwrap_or_else(|e| Err(e.to_string()));
            let Ok(Some(output)) = this.update(cx, |this, cx| this.agent_started(id, started, cx)) else { return };
            while let Ok(item) = output.recv().await {
                let exited = matches!(item, setup_agent::Output::Exit(_));
                if this.update(cx, |this, cx| this.agent_output(id, item, cx)).is_err() || exited { break; }
            }
        });
        // O relógio do "Começou há": um redesenho por segundo enquanto o agente trabalha.
        let tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let Ok(active) = this.update(cx, |this, cx| {
                let active = this.device.status.fix.as_ref().is_some_and(|run| run.id == id && run.active());
                if active && this.settings == Some(Page::Status) { cx.notify(); }
                active
            }) else { return };
            if !active { return; }
        });
        // O mesmo teto do conserto do instalador: passou, para como o "Parar".
        let cap = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(setup_agent::MAX_RUN).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(run) = this.device.status.fix.as_mut().filter(|run| run.id == id && run.active()) { run.cap_pending = true; }
                this.agent_stop(id, cx);
            });
        });
        if let Some(run) = self.device.status.fix.as_mut() { run.tasks.extend([reader, tick, cap]); }
        cx.notify();
    }

    fn agent_started(&mut self, id: u64, started: Result<agent_fix::Started, String>, cx: &mut Context<Self>)
        -> Option<async_channel::Receiver<setup_agent::Output>> {
        // Descartada enquanto o agente subia: o `Drop` marcou o `Handle`, e quem subiu já o encerrou. Se a força
        // falhou, o agente segue vivo sem tela que o mostre: fica no log.
        let Some(run) = self.device.status.fix.as_mut().filter(|run| run.id == id) else {
            if let Ok(agent_fix::Started { stop_failed: true, .. }) = &started {
                crate::log_line("status: status_agent_stop_failed_discarded: o agente sobreviveu à parada de uma execução descartada");
            }
            return None;
        };
        match started {
            Ok(agent_fix::Started { output, stop_failed }) => {
                run.phase = RunPhase::Running;
                // O Parar da subida não derrubou o agente: o botão volta, e o teto que já passou tenta de novo.
                if stop_failed {
                    run.stop_failed = true;
                    if run.cap_pending { self.agent_stop(id, cx); }
                }
                cx.notify();
                Some(output)
            }
            // Parado (ou no teto) antes de o agente nascer: nada rodou, não há o que reler.
            Err(error) if error == agent_fix::ABANDONED => {
                (run.ended, run.phase) = (Some(run.started.elapsed()), RunPhase::Failed { error: None });
                cx.notify();
                None
            }
            Err(error) => {
                crate::log_line(&format!("status: o agente {} não subiu: {error}", run.agent.id()));
                (run.ended, run.phase) = (Some(run.started.elapsed()), RunPhase::Failed { error: Some(error) });
                cx.notify();
                None
            }
        }
    }

    fn agent_output(&mut self, id: u64, item: setup_agent::Output, cx: &mut Context<Self>) {
        let Some(run) = self.device.status.fix.as_mut().filter(|run| run.id == id) else { return };
        match item {
            setup_agent::Output::Line(line) => { let (agent, at) = (run.agent, run.started.elapsed()); run.progress.feed(agent, &line, at); }
            setup_agent::Output::Exit(ended) => {
                (run.ended, run.phase, run.progress.exit) = (Some(run.started.elapsed()), RunPhase::Checking, Some(ended));
                // O grupo dele já saiu junto (`agent::spawn`).
                agent_fix::exited(&run.handle);
                let Some(home) = std::env::home_dir() else {
                    run.phase = RunPhase::Failed { error: Some("HOME".to_owned()) };
                    return cx.notify();
                };
                let (done, checked) = tokio::sync::oneshot::channel();
                self.runtime.spawn_blocking(move || { let _ = done.send(check(home)); });
                let task = cx.spawn(async move |this, cx| {
                    let checked = checked.await.ok();
                    let _ = this.update(cx, |this, cx| this.agent_checked(id, checked, cx));
                });
                if let Some(run) = self.device.status.fix.as_mut() { run.tasks.push(task); }
            }
        }
        cx.notify();
    }

    fn agent_checked(&mut self, id: u64, checked: Option<(LocalState, Option<String>)>, cx: &mut Context<Self>) {
        let Some(run) = self.device.status.fix.as_mut().filter(|run| run.id == id) else { return };
        let fixed = matches!(&checked, Some((LocalState::Running, _)));
        run.phase = match checked {
            Some((LocalState::Running, version)) => RunPhase::Fixed { version },
            _ => RunPhase::Failed { error: None },
        };
        // A queda lê de novo: de pé, a página normal volta por baixo; parado, os consertos guiados valem para agora.
        if self.local_probe().is_some() { self.local_refresh(); } else if fixed { self.status_retry(cx); }
        cx.notify();
    }

    /// "Parar" e o teto: a saída pedida, a força depois e o grupo inteiro, fora da janela. O `Exit` que vem depois leva
    /// à releitura do servidor.
    fn agent_stop(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(run) = self.device.status.fix.as_mut().filter(|run| run.id == id && run.stoppable()) else { return };
        (run.stop_requested, run.stop_failed, run.cap_pending) = (true, false, false);
        // Ainda subindo sem processo: a marca basta, quem sobe o encerra e a execução acaba como parada.
        let process = if run.phase == RunPhase::Starting { agent_fix::stop_mark(&run.handle) } else { agent_fix::process(&run.handle) };
        let Some(owner) = process else { return cx.notify() };
        let (done, stopped) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || { let _ = done.send(agent_fix::stop(&owner)); });
        let task = cx.spawn(async move |this, cx| {
            // Sobreviveu à força: a tela diz isso e o Parar volta.
            if stopped.await.unwrap_or(false) { return; }
            let _ = this.update(cx, |this, cx| {
                let Some(run) = this.device.status.fix.as_mut().filter(|run| run.id == id) else { return };
                run.stop_failed = true;
                let retry = run.cap_pending;
                if retry { this.agent_stop(id, cx); }
                cx.notify();
            });
        });
        run.tasks.push(task);
        cx.notify();
    }

    /// "Ver Status" e os consertos guiados: a execução sai da tela.
    fn agent_dismiss(&mut self, cx: &mut Context<Self>) {
        self.device.status.fix = None;
        if self.device.status.down.is_none() { self.status_retry(cx); }
        cx.notify();
    }

    // -----------------------------------------------------------------------------------------
    // Desenho

    /// Estado 1: o botão, a escolha do agente com a conta e o "Tentar subir de novo" à direita.
    pub(super) fn agent_bar(&self, retry: Option<Button>, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let choices = self.agent_choices().to_vec();
        let selected = self.device.status.agent_pick.min(choices.len().saturating_sub(1));
        let choice = choices[selected].clone();
        let agent = choice.agent;
        let account = choice.account.clone().unwrap_or_else(|| tr("status_agent_account_default"));
        let picker = if choices.len() > 1 {
            let labels: Vec<String> = choices.iter().map(|c| short_name(c.agent).to_owned()).collect();
            super::super::settings::segments("status-agent-pick", &labels, selected, labels.len(), busy, String::new(),
                |this, n, _, cx| { this.device.status.agent_pick = n; cx.notify(); }, cx)
        } else {
            div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(short_name(agent)).into_any_element()
        };
        div().mt(px(12.)).flex().flex_col().gap(px(8.))
            .child(div().flex().flex_wrap().items_center().gap(px(10.))
                .child(Button::new("status-agent-fix").primary().small().icon(IconName::Sparkles).label(tr("status_agent_fix")).disabled(busy)
                    .accessibility_label(fill(tr("status_agent_fix_with"), &[("agent", agent.name()), ("account", &account)]))
                    .on_click(cx.listener(move |this, _, _, cx| this.agent_start(agent, cx))))
                .child(picker)
                .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(account))
                .children(retry.map(|button| div().ml_auto().child(button))))
            .child(div().text_size(px(12.)).text_color(theme::faint()).whitespace_normal().child(tr("status_agent_note")))
            .into_any_element()
    }

    /// Estados 2, 3 e 4, no lugar do diagnóstico.
    pub(super) fn render_agent(&self, cx: &mut Context<Self>) -> Option<Vec<AnyElement>> {
        let run = self.device.status.fix.as_ref()?;
        let elapsed = agent_fix::clock(run.elapsed());
        let commands = run.progress.commands();
        let name = run.agent.name();
        Some(match &run.phase {
            RunPhase::Starting | RunPhase::Running | RunPhase::Checking => {
                let checking = run.phase == RunPhase::Checking;
                let sub = if checking { tr("status_agent_checking") } else {
                    format!("{} · {}", fill(tr("status_agent_started_ago"), &[("time", &elapsed)]),
                        count_text(commands, "status_agent_commands_so_far_one", "status_agent_commands_so_far"))
                };
                let id = run.id;
                let stop = Button::new("status-agent-stop").danger().small().icon(IconName::Square).label(tr("status_agent_stop"))
                    .disabled(!run.stoppable())
                    .on_click(cx.listener(move |this, _, _, cx| this.agent_stop(id, cx)));
                let stop_failed = run.stop_failed.then(|| div().mt(px(8.)).text_size(px(12.5)).text_color(theme::danger()).whitespace_normal()
                    .child(tr("status_agent_stop_failed")));
                let title = fill(tr("status_agent_working_title"), &[("agent", name)]);
                let head = settings_box().flex_row().items_start().gap(px(16.)).p(px(16.))
                    .child(big_icon(spinner("status-agent-spin", 22., theme::warning(), title.clone()).into_any_element(), theme::warning()))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                        .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(title))
                        .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(sub))
                        .children(stop_failed))
                    .child(div().flex_shrink_0().child(stop));
                let mut out = vec![head.into_any_element()];
                if !run.progress.steps.is_empty() || run.progress.said.is_some() {
                    out.push(settings_box().px(px(16.)).py(px(12.))
                        .children((!run.progress.steps.is_empty()).then(|| steps_list(run)))
                        .children(run.progress.said.clone().map(|said| quote(said).mt(px(12.))))
                        .into_any_element());
                }
                out
            }
            RunPhase::Fixed { version } => {
                let sub = match version {
                    Some(version) => fill(tr("status_agent_fixed_sub"), &[("agent", name), ("time", &elapsed), ("version", version)]),
                    None => fill(tr("status_agent_fixed_sub_plain"), &[("agent", name), ("time", &elapsed)]),
                };
                let items = run.progress.summary.as_deref().map(agent_fix::summary_items).unwrap_or_default();
                let list = (!items.is_empty()).then(|| div().id("status-agent-summary").role(Role::List).mt(px(10.)).flex().flex_col().gap(px(5.))
                    .text_size(px(13.)).children(items.into_iter().enumerate().map(|(i, item)| div().id(SharedString::from(format!("status-agent-summary-{i}")))
                        .role(Role::ListItem).flex().items_start().gap(px(8.))
                        .child(div().mt(px(7.)).size(px(5.)).flex_shrink_0().rounded_full().bg(theme::success()))
                        .child(div().flex_1().min_w_0().whitespace_normal().child(item)))));
                let head = settings_box().flex_row().items_start().gap(px(16.)).p(px(16.))
                    .child(big_icon(chrome::small_icon(IconName::CircleCheck, 24., theme::success()).into_any_element(), theme::success()))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                        .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(tr("status_agent_fixed_title")))
                        .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(sub))
                        .children(list)
                        .child(div().mt(px(12.)).child(Button::new("status-agent-done").primary().small().label(tr("status_agent_see_status"))
                            .on_click(cx.listener(|this, _, _, cx| this.agent_dismiss(cx))))));
                let mut out = vec![head.into_any_element()];
                out.extend(self.all_steps(run, cx));
                out
            }
            RunPhase::Failed { .. } => {
                let sub = format!("{} · {}", fill(tr("status_agent_failed_after"), &[("time", &elapsed)]),
                    count_text(commands, "status_agent_commands_one", "status_agent_commands"));
                let title = fill(tr("status_agent_failed_title"), &[("agent", name)]);
                let head = settings_box().flex_row().items_start().gap(px(16.)).p(px(16.)).border_color(theme::danger().opacity(0.45))
                    .child(big_icon(chrome::small_icon(IconName::TriangleAlert, 24., theme::danger()).into_any_element(), theme::danger()))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                        .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(title))
                        .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(sub))
                        .child(quote(outcome_text(run)).mt(px(10.)))
                        .child(self.failed_actions(run, cx)));
                let mut out = vec![head.into_any_element()];
                out.extend(self.all_steps(run, cx));
                out
            }
        })
    }

    /// Os consertos guiados que valem para a leitura de agora, o outro agente e o relatório.
    fn failed_actions(&self, run: &AgentRun, cx: &mut Context<Self>) -> AnyElement {
        let fixes = self.local_probe().and_then(|probe| down_model(probe, Os::current(), Attempt::Done)).map(|view| view.fixes).unwrap_or_default();
        let other = self.agent_choices().iter().map(|c| c.agent).find(|agent| *agent != run.agent);
        let text = report(run);
        div().mt(px(12.)).flex().flex_wrap().items_center().gap(px(8.))
            .children(fixes.into_iter().enumerate().map(|(i, row)| {
                let (fix, ok) = (row.fix.clone(), row.button.clone());
                let button = Button::new(SharedString::from(format!("status-agent-fix-{i}"))).small().label(row.title.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.device.status.fix = None;
                        this.local_fix(fix.clone(), ok.clone(), window, cx);
                        cx.notify();
                    }));
                // Encerrar processo nunca é o botão de destaque.
                if row.primary && !matches!(row.fix, Fix::FreePort(_)) { button.primary() } else { button.outline() }
            }))
            .children(other.map(|agent| Button::new("status-agent-other").outline().small().icon(IconName::Sparkles)
                .label(fill(tr("status_agent_try_other"), &[("agent", short_name(agent))]))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(n) = this.agent_choices().iter().position(|c| c.agent == agent) { this.device.status.agent_pick = n; }
                    this.agent_start(agent, cx);
                }))))
            .child(Button::new("status-agent-report").ghost().small().icon(IconName::Copy).label(tr("status_agent_copy_report"))
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))))
            // Sem conserto guiado nem outro agente, é a saída da falha: o diagnóstico volta, com o botão do agente.
            .child(Button::new("status-agent-back").ghost().small().label(tr("status_agent_see_status"))
                .on_click(cx.listener(|this, _, _, cx| this.agent_dismiss(cx))))
            .into_any_element()
    }

    /// "Todos os passos (N)", fechado até a pessoa abrir.
    fn all_steps(&self, run: &AgentRun, cx: &mut Context<Self>) -> Option<AnyElement> {
        if run.progress.steps.is_empty() { return None; }
        let open = run.steps_open;
        let toggle = Button::new("status-agent-steps").ghost().small().icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
            .label(fill(tr("status_agent_all_steps"), &[("n", &run.progress.steps.len().to_string())]))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(run) = this.device.status.fix.as_mut() { run.steps_open = !run.steps_open; }
                cx.notify();
            }));
        Some(settings_box().px(px(12.)).py(px(8.)).child(div().child(toggle))
            .when(open, |el| el.child(div().px(px(4.)).pt(px(6.)).child(steps_list(run))))
            .into_any_element())
    }
}

fn quote(text: String) -> Div {
    div().pl(px(12.)).border_l_2().border_color(theme::accent()).text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(text)
}

fn steps_list(run: &AgentRun) -> AnyElement {
    let now = run.elapsed();
    div().id("status-agent-steps-list").role(Role::List).flex().flex_col()
        .children(run.progress.steps.iter().enumerate().map(|(i, step)| {
            let title = step_title(&step.action);
            let dot = match step.state {
                StepState::Done => chrome::small_icon(IconName::Check, 13., theme::success()).into_any_element(),
                StepState::Failed => chrome::small_icon(IconName::Close, 13., theme::danger()).into_any_element(),
                StepState::Running => spinner(&format!("status-agent-step-{i}-spin"), 13., theme::muted(), tr("status_down_step_running")).into_any_element(),
            };
            let took = agent_fix::clock(step.took.unwrap_or_else(|| now.saturating_sub(step.at)));
            let mut code: Vec<String> = match &step.action {
                Action::Command { command, .. } => command.lines().take(4).enumerate()
                    .map(|(n, line)| if n == 0 { format!("$ {line}") } else { format!("  {line}") }).collect(),
                _ => Vec::new(),
            };
            code.extend(step.output.iter().cloned());
            let state = match step.state { StepState::Done => tr("status_agent_step_done"), StepState::Failed => tr("status_agent_step_failed"),
                StepState::Running => tr("status_down_step_running") };
            div().id(SharedString::from(format!("status-agent-step-{i}"))).role(Role::ListItem).aria_label(format!("{title}, {state}, {took}"))
                .flex().items_start().gap(px(10.)).py(px(8.)).when(i > 0, |el| el.border_t_1().border_color(theme::border()))
                .child(div().pt(px(2.)).size(px(13.)).flex_shrink_0().child(dot))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(6.))
                    .child(div().text_size(px(12.5)).whitespace_normal().child(title))
                    .children((!code.is_empty()).then(|| div().px(px(10.)).py(px(8.)).rounded(px(8.)).bg(theme::inset()).font_family(theme::MONO)
                        .text_size(px(11.5)).line_height(px(17.)).text_color(theme::muted()).whitespace_normal()
                        .children(code.into_iter().map(|line| div().child(line))))))
                .child(div().flex_shrink_0().text_size(px(11.5)).text_color(theme::faint()).child(took))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn stop_retries_after_a_failed_stop_while_the_agent_lives() {
        assert!(can_stop(&RunPhase::Starting, false, false) && can_stop(&RunPhase::Running, false, false));
        // Pedido em curso: o botão espera o resultado.
        assert!(!can_stop(&RunPhase::Running, true, false));
        // O Parar não alcançou o processo: o teto e um novo clique tentam de novo.
        assert!(can_stop(&RunPhase::Running, true, true) && can_stop(&RunPhase::Starting, true, true));
        assert!(!can_stop(&RunPhase::Checking, true, true) && !can_stop(&RunPhase::Failed { error: None }, false, false));
    }

    fn finished_run() -> AgentRun {
        AgentRun { agent: Agent::Claude, id: 1, started: Instant::now(), ended: Some(Duration::from_secs(3)), progress: Progress::default(),
            phase: RunPhase::Failed { error: None }, handle: agent_fix::Handle::default(), stop_requested: false, stop_failed: false,
            cap_pending: false, steps_open: false, diagnosis: "Failure code: launch_failed".into(), tasks: Vec::new(), _quit: None }
    }

    #[test]
    fn copied_report_hides_the_server_token() {
        let mut run = finished_run();
        run.progress.feed(Agent::Claude, r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cat ~/.hangar/config/.env"}}]}}"#, Duration::ZERO);
        run.progress.feed(Agent::Claude, r#"{"type":"user","message":{"content":[{"tool_use_id":"t1","type":"tool_result","content":"CP_PORT=8765\nCP_AUTH_TOKEN=s3cr3t-fixture-token"}]}}"#, Duration::ZERO);
        run.progress.feed(Agent::Claude, "curl -H 'Authorization: Bearer outro-fixture-token' falhou", Duration::ZERO);
        let text = report(&run);
        assert!(!text.contains("s3cr3t-fixture-token") && !text.contains("outro-fixture-token"), "{text}");
        assert!(text.contains("CP_PORT=8765") && text.contains("launch_failed"), "{text}");
    }

    #[test]
    fn a_stop_that_missed_the_agent_does_not_explain_its_exit() {
        crate::i18n::english_in_this_thread(false);
        let mut run = finished_run();
        run.progress.exit = Some(setup_agent::Ended::Code(2));
        run.stop_requested = true;
        assert_eq!(outcome_text(&run), tr("status_agent_stopped_by_you"));
        run.stop_failed = true;
        assert_eq!(outcome_text(&run), fill(tr("status_agent_exit"), &[("code", "2")]));
    }

    #[test]
    fn the_check_waits_while_the_manager_will_restart_the_server() {
        assert!(restarting(ServiceState::Restarting, Os::Linux) && restarting(ServiceState::Restarting, Os::Macos));
        // A tarefa do Windows segue `Running` com o lançador esperando para religar.
        assert!(restarting(ServiceState::Running, Os::Windows));
        // No Linux, `Running` com a porta muda é a subida presa: não espera.
        assert!(!restarting(ServiceState::Running, Os::Linux));
        assert!(!restarting(ServiceState::Failed, Os::Windows) && !restarting(ServiceState::Stopped, Os::Linux));
    }
}
