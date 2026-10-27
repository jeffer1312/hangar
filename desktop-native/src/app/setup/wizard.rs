//! A entidade do assistente: escolhas, conferência prévia, as duas execuções do script, a senha de administrador e a conexão
//! no fim. Ocupa a janela inteira enquanto existe; o render mora em `screens.rs`.
use super::*;
use super::askpass::{self, Vault};
use super::agent::{self, Agent};
use super::codes::{self, Fix};
use super::failure::{AgentPhase, Failure, FailureAction, Outbox};
use super::repo;
use super::report::{self, Outcome, Payload};
use super::flow::{self, PasswordMode, PasswordProblem, PhoneOutcome, Primary, Runs, Screen, View};
use super::local::{self, LocalInstall};
use super::marks::{End, Progress};
use super::phone::{self, Qr};
use super::precheck::{self, Check, CheckRow};
use super::run::{self, Kind, Options, RunRecord, SetupState};
use super::system;
use super::tail::Tail;
use crate::single_instance::AskpassRequest;
use std::path::Path;

const POLL_EVERY: Duration = Duration::from_millis(400);
/// Voltas sem saída nova antes de perguntar se o processo ainda vive (~5 s).
const QUIET_CHECKS: u32 = 12;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AppCopy { Done(PathBuf), Skipped, Failed(String) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterPassword { InstallGit, RefreshPackages }

/// `Quiet` leva o pid e a hora de início: o processo só vale se for o mesmo que o app iniciou.
enum Watch { Grew, Quiet(u32, String), Stop }

pub(crate) struct SetupWizard {
    hangar: WeakEntity<Hangar>,
    runtime: tokio::runtime::Handle,
    /// Lido ao abrir: depois de uma troca o Linux responde "<caminho> (deleted)" (`update.rs`).
    exe: Option<PathBuf>,
    mode: local::Mode,
    /// No pacote, `~/.hangar`: é a pasta que os textos com `{pasta}` mostram.
    pub(super) dest: PathBuf,
    located: bool,
    pub(super) agents: Vec<&'static str>,
    agents_touched: bool,
    pub(super) installed: Vec<&'static str>,
    pub(super) outside: bool,
    pub(super) password_mode: PasswordMode,
    pub(super) password: Entity<InputState>,
    pub(super) confirm: Entity<InputState>,
    /// `None` enquanto confere.
    pub(super) precheck: Option<Vec<CheckRow>>,
    pkg: Option<&'static str>,
    pub(super) started: bool,
    /// O que o app faz antes das marcas chegarem (instalar o git, baixar o bootstrap).
    pub(super) preparing: Option<String>,
    git_ready: bool,
    bootstrap: Option<PathBuf>,
    /// A senha do celular desta execução, só em memória.
    token: Option<String>,
    pub(super) runs: Runs,
    check_tail: Option<Tail>,
    install_tail: Option<Tail>,
    records: (Option<RunRecord>, Option<RunRecord>),
    polling: bool,
    pub(super) finished: bool,
    pub(super) failure: Option<Failure>,
    /// O relatório da falha na tela, já limpo; `None` enquanto monta.
    pub(super) report: Option<String>,
    /// A falha de que o relatório fala enquanto ele ainda não foi entregue ao envio; `None` depois (a caixa trava).
    pub(super) report_about: Option<Failure>,
    /// Um relatório que fica pronto depois de outra falha não substitui o da tela.
    report_gen: u64,
    pub(super) send: bool,
    /// Os relatórios entregues e o estado de cada envio, mostrados em qualquer etapa.
    pub(super) outbox: Outbox,
    /// Entregues antes de ficarem prontos: (geração, resultado, agente); saem quando a montagem termina.
    send_requested: Vec<(u64, Outcome, Option<Agent>)>,
    /// "Atualizar e tentar de novo" rodando: o painel ignora os outros botões.
    pub(super) refreshing: bool,
    /// Instalados e logados, detectados a cada falha.
    pub(super) agents_ready: Vec<Agent>,
    pub(super) agent: Option<AgentRun>,
    /// Numera as execuções do agente: o teto de 20 min de uma nunca pára outra.
    agent_runs: u64,
    /// O relatório como estava quando o agente foi chamado; o final é ele mais a seção do conserto.
    report_base: Option<String>,
    /// Os segredos lidos antes de o cofre esquecer (senha de administrador, código do askpass, senha do celular): o
    /// relatório do conserto é montado depois do `forget` e tem de limpá-los também.
    report_secrets: Vec<String>,
    /// O envio do relatório que leva a correção (reconferência falhou com mudança na pasta), pelo id da fila.
    pub(super) fix_sent: Option<u64>,
    /// O que a abertura desfez de um conserto que o app interrompeu ao cair; fica na tela até a pessoa fechar.
    pub(super) recovered: Option<Recovery>,
    /// A recuperação ainda roda: um agente novo agora gravaria a anotação que ela apaga no fim.
    recovering: bool,
    /// "Consertar agora" da roda do mouse confirmado: a próxima instalação leva `-ConsertarRoda`.
    fix_mouse_wheel: bool,
    vault: Vault,
    waiting: Vec<AskpassRequest>,
    prompt: Option<Entity<PasswordPrompt>>,
    after_password: Option<AfterPassword>,
    opened_link: Option<String>,
    pub(super) app_copy: Option<AppCopy>,
    /// Endereço conectado no fim, ou o motivo de não ter conectado.
    pub(super) connection: Option<Result<String, String>>,
    /// Acabou escondido com o app ligado a outro servidor: a conexão local espera o "Abrir Hangar".
    connect_later: bool,
    /// Reaberto com o FIM já no arquivo: o fim aconteceu com o app fechado.
    resumed_ended: bool,
    pub(super) viewing: Screen,
    pub(super) details_open: bool,
    pub(super) phone: Option<PhoneOutcome>,
    pub(super) qr: Qr,
    /// A conta Tailscale já respondeu `Running` ao `tailscale status`.
    pub(super) tailscale_running: bool,
    tailscale_checking: bool,
    ticks: u32,
    pub(super) focus: FocusHandle,
    /// Para fechar a janela de senha de fora de um update com a janela emprestada (`fail`/`finish`).
    window: AnyWindowHandle,
    _subscriptions: Vec<Subscription>,
}

impl Drop for SetupWizard {
    fn drop(&mut self) {
        crate::single_instance::serve_askpass(None);
        for request in self.waiting.drain(..) { let _ = request.reply.send(None); }
        // Fechou sem passar pelo `close` (janela fechada): pára o agente e desfaz a pasta.
        self.abandon_agent();
    }
}

/// Uma execução do "Pedir ajuda": o que ele fez, em que pé está e a anotação da pasta até ser desfeita.
pub(crate) struct AgentRun {
    pub(super) agent: Agent,
    pub(super) phase: AgentPhase,
    pub(super) transcript: agent::Transcript,
    number: u64,
    pid: Option<u32>,
    /// Instante de início do processo: sem ele o agente nunca é parado (o pid pode já ser de outro).
    started: String,
    snapshot: Option<repo::Snapshot>,
    pub(super) restored: Option<repo::Restored>,
    pub(super) timed_out: bool,
    /// "Parar" sem identidade do processo: ele segue rodando, e a tela diz isso.
    pub(super) not_stopped: bool,
    /// Desfazendo: a resposta da tarefa que desfaz, para quem fechar no meio mandar o relatório com o diff.
    restore_done: Option<tokio::sync::oneshot::Receiver<repo::Restored>>,
}

impl AgentRun {
    fn new(agent: Agent, number: u64) -> Self {
        Self { agent, phase: AgentPhase::Starting, transcript: agent::Transcript::default(), number, pid: None, started: String::new(),
            snapshot: None, restored: None, timed_out: false, not_stopped: false, restore_done: None }
    }

    /// Subindo, trabalhando ou desfazendo: a pasta é dele, nada mais roda nela.
    pub(super) fn busy(&self) -> bool { matches!(self.phase, AgentPhase::Starting | AgentPhase::Running | AgentPhase::Restoring) }
}

/// O agente que subiu, com a anotação da pasta já gravada com pid e identidade.
struct Started { snapshot: repo::Snapshot, pid: u32, started: String, output: async_channel::Receiver<agent::Output> }

/// Monta a chamada, anota a pasta, grava a anotação e só então solta o agente; sem anotação ele não roda. Roda fora da
/// thread da janela: o PATH refeito chama o PowerShell/npm e o snapshot chama o git.
fn start_agent(agent: Agent, report: &str, dest: &Path, options: &Options, state_dir: &Path, english: bool) -> Result<Started, String> {
    let path = system::refreshed_path();
    let program = agent::program(agent, &path).ok_or_else(|| tr("setup_agent_missing"))?;
    let work = state_dir.join("agent");
    let home = std::env::home_dir().unwrap_or_else(|| dest.to_owned());
    let recheck = agent::recheck_command(dest, options, local::read_install(dest).token.is_some(), cfg!(windows));
    let launch = agent::Launch { program, args: agent::args(agent, &home, dest, &work), work,
        env: agent::agent_env(std::env::vars(), &path), prompt: agent::prompt(report, dest, &recheck, english) };
    let mut snapshot = repo::snapshot(dest)?;
    repo::save_at(state_dir, &snapshot).map_err(|e| e.to_string())?;
    let (pid, output) = match agent::spawn(launch) {
        Ok(spawned) => spawned,
        Err(why) => { repo::clear_at(state_dir); return Err(why); }
    };
    // Gone = já saiu (o `Exit` chega pela saída); Unknown depois das tentativas fica sem identidade e nunca é parado.
    let started = (0..20).find_map(|_| match run::read_identity(pid) {
        run::Identity::Known(id) => Some(id),
        run::Identity::Gone => Some(String::new()),
        run::Identity::Unknown => { std::thread::sleep(Duration::from_millis(100)); None }
    }).unwrap_or_else(|| { crate::log_line(&format!("assistente: agente {pid} sem identidade; não poderá ser parado")); String::new() });
    (snapshot.agent_pid, snapshot.agent_started) = (Some(pid), started.clone());
    // Com pid e identidade gravados, a próxima abertura pára um agente que sobrou de um app que caiu.
    if let Err(e) = repo::save_at(state_dir, &snapshot) { crate::log_line(&format!("assistente: anotação sem pid: {e}")); }
    Ok(Started { snapshot, pid, started, output })
}

/// Uma restauração por vez no processo: fechar no meio do conserto e reabrir logo depois rodaria duas na mesma pasta.
static RESTORING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Desfaz e só então esquece a anotação: se o app cair no meio, a próxima abertura desfaz. A segunda que chegar espera a
/// primeira e, com a pasta já devolvida, não acha nada para desfazer.
fn finish_restore(snapshot: repo::Snapshot) -> repo::Restored {
    let _one = RESTORING.lock().unwrap_or_else(|e| e.into_inner());
    let restored = repo::restore(&snapshot);
    if restored.errors.is_empty() && let Some(dir) = run::state_dir() { repo::clear_at(&dir); }
    if !restored.errors.is_empty() { crate::log_line(&format!("assistente: pasta não voltou ao original: {:?}", restored.errors)); }
    restored
}

/// Espera o processo sumir (ou o pid passar a ser de outro); `false` se ainda vive no prazo.
fn wait_gone(pid: u32, started: &str, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        match run::read_identity(pid) {
            run::Identity::Gone => return true,
            run::Identity::Known(id) if id != started => return true,
            _ => {}
        }
        if Instant::now() >= deadline { return false; }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// O que fazer com o pid anotado do agente, decidido ANTES de qualquer sinal.
#[derive(Debug, PartialEq, Eq)]
enum StopPlan {
    /// A identidade lida agora é a anotada: o processo é o agente, e o grupo dele também.
    Signal,
    /// O pid sumiu ou já é de outro processo (um app que caiu horas antes, um shell do tmux): nenhum sinal, a ele nem ao grupo.
    Gone,
    /// Sem identidade anotada ou leitura falhou (K2): nenhum sinal; quem chama avisa que não pôde parar.
    Unverified,
}

fn stop_plan(recorded: &str, now: &run::Identity) -> StopPlan {
    match now {
        _ if recorded.is_empty() => StopPlan::Unverified,
        run::Identity::Known(id) if id == recorded => StopPlan::Signal,
        run::Identity::Known(_) | run::Identity::Gone => StopPlan::Gone,
        run::Identity::Unknown => StopPlan::Unverified,
    }
}

/// Pára o agente e espera ele sair — uma última escrita depois de desfazer passaria —; ainda vivo em 10 s, mata sem
/// escolha. Só sinaliza com a identidade conferida agora; `false` = não deu para conferir (quem chama avisa e desfaz
/// mesmo assim). Bloqueia: só fora da janela.
fn stop_agent_process(pid: u32, started: &str) -> bool {
    match stop_plan(started, &run::read_identity(pid)) {
        StopPlan::Gone => {
            crate::log_line(&format!("assistente: agente {pid} já não existe ou o número é de outro processo; nenhum sinal enviado"));
            return true;
        }
        StopPlan::Unverified => {
            crate::log_line(&format!("assistente: agente {pid} sem identidade conferida; não foi parado"));
            return false;
        }
        StopPlan::Signal => {}
    }
    run::stop(pid, started);
    if !wait_gone(pid, started, Duration::from_secs(10)) {
        run::kill(pid, started);
        if !wait_gone(pid, started, Duration::from_secs(3)) { crate::log_line(&format!("assistente: agente {pid} não saiu nem morto")); }
    }
    sweep_group(pid);
    true
}

/// O líder conferido saiu, mas um comando dele pode seguir no grupo e escrever na pasta depois de desfeita. Enquanto o
/// grupo tem membros, o número dele não volta a ser usado: ainda é o grupo do agente. Só para líder conferido.
fn sweep_group(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while run::group_alive(pid) && Instant::now() < deadline {
        run::kill_group(pid);
        std::thread::sleep(Duration::from_millis(100));
    }
    if run::group_alive(pid) { crate::log_line(&format!("assistente: grupo do agente {pid} ainda vivo")); }
}

/// Pára (se ainda roda) e desfaz. O `bool` diz que o agente não pôde ser parado: a pasta é desfeita mesmo assim.
fn stop_and_restore(pid: Option<u32>, started: &str, snapshot: Option<repo::Snapshot>) -> (Option<repo::Restored>, bool) {
    let not_stopped = pid.is_some_and(|pid| !stop_agent_process(pid, started));
    (snapshot.map(finish_restore), not_stopped)
}

/// O agente subiu mas ninguém o acompanha mais (assistente fechado enquanto subia).
fn abandon_started(started: Started) {
    let Started { snapshot, pid, started, output } = started;
    drop(output);
    stop_and_restore(Some(pid), &started, Some(snapshot));
}

/// O que a abertura fez com um conserto interrompido; mostrado até a pessoa fechar o aviso.
pub(crate) struct Recovery {
    pub(super) restored: repo::Restored,
    /// O agente que sobrou não pôde ser parado (sem identidade).
    pub(super) not_stopped: bool,
    /// Aberto sozinho com a pasta já seguindo em frente (ou anotação de mais de um dia): nada foi desfeito.
    pub(super) skipped: bool,
    /// Aberto pelo menu com a pasta já seguindo em frente: nada foi desfeito, e estes são os arquivos que "Desfazer mesmo
    /// assim" voltaria (a anotação fica até a pessoa escolher); `Err` = a lista não pôde ser lida.
    pub(super) offer: Option<Result<Vec<String>, String>>,
}

/// Quem pediu a recuperação de um conserto interrompido.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Recover {
    /// O app abriu sozinho ao iniciar: pasta adiante ou anotação velha não se desfaz, arquiva.
    Launch,
    /// Aberto pelo menu: pasta adiante não se desfaz sozinha, oferece o botão.
    Menu,
    /// "Desfazer mesmo assim": desfaz o que houver.
    Anyway,
}

/// Arquiva a anotação e registra: o assistente deixa de abrir sozinho por causa dela.
fn archive_annotation(dir: &Path, why: &str) {
    match repo::archive_at(dir) {
        Some(to) => crate::log_line(&format!("assistente: anotação do conserto arquivada ({why}): {}", to.display())),
        None => crate::log_line(&format!("assistente: anotação do conserto não arquivada ({why})")),
    }
}

/// O app caiu com o agente rodando: pára quem sobrou e devolve a pasta pela anotação. Aberto sozinho na abertura do app,
/// não desfaz uma pasta que já seguiu em frente nem uma anotação velha: arquiva e avisa. Pelo menu, pasta adiante também
/// não se desfaz sozinha (voltaria uma atualização): avisa com a lista e espera "Desfazer mesmo assim". A segunda
/// restauração que falha também arquiva, para o assistente não abrir a cada início sem saída.
fn recover_agent_edits(mode: Recover) -> Option<Recovery> {
    let dir = run::state_dir()?;
    let snapshot = repo::load_at(&dir)?;
    let (pid, started) = (snapshot.agent_pid, snapshot.agent_started.clone());
    if mode == Recover::Launch && (repo::stale(&snapshot) || repo::moved_on(&snapshot)) {
        let not_stopped = pid.is_some_and(|pid| !stop_agent_process(pid, &started));
        archive_annotation(&dir, "a pasta mudou depois do agente ou a anotação é velha");
        return Some(Recovery { restored: repo::Restored::default(), not_stopped, skipped: true, offer: None });
    }
    if mode == Recover::Menu && repo::moved_on(&snapshot) {
        let not_stopped = pid.is_some_and(|pid| !stop_agent_process(pid, &started));
        return Some(Recovery { restored: repo::Restored::default(), not_stopped, skipped: true, offer: Some(repo::pending_changes(&snapshot)) });
    }
    let mut kept = snapshot.clone();
    let (restored, not_stopped) = stop_and_restore(pid, &started, Some(snapshot));
    let restored = restored?;
    if !restored.errors.is_empty() {
        kept.recover_failures += 1;
        if kept.recover_failures >= 2 { archive_annotation(&dir, "segunda restauração com erro"); }
        else if let Err(e) = repo::save_at(&dir, &kept) { crate::log_line(&format!("assistente: anotação sem contagem: {e}")); }
    }
    Some(Recovery { restored, not_stopped, skipped: false, offer: None })
}

/// A seção do conserto no relatório: comandos, explicação, o diff do que foi desfeito e as notas.
fn agent_section(run: &AgentRun, restored: &repo::Restored, recheck: report::Recheck) -> report::AgentSection {
    let name = run.agent.name();
    report::AgentSection { agent: name.to_owned(), commands: run.transcript.commands.clone(),
        explanation: run.transcript.explanation.clone().unwrap_or_else(|| tr("setup_agent_no_explanation").replace("{agente}", name)),
        diff: restored.diff.clone(), notes: report::restore_notes(restored.head_moved.as_ref(), &restored.errors, run.timed_out), recheck }
}

/// O assistente rodando suspende a procura e a troca do app (`update.rs`).
fn suspend_updates(on: bool, cx: &mut App) {
    if let Some(updater) = cx.try_global::<crate::update::Handle>().map(|handle| handle.0.clone()) {
        updater.update(cx, |updater, cx| updater.set_suspended(on, cx));
    }
}

impl SetupWizard {
    /// `at_launch`: o app abriu sozinho ao iniciar (a recuperação de um conserto interrompido fica mais cautelosa).
    pub(super) fn new(hangar: WeakEntity<Hangar>, runtime: tokio::runtime::Handle, origin: Origin, at_launch: bool, window: &mut Window,
        cx: &mut Context<Self>) -> Self {
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(tr("setup_password_field")));
        let confirm = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(tr("setup_password_confirm")));
        let subscriptions = vec![cx.subscribe(&password, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&confirm, |_, _, _: &InputEvent, cx| cx.notify())];
        // Pedidos de senha do `--askpass` chegam pela instância única; cada um entra por um update novo.
        let (askpass_tx, askpass_rx) = async_channel::unbounded::<AskpassRequest>();
        crate::single_instance::serve_askpass(Some(askpass_tx));
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(request) = askpass_rx.recv().await {
                if this.update_in(cx, |w, window, cx| w.on_askpass(request, window, cx)).is_err() { break; }
            }
        }).detach();
        let (mode, dest) = local::fresh_here();
        let mut wizard = Self {
            hangar, runtime, exe: std::env::current_exe().ok(), mode, dest, located: false,
            agents: vec!["claude"], agents_touched: false, installed: Vec::new(), outside: false, password_mode: PasswordMode::Generate,
            password, confirm, precheck: None, pkg: None, started: false, preparing: None,
            git_ready: false, bootstrap: None, token: None, runs: Runs::default(), check_tail: None, install_tail: None, records: (None, None),
            polling: false, finished: false, failure: None,
            report: None, report_about: None, report_gen: 0, send: true, outbox: Outbox::default(), send_requested: Vec::new(), refreshing: false,
            agents_ready: Vec::new(), agent: None, agent_runs: 0, report_base: None, report_secrets: Vec::new(), fix_sent: None, recovered: None,
            recovering: true,
            fix_mouse_wheel: false, vault: Vault::default(), waiting: Vec::new(), prompt: None, after_password: None,
            opened_link: None, app_copy: None, connection: None, connect_later: false, resumed_ended: false, viewing: Screen::Welcome, details_open: false, phone: None,
            qr: Qr::Idle, tailscale_running: false, tailscale_checking: false, ticks: 0,
            focus: cx.focus_handle(), window: window.window_handle(), _subscriptions: subscriptions,
        };
        wizard.recover(if at_launch { Recover::Launch } else { Recover::Menu }, cx);
        match origin {
            Origin::Entry(install) => { wizard.apply_install(install); wizard.located = true; }
            Origin::Menu => {}
            Origin::Resume(state) => wizard.resume(state, window, cx),
        }
        if !wizard.started { wizard.recheck(window, cx); }
        wizard.focus.focus(window, cx);
        wizard
    }

    /// O app caiu no meio de um conserto: pára o agente que sobrou e devolve a pasta, fora da thread da janela, e avisa.
    fn recover(&mut self, mode: Recover, cx: &mut Context<Self>) {
        self.recovering = true;
        let recovery = cx.background_executor().spawn(async move { recover_agent_edits(mode) });
        cx.spawn(async move |this, cx| {
            let recovered = recovery.await;
            let _ = this.update(cx, |w, cx| {
                w.recovering = false;
                w.recovered = recovered.filter(|r| r.skipped || r.not_stopped || !r.restored.changed.is_empty()
                    || !r.restored.errors.is_empty() || r.restored.head_moved.is_some());
                cx.notify();
            });
        }).detach();
    }

    /// "Desfazer mesmo assim" do aviso de pasta adiante. Com um agente novo chamado, a anotação já é a dele: não mexe.
    pub(super) fn undo_anyway(&mut self, cx: &mut Context<Self>) {
        if self.agent.is_some() || self.recovering || !self.recovered.as_ref().is_some_and(|r| r.offer.is_some()) { return; }
        self.recovered = None;
        self.recover(Recover::Anyway, cx);
        cx.notify();
    }

    /// A pasta achada decide onde roda e se a senha do celular já existe ("senha atual mantida").
    fn apply_install(&mut self, install: Option<LocalInstall>) {
        (self.mode, self.dest) = install.as_ref().map(|i| (i.mode, i.dir.clone())).unwrap_or_else(local::fresh_here);
        self.password_mode = if install.as_ref().is_some_and(|i| i.token.is_some()) { PasswordMode::Keep } else { PasswordMode::Generate };
    }

    /// Reabrir no meio: as escolhas vêm do `state.json`, o andamento do arquivo de cada execução, lido do começo.
    fn resume(&mut self, state: SetupState, window: &mut Window, cx: &mut Context<Self>) {
        (self.mode, self.dest) = (state.mode, state.dest);
        self.located = true;
        self.agents = state.options.agents.iter().filter_map(|a| system::AGENTS.iter().find(|(id, _)| id == a).map(|(id, _)| *id)).collect();
        self.agents_touched = true;
        self.outside = state.options.outside;
        // Só a conferência começou: a senha do celular escolhida morava na memória e o `--check` não a grava. Para a conferência
        // e volta à tela 1 sem começar, para a pessoa escolher a senha de novo.
        let Some(install) = state.install else {
            // Sem hora de início guardada o pid pode já ser de outro processo: não mata.
            if let Some(record) = state.check.filter(|r| !r.started.is_empty()) {
                cx.background_executor().spawn(async move { run::stop(record.pid, &record.started) }).detach();
            }
            run::clear_state();
            let token = local::read_install(&self.dest).token.is_some();
            self.password_mode = if token { PasswordMode::Keep } else { PasswordMode::Generate };
            return;
        };
        // O passo 0 já gravou a senha do celular: nunca uma senha nova no meio.
        self.password_mode = PasswordMode::Keep;
        (self.started, self.git_ready, self.precheck) = (true, true, Some(Vec::new()));
        self.vault = Vault::new(state.askpass_code);
        if let Some(record) = state.check {
            self.check_tail = Some(Tail::new(record.log.clone()));
            self.runs.check = Some(Progress::default());
            self.records.0 = Some(record);
        }
        self.install_tail = Some(Tail::new(install.log.clone()));
        self.runs.install = Some(Progress::default());
        self.records.1 = Some(install);
        self.bootstrap = run::state_dir().map(|dir| dir.join(run::bootstrap_name(cfg!(windows)))).filter(|p| p.is_file());
        self.read_tails(false);
        self.resumed_ended = self.runs.end().is_some();
        self.go(self.frontier_now(), cx);
        suspend_updates(true, cx);
        self.start_poll(window, cx);
    }

    pub(super) fn view(&self) -> View<'_> {
        View { started: self.started, precheck_blocked: self.precheck.as_deref().is_some_and(precheck::blocked), runs: &self.runs,
            failed_at: self.failure.as_ref().map(|f| f.screen), phone: self.phone }
    }

    fn frontier_now(&self) -> Screen { flow::frontier(&flow::screens(self.outside), &self.view()) }

    pub(super) fn password_problem(&self, cx: &App) -> Option<PasswordProblem> {
        flow::phone_password(&self.password_mode, &self.password.read(cx).value(), &self.confirm.read(cx).value()).err()
    }

    pub(super) fn can_start(&self, cx: &App) -> bool {
        !self.agents.is_empty() && self.password_problem(cx).is_none() && self.precheck.as_deref().is_some_and(|rows| !precheck::blocked(rows))
    }

    /// Começou e ainda não acabou nem falhou: fechar a janela não pode soltar o canal da senha nem religar a atualização.
    pub(super) fn is_running(&self) -> bool { self.started && !self.finished && self.failure.is_none() }

    fn options(&self) -> Options {
        Options { agents: self.agents.iter().map(|a| a.to_string()).collect(), outside: self.outside, fix_mouse_wheel: self.fix_mouse_wheel }
    }

    pub(super) fn recheck(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.precheck = None;
        let (locate, mode, dest) = (!self.located, self.mode, self.dest.clone());
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let install = if locate { tokio::task::spawn_blocking(local::find_install).await.ok().flatten() } else { None };
            // Achado nada, `prechecked` volta ao destino de máquina nova, que é o que já está em `mode`/`dest`.
            let (mode, dest) = install.as_ref().map(|i| (i.mode, i.dir.clone())).unwrap_or((mode, dest));
            let facts = precheck::gather(dest, mode == local::Mode::Package).await;
            let installed = tokio::task::spawn_blocking(|| system::installed_agents(&system::refreshed_path())).await.unwrap_or_default();
            let _ = done.send((locate.then_some(install), facts, installed));
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((install, facts, installed)) = result.await else { return };
            let _ = this.update_in(cx, |w, window, cx| w.prechecked(install, facts, installed, window, cx));
        }).detach();
        cx.notify();
    }

    fn prechecked(&mut self, install: Option<Option<LocalInstall>>, facts: precheck::Facts, installed: Vec<&'static str>,
        window: &mut Window, cx: &mut Context<Self>) {
        if let Some(install) = install { self.apply_install(install); self.located = true; }
        self.pkg = facts.pkg;
        self.precheck = Some(precheck::rows(&facts));
        self.installed = installed;
        if !self.agents_touched { self.agents = flow::default_agents(&self.installed); }
        // Só para provar telas sem mouse: `HANGAR_SETUP_DEMO=start` (só Wi-Fi) ou `start-fora` (com a Tailscale).
        let demo = std::env::var("HANGAR_SETUP_DEMO").unwrap_or_default();
        if demo.starts_with("start") && !self.started {
            self.outside = demo == "start-fora";
            if self.can_start(cx) { self.start(window, cx); }
        }
        cx.notify();
    }

    pub(super) fn toggle_agent(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if self.started { return; }
        self.agents = flow::toggle_agent(&self.agents, id);
        self.agents_touched = true;
        cx.notify();
    }

    pub(super) fn set_outside(&mut self, on: bool, cx: &mut Context<Self>) {
        if !self.started { self.outside = on; cx.notify(); }
    }

    pub(super) fn set_password_mode(&mut self, mode: PasswordMode, cx: &mut Context<Self>) {
        if !self.started && self.password_mode != PasswordMode::Keep { self.password_mode = mode; cx.notify(); }
    }

    pub(super) fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.started || !self.can_start(cx) { return; }
        let before = self.frontier_now();
        self.token = flow::phone_password(&self.password_mode, &self.password.read(cx).value(), &self.confirm.read(cx).value()).ok().flatten();
        self.started = true;
        self.vault = Vault::new(askpass::new_code());
        suspend_updates(true, cx);
        self.continue_start(window, cx);
        self.follow(before, cx);
    }

    /// Cada pedaço antes do script termina chamando de novo: git → bootstrap → conferência.
    fn continue_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.failure.is_some() { return; }
        if !self.git_ready && self.precheck.as_deref().is_some_and(precheck::needs_git) {
            if !cfg!(windows) && self.vault.password().is_none() {
                self.after_password = Some(AfterPassword::InstallGit);
                return self.open_password_prompt(tr("setup_sudo_reason").replace("{motivo}", &tr("setup_sudo_reason_git")), window, cx);
            }
            return self.install_git(window, cx);
        }
        if self.bootstrap.is_none() { return self.download(window, cx); }
        self.launch(Kind::Check, window, cx);
    }

    fn install_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preparing = Some(tr("setup_git_installing"));
        let (pkg, password) = (self.pkg, self.vault.password().map(str::to_owned));
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || { let _ = done.send(precheck::install_git(pkg, password.as_deref())); });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = result.await.unwrap_or_else(|_| Err(String::new()));
            let _ = this.update_in(cx, |w, window, cx| match outcome {
                Ok(()) => {
                    w.git_ready = true;
                    w.preparing = None;
                    if let Some(row) = w.precheck.iter_mut().flatten().find(|r| r.check == Check::Git) { row.ok = true; }
                    w.continue_start(window, cx);
                }
                Err(why) => w.fail(Failure::app(None, format!("{}\n{why}", tr("setup_git_failed")), Screen::Prepare), cx),
            });
        }).detach();
        cx.notify();
    }

    fn download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preparing = Some(tr("setup_prepare_downloading"));
        let dir = run::state_dir();
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let fetched = match dir { Some(dir) => run::fetch_bootstrap(&dir, cfg!(windows)).await, None => Err("APPDATA/XDG_CONFIG_HOME".into()) };
            let _ = done.send(fetched);
        });
        cx.spawn_in(window, async move |this, cx| {
            let fetched = result.await.unwrap_or_else(|_| Err(String::new()));
            let _ = this.update_in(cx, |w, window, cx| match fetched {
                Ok(path) => { w.bootstrap = Some(path); w.continue_start(window, cx); }
                Err(why) => w.fail(Failure::app(None, tr("setup_failure_download").replace("{erro}", &why), Screen::Prepare), cx),
            });
        }).detach();
        cx.notify();
    }

    /// Linux: o atalho do askpass ao lado do `state.json`, e o código desta instalação no ambiente.
    fn askpass_env(&self) -> Option<(PathBuf, String)> {
        if cfg!(windows) { return None; }
        let wrapper = askpass::write_wrapper(&run::state_dir()?, self.exe.as_ref()?).ok()?;
        Some((wrapper, self.vault.code().to_owned()))
    }

    fn launch(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bootstrap) = self.bootstrap.clone() else { return };
        let log = run::log_path(kind);
        let launch = run::Launch { kind, bootstrap, mode: self.mode, dest: self.dest.clone(), options: self.options(), log: log.clone(), token: self.token.clone(),
            askpass: self.askpass_env() };
        match run::spawn(&launch) {
            Ok(pid) => {
                // A hora de início chega do fundo (`learn_identity`): no Windows ela abre um PowerShell.
                let record = RunRecord { kind, log: log.clone(), pid, started: String::new() };
                match kind {
                    Kind::Check => { self.runs.check = Some(Progress::default()); self.check_tail = Some(Tail::new(log)); self.records.0 = Some(record); }
                    Kind::Install => { self.runs.install = Some(Progress::default()); self.install_tail = Some(Tail::new(log)); self.records.1 = Some(record); }
                }
                self.preparing = None;
                // A roda do mouse é consertada uma vez: a próxima tentativa não fecha as sessões de novo sem perguntar.
                if kind == Kind::Install { self.fix_mouse_wheel = false; }
                self.save_state();
                self.learn_identity(pid, cx);
                if !self.polling { self.start_poll(window, cx); }
            }
            Err(why) => self.fail(Failure::app(None, tr("setup_failure_start").replace("{erro}", &why), Screen::Prepare), cx),
        }
        cx.notify();
    }

    fn save_state(&self) {
        let state = SetupState { mode: self.mode, dest: self.dest.clone(), options: self.options(), askpass_code: self.vault.code().to_owned(),
            check: self.records.0.clone(), install: self.records.1.clone() };
        if let Err(error) = run::save_state(&state) { crate::log_line(&format!("assistente: state.json não gravado: {error}")); }
    }

    /// Lê a hora de início do processo fora da thread da janela e a grava no registro e no `state.json`.
    fn learn_identity(&self, pid: u32, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move { run::read_identity(pid) });
        cx.spawn(async move |this, cx| {
            // Gone/Unknown ficam para a verificação da cadência Quiet, que tenta de novo.
            if let run::Identity::Known(id) = task.await { let _ = this.update(cx, |w, _| w.set_started(pid, id)); }
        }).detach();
    }

    fn set_started(&mut self, pid: u32, started: String) {
        for record in [self.records.0.as_mut(), self.records.1.as_mut()].into_iter().flatten() {
            if record.pid == pid { record.started = started.clone(); }
        }
        self.save_state();
    }

    /// Pid e hora de início da execução que anda: a instalação, quando já começou.
    fn active_run(&self) -> Option<(u32, String)> {
        self.records.1.as_ref().or(self.records.0.as_ref()).map(|r| (r.pid, r.started.clone()))
    }

    /// ponytail: lê na thread da janela, até 1 MiB por volta (`tail::READ_CAP`). Mover para o executor de fundo se pesar.
    /// `last`: o processo morreu, então o pedaço sem `\n` no fim do arquivo também é uma linha (um FIM sem quebra).
    fn read_tails(&mut self, last: bool) -> bool {
        let mut grew = false;
        for (tail, progress) in [(&mut self.check_tail, &mut self.runs.check), (&mut self.install_tail, &mut self.runs.install)] {
            let (Some(tail), Some(progress)) = (tail.as_mut(), progress.as_mut()) else { continue };
            // Arquivo ainda não criado: tenta na próxima volta.
            if let Ok(lines) = if last { tail.read_final() } else { tail.read_new() } {
                grew |= !lines.is_empty();
                for line in &lines { progress.feed(line); }
            }
        }
        grew
    }

    fn start_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.polling = true;
        cx.spawn_in(window, async move |this, cx| {
            let mut quiet = 0u32;
            loop {
                cx.background_executor().timer(POLL_EVERY).await;
                let Ok(watch) = this.update_in(cx, |w, window, cx| w.tick(window, cx)) else { return };
                match watch {
                    Watch::Stop => return,
                    Watch::Grew => quiet = 0,
                    Watch::Quiet(pid, started) => {
                        quiet += 1;
                        if quiet % QUIET_CHECKS != 0 { continue; }
                        // A identidade lê o /proc (Linux) ou abre o PowerShell (Windows): fora da thread da janela.
                        // Sem hora de início guardada: `Known` a guarda, `Gone` é prova de que acabou, `Unknown` conta como vivo.
                        let (alive, learned) = cx.background_executor().spawn(async move {
                            if !started.is_empty() { return (run::alive(pid, &started), None); }
                            match run::read_identity(pid) {
                                run::Identity::Known(id) => (true, Some(id)),
                                run::Identity::Gone => (false, None),
                                run::Identity::Unknown => (true, None),
                            }
                        }).await;
                        if let Some(id) = learned && this.update(cx, |w, _| w.set_started(pid, id)).is_err() { return; }
                        if !alive && this.update(cx, |w, cx| w.process_gone(cx)).is_err() { return; }
                    }
                }
            }
        }).detach();
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Watch {
        if !self.polling { return Watch::Stop; }
        let before = self.frontier_now();
        let grew = self.read_tails(false);
        if self.runs.latest().is_some_and(Progress::mismatch) {
            if let Some((pid, started)) = self.active_run() {
                // Sem identidade guardada não há o que conferir: nenhum sinal, o script termina sozinho.
                if started.is_empty() {
                    crate::log_line(&format!("assistente: versão diferente; script {pid} sem identidade guardada, nenhum sinal enviado"));
                } else {
                    // `stop` confere a identidade e, no Windows, roda o `taskkill`: fora da thread da janela.
                    cx.background_executor().spawn(async move { run::stop(pid, &started); }).detach();
                }
            }
            self.fail(Failure::app(Some("versao-diferente"), tr("setup_failure_protocol"), before), cx);
            return Watch::Stop;
        }
        self.open_new_link(cx);
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks % 8 == 0 { self.watch_tailscale(cx); }
        if self.runs.install.is_none() {
            match self.runs.check.as_ref().and_then(|p| p.end) {
                Some(End::Ok | End::Pending) => {
                    self.settle_mode();
                    self.launch(Kind::Install, window, cx);
                    self.follow(before, cx);
                    return Watch::Grew;
                }
                Some(End::Failed) => {
                    if let Some(failure) = self.runs.check.as_ref().map(|p| Failure::from_progress(p, Screen::Prepare)) { self.fail(failure, cx); }
                    return Watch::Stop;
                }
                None => {}
            }
        } else if self.runs.end().is_some() {
            self.finish(window, cx);
            self.follow(before, cx);
            return Watch::Stop;
        }
        self.follow(before, cx);
        if grew { cx.notify(); return Watch::Grew; }
        self.active_run().map_or(Watch::Stop, |(pid, started)| Watch::Quiet(pid, started))
    }

    /// `##HANGAR-LINK## tailscale-login`: abre o navegador uma vez por link novo. Com o dublê não abre sozinho (o link é de
    /// mentira); o botão "Entrar na Tailscale" continua abrindo.
    fn open_new_link(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self.runs.latest().and_then(|p| p.link("tailscale-login")).map(str::to_owned) else { return };
        if self.opened_link.as_deref() == Some(url.as_str()) { return; }
        if run::test_bootstrap().is_none() { cx.open_url(&url); }
        self.opened_link = Some(url);
    }

    /// O processo sumiu: lê o que sobrou; sem `##HANGAR-FIM##`, a instalação foi interrompida.
    fn process_gone(&mut self, cx: &mut Context<Self>) {
        if !self.polling || self.read_tails(false) { return; }
        self.read_tails(true);
        let ended = self.runs.latest().is_none_or(|p| p.end.is_some());
        if !ended { let screen = self.frontier_now(); self.fail(Failure::app(None, tr("setup_failure_interrupted"), screen), cx); }
    }

    /// O que o script deixou instalado vale para o doctor, a conexão e a retomada (`local::settle`).
    fn settle_mode(&mut self) {
        // Antes de qualquer execução o script não instalou nada, e gravar o `state.json` faria o app retomar ao abrir.
        if self.mode != local::Mode::Package || self.records == (None, None) { return; }
        let now = local::settle((self.mode, self.dest.clone()), local::installed_now());
        if now != (self.mode, self.dest.clone()) { (self.mode, self.dest) = now; self.save_state(); }
    }

    pub(super) fn fail(&mut self, mut failure: Failure, cx: &mut Context<Self>) {
        // Antes do relatório: o doctor e o `.env` são os do que ficou instalado.
        self.settle_mode();
        // O relatório da falha anterior que ainda não saiu entra na fila agora: a nova nunca o apaga.
        self.send_before_leaving(cx);
        // Antes do `forget`: a senha de administrador e o código do askpass também são procurados e trocados no relatório.
        let secrets = self.secret_values();
        failure.fixes = codes::buttons(failure.code.as_deref(), self.agents.iter().any(|a| *a != "claude"));
        self.viewing = failure.screen;
        self.failure = Some(failure.clone());
        (self.polling, self.preparing, self.refreshing) = (false, None, false);
        self.vault.forget();
        self.after_password = None;
        for request in self.waiting.drain(..) { let _ = request.reply.send(None); }
        self.close_prompt(cx);
        suspend_updates(false, cx);
        // A reconferência depois do agente falhou: o relatório é o de antes mais o conserto, e sai agora.
        if self.agent.as_ref().is_some_and(|run| run.phase == AgentPhase::Rechecking) {
            let detail = format!("{} {}", failure.code.as_deref().unwrap_or("-"), failure.text);
            self.report_secrets.extend(secrets);
            self.finish_agent(report::Recheck::Failed(detail), cx);
        } else {
            (self.agent, self.fix_sent, self.report_base) = (None, None, None);
            // Guardados para o relatório do conserto, se a pessoa chamar o agente: o cofre já esqueceu.
            self.report_secrets = secrets.clone();
            self.start_report(failure, secrets, cx);
            self.detect_agents(cx);
        }
        cx.notify();
    }

    /// Senha do celular escolhida, a senha de administrador ainda na memória e o código do askpass; o `CP_AUTH_TOKEN` do
    /// `.env` é lido junto com o doctor, fora da thread da janela.
    fn secret_values(&self) -> Vec<String> {
        let mut values: Vec<String> = self.token.iter().cloned().collect();
        values.extend(self.vault.password().map(str::to_owned));
        values.push(self.vault.code().to_owned());
        values
    }

    fn start_report(&mut self, failure: Failure, mut secrets: Vec<String>, cx: &mut Context<Self>) {
        self.report_gen += 1;
        let generation = self.report_gen;
        (self.report, self.send) = (None, true);
        self.report_about = Some(failure.clone());
        let logs: Vec<(String, PathBuf)> = [("check", self.records.0.as_ref()), ("install", self.records.1.as_ref())].into_iter()
            .filter_map(|(run, record)| record.map(|r| (run.to_owned(), r.log.clone()))).collect();
        let (mode, dest) = (self.mode, self.dest.clone());
        let about = failure.clone();
        // O doctor leva segundos: fora da thread da janela.
        let task = cx.background_executor().spawn(async move {
            secrets.extend(local::read_install(&dest).token);
            let secrets = report::Secrets::here(secrets);
            let facts = report::Facts { step: failure.screen, code: failure.code.clone(), text: failure.text.clone(),
                system: report::system_line(), app: report::app_line(), doctor: report::doctor(mode, &dest),
                logs: logs.into_iter().map(|(run, path)| (run, report::read_log(&path))).collect() };
            report::compose(&facts, &secrets)
        });
        cx.spawn(async move |this, cx| {
            let text = task.await;
            let _ = this.update(cx, |w, cx| w.report_ready(generation, about, text, cx));
        }).detach();
    }

    /// Entregas feitas enquanto montava saem agora, mesmo que outra falha já esteja na tela.
    fn report_ready(&mut self, generation: u64, about: Failure, text: String, cx: &mut Context<Self>) {
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.send_requested).into_iter().partition(|(g, ..)| *g == generation);
        self.send_requested = later;
        for (_, outcome, agent) in now {
            self.dispatch(report::payload(about.screen, about.code.clone(), outcome, agent.map(Agent::id), text.clone()), cx);
        }
        if self.report_gen == generation { self.report = Some(text); }
        self.try_demo_agent(cx);
        cx.notify();
    }

    /// Entrega o relatório da falha na tela ao envio, uma vez, se a caixa está marcada; ainda montando, sai quando ficar
    /// pronto. Depois a caixa trava: a pessoa já saiu do painel. Devolve o id na fila quando saiu agora.
    fn send_report(&mut self, outcome: Outcome, agent: Option<Agent>, cx: &mut Context<Self>) -> Option<u64> {
        let about = self.report_about.take()?;
        let mut sent = None;
        if self.send {
            match self.report.clone() {
                Some(text) => sent = Some(self.dispatch(report::payload(about.screen, about.code.clone(), outcome, agent.map(Agent::id), text), cx)),
                None => self.send_requested.push((self.report_gen, outcome, agent)),
            }
        }
        cx.notify();
        sent
    }

    fn dispatch(&mut self, payload: Payload, cx: &mut Context<Self>) -> u64 {
        let (id, payload) = self.outbox.push(payload);
        self.send_now(id, payload, cx);
        id
    }

    fn send_now(&mut self, id: u64, payload: Payload, cx: &mut Context<Self>) {
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move { let _ = done.send(report::send(payload).await); });
        cx.spawn(async move |this, cx| {
            let outcome = result.await.unwrap_or_else(|_| Err(String::new()));
            let _ = this.update(cx, |w, cx| { w.outbox.finish(id, outcome); cx.notify(); });
        }).detach();
        cx.notify();
    }

    /// "Sem agente, sai na hora": na primeira ação da pessoa depois da falha (tentar de novo, um botão da frase, fechar).
    fn send_before_leaving(&mut self, cx: &mut Context<Self>) {
        // Com o agente chamado, o relatório sai depois dele, levando o que ele fez.
        if self.agent.is_some() { return; }
        self.send_report(Outcome::Aberto, None, cx);
    }

    /// Fecha a janela de senha sem acionar o `on_close` (que cancelaria); adiado porque a janela pode estar emprestada.
    fn close_prompt(&mut self, cx: &mut Context<Self>) {
        if self.prompt.take().is_none() { return; }
        let window = self.window;
        cx.defer(move |cx| { let _ = window.update(cx, |_, window, cx| window.close_dialog(cx)); });
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.polling = false;
        self.settle_mode();
        // A reconferência do agente acabou: o relatório dele limpa também a senha desta execução, lida antes do `forget`.
        if self.agent.as_ref().is_some_and(|run| run.phase == AgentPhase::Rechecking) { self.report_secrets.extend(self.secret_values()); }
        self.vault.forget();
        for request in self.waiting.drain(..) { let _ = request.reply.send(None); }
        self.close_prompt(cx);
        suspend_updates(false, cx);
        if self.runs.end() == Some(End::Failed) {
            let screen = self.frontier_now();
            if let Some(failure) = self.runs.install.as_ref().map(|p| Failure::from_progress(p, screen)) { self.fail(failure, cx); }
            return;
        }
        if self.agent.as_ref().is_some_and(|run| run.phase == AgentPhase::Rechecking) { self.finish_agent(report::Recheck::Passed, cx); }
        // Reaberto com o fim já lido: não copia nem conecta duas vezes nesta abertura.
        if self.finished { return; }
        self.finished = true;
        self.copy_app(cx);
        self.connect_local(window, cx);
        cx.notify();
    }

    pub(super) fn copy_app(&mut self, cx: &mut Context<Self>) {
        if run::test_bootstrap().is_some() { self.app_copy = Some(AppCopy::Skipped); return; }
        let Some(exe) = self.exe.clone() else { self.app_copy = Some(AppCopy::Failed("current_exe".into())); return };
        self.app_copy = None;
        let task = cx.background_executor().spawn(async move { super::app_copy::install_self(&exe) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |w, cx| {
                w.app_copy = Some(match result { Ok(path) => AppCopy::Done(path), Err(why) => AppCopy::Failed(why) });
                cx.notify();
            });
        }).detach();
    }

    fn connect_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let install = local::read_install(&self.dest);
        let Some(token) = install.token.clone() else {
            // Sem conferir o lugar do `.env`, o motivo dessa falha entra no lugar do caminho.
            let env = local::env_file(std::env::home_dir().as_deref(), &self.dest).map_or_else(|e| e, |env| env.display().to_string());
            self.connection = Some(Err(tr("setup_connection_missing").replace("{path}", &env)));
            return;
        };
        let address = local::address(install.port);
        // Escondido com o app já ligado a um servidor: não troca de servidor calado; conecta no "Abrir Hangar".
        let me = cx.entity_id();
        let hidden_while_connected = self.hangar.read_with(cx, |hangar, _| hangar.api.is_some()
            && hangar.setup_hidden.as_ref().is_some_and(|hidden| hidden.entity_id() == me)).unwrap_or(false);
        // Reaberto depois do fim com uma conexão salva: também não troca calado.
        let resumed_over_saved = self.resumed_ended && super::super::load_connection().is_some();
        if hidden_while_connected || resumed_over_saved { self.connect_later = true; return; }
        self.connection = Some(Ok(address.clone()));
        let _ = self.hangar.update(cx, |hangar, cx| hangar.setup_connect(address, token, window, cx));
    }

    pub(super) fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Pendência não para o script: rodar de novo antes do FIM seria um segundo instalador na mesma pasta.
        // Agente trabalhando: a pasta é dele até ser desfeita (a reconferência entra pelo `Rechecking`, que não é ocupado).
        if self.is_running() || self.refreshing || self.agent.as_ref().is_some_and(AgentRun::busy) { return; }
        // Sem agente a caminho da reconferência, a senha de administrador guardada para o relatório dele sai da memória.
        if self.agent.is_none() { self.report_secrets.clear(); }
        // Sair da falha por qualquer botão de tentar de novo entrega o relatório dela.
        self.send_before_leaving(cx);
        let before = self.frontier_now();
        (self.failure, self.finished, self.started) = (None, false, true);
        self.runs = Runs::default();
        (self.check_tail, self.install_tail, self.records) = (None, None, (None, None));
        (self.app_copy, self.connection, self.opened_link, self.connect_later, self.resumed_ended) = (None, None, None, false, false);
        (self.qr, self.tailscale_running) = (Qr::Idle, false);
        // A senha escolhida continua em `token`; sem ela, o instalador mantém a do `.env` ou gera uma.
        // A senha pedida pelo botão da frase (pacotes) vale para o script que roda agora: não pergunta de novo.
        let kept = self.vault.password().map(str::to_owned);
        self.vault = Vault::new(askpass::new_code());
        if let Some(password) = kept { self.vault.remember(password); }
        suspend_updates(true, cx);
        self.continue_start(window, cx);
        self.follow(before, cx);
    }

    pub(super) fn failure_action(&mut self, action: FailureAction, window: &mut Window, cx: &mut Context<Self>) {
        let busy = self.agent.as_ref().is_some_and(AgentRun::busy);
        match action {
            FailureAction::ToggleDetails => { self.details_open = !self.details_open; cx.notify(); }
            // Agente trabalhando: nada sobe na pasta que ele edita, e o relatório espera ele terminar.
            FailureAction::Fix(_) | FailureAction::AskAgent(_) | FailureAction::ToggleSend if busy => {}
            FailureAction::ToggleSend => if self.report_about.is_some() { self.send = !self.send; cx.notify(); },
            FailureAction::SendAgain(id) => if let Some(payload) = self.outbox.again(id) { self.send_now(id, payload, cx); },
            FailureAction::Dismiss(id) => { self.outbox.dismiss(id); cx.notify(); }
            // A lista de pacotes ainda atualiza: rodar o script agora bateria na trava do apt.
            FailureAction::Fix(_) | FailureAction::AskAgent(_) if self.refreshing => {}
            FailureAction::Fix(fix) => self.apply_fix(fix, window, cx),
            FailureAction::AskAgent(agent) => self.ask_agent(agent, cx),
            FailureAction::StopAgent => self.stop_agent(None, cx),
        }
    }

    fn detect_agents(&mut self, cx: &mut Context<Self>) {
        self.agents_ready.clear();
        let (clone, dest) = (self.mode == local::Mode::Git, self.dest.clone());
        // Sem a pasta ser a raiz do próprio repositório não há como anotar e desfazer (antes do clone, ou dentro de outro
        // repositório como dotfiles em `~`): o botão nem aparece. No pacote, nunca: não há o que desfazer pelo git.
        let task = cx.background_executor().spawn(async move {
            if clone && repo::own_repo(&dest) { agent::available(&system::refreshed_path()) } else { Vec::new() }
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |w, cx| if w.failure.is_some() && w.agent.is_none() {
                w.agents_ready = found;
                w.try_demo_agent(cx);
                cx.notify();
            });
        }).detach();
    }

    /// Só para provar telas sem mouse: `HANGAR_SETUP_DEMO_AGENT=1` pede ajuda ao primeiro agente assim que puder.
    fn try_demo_agent(&mut self, cx: &mut Context<Self>) {
        if std::env::var_os("HANGAR_SETUP_DEMO_AGENT").is_none() || self.agent.is_some() || self.report.is_none() { return; }
        if let Some(first) = self.agents_ready.first().copied() { self.details_open = true; self.ask_agent(first, cx); }
    }

    /// "Fechar" do aviso da recuperação: com algo que não voltou (ou a pessoa não quis desfazer), a anotação é arquivada —
    /// senão o assistente abriria a cada início sem saída. Com um agente novo já chamado, a anotação é a dele: fica.
    pub(super) fn dismiss_recovered(&mut self, cx: &mut Context<Self>) {
        let why = self.recovered.take().and_then(|r| if r.offer.is_some() { Some("aviso fechado sem desfazer a pasta adiante") }
            else if !r.restored.errors.is_empty() { Some("aviso fechado com a pasta sem voltar toda") } else { None });
        if let Some(why) = why && self.agent.is_none() && let Some(dir) = run::state_dir() {
            // Até arquivar, `ask_agent` recusa: senão a anotação nova dele seria a arquivada.
            self.recovering = true;
            let task = cx.background_executor().spawn(async move { archive_annotation(&dir, why) });
            cx.spawn(async move |this, cx| {
                task.await;
                let _ = this.update(cx, |w, cx| { w.recovering = false; cx.notify(); });
            }).detach();
        }
        cx.notify();
    }

    pub(super) fn ask_agent(&mut self, agent: Agent, cx: &mut Context<Self>) {
        // Um agente por falha; nunca durante a recuperação de um conserto interrompido (ela apaga a anotação no fim).
        if self.agent.is_some() || self.refreshing || self.recovering || self.failure.is_none() {
            // Só o nome da fase: a de falha leva o texto livre do agente, que não vai ao diário.
            let phase = self.agent.as_ref().map(|run| match &run.phase {
                AgentPhase::Failed(_) => "Failed".to_owned(),
                other => format!("{other:?}"),
            });
            crate::log_line(&format!("setup: pedido de ajuda recusado (agente={phase:?} atualizando={} recuperando={} falha={})",
                self.refreshing, self.recovering, self.failure.is_some()));
            return;
        }
        let (Some(report), Some(state_dir)) = (self.report.clone(), run::state_dir()) else {
            crate::log_line(&format!("setup: pedido de ajuda recusado (relatório pronto={} pasta de estado={})",
                self.report.is_some(), run::state_dir().is_some()));
            return;
        };
        // O relatório desta falha já saiu por um botão da frase: o do conserto sai de novo, com o que o agente fez.
        if self.report_about.is_none() { self.report_about = self.failure.clone(); }
        self.agent_runs += 1;
        self.report_base = Some(report.clone());
        self.agent = Some(AgentRun::new(agent, self.agent_runs));
        suspend_updates(true, cx);
        let (dest, options, english, window) = (self.dest.clone(), self.options(), crate::i18n::english(), self.window);
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || { let _ = done.send(start_agent(agent, &report, &dest, &options, &state_dir, english)); });
        cx.spawn(async move |this, cx| {
            let mut started = Some(result.await.unwrap_or_else(|_| Err(tr("setup_agent_crashed"))));
            let output = match this.update(cx, |w, cx| started.take().and_then(|s| w.agent_started(s, cx))) {
                Ok(Some(output)) => output,
                Ok(None) => return,
                Err(_) => {
                    // O assistente sumiu enquanto ele subia: ninguém mais o acompanha.
                    if let Some(Ok(s)) = started { cx.background_executor().spawn(async move { abandon_started(s) }).detach(); }
                    return;
                }
            };
            while let Ok(item) = output.recv().await {
                let exited = matches!(item, agent::Output::Exit);
                if this.update(cx, |w, cx| w.agent_output(item, cx)).is_err() { return; }
                if exited { break; }
            }
            let Ok(Some((snapshot, done))) = this.update(cx, |w, cx| w.take_snapshot(cx)) else { return };
            let restored = cx.background_executor().spawn(async move {
                // Saiu sozinho: um filho dele ainda vivo no grupo escreveria na pasta depois de desfeita. Só com a
                // identidade conferida na largada o grupo é sabidamente dele.
                if let Some(pid) = snapshot.agent_pid.filter(|_| !snapshot.agent_started.is_empty()) { sweep_group(pid); }
                finish_restore(snapshot)
            }).await;
            // Fechado enquanto desfazia: quem fechou espera esta resposta para mandar o relatório com o diff.
            let _ = done.send(restored.clone());
            let _ = window.update(cx, |_, window, cx| this.update(cx, |w, cx| w.recheck_after_agent(restored, window, cx)));
        }).detach();
        cx.notify();
    }

    fn agent_started(&mut self, started: Result<Started, String>, cx: &mut Context<Self>) -> Option<async_channel::Receiver<agent::Output>> {
        match started {
            Ok(s) => {
                let Some(run) = self.agent.as_mut().filter(|run| run.phase == AgentPhase::Starting) else {
                    cx.background_executor().spawn(async move { abandon_started(s) }).detach();
                    return None;
                };
                let Started { snapshot, pid, started, output } = s;
                (run.snapshot, run.pid, run.started, run.phase) = (Some(snapshot), Some(pid), started, AgentPhase::Running);
                let number = run.number;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(agent::MAX_RUN).await;
                    let _ = this.update(cx, |w, cx| w.stop_agent(Some(number), cx));
                }).detach();
                cx.notify();
                Some(output)
            }
            Err(why) => {
                let run = self.agent.as_mut()?;
                run.phase = AgentPhase::Failed(why.clone());
                suspend_updates(false, cx);
                self.finish_agent(report::Recheck::NotRun(why), cx);
                None
            }
        }
    }

    fn agent_output(&mut self, item: agent::Output, cx: &mut Context<Self>) {
        let Some(run) = self.agent.as_mut() else { return };
        match item {
            agent::Output::Line(line) => { let agent = run.agent; run.transcript.feed(agent, &line); }
            agent::Output::Exit => { run.pid = None; run.phase = AgentPhase::Restoring; }
        }
        cx.notify();
    }

    /// `None` quando o `close` já levou a execução. O canal leva o resultado a quem fechar no meio.
    fn take_snapshot(&mut self, cx: &mut Context<Self>) -> Option<(repo::Snapshot, tokio::sync::oneshot::Sender<repo::Restored>)> {
        let run = self.agent.as_mut()?;
        run.phase = AgentPhase::Restoring;
        cx.notify();
        let snapshot = run.snapshot.take()?;
        let (done, wait) = tokio::sync::oneshot::channel();
        run.restore_done = Some(wait);
        Some((snapshot, done))
    }

    /// Reconfere rodando o script inteiro, como o "Tentar de novo": passou vai a "Tudo pronto", falhou volta ao painel.
    fn recheck_after_agent(&mut self, restored: repo::Restored, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.agent.as_mut() else { return };
        (run.restored, run.phase) = (Some(restored), AgentPhase::Rechecking);
        self.retry(window, cx);
    }

    /// "Parar" (`timer: None`) ou o teto de 20 min da execução `timer`; o `taskkill` do Windows roda fora da janela.
    pub(super) fn stop_agent(&mut self, timer: Option<u64>, cx: &mut Context<Self>) {
        let Some(run) = self.agent.as_mut() else { return };
        if run.phase != AgentPhase::Running || timer.is_some_and(|n| n != run.number) { return; }
        let Some(pid) = run.pid else { return };
        run.timed_out = timer.is_some();
        // Sem identidade (K2) nunca se mata por pid nu: ele segue rodando e a tela diz isso.
        if run.started.is_empty() {
            run.not_stopped = true;
            crate::log_line(&format!("assistente: agente {pid} sem identidade gravada; não foi parado"));
            return cx.notify();
        }
        let started = run.started.clone();
        cx.background_executor().spawn(async move { stop_agent_process(pid, &started) }).detach();
        cx.notify();
    }

    /// Fecha o conserto: o relatório final é o de antes mais a seção do agente, e sai "consertado ali" ou "aberto". O
    /// `.env` (senha do servidor) é lido fora da thread da janela; o envio sai quando o texto fica pronto.
    fn finish_agent(&mut self, recheck: report::Recheck, cx: &mut Context<Self>) {
        let mut secrets = std::mem::take(&mut self.report_secrets);
        secrets.extend(self.secret_values());
        let Some(run) = self.agent.as_mut() else { return };
        // Agente que terminou com erro (limite de turnos, API) não consertou, diga a explicação o que disser.
        let fixed = recheck == report::Recheck::Passed && !run.transcript.failed;
        if !matches!(run.phase, AgentPhase::Failed(_)) { run.phase = AgentPhase::Done { fixed }; }
        let restored = run.restored.clone().unwrap_or_default();
        let section = agent_section(run, &restored, recheck);
        let (agent, changed) = (run.agent, !restored.diff.is_empty());
        let (base, dest) = (self.report_base.take(), self.dest.clone());
        let task = cx.background_executor().spawn(async move {
            secrets.extend(local::read_install(&dest).token);
            base.map(|base| report::with_agent(&base, &section, &report::Secrets::here(secrets)))
        });
        let outcome = if fixed { Outcome::Consertado } else { Outcome::Aberto };
        // Se o assistente fechar antes do texto ficar pronto, o envio sai daqui mesmo, com a escolha da caixa de agora.
        let (fallback, runtime) = (self.report_about.clone().filter(|_| self.send), self.runtime.clone());
        cx.spawn(async move |this, cx| {
            let text = task.await;
            let delivered = this.update(cx, |w, cx| {
                if let Some(text) = text.clone() { w.report = Some(text); }
                let sent = w.send_report(outcome, Some(agent), cx);
                w.fix_sent = sent.filter(|_| !fixed && changed);
                cx.notify();
            }).is_ok();
            if delivered { return; }
            let Some(about) = fallback else { return };
            let Some(text) = text else {
                crate::log_line("assistente: relatório do conserto não enviado (assistente fechado antes de montar)");
                return;
            };
            let payload = report::payload(about.screen, about.code, outcome, Some(agent.id()), text);
            runtime.spawn(async move {
                let result = report::send(payload).await;
                crate::log_line(&format!("assistente: relatório do conserto enviado ao fechar: {}", result.err().unwrap_or_else(|| "ok".into())));
            });
        }).detach();
        cx.notify();
    }

    /// Fechar no meio do conserto (pelo `close` ou pela janela): pára o agente, desfaz a pasta e manda o relatório com o
    /// que houve. Tudo no runtime: a entidade some logo depois, e a janela não espera o `taskkill` nem o git.
    fn abandon_agent(&mut self) {
        let Some(mut run) = self.agent.take_if(|run| run.busy()) else { return };
        let mut secrets = std::mem::take(&mut self.report_secrets);
        secrets.extend(self.secret_values());
        let (base, dest) = (self.report_base.take(), self.dest.clone());
        let about = self.report_about.take().filter(|_| self.send);
        let restoring = run.restore_done.take();
        self.runtime.spawn(async move {
            // Desfazendo: espera a tarefa que desfaz (ela segue sem a entidade), e o diff dela vai no relatório.
            let restoring = match restoring { Some(wait) => Some(wait.await.unwrap_or_default()), None => None };
            let payload = tokio::task::spawn_blocking(move || {
                let restored = match restoring {
                    Some(restored) => restored,
                    None => stop_and_restore(run.pid, &run.started.clone(), run.snapshot.take()).0.unwrap_or_default(),
                };
                let (base, about) = (base?, about?);
                secrets.extend(local::read_install(&dest).token);
                let section = agent_section(&run, &restored, report::Recheck::NotRun(tr("setup_agent_closed")));
                let text = report::with_agent(&base, &section, &report::Secrets::here(secrets));
                Some(report::payload(about.screen, about.code, Outcome::Aberto, Some(run.agent.id()), text))
            }).await.ok().flatten();
            if let Some(payload) = payload {
                let result = report::send(payload).await;
                crate::log_line(&format!("assistente: relatório do conserto interrompido: {}", result.err().unwrap_or_else(|| "ok".into())));
            }
        });
    }

    fn apply_fix(&mut self, fix: Fix, window: &mut Window, cx: &mut Context<Self>) {
        // A roda do mouse só conta como ação depois de confirmada.
        if !fix.shows_help() && fix != Fix::FixMouseWheel { self.send_before_leaving(cx); }
        match fix {
            Fix::Retry | Fix::Recheck | Fix::AskAgain => self.retry(window, cx),
            Fix::HowToAllow | Fix::HowToStart => {
                let dest = self.dest.display().to_string();
                if let Some(failure) = self.failure.as_mut() {
                    failure.help = if failure.help.is_some() { None }
                        else { failure.code.as_deref().and_then(codes::help).map(|help| help.replace("{pasta}", &dest)) };
                }
                cx.notify();
            }
            Fix::OpenStore => cx.open_url(codes::STORE_URL),
            Fix::OpenTailscaleSettings => cx.open_url(super::screens::TAILSCALE_DNS),
            Fix::OpenDeveloperSettings => cx.open_url(codes::DEVELOPER_SETTINGS_URL),
            Fix::TailscaleLogin => {
                let url = self.runs.latest().and_then(|p| p.link("tailscale-login")).unwrap_or(codes::TAILSCALE_LOGIN_URL).to_owned();
                cx.open_url(&url);
            }
            Fix::UpdateApp => cx.open_url(if crate::i18n::english() { codes::DOWNLOAD_URL_EN } else { codes::DOWNLOAD_URL }),
            // Só aparece com outro agente escolhido (`codes::buttons`): segue sem o Claude Code, que é o que o script cobra.
            Fix::InstallLater => {
                self.agents.retain(|a| *a != "claude");
                self.agents_touched = true;
                self.retry(window, cx);
            }
            Fix::RefreshPackages => {
                self.after_password = Some(AfterPassword::RefreshPackages);
                let reason = tr("setup_sudo_reason").replace("{motivo}", &tr("setup_sudo_reason_packages"));
                self.open_password_prompt(reason, window, cx);
            }
            // Reiniciar o psmux fecha as sessões abertas: só com o sim da pessoa.
            Fix::FixMouseWheel => {
                let this = cx.entity().downgrade();
                chrome::confirm_alert(window, cx, tr("setup_fix_mouse_wheel"), tr("setup_fix_mouse_wheel_confirm"), tr("setup_continue"),
                    ButtonVariant::Danger, move |window, cx| {
                        let _ = this.update(cx, |w, cx| {
                            if w.is_running() || w.refreshing { return; }
                            w.fix_mouse_wheel = true;
                            w.retry(window, cx);
                        });
                        true
                    });
            }
        }
    }

    fn refresh_packages(&mut self, password: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing { return; }
        self.refreshing = true;
        cx.notify();
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || { let _ = done.send(precheck::refresh_package_list(&password)); });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = result.await.unwrap_or_else(|_| Err(String::new()));
            let _ = this.update_in(cx, |w, window, cx| {
                if !std::mem::take(&mut w.refreshing) { return; }
                // A pessoa já tentou de novo enquanto a lista atualizava: não derruba a execução nova.
                let Some(screen) = w.failure.as_ref().map(|f| f.screen) else { return };
                match outcome {
                    Ok(()) => w.retry(window, cx),
                    Err(why) => w.fail(Failure::app(Some("pacotes-desatualizados"), format!("{}\n{why}", tr("setup_packages_failed")), screen), cx),
                }
            });
        }).detach();
    }

    fn follow(&mut self, before: Screen, cx: &mut Context<Self>) {
        let next = flow::follow(self.viewing, before, self.frontier_now());
        if next != self.viewing { self.go(next, cx); } else { self.want_qr(cx); }
    }

    pub(super) fn go(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.viewing = screen;
        // A tela 5 pede o QR ao abrir, só depois do fim da instalação (o backend está de pé).
        self.want_qr(cx);
        cx.notify();
    }

    /// Só pede o código quando ele ainda não foi pedido: Failed e NoAddress esperam o botão da tela.
    fn want_qr(&mut self, cx: &mut Context<Self>) {
        if self.viewing == Screen::Phone && matches!(self.qr, Qr::Idle) && flow::status(Screen::Phone, &self.view()) != flow::Status::Pending { self.load_qr(cx); }
    }

    pub(super) fn load_qr(&mut self, cx: &mut Context<Self>) {
        if matches!(self.qr, Qr::Loading | Qr::Shown { .. }) { return; }
        let install = local::read_install(&self.dest);
        let Some(api) = install.token.as_ref().and_then(|token| crate::api::Api::new(&local::address(install.port), token).ok()) else {
            let env = local::env_file(std::env::home_dir().as_deref(), &self.dest).map_or_else(|e| e, |env| env.display().to_string());
            self.qr = Qr::Failed(tr("setup_connection_missing").replace("{path}", &env));
            return cx.notify();
        };
        self.qr = Qr::Loading;
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move { let _ = done.send(phone::load(api).await); });
        cx.spawn(async move |this, cx| {
            let loaded = result.await.unwrap_or_else(|_| Err(tr("connection_failed")));
            let _ = this.update(cx, |w, cx| {
                w.qr = match loaded {
                    Ok(Some(pairing)) => Qr::Shown { url: pairing.url, tailscale: pairing.tailscale,
                        image: Arc::new(Image::from_bytes(ImageFormat::Svg, pairing.svg)) },
                    Ok(None) => Qr::NoAddress,
                    Err(why) => Qr::Failed(why),
                };
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// Com o link de login na tela, confere a conta pelo `tailscale status`; o teto de 5 min é o do script.
    fn watch_tailscale(&mut self, cx: &mut Context<Self>) {
        if self.tailscale_running || self.tailscale_checking { return; }
        let waiting_login = self.runs.latest().and_then(|p| p.link("tailscale-login")).is_some()
            && self.runs.step(super::marks::Step::Tailscale) == Some(super::marks::State::Doing);
        if !waiting_login { return; }
        self.tailscale_checking = true;
        let task = cx.background_executor().spawn(async move { phone::tailscale_status() });
        cx.spawn(async move |this, cx| {
            let running = task.await;
            let _ = this.update(cx, |w, cx| {
                w.tailscale_checking = false;
                if running && !w.tailscale_running { w.tailscale_running = true; cx.notify(); }
            });
        }).detach();
    }

    pub(super) fn back(&mut self, cx: &mut Context<Self>) {
        let previous = flow::prev(&flow::screens(self.outside), self.viewing);
        self.go(previous, cx);
    }

    pub(super) fn press_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = flow::screens(self.outside);
        match flow::primary(self.viewing, self.started) {
            Primary::Start => self.start(window, cx),
            Primary::Continue => {
                let target = if self.viewing == Screen::Welcome { self.frontier_now() } else { flow::next(&list, self.viewing) };
                self.go(target, cx);
            }
            Primary::PhoneConnected => self.finish_phone(PhoneOutcome::Connected, cx),
            Primary::OpenHangar => self.open_hangar(window, cx),
        }
    }

    pub(super) fn finish_phone(&mut self, outcome: PhoneOutcome, cx: &mut Context<Self>) {
        self.phone = Some(outcome);
        self.go(Screen::Done, cx);
    }

    fn open_hangar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        run::clear_state();
        if std::mem::take(&mut self.connect_later) { self.connect_local(window, cx); }
        let _ = self.hangar.update(cx, |hangar, cx| hangar.close_setup(window, cx));
    }

    /// Com o script rodando a entidade só sai da tela (o canal da senha e a atualização suspensa continuam); sem nada
    /// rodando, larga tudo.
    pub(super) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Fechar no meio do conserto: pára o agente, desfaz a pasta e manda o relatório com o que houve.
        self.abandon_agent();
        self.send_before_leaving(cx);
        if self.is_running() {
            let _ = self.hangar.update(cx, |hangar, cx| hangar.hide_setup(window, cx));
            return;
        }
        // Sem nada rodando o fim (ou a falha) já foi visto: o `state.json` não pode reabrir o assistente a cada início.
        run::clear_state();
        // O que falhou ao enviar tenta uma última vez; a janela some, então o resultado vai só para o diário.
        for payload in self.outbox.failed() {
            self.runtime.spawn(async move {
                let result = report::send(payload).await;
                crate::log_line(&format!("assistente: relatório reenviado ao fechar: {}", result.err().unwrap_or_else(|| "ok".into())));
            });
        }
        suspend_updates(false, cx);
        let _ = self.hangar.update(cx, |hangar, cx| hangar.close_setup(window, cx));
    }

    fn on_askpass(&mut self, request: AskpassRequest, window: &mut Window, cx: &mut Context<Self>) {
        // `--retry`: o `sudo -S` do script recusou a senha guardada; ela sai da memória e a janela volta com o aviso.
        let retry = request.retry && self.vault.answer(&request.code) != askpass::Answer::Refuse;
        if retry { self.vault.reject(); }
        match self.vault.answer(&request.code) {
            askpass::Answer::Refuse => { let _ = request.reply.send(None); }
            askpass::Answer::Known(password) => { let _ = request.reply.send(Some(password)); }
            askpass::Answer::Ask => {
                // O motivo vem do script (`app_sudo`); sem ele, o item em andamento.
                let what = Some(request.prompt.trim().to_owned()).filter(|t| !t.is_empty())
                    .or_else(|| self.runs.latest().and_then(Progress::doing_item).map(|i| i.text.clone()).filter(|t| !t.is_empty()))
                    .unwrap_or_else(|| tr("setup_sudo_reason_generic"));
                self.waiting.push(request);
                self.open_password_prompt(tr("setup_sudo_reason").replace("{motivo}", &what), window, cx);
                if retry {
                    if let Some(prompt) = self.prompt.clone() {
                        prompt.update(cx, |p, cx| { p.error = Some(tr("setup_sudo_wrong")); cx.notify(); });
                    }
                }
            }
        }
    }

    fn open_password_prompt(&mut self, reason: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt.is_some() { return; }
        let input = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(tr("setup_sudo_field")));
        let wizard = cx.entity().downgrade();
        let prompt = cx.new(|_| PasswordPrompt { wizard: wizard.clone(), input: input.clone(), reason, error: None, checking: false });
        self.prompt = Some(prompt.clone());
        let (on_ok, on_close) = (wizard.clone(), wizard);
        window.open_dialog(cx, move |dialog, _, cx| {
            let checking = prompt.read(cx).checking;
            let (on_ok, on_close) = (on_ok.clone(), on_close.clone());
            popup::dialog(dialog).w(px(440.)).title(tr("setup_sudo_title")).child(prompt.clone())
                .keyboard(!checking).overlay_closable(false).close_button(!checking)
                // Enter confere a senha; o diálogo só fecha quando o sudo aceitar.
                .on_ok(move |_, window, cx| { let _ = on_ok.update(cx, |w, cx| w.submit_password(window, cx)); false })
                .on_close(move |_, window, cx| {
                    let _ = on_close.update(cx, |w, cx| if w.prompt.is_some() { w.cancel_password(window, cx) });
                })
        });
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn submit_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.clone() else { return };
        let (input, checking) = { let p = prompt.read(cx); (p.input.clone(), p.checking) };
        let password = input.read(cx).value().to_string();
        if password.is_empty() || checking { return; }
        prompt.update(cx, |p, cx| { (p.checking, p.error) = (true, None); cx.notify(); });
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || {
            let accepted = askpass::sudo_accepts(&password, &system::refreshed_path());
            let _ = done.send((accepted, password));
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((accepted, password)) = result.await else { return };
            let _ = this.update_in(cx, |w, window, cx| w.password_checked(accepted, password, window, cx));
        }).detach();
    }

    fn password_checked(&mut self, accepted: Result<askpass::SudoCheck, String>, password: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.clone() else { return };
        match accepted {
            Ok(askpass::SudoCheck::Denied(why)) => {
                // O pedido do script recebe a senha mesmo assim: o sudo dele recusa e o script dá `sem-sudo` com a causa.
                for request in self.waiting.drain(..) { let _ = request.reply.send(Some(password.clone())); }
                self.prompt = None;
                window.defer(cx, |window, cx| window.close_dialog(cx));
                if self.after_password.take().is_some() {
                    let screen = self.failure.as_ref().map_or(Screen::Prepare, |f| f.screen);
                    self.fail(Failure::app(Some("sem-sudo"), format!("{}\n{why}", tr("setup_code_sem_sudo")), screen), cx);
                }
                cx.notify();
            }
            Ok(askpass::SudoCheck::Accepted) => {
                let for_git = self.after_password == Some(AfterPassword::InstallGit);
                // A senha da lista de pacotes fica para o script que o `retry` roda logo depois.
                let for_packages = self.after_password == Some(AfterPassword::RefreshPackages);
                // O pedido do script espera 600 s: com o prazo vencido o canal já fechou, e a senha não entra no cofre.
                let delivered = std::mem::take(&mut self.waiting).into_iter()
                    .fold(false, |any, request| request.reply.send(Some(password.clone())).is_ok() || any);
                self.prompt = None;
                // Adiado: fechar agora chamaria o `on_close` dentro deste update do assistente.
                window.defer(cx, |window, cx| window.close_dialog(cx));
                if delivered || for_git || for_packages { self.vault.remember(password.clone()); }
                match self.after_password.take() {
                    Some(AfterPassword::InstallGit) => self.continue_start(window, cx),
                    Some(AfterPassword::RefreshPackages) => self.refresh_packages(password, window, cx),
                    None => {}
                }
                cx.notify();
            }
            Ok(askpass::SudoCheck::Wrong) => prompt.update(cx, |p, cx| { (p.checking, p.error) = (false, Some(tr("setup_sudo_wrong"))); cx.notify(); }),
            Err(why) => prompt.update(cx, |p, cx| { (p.checking, p.error) = (false, Some(why)); cx.notify(); }),
        }
    }

    fn cancel_password(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        for request in self.waiting.drain(..) { let _ = request.reply.send(None); }
        if self.after_password.take() == Some(AfterPassword::InstallGit) {
            self.fail(Failure::app(Some("senha-cancelada"), tr("setup_sudo_cancelled"), Screen::Prepare), cx);
        }
        cx.notify();
    }
}

/// O corpo da janela de senha: o motivo, o campo e os dois botões.
pub(crate) struct PasswordPrompt {
    wizard: WeakEntity<SetupWizard>,
    input: Entity<InputState>,
    reason: String,
    error: Option<String>,
    checking: bool,
}

impl Render for PasswordPrompt {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let submit = self.wizard.clone();
        div().flex().flex_col().gap_3()
            .child(div().text_sm().whitespace_normal().child(self.reason.clone()))
            .child(Input::new(&self.input).aria_label(tr("setup_sudo_field")))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("setup_sudo_kept")))
            .when(self.checking, |el| el.child(div().id("setup-sudo-checking").role(Role::Status).text_sm().text_color(theme::muted())
                .child(tr("setup_sudo_checking"))))
            .when_some(self.error.clone(), |el, error| el.child(div().id("setup-sudo-error").role(Role::Alert).text_sm()
                .text_color(theme::danger()).child(error)))
            .child(div().flex().justify_end().gap_2()
                .child(Button::new("setup-sudo-cancel").label(tr("setup_sudo_cancel")).disabled(self.checking)
                    .on_click(|_, window, cx| window.close_dialog(cx)))
                .child(Button::new("setup-sudo-ok").primary().label(tr("setup_sudo_ok")).disabled(self.checking)
                    .on_click(move |_, window, cx| { let _ = submit.update(cx, |w, cx| w.submit_password(window, cx)); })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob da gpui_kit (via `use super::*`) traz um `test` que colide com o atributo padrão.
    use core::prelude::v1::test;

    #[test]
    fn only_the_recorded_identity_is_ever_signalled() {
        let known = |id: &str| run::Identity::Known(id.to_owned());
        assert_eq!(stop_plan("100", &known("100")), StopPlan::Signal);
        // O pid virou outro processo (um shell do tmux depois de o app cair): nenhum sinal, nem ao grupo.
        assert_eq!(stop_plan("100", &known("200")), StopPlan::Gone);
        assert_eq!(stop_plan("100", &run::Identity::Gone), StopPlan::Gone);
        assert_eq!(stop_plan("100", &run::Identity::Unknown), StopPlan::Unverified);
        assert_eq!(stop_plan("", &known("")), StopPlan::Unverified);
        assert_eq!(stop_plan("", &known("100")), StopPlan::Unverified);
    }
}
