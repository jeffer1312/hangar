//! Contas Google do Antigravity (`/api/antigravity-contas`): a lista com o login e os Termos de cada conta, e o painel de
//! entrar, com o link do Google e o código colado. O Hangar nunca lê o login do agy: quem diz "logado" é o servidor, pelo
//! próprio agy, e os Termos do Google quem aceita é a pessoa, no terminal.
use super::*;

/// Uma conta como `GET /api/antigravity-contas` manda.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct AgyAccount {
    /// `default` ou o nome da pasta adicional: é ele que vai para as rotas.
    pub(super) id: String,
    #[serde(rename = "padrao", default)] pub(super) default: bool,
    /// O id da linha em `/api/credenciais` (`antigravity:<pasta>`): o elo entre as duas listas.
    #[serde(rename = "credencial", default)] pub(super) credential: String,
    /// `None`: o agy não respondeu a tempo; não é "sem login".
    #[serde(rename = "logado")] pub(super) signed_in: Option<bool>,
    #[serde(rename = "termos_aceitos", default)] pub(super) terms: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct AgyList {
    #[serde(rename = "instalado", default)] pub(super) installed: bool,
    #[serde(rename = "contas", default)] pub(super) accounts: Vec<AgyAccount>,
}

#[derive(Clone, Deserialize)]
struct Attempt {
    attempt_id: String,
    #[serde(rename = "estado")] state: String,
    link: Option<String>,
    #[serde(rename = "saida")] tail: Option<Vec<String>>,
}

/// O painel do Antigravity, no lugar da lista enquanto existe (como o do Codex). Só a conta padrão entra: o `agy` com
/// terminal ignora o `GEMINI_HOME` e grava o login de qualquer conta na pasta da padrão.
pub(super) struct AgyFlow {
    /// A conta do login (`default`, ou a da linha que pediu); `None` = ainda em "Qual conta".
    account: Option<String>,
    code: Entity<InputState>,
    has_code: bool,
    attempt: Option<Attempt>,
    /// O link da tentativa, só https e sem usuário.
    url: Option<String>,
    /// Abrindo o login.
    busy: bool,
    sending: bool,
    /// O código chegou ao agy: falta ele confirmar o login.
    sent: bool,
    cancelling: bool,
    /// A tentativa acabou sem login: o fim da saída do agy, que diz por quê.
    failed: Option<String>,
    /// A leitura periódica parou numa falha: o painel oferece ler de novo.
    read_failed: bool,
    error: Option<String>,
    copied: bool,
    turn: u64,
    task: Option<JoinHandle<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Drop for AgyFlow {
    // Tarefa do tokio não morre com o handle: sem isto a leitura seguiria depois do painel fechado.
    fn drop(&mut self) { if let Some(task) = self.task.take() { task.abort(); } }
}

impl AgyFlow {
    /// O servidor segura um processo `agy --login` enquanto a tentativa espera o código.
    fn open_attempt(&self) -> Option<(String, String)> {
        let attempt = self.attempt.as_ref().filter(|a| matches!(a.state.as_str(), "iniciando" | "aguardando_codigo"))?;
        Some((self.account.clone()?, attempt.attempt_id.clone()))
    }
}

pub(in crate::app) enum AgyReply {
    List(u64, Result<Value, Failure>),
    /// Tentativa começada ou lida; `true` = acabou de começar, a leitura periódica começa daqui.
    Attempt(u64, Result<Value, Failure>, bool),
    /// A resposta do código, pela tentativa a que ele foi.
    Code(String, Result<Value, Failure>),
    Cancelled(u64, Result<Value, Failure>),
}

/// O id da rota de uma linha de `/api/credenciais` sem a lista do Antigravity: o nome da pasta adicional, senão a padrão.
pub(super) fn route_id(path: Option<&str>) -> String {
    path.map(|p| p.trim_end_matches(['/', '\\'])).and_then(|p| p.rsplit(['/', '\\']).next())
        .and_then(|folder| folder.strip_prefix(".antigravity-")).filter(|name| !name.is_empty())
        .map_or_else(|| "default".to_owned(), str::to_owned)
}

/// As linhas do Antigravity ganham o login e os Termos lidos pela lista dele; o resto passa como veio.
pub(super) fn merge(list: &[Credential], agy: &[AgyAccount]) -> Vec<Credential> {
    list.iter().map(|c| {
        let mut c = c.clone();
        if c.kind == "antigravity" { c.agy = agy.iter().find(|a| a.credential == c.id).cloned(); }
        c
    }).collect()
}

/// Frase do código que as rotas do Antigravity devolvem; os outros seguem pela frase comum.
pub(super) fn agy_failure(error: &Failure) -> String {
    let key = match error.code.as_deref() {
        Some("erro_codigo_vazio") => "accounts_agy_code_empty",
        Some("erro_codigo_invalido") => "accounts_agy_code_invalid",
        Some("erro_codigo_ja_enviado") => "accounts_agy_code_sent",
        Some("erro_agy_ausente") => "accounts_agy_missing",
        Some("erro_nome_conta_invalido") => "accounts_agy_name_invalid",
        Some("erro_conta_existe") => "accounts_agy_name_taken",
        Some("erro_tentativa_inexistente") => "accounts_agy_attempt_lost",
        _ => return Hangar::setting_failure(error),
    };
    tr(key)
}

/// O logo do Antigravity no quadrado da cor dele.
pub(super) fn agy_logo(size: f32) -> Div {
    let (color, _) = theme::provider("antigravity");
    div().size(px(size)).flex_shrink_0().rounded(px(size * 0.28)).bg(color.opacity(0.16)).flex().items_center()
        .justify_center().children(chrome::provider_logo("antigravity", size * 0.56, color))
}

/// O topo dos painéis do Antigravity (login e Termos): logo, título e a máquina onde o agy roda.
pub(super) fn agy_panel_head(title: String, server: &str) -> Div {
    div().flex().items_center().gap(px(12.)).child(agy_logo(36.))
        .child(div().flex().flex_col().gap(px(2.)).min_w_0()
            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("accounts_login_server").replace("{server}", server))))
}

impl Hangar {
    fn agy_send(&self) -> impl Fn(AgyReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let done = self.accounts_send_later();
        move |reply| done(AccountsReply::Antigravity(reply))
    }

    /// Nova vez do painel: a tarefa da vez anterior para, e só respostas desta vez valem.
    fn agy_turn(&mut self) -> Option<(u64, Api)> {
        let api = self.api.clone()?;
        self.accounts.keys_seq += 1;
        let turn = self.accounts.keys_seq;
        let flow = self.accounts.antigravity.as_mut()?;
        if let Some(task) = flow.task.take() { task.abort(); }
        flow.turn = turn;
        Some((turn, api))
    }

    fn agy_flow(&mut self, turn: u64) -> Option<&mut AgyFlow> { self.accounts.antigravity.as_mut().filter(|f| f.turn == turn) }

    /// A lista do Antigravity roda o `agy models` em cada conta: só com a página de contas aberta.
    pub(super) fn load_agy_accounts(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.accounts.agy.start();
        let done = self.agy_send();
        self.runtime.spawn(async move { done(AgyReply::List(seq, api.server_read(&["antigravity-contas"], &[], 60).await)).await });
        cx.notify();
    }

    /// "Abrir no terminal" do painel dos Termos que falhou: abre uma sessão do agy com terminal na casa da máquina. A
    /// tela dos Termos chega ao chat como cartão, com as opções do agy, e só a escolha da pessoa vira tecla.
    pub(super) fn start_agy_terms(&mut self, home: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let connection = self.connection;
        let body = json!({"name": "agy-termos", "cwd": home, "provider": "antigravity", "headless": false});
        let task = self.runtime.spawn(async move { api.server_send(reqwest::Method::POST, &["sessions"], Some(body), 120).await });
        cx.spawn_in(window, async move |this, cx| {
            let joined = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let error = match joined {
                    Ok(Ok(value)) => match serde_json::from_value::<SessionInfo>(value) {
                        // Trocou de servidor no meio: o nome não é desta conexão e abriria outra sessão, ou nenhuma.
                        Ok(_) if this.connection != connection => tr("accounts_agy_terms_server_changed"),
                        Ok(session) => { this.select(session, window, cx); return }
                        Err(_) => tr("invalid_response"),
                    },
                    Ok(Err(failure)) => Hangar::failure(&failure),
                    Err(_) => tr("delivery_uncertain"),
                };
                window.push_notification(Notification::error(error), cx);
            });
        }).detach();
    }

    /// Abre o painel. Com `account` (o Entrar da linha) o login começa já; sem, mostra "Qual conta".
    pub(super) fn start_agy(&mut self, account: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() || self.api.is_none() { return; }
        let code = cx.new(|cx| InputState::new(window, cx).placeholder(tr("accounts_agy_code_placeholder")));
        let coded = cx.subscribe_in(&code, window, |this: &mut Hangar, input, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let filled = !input.read(cx).value().trim().is_empty();
                if let Some(flow) = this.accounts.antigravity.as_mut() { flow.has_code = filled; }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.agy_submit(cx),
            _ => {}
        });
        self.accounts.outcome = None;
        self.accounts.antigravity = Some(AgyFlow { account: None, code, has_code: false, attempt: None, url: None,
            busy: false, sending: false, sent: false, cancelling: false, failed: None, read_failed: false, error: None, copied: false, turn: 0, task: None,
            _subscriptions: vec![coded] });
        match account {
            Some(id) => self.agy_begin(id, cx),
            // "Qual conta" mostra se a padrão já tem login.
            None if !self.accounts.agy.loading && self.accounts.agy.ok().is_none() => self.load_agy_accounts(cx),
            None => {}
        }
        cx.notify();
    }

    /// Entrar: pede ao agy o login da conta.
    fn agy_begin(&mut self, id: String, cx: &mut Context<Self>) {
        if self.accounts.antigravity.as_ref().is_none_or(|f| f.busy || f.sending || f.cancelling || f.open_attempt().is_some()) { return; }
        let Some((turn, api)) = self.agy_turn() else { return };
        let done = self.agy_send();
        let Some(flow) = self.accounts.antigravity.as_mut() else { return };
        (flow.busy, flow.sending, flow.sent, flow.error, flow.failed, flow.read_failed, flow.attempt, flow.url, flow.copied) =
            (true, false, false, None, None, false, None, None, false);
        flow.account = Some(id.clone());
        flow.task = Some(self.runtime.spawn(async move {
            done(AgyReply::Attempt(turn, api.server_post(&["antigravity-contas", &id, "login"], 30).await, true)).await
        }));
        cx.notify();
    }

    /// Lê a tentativa a cada segundo: o link aparece e, depois do código, o fim do login.
    fn agy_poll(&mut self, cx: &mut Context<Self>) {
        let Some((id, attempt)) = self.accounts.antigravity.as_ref().and_then(AgyFlow::open_attempt) else { return };
        let Some((turn, api)) = self.agy_turn() else { return };
        let done = self.agy_send();
        let Some(flow) = self.accounts.antigravity.as_mut() else { return };
        (flow.error, flow.read_failed) = (None, false);
        flow.task = Some(self.runtime.spawn(async move {
            loop {
                let result = api.server_read(&["antigravity-contas", &id, "login"], &[("attempt_id", &attempt)], 10).await;
                let going = matches!(&result, Ok(value) if matches!(value.get("estado").and_then(Value::as_str), Some("iniciando" | "aguardando_codigo")));
                done(AgyReply::Attempt(turn, result, false)).await;
                if !going { break; }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }));
        cx.notify();
    }

    /// O código vai à tentativa de agora; a leitura periódica segue e é ela que vê o login acabar.
    fn agy_submit(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let done = self.agy_send();
        let Some(flow) = self.accounts.antigravity.as_mut().filter(|f| !f.sending && !f.sent && !f.cancelling) else { return };
        let Some((id, attempt)) = flow.open_attempt().filter(|_| flow.attempt.as_ref().is_some_and(|a| a.state == "aguardando_codigo")) else { return };
        let code = flow.code.read(cx).value().trim().to_owned();
        if code.is_empty() { return; }
        (flow.sending, flow.error) = (true, None);
        self.runtime.spawn(async move {
            let body = json!({"attempt_id": attempt, "codigo": code});
            done(AgyReply::Code(attempt, api.server_send(reqwest::Method::POST, &["antigravity-contas", &id, "login", "codigo"], Some(body), 30).await)).await
        });
        cx.notify();
    }

    /// Cancelar: fecha a tentativa aberta no servidor antes de sair; sem tentativa, só fecha o painel.
    fn agy_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = self.accounts.antigravity.as_ref().filter(|f| !f.busy && !f.cancelling) else { return };
        let Some((id, attempt)) = flow.open_attempt() else { return self.agy_close(window, cx) };
        let Some((turn, api)) = self.agy_turn() else { return };
        let done = self.agy_send();
        let Some(flow) = self.accounts.antigravity.as_mut() else { return };
        (flow.cancelling, flow.error) = (true, None);
        flow.task = Some(self.runtime.spawn(async move {
            done(AgyReply::Cancelled(turn, api.server_delete(&["antigravity-contas", &id, "login"], &[("attempt_id", &attempt)], 15).await)).await
        }));
        cx.notify();
    }

    /// Fechar: o painel sai e a lista relida mostra a conta como o servidor a vê.
    fn agy_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts.antigravity.as_ref().is_some_and(|f| f.busy || f.cancelling) { return; }
        self.accounts.antigravity = None;
        self.load_accounts(false, cx);
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    /// A página saiu com o painel aberto: a tentativa parada no servidor (um processo do agy esperando o código) fecha aqui.
    pub(super) fn agy_left(&mut self) {
        self.accounts.terms_after_login = false;
        self.agy_terms_close();
        let Some(flow) = self.accounts.antigravity.take() else { return };
        let (Some((id, attempt)), Some(api)) = (flow.open_attempt(), self.api.clone()) else { return };
        self.runtime.spawn(async move {
            if let Err(error) = api.server_delete(&["antigravity-contas", &id, "login"], &[("attempt_id", &attempt)], 15).await {
                eprintln!("login do Antigravity sem tela não foi fechado no servidor: {}", error.detail);
            }
        });
    }

    pub(super) fn receive_agy(&mut self, reply: AgyReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            AgyReply::List(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e))
                    .and_then(|v| serde_json::from_value::<AgyList>(v).map_err(|_| tr("invalid_response")));
                match parsed {
                    // Releitura que falhou com a anterior na tela: ela fica (a linha cai no que a lista de contas sabe).
                    Err(error) if self.accounts.agy.ok().is_some() => if seq == self.accounts.agy.seq {
                        eprintln!("[contas] lista do Antigravity não veio: {error}");
                        self.accounts.agy.loading = false;
                    },
                    parsed => { self.accounts.agy.finish(seq, parsed); }
                }
                // Login da padrão concluído com os Termos pendentes: o painel deles abre sozinho (D1).
                if seq == self.accounts.agy.seq && std::mem::take(&mut self.accounts.terms_after_login) && self.accounts.antigravity.is_none()
                    && self.accounts.agy.ok().is_some_and(|l| l.accounts.iter().any(|a| a.default && !a.terms)) {
                    self.open_agy_terms(cx);
                }
            }
            AgyReply::Attempt(turn, result, started) => {
                let Some(flow) = self.agy_flow(turn) else { return };
                flow.busy = false;
                let attempt = match result.map(serde_json::from_value::<Attempt>) {
                    Ok(Ok(attempt)) => attempt,
                    Ok(Err(_)) => { flow.error = Some(tr("invalid_response")); return; }
                    Err(error) => {
                        // O pedido do login falhou, ou o servidor não tem mais a tentativa (reiniciou, ou outra a trocou):
                        // esperar por ela não adianta. Leitura que só não veio deixa a tentativa e oferece ler de novo.
                        if started || error.status == Some(404) {
                            (flow.failed, flow.attempt, flow.sending, flow.sent) = (Some(String::new()), None, false, false);
                        } else { flow.read_failed = true; }
                        flow.error = Some(agy_failure(&error));
                        return;
                    }
                };
                flow.error = None;
                if let Some(url) = attempt.link.as_deref().and_then(|u| url::Url::parse(u).ok())
                    .filter(|u| u.scheme() == "https" && u.username().is_empty() && u.password().is_none()) {
                    flow.url = Some(url.to_string());
                }
                let state = attempt.state.clone();
                match state.as_str() {
                    "concluido" => {
                        let name = match flow.account.as_deref() { Some("default") | None => tr("accounts_agy_default_name"), Some(id) => id.to_owned() };
                        let default = flow.account.as_deref().is_none_or(|id| id == "default");
                        flow.task = None;
                        self.accounts.antigravity = None;
                        self.accounts.terms_after_login = default;
                        self.accounts.outcome = Some((tr("accounts_agy_connected").replace("{name}", &name), false));
                        self.load_accounts(true, cx);
                        self.root_focus.focus(window, cx);
                    }
                    "falhou" => {
                        (flow.failed, flow.attempt, flow.sending, flow.sent, flow.task) =
                            (Some(attempt.tail.clone().unwrap_or_default().join("\n")), None, false, false, None);
                    }
                    _ => {
                        flow.attempt = Some(attempt);
                        if started { self.agy_poll(cx); }
                    }
                }
            }
            AgyReply::Code(attempt, result) => {
                let Some(flow) = self.accounts.antigravity.as_mut().filter(|f| f.attempt.as_ref().is_some_and(|a| a.attempt_id == attempt)) else { return };
                flow.sending = false;
                match result {
                    Ok(_) => flow.sent = true,
                    // Sem resposta o código pode ter chegado: a leitura diz, e mandar de novo responde "já enviado".
                    Err(error) if error.uncertain => flow.error = Some(tr("accounts_agy_code_uncertain")),
                    Err(error) => flow.error = Some(agy_failure(&error)),
                }
            }
            AgyReply::Cancelled(turn, result) => {
                let Some(flow) = self.agy_flow(turn) else { return };
                (flow.cancelling, flow.task) = (false, None);
                match result {
                    Ok(_) => { flow.attempt = None; self.agy_close(window, cx); }
                    // Sem confirmação de que fechou, a tentativa continua valendo e os botões continuam lá.
                    Err(error) => flow.error = Some(if error.uncertain { tr("accounts_login_stop_uncertain") } else { agy_failure(&error) }),
                }
            }
        }
    }

    /// O painel no lugar da lista: "Qual conta", depois o link e o código, como no mock aprovado.
    pub(super) fn render_agy(&self, cx: &mut Context<Self>) -> Option<Div> {
        let f = self.accounts.antigravity.as_ref()?;
        let head = agy_panel_head(tr("accounts_add_antigravity"), &self.server_label(cx));
        let line = |text: String, color: Hsla| div().text_size(px(13.)).whitespace_normal().child(div().text_color(color).child(text));
        let footer = || div().mt(px(4.)).flex().justify_end().gap(px(8.));
        let terms = div().text_size(px(12.5)).whitespace_normal().child(div().text_color(theme::warning()).child(tr("accounts_agy_terms_note")));
        let cancel = Button::new("accounts-agy-cancel").outline().small()
            .label(tr(if f.cancelling { "accounts_agy_cancelling" } else { "cancel" })).disabled(f.busy || f.cancelling)
            .on_click(cx.listener(|this, _, window, cx| this.agy_cancel(window, cx)));
        let mut panel = settings_box().mt(px(24.)).p(px(20.)).gap(px(16.)).child(head);

        // Acabou sem login: o fim da saída do agy diz por quê.
        if let Some(tail) = &f.failed {
            let account = f.account.clone();
            return Some(panel.child(line(tr("accounts_agy_failed"), theme::danger()))
                .when(!tail.trim().is_empty(), |el| el.child(div().p(px(10.)).rounded(px(8.)).border_1().border_color(theme::border())
                    .bg(theme::inset()).font_family(theme::MONO).text_size(px(11.5)).text_color(theme::muted()).whitespace_normal().child(tail.clone())))
                .children(f.error.clone().map(|e| line(e, theme::danger())))
                .child(footer()
                    .child(Button::new("accounts-agy-back").outline().small().label(tr("settings_back"))
                        .on_click(cx.listener(|this, _, window, cx| this.agy_close(window, cx))))
                    .when_some(account, |el, id| el.child(Button::new("accounts-agy-retry").primary().small().label(tr("accounts_retry"))
                        .on_click(cx.listener(move |this, _, _, cx| this.agy_begin(id.clone(), cx)))))));
        }

        // A tentativa aberta: o link do Google e o código que a página do Antigravity mostra.
        if let Some(attempt) = f.attempt.as_ref() {
            let name = match f.account.as_deref() { Some("default") | None => tr("accounts_agy_default_name"), Some(id) => id.to_owned() };
            panel = panel.child(div().flex().items_center().gap(px(8.)).text_size(px(13.)).child(agy_logo(22.))
                .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(name)));
            if attempt.state == "iniciando" || f.url.is_none() {
                return Some(panel.child(div().flex().items_center().gap(px(8.)).text_size(px(13.)).text_color(theme::muted())
                        .child(chrome::Spinner::new("accounts-agy-starting", IconName::LoaderCircle, px(14.), theme::accent()))
                        .child(tr("accounts_agy_starting")))
                    .children(f.error.clone().map(|e| line(e, theme::danger())))
                    .child(footer().child(cancel)));
            }
            let number = |n: &'static str| div().size(px(22.)).flex_shrink_0().rounded_full().border_1().border_color(theme::border_strong())
                .flex().items_center().justify_center().text_size(px(12.)).text_color(theme::muted()).child(n);
            let step = |n: &'static str, title: &'static str, body: AnyElement| div().flex().gap(px(12.)).child(number(n))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(8.)).child(div().font_weight(FontWeight::MEDIUM).whitespace_normal().child(tr(title))).child(body));
            let url = f.url.clone().unwrap_or_default();
            let (open, copy) = (url.clone(), url.clone());
            let link = div().flex().flex_col().gap(px(8.))
                .child(div().p(px(8.)).rounded(px(8.)).border_1().border_color(theme::border()).bg(theme::inset())
                    .font_family(theme::MONO).text_size(px(11.)).text_color(theme::accent_text()).whitespace_normal().child(url))
                .child(div().flex().gap(px(8.))
                    .child(Button::new("accounts-agy-open").outline().small().icon(IconName::ExternalLink).label(tr("accounts_agy_open_link"))
                        .on_click(move |_, _, cx| cx.open_url(&open)))
                    .child(Button::new("accounts-agy-copy").outline().small().icon(IconName::Copy)
                        .label(tr(if f.copied { "accounts_login_copied" } else { "accounts_agy_copy" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            if let Some(flow) = this.accounts.antigravity.as_mut() { flow.copied = true; }
                            cx.notify();
                        }))));
            let locked = f.sending || f.sent || f.cancelling;
            let code = div().max_w(px(420.)).child(Input::new(&f.code).disabled(locked).aria_label(tr("accounts_agy_step_code")));
            let send = Button::new("accounts-agy-send").primary().small()
                .label(tr(if f.sent { "accounts_agy_checking" } else if f.sending { "accounts_agy_sending" } else { "accounts_agy_send" }))
                .disabled(locked || !f.has_code).on_click(cx.listener(|this, _, _, cx| this.agy_submit(cx)));
            return Some(panel.child(step("1", "accounts_agy_step_open", link.into_any_element()))
                .child(step("2", "accounts_agy_step_code", code.into_any_element()))
                .children(f.error.clone().map(|e| line(e, theme::danger())))
                .child(footer().when(f.read_failed, |el| el.child(Button::new("accounts-agy-reread").outline().small()
                        .label(tr("accounts_retry")).on_click(cx.listener(|this, _, _, cx| this.agy_poll(cx)))))
                    .child(cancel).child(send))
                .child(terms));
        }

        // "Qual conta": só a padrão, a que o agy já usa nesta máquina.
        let list = self.accounts.agy.ok();
        let default = list.and_then(|l| l.accounts.iter().find(|a| a.default));
        let missing = list.is_some_and(|l| !l.installed);
        let dot = |on: bool| div().size(px(8.)).flex_shrink_0().rounded_full().bg(if on { theme::success() } else { theme::faint() });
        let choice = |content: Div| div().px(px(12.)).py(px(10.)).rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
            .flex().items_center().gap(px(10.)).child(content);
        let default_signed = default.and_then(|a| a.signed_in) == Some(true);
        let default_row = choice(div().flex_1().min_w_0().flex().items_center().gap(px(8.)).child(dot(default_signed)).child(agy_logo(22.))
                .child(div().min_w_0().whitespace_normal().text_size(px(13.)).child(tr("accounts_agy_default_choice"))))
            .child(Button::new("accounts-agy-default").outline().small().label(tr("accounts_agy_sign_in")).disabled(f.busy || missing)
                .on_click(cx.listener(|this, _, _, cx| this.agy_begin("default".into(), cx))));
        Some(panel
            .child(div().flex().flex_col().gap(px(8.))
                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr("accounts_agy_which")))
                .child(default_row))
            .when(missing, |el| el.child(line(tr("accounts_agy_missing"), theme::warning())))
            .children(f.error.clone().map(|e| line(e, theme::danger())))
            .child(footer().child(cancel))
            .child(terms))
    }
}

#[cfg(test)]
mod tests {
    use super::{AgyList, route_id};
    use serde_json::json;

    #[test]
    fn route_id_is_the_extra_folder_name_or_the_default() {
        assert_eq!(route_id(Some("/home/a/.antigravity-trabalho")), "trabalho");
        assert_eq!(route_id(Some("/home/a/.antigravity-trabalho/")), "trabalho");
        assert_eq!(route_id(Some("C:\\Users\\a\\.antigravity-casa")), "casa");
        assert_eq!(route_id(Some("/home/a/.gemini")), "default");
        assert_eq!(route_id(Some("/home/a/.antigravity-")), "default");
        assert_eq!(route_id(None), "default");
    }

    #[test]
    fn the_server_list_reads_unknown_sign_in_as_none() {
        let list: AgyList = serde_json::from_value(json!({"instalado": true, "contas": [
            {"id": "default", "padrao": true, "pasta": "/h/.gemini", "credencial": "antigravity:/h/.gemini", "logado": null,
                "termos_aceitos": true, "em_uso": false},
            {"id": "trabalho", "padrao": false, "pasta": "/h/.antigravity-trabalho", "credencial": "antigravity:/h/.antigravity-trabalho",
                "logado": false, "termos_aceitos": false, "em_uso": false}]})).unwrap();
        assert!(list.installed);
        assert_eq!(list.accounts[0].signed_in, None);
        assert_eq!(list.accounts[1].signed_in, Some(false));
        assert!(!list.accounts[1].terms && list.accounts[1].credential == "antigravity:/h/.antigravity-trabalho");
    }
}
