use crate::mods_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use hangar_server::mods::click::Limits;
use hangar_server::mods::model::*;
use hangar_server::mods::state::*;
use hangar_server::mods::terminal::TerminalLink;
use mods_support::pane::{Effect::*, FakePane, view};

#[tokio::test]
async fn the_link_takes_the_app_requests_to_the_click() {
    let mods = Mods::default();
    let pane = Arc::new(FakePane::new(&mods, "t", "tmux-01-tres-paineis-150"));
    let link = TerminalLink::new("t".into(), 1, pane.clone(), mods.clone(), Limits::quick());
    mods.attach_terminal("t", "proc-t", 1, link.clone());
    mods.terminal_ui("t", view(&[("pm-mock-pm", "xx-00000", "pm-a", "xxxxx"), ("pm-mock-mr", "MR ●2", "mr-a", "xxxxx"),
        ("pm-mock-jenkins", "Jenkins", "jenkins-a", "xxxxx")]));
    pane.on_click((0, 104), vec![Show("tmux-02-apos-clicar-mr-150")]);
    let Turn { link: surface, .. } = mods.link("t").unwrap();
    let deadline = || Instant::now() + Duration::from_secs(5);
    assert_eq!(surface.call(ModsCall::Show { site: "pm-mock-mr".into() }, deadline()).await.unwrap()["shown_id"], "pm-mock-mr");
    // A resposta sai antes da limpeza: a reserva do pane é solta logo depois.
    tokio::time::timeout(Duration::from_secs(5), async { while pane.held() { tokio::time::sleep(Duration::from_millis(5)).await; } })
        .await.expect("reserva do pane solta");
    let typing = surface.call(ModsCall::Input { site: "pm-mock-mr".into(), plugin: "vitrine".into(), key: "k".into(), submit: true, value: "x".into() }, deadline()).await;
    assert_eq!(typing.unwrap_err().code, "erro_mod_sem_digitacao");
    assert_eq!(link.read_shown().await.as_deref(), Some("pm-mock-mr"));
}

#[tokio::test]
async fn a_request_without_time_does_nothing_in_the_mod() {
    let mods = Mods::default();
    let pane = Arc::new(FakePane::new(&mods, "t", "tmux-01-tres-paineis-150"));
    let link = TerminalLink::new("t".into(), 1, pane.clone(), mods.clone(), Limits::quick());
    mods.attach_terminal("t", "proc-t", 1, link.clone());
    mods.terminal_ui("t", view(&[("pm-mock-pm", "xx-00000", "pm-a", "xxxxx"), ("pm-mock-mr", "MR ●2", "mr-a", "xxxxx")]));
    let result = link.call(ModsCall::Show { site: "pm-mock-mr".into() }, Instant::now() + Duration::from_millis(300)).await;
    assert_eq!(result.unwrap_err().code, "erro_mod_clique_sem_resposta");
    assert!(pane.actions().is_empty());
}

/// O terminal que se desliga no meio de um clique não avisa de novo: a reposição do mínimo espera a vez do
/// clique, em vez de desistir, e roda depois da limpeza dele.
#[tokio::test]
async fn a_terminal_leaving_during_a_click_gets_the_floor_after_it() {
    let mods = Mods::default();
    let pane = Arc::new(FakePane::new(&mods, "t", "tmux-01-tres-paineis-150"));
    let link = TerminalLink::new("t".into(), 1, pane.clone(), mods.clone(), Limits::quick());
    mods.attach_terminal("t", "proc-t", 1, link.clone());
    mods.terminal_ui("t", view(&[("pm-mock-pm", "xx-00000", "pm-a", "xxxxx"), ("pm-mock-mr", "MR ●2", "mr-a", "xxxxx"),
        ("pm-mock-jenkins", "Jenkins", "jenkins-a", "xxxxx")]));
    pane.stall_on("click 0 104");
    let call = tokio::spawn(link.call(ModsCall::Show { site: "pm-mock-mr".into() }, Instant::now() + Duration::from_secs(1)));
    tokio::time::timeout(Duration::from_secs(5), async { while pane.actions().is_empty() { tokio::time::sleep(Duration::from_millis(5)).await; } })
        .await.expect("o clique chegou ao pane");
    // O terminal sai com o clique em curso e deixa a janela abaixo do mínimo; o aviso chega ao vigia.
    pane.clients(0);
    pane.queue(&["tmux-100-caixa-100"]);
    let floor = tokio::spawn({ let link = link.clone(); async move { link.floor().await } });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!floor.is_finished(), "a reposição espera a vez do clique");
    assert_eq!(pane.actions(), ["click 0 104"]);
    assert_eq!(call.await.unwrap().unwrap_err().code, "erro_mod_clique_sem_resposta");
    tokio::time::timeout(Duration::from_secs(5), floor).await.expect("a reposição terminou").unwrap();
    assert_eq!(pane.actions(), ["click 0 104", "resize 144 45"]);
    assert!(!pane.held());
}

/// O elo de uma vida substituída não lê o espelho da sessão que reabriu com o mesmo nome.
#[tokio::test]
async fn the_link_of_a_replaced_life_reads_nothing() {
    let mods = Mods::default();
    let pane = Arc::new(FakePane::new(&mods, "t", "tmux-02-apos-clicar-mr-150"));
    let old = TerminalLink::new("t".into(), 1, pane.clone(), mods.clone(), Limits::quick());
    mods.attach_terminal("t", "proc-t", 1, old.clone());
    let new = TerminalLink::new("t".into(), 2, pane.clone(), mods.clone(), Limits::quick());
    mods.attach_terminal("t", "proc-t", 2, new.clone());
    mods.terminal_ui("t", view(&[("pm-mock-pm", "xx-00000", "pm-a", "xxxxx"), ("pm-mock-mr", "MR ●2", "mr-a", "xxxxx"),
        ("pm-mock-jenkins", "Jenkins", "jenkins-a", "xxxxx")]));
    assert_eq!(new.read_shown().await.as_deref(), Some("pm-mock-mr"));
    assert_eq!(old.read_shown().await, None);
}

/// O vigia sobe um cliente de controle de verdade: só onde há tmux.
#[cfg(unix)]
mod tmux {
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use hangar_server::mods::click::{Limits, Pane, PaneFuture, PaneOp, PaneReply};
    use hangar_server::mods::model::pane_failed;
    use hangar_server::mods::state::Mods;
    use hangar_server::mods::terminal::TerminalLink;
    use hangar_server::terminal_input::*;

    struct Facts;
    impl TerminalServices for Facts {
        fn facts<'a>(&'a self, _: &'a TerminalBinding) -> ServiceFuture<'a, InputFacts> { Box::pin(async { Err(ServiceError("sem fatos")) }) }
        fn publish<'a>(&'a self, _: &'a TerminalBinding, _: PluginRequest) -> ServiceFuture<'a, PluginReply> { Box::pin(async { Ok(PluginReply::Unavailable) }) }
    }

    /// O pane de verdade, sem o executor: só para o vigia, que lê e redimensiona.
    struct DriverPane(Arc<TerminalDriver>);
    impl Pane for DriverPane {
        fn op(&self, op: PaneOp, _: Instant) -> PaneFuture {
            let driver = self.0.clone();
            Box::pin(async move {
                let failed = |e: IoFailure| pane_failed(e.code);
                Ok(match op {
                    PaneOp::Formats => PaneReply::Formats(driver.mods_formats().await.map_err(failed)?),
                    PaneOp::Clients => PaneReply::Clients(driver.mods_clients().await.map_err(failed)?),
                    PaneOp::Screen => PaneReply::Screen(driver.mods_screen().await.map_err(failed)?),
                    PaneOp::Resize { columns, rows } => { driver.resize(columns, rows).await.map_err(failed)?; PaneReply::Done }
                    _ => return Err(pane_failed("fora do teste")),
                })
            })
        }
    }

    struct IsolatedMux(String);
    impl Drop for IsolatedMux {
        fn drop(&mut self) { let _ = std::process::Command::new("tmux").args(["-L", &self.0, "kill-server"]).status(); }
    }

    const CLIENT: &str = r#"import os, pty, sys, fcntl, termios, struct, signal
pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.execvp("tmux", ["tmux", "-L", sys.argv[1], "attach", "-t", "=test"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
os.kill(pid, signal.SIGWINCH)
while True:
    try:
        os.read(fd, 65536)
    except OSError:
        break
"#;

    fn window(label: &str) -> String {
        let out = std::process::Command::new("tmux").args(["-L", label, "display-message", "-p", "-t", "=test:", "#{window_width}x#{window_height}"]).output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    async fn until(label: &str, size: &str) -> bool {
        for _ in 0..150 {
            if window(label) == size { return true; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn the_watch_restores_the_floor_when_the_last_terminal_leaves() {
        let label = format!("hangar-mods-watch-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
        let _guard = IsolatedMux(label.clone());
        // `-f /dev/null`: sem o `~/.tmux.conf` da máquina, a linha de status fica ligada e o terminal de 100x30
        // deixa a janela em 100x29.
        assert!(std::process::Command::new("tmux").args(["-L", &label, "-f", "/dev/null", "new-session", "-d", "-s", "test", "-x", "200", "-y", "50", "sleep 600"]).status().unwrap().success());
        let meta = std::process::Command::new("tmux").args(["-L", &label, "display-message", "-p", "-t", "=test:", "#{pane_id}\t#{session_created}"]).output().unwrap();
        let meta = String::from_utf8(meta.stdout).unwrap();
        let mut fields = meta.trim().split('\t');
        let binding = TerminalBinding { name: "test".into(), pane: fields.next().unwrap().into(), conversation: "c".into(), generation: 1,
            created: fields.next().unwrap().parse().unwrap(), mux_argv: vec!["tmux".into(), "-L".into(), label.clone()], windows: false, clipboard_lock_path: None };
        let driver = Arc::new(TerminalDriver::new(binding.clone(), Arc::new(Facts), Arc::new(ProcessIo::default()), InputLimits::default()));
        let mods = Mods::default();
        let link = TerminalLink::new("test".into(), 1, Arc::new(DriverPane(driver)), mods.clone(), Limits::default());
        mods.attach_terminal("test", "proc-test", 1, link.clone());
        link.watch(&binding.mux_argv);
        let mut client = std::process::Command::new("python3").arg("-c").arg(CLIENT).arg(&label).spawn().unwrap();
        assert!(until(&label, "100x29").await, "a janela segue o terminal ligado: {}", window(&label));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(window(&label), "100x29", "com terminal ligado o Hangar não redimensiona");
        let _ = client.kill();
        let _ = client.wait();
        assert!(until(&label, "144x40").await, "sem terminal ligado volta o mínimo: {}", window(&label));
        assert_eq!(clients(&label), "1", "só o cliente de controle do vigia segue ligado");
        // A sessão sai do Rust: o vigia para e o cliente de controle dele sai junto.
        mods.forget("test", 1);
        let mut gone = false;
        for _ in 0..150 {
            if clients(&label) == "0" { gone = true; break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(gone, "o cliente de controle do vigia ficou ligado: {}", clients(&label));
    }

    fn clients(label: &str) -> String {
        let out = std::process::Command::new("tmux").args(["-L", label, "display-message", "-p", "-t", "=test:", "#{session_attached}"]).output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
}
