//! Reserva pré-aquecida: com a tela de nova conversa pronta para um Claude sem terminal, o servidor mantém um processo já
//! subido que a criação adota (`PUT/DELETE /api/sessions/prewarm`, `"prewarm"` no `POST /api/sessions`). Falha não aparece
//! na tela: vai só ao log.
use super::*;

/// Espera depois da última troca de escolha antes de pedir a reserva.
const DEBOUNCE: Duration = Duration::from_millis(600);
/// O servidor solta a reserva sem renovação em 90 s.
const RENEW: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(200);

/// O pedido de reserva: a máquina e o corpo da criação sem nome.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Want { pub(super) machine: String, pub(super) body: Value }

#[derive(Debug, PartialEq)]
pub(super) enum Step { Idle, Put, Release }

/// Quando pedir, renovar e soltar; o relógio vem de fora, para o teste.
#[derive(Default)]
pub(super) struct Plan { want: Option<Want>, since: Option<Instant>, sent: Option<Instant>, held: Option<String> }

impl Plan {
    pub(super) fn step(&mut self, want: Option<Want>, now: Instant) -> Step {
        if want != self.want { (self.want, self.since, self.sent) = (want, Some(now), None); }
        // Sem pedido, ou o pedido em outra máquina: a reserva de lá não é trocada pelo PUT seguinte, sai antes.
        if self.held.is_some() && self.held.as_ref() != self.want.as_ref().map(|w| &w.machine) { self.held = None; return Step::Release; }
        let Some(want) = &self.want else { return Step::Idle };
        let due = match self.sent {
            None => self.since.is_some_and(|t| now.duration_since(t) >= DEBOUNCE),
            Some(t) => now.duration_since(t) >= RENEW,
        };
        if !due { return Step::Idle; }
        (self.sent, self.held) = (Some(now), Some(want.machine.clone()));
        Step::Put
    }

    /// A criação vai levar a reserva: a renovação para antes do POST, e depois dele o pedido volta a esperar a pausa.
    pub(super) fn stop(&mut self) { (self.want, self.sent) = (None, None); }
}

pub(super) struct Reserve {
    plan: Plan,
    client: String,
    runtime: Arc<Runtime>,
    /// A máquina que pode ter uma reserva deste cliente, mesmo depois de o diálogo trocar de máquina.
    held: Option<Api>,
    /// A última reserva confirmada (pedido, id, nome).
    got: Option<(Want, String, String)>,
    seq: u64,
    /// O diálogo fechou (a entidade pode durar um pouco mais).
    pub(super) closed: bool,
    /// O `Hangar` da tela sem sessão ou do diálogo: a janela na bandeja esconde os dois.
    pub(super) host: Option<WeakEntity<Hangar>>,
}

impl Reserve {
    pub(super) fn new(runtime: Arc<Runtime>) -> Self {
        let mut raw = [0u8; 16];
        // Sem aleatório, telas diferentes dividiriam o mesmo cliente e soltariam a reserva uma da outra.
        if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut raw).is_err() {
            crate::log_line("prewarm: sem número aleatório; cliente pelo pid e pela hora");
            let seed = format!("{}-{:?}", std::process::id(), std::time::SystemTime::now());
            raw.copy_from_slice(&ring::digest::digest(&ring::digest::SHA256, seed.as_bytes()).as_ref()[..16]);
        }
        let client = raw.iter().map(|b| format!("{b:02x}")).collect();
        Self { plan: Plan::default(), client, runtime, held: None, got: None, seq: 0, closed: false, host: None }
    }

    /// Solta a reserva deste cliente, inclusive a que ainda está subindo; a resposta de um PUT em voo deixa de valer.
    pub(super) fn release(&mut self) -> Option<JoinHandle<()>> {
        self.seq += 1;
        let id = self.got.take().map(|(_, id, _)| id).unwrap_or_else(|| "-".into());
        let api = self.held.take()?;
        let client = self.client.clone();
        Some(self.runtime.spawn(async move {
            if let Err(error) = api.server_send_query(reqwest::Method::DELETE, &["sessions", "prewarm", &id], &[("client_id", &client)], None, 5).await {
                crate::log_line(&format!("prewarm-soltar: {}", error.detail));
            }
        }))
    }

    /// O id e o nome da reserva confirmada, se ela é deste mesmo pedido.
    pub(super) fn adoptable(&self, want: &Want) -> Option<(String, String)> {
        self.got.as_ref().filter(|(w, ..)| w == want).map(|(_, id, name)| (id.clone(), name.clone()))
    }

    /// A reserva foi (ou não) neste POST: a repetição não reenvia um id já adotado nem força o nome dele, e a resposta de
    /// um PUT em voo deixa de valer. A próxima confirmação chega depois da pausa.
    pub(super) fn stop(&mut self) { self.plan.stop(); self.got = None; self.seq += 1; }
}

impl Drop for Reserve {
    // Toda saída da tela que solta a entidade (troca de máquina, criação concluída, conexão nova) solta a reserva.
    fn drop(&mut self) { drop(self.release()); }
}

impl NewSession {
    pub(super) fn start_prewarm(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if this.update(cx, |this, cx| this.prewarm_tick(cx)).is_err() { break; }
        }).detach();
        let quit = cx.on_app_quit(|this, _| {
            let task = this.prewarm.release();
            async move { if let Some(task) = task { let _ = task.await; } }
        });
        self._subscriptions.push(quit);
    }

    /// O pedido de reserva das escolhas atuais, ou `None` quando elas não admitem reserva (só Claude sem terminal, sem motor e
    /// sem worktree a criar).
    pub(super) fn prewarm_want(&self, cx: &App) -> Option<Want> {
        if self.scheduled || self.provider != "claude" || !self.headless || !self.engine.is_empty() || self.baton.is_some() || self.is_transfer()
            || self.new_branch || !self.branch.is_empty() || self.target().is_some() { return None; }
        let picked = self.picked.clone()?;
        let cwd = self.existing.clone().unwrap_or(picked);
        let mut body = self.open_body("", &cwd, "", cx);
        body.as_object_mut()?.remove("name");
        Some(Want { machine: self.link.api.identity(), body })
    }

    fn prewarm_shown(&self, cx: &mut Context<Self>) -> bool {
        if self.prewarm.closed { return false; }
        let Some(host) = &self.prewarm.host else { return true };
        let me = cx.entity_id();
        // Só a bandeja conta como escondida: perder o foco não (a pessoa volta da outra janela), e minimizar não se distingue
        // de estar coberta.
        host.upgrade().is_some_and(|host| {
            let host = host.read(cx);
            !host.window_tray.hidden && (!self.compact
                || host.new_chat_screen() && host.reopen.is_none() && host.new_chat.as_ref().is_some_and(|view| view.entity_id() == me))
        })
    }

    fn prewarm_tick(&mut self, cx: &mut Context<Self>) {
        // A criação em voo é dona da reserva: nem renova nem solta.
        if self.creating { return; }
        let want = if !self.prewarm_shown(cx) { None } else if !self.can_create(cx) { return } else { self.prewarm_want(cx) };
        match self.prewarm.plan.step(want, Instant::now()) {
            Step::Idle => {}
            Step::Release => drop(self.prewarm.release()),
            Step::Put => self.prewarm_put(cx),
        }
    }

    fn prewarm_put(&mut self, cx: &mut Context<Self>) {
        let Some(want) = self.prewarm.plan.want.clone() else { return };
        let mut body = want.body.clone();
        body["client_id"] = json!(self.prewarm.client);
        self.prewarm.seq += 1;
        let seq = self.prewarm.seq;
        let api = self.link.api.clone();
        self.prewarm.held = Some(api.clone());
        let task = self.link.runtime.spawn(async move { api.server_send(reqwest::Method::PUT, &["sessions", "prewarm"], Some(body), 30).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = task.await else { return };
            let _ = this.update(cx, |this, _| {
                if seq != this.prewarm.seq { return; }
                let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
                this.prewarm.got = match result {
                    Ok(value) => text(&value, "id").zip(text(&value, "name")).map(|(id, name)| (want, id, name)),
                    Err(error) => { crate::log_line(&format!("prewarm-pedir: {}", error.detail)); None }
                };
            });
        }).detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn want(machine: &str, cwd: &str) -> Option<Want> { Some(Want { machine: machine.into(), body: json!({"cwd": cwd}) }) }

    #[test]
    fn asks_only_after_the_choices_stay_still() {
        let (mut plan, t) = (Plan::default(), Instant::now());
        assert_eq!(plan.step(want("m", "/a"), t), Step::Idle);
        assert_eq!(plan.step(want("m", "/a"), t + Duration::from_millis(599)), Step::Idle);
        // Outra pasta recomeça a espera.
        assert_eq!(plan.step(want("m", "/b"), t + Duration::from_millis(500)), Step::Idle);
        assert_eq!(plan.step(want("m", "/b"), t + Duration::from_millis(1099)), Step::Idle);
        assert_eq!(plan.step(want("m", "/b"), t + Duration::from_millis(1100)), Step::Put);
        assert_eq!(plan.step(want("m", "/b"), t + Duration::from_millis(1300)), Step::Idle);
    }

    #[test]
    fn renews_every_30_seconds() {
        let (mut plan, t) = (Plan::default(), Instant::now());
        plan.step(want("m", "/a"), t);
        assert_eq!(plan.step(want("m", "/a"), t + DEBOUNCE), Step::Put);
        assert_eq!(plan.step(want("m", "/a"), t + DEBOUNCE + Duration::from_millis(29_999)), Step::Idle);
        assert_eq!(plan.step(want("m", "/a"), t + DEBOUNCE + RENEW), Step::Put);
    }

    #[test]
    fn hiding_the_screen_releases_the_reserve() {
        let (mut plan, t) = (Plan::default(), Instant::now());
        plan.step(want("m", "/a"), t);
        assert_eq!(plan.step(want("m", "/a"), t + DEBOUNCE), Step::Put);
        assert_eq!(plan.step(None, t + DEBOUNCE * 2), Step::Release);
        assert_eq!(plan.step(None, t + DEBOUNCE * 3), Step::Idle);
    }

    #[test]
    fn nothing_to_release_when_nothing_was_asked() {
        let (mut plan, t) = (Plan::default(), Instant::now());
        plan.step(want("m", "/a"), t);
        assert_eq!(plan.step(None, t + Duration::from_millis(100)), Step::Idle);
    }

    #[test]
    fn other_machine_releases_the_old_then_waits() {
        let (mut plan, t) = (Plan::default(), Instant::now());
        plan.step(want("m1", "/a"), t);
        assert_eq!(plan.step(want("m1", "/a"), t + DEBOUNCE), Step::Put);
        assert_eq!(plan.step(want("m2", "/a"), t + DEBOUNCE * 2), Step::Release);
        assert_eq!(plan.step(want("m2", "/a"), t + DEBOUNCE * 2 + Duration::from_millis(100)), Step::Idle);
        assert_eq!(plan.step(want("m2", "/a"), t + DEBOUNCE * 3), Step::Put);
    }

    #[test]
    fn stop_before_create_holds_the_renewal_and_keeps_the_reserve() {
        let (mut plan, t) = (Plan::default(), Instant::now());
        plan.step(want("m", "/a"), t);
        assert_eq!(plan.step(want("m", "/a"), t + DEBOUNCE), Step::Put);
        plan.stop();
        // A criação falhou e a tela continua: pede de novo só depois da pausa, sem soltar a da mesma máquina.
        let back = t + RENEW * 2;
        assert_eq!(plan.step(want("m", "/a"), back), Step::Idle);
        assert_eq!(plan.step(want("m", "/a"), back + DEBOUNCE), Step::Put);
    }

    #[test]
    fn stop_forgets_the_ticket_so_a_retry_goes_without_it() {
        let mut reserve = Reserve::new(Arc::new(Runtime::new().unwrap()));
        let asked = want("m", "/a").unwrap();
        reserve.got = Some((asked.clone(), "k1".into(), "reserva".into()));
        assert_eq!(reserve.adoptable(&asked), Some(("k1".into(), "reserva".into())));
        let seq = reserve.seq;
        reserve.stop();
        assert_eq!(reserve.adoptable(&asked), None);
        // A resposta de um PUT pedido antes não repõe o id já levado.
        assert_ne!(reserve.seq, seq);
    }
}
