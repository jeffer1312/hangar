//! Contas Claude: entrar (e renovar o login), sair, remover e adicionar. Cada escrita vai ao servidor uma vez e não
//! se repete sozinha: queda depois de enviar é resultado incerto, dito assim, e a lista relida mostra o que valeu.
//! Uma ação em curso segura os controles das outras, para que a resposta sempre ache quem a pediu.
use super::*;
use gpui_kit::component::WindowExt;
use tokio::sync::oneshot;

/// Tentativa de login numa conta; enquanto existe, a página mostra os passos no lugar da lista, como no web.
pub(super) struct SignIn {
    /// Nome no disco: é ele que vai para as rotas, nunca o apelido.
    label: String,
    name: String,
    /// Número da tentativa: resposta de outra tentativa não mexe nesta.
    attempt: u64,
    /// O servidor tem (ou pode ter, depois de um início incerto) uma tentativa aberta nesta conta.
    open: bool,
    starting: bool,
    sending: bool,
    stopping: bool,
    /// A tentativa acabou sem login: sobram Voltar e Tentar de novo.
    failed: bool,
    url: Option<String>,
    copied: bool,
    step_failed: bool,
    error: Option<String>,
    /// Conectada: e-mail e plano que o servidor confirmou.
    done: Option<(Option<String>, Option<String>)>,
    code: Entity<InputState>,
    has_code: bool,
    poll: Option<JoinHandle<()>>,
    _subscription: Subscription,
}

impl Drop for SignIn {
    // Tarefa do tokio não morre com o handle: sem isto a leitura do passo seguiria no servidor anterior.
    fn drop(&mut self) { if let Some(poll) = self.poll.take() { poll.abort(); } }
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::app) enum ChangeKind { SignOut, Remove }

/// Sair ou remover em voo.
pub(super) struct Change { pub(super) id: String, pub(super) kind: ChangeKind }

/// Começar e confirmar levam de volta se a tentativa ainda tinha dono na tela. Sem dono (página fechada, ou a
/// resposta de uma conexão antiga descartada no caminho, que derruba o remetente), a própria tarefa fecha a
/// tentativa no servidor dela.
pub(in crate::app) enum ActionReply {
    Started(u64, Result<Value, Failure>, oneshot::Sender<bool>),
    Step(u64, Result<Value, Failure>),
    Code(u64, Result<Value, Failure>, oneshot::Sender<bool>),
    Stopped(u64, bool, Result<Value, Failure>),
    Changed(String, String, ChangeKind, Result<Value, Failure>),
    Created(String, Result<Value, Failure>),
}

/// `Catalog(true)`: provedores para um modelo do Claude Code; `Catalog(false)`: para uma chave de outro agente.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum AddStep { Choose, Subscriptions, Claude, Catalog(bool) }

/// O diálogo "Adicionar conta…". É uma entidade própria porque o diálogo é desenhado durante o desenho da janela,
/// quando o estado do `Hangar` não pode ser lido.
pub(super) struct AddAccount {
    hangar: WeakEntity<Hangar>,
    step: AddStep,
    name: Entity<InputState>,
    slug: String,
    /// Conta padrão sem login: uma conta nova não resolve, o login vai para ela (como no web).
    base: Option<(String, String)>,
    saving: bool,
    error: Option<String>,
    _subscription: Subscription,
}

/// Uma escrita de login por vez por servidor e conta, também entre conexões: a limpeza de uma tentativa antiga
/// nunca alcança a tentativa nova da mesma conta (ida e volta A → B → A).
// ponytail: o mapa só cresce, uma entrada por conta que já tentou entrar; limpar quando o número incomodar.
fn login_turn(api: &Api, label: &str) -> Arc<tokio::sync::Mutex<()>> {
    static TURNS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = std::sync::OnceLock::new();
    TURNS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
        .entry(format!("{}\n{label}", api.identity())).or_default().clone()
}

/// Fecha a tentativa sem tela. É o único resultado que não aparece (não há tela onde mostrá-lo): a falha vai
/// para o log do app.
async fn cancel_unseen(api: &Api, label: &str) {
    if let Err(error) = api.server_post(&["conta-estado", label, "login", "cancelar"], 15).await {
        eprintln!("login de {label} sem tela não foi fechado no servidor: {}", error.detail);
    }
}

/// Nome da pasta como o servidor aceita (`[a-z0-9][a-z0-9_-]{0,31}`): minúsculas, sem acento, o resto vira hífen.
pub(super) fn account_slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.trim().to_lowercase().chars() {
        let ch = match ch {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a', 'é' | 'è' | 'ê' | 'ë' => 'e', 'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o', 'ú' | 'ù' | 'û' | 'ü' => 'u', 'ç' => 'c', 'ñ' => 'n', ch => ch,
        };
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' { out.push(ch); } else if !out.ends_with('-') { out.push('-'); }
    }
    out.trim_matches('-').chars().take(32).collect::<String>().trim_end_matches('-').to_owned()
}

impl AddAccount {
    fn new(hangar: WeakEntity<Hangar>, step: AddStep, base: Option<(String, String)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(tr("accounts_add_name_placeholder")));
        let subscription = cx.subscribe_in(&name, window, |this: &mut AddAccount, input, event: &InputEvent, window, cx| match event {
            InputEvent::Change => { this.slug = account_slug(&input.read(cx).value()); cx.notify(); }
            InputEvent::PressEnter { .. } => this.create(window, cx),
            _ => {}
        });
        Self { hangar, step, name, slug: String::new(), base, saving: false, error: None, _subscription: subscription }
    }

    fn title(&self) -> String {
        tr(match self.step {
            AddStep::Choose => "accounts_add_title", AddStep::Subscriptions => "accounts_add_subscription", AddStep::Claude => "accounts_add_claude",
            AddStep::Catalog(true) => "accounts_add_model_path", AddStep::Catalog(false) => "accounts_add_key_path",
        })
    }

    fn go(&mut self, step: AddStep, window: &mut Window, cx: &mut Context<Self>) {
        (self.step, self.error) = (step, None);
        if step == AddStep::Claude && self.base.is_none() { self.name.update(cx, |input, cx| input.focus(window, cx)); }
        // O título do diálogo sai do desenho do Hangar.
        let _ = self.hangar.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    fn create(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.step != AddStep::Claude || self.base.is_some() || self.slug.is_empty() { return; }
        (self.saving, self.error) = (true, None);
        let slug = self.slug.clone();
        // O Hangar redesenha junto: é ele que desenha o diálogo, que não fecha enquanto cria.
        if !self.hangar.update(cx, |hangar, cx| { cx.notify(); hangar.create_account(slug) }).unwrap_or(false) {
            (self.saving, self.error) = (false, Some(tr("settings_offline")));
        }
        cx.notify();
    }
}

/// Uma escolha do diálogo: título, o que é e o botão (desligado dizendo "chega na próxima versão" quando ainda não chegou).
fn choice(title: String, description: String, action: Button) -> Div {
    div().flex().items_center().gap(px(12.)).py(px(10.)).border_t_1().border_color(theme::border())
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().font_weight(FontWeight::MEDIUM).child(title))
            .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(description)))
        .child(div().flex_shrink_0().child(action))
}

impl Render for AddAccount {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let saving = self.saving;
        let back = Button::new("accounts-add-back").ghost().small().icon(IconName::ArrowLeft).label(tr("settings_back")).disabled(saving)
            .on_click(cx.listener(|this, _, window, cx| {
                let previous = if this.step == AddStep::Claude { AddStep::Subscriptions } else { AddStep::Choose };
                this.go(previous, window, cx);
            }));
        let body = match self.step {
            AddStep::Choose => div().flex().flex_col()
                .child(choice(tr("accounts_add_subscription"), tr("accounts_add_subscription_desc"),
                    Button::new("accounts-add-pick-subscription").outline().small().label(tr("accounts_add_pick"))
                        .on_click(cx.listener(|this, _, window, cx| this.go(AddStep::Subscriptions, window, cx)))))
                .child(choice(tr("accounts_add_model_path"), tr("accounts_add_model_desc"),
                    Button::new("accounts-add-pick-model").outline().small().label(tr("accounts_add_pick"))
                        .on_click(cx.listener(|this, _, window, cx| this.go(AddStep::Catalog(true), window, cx)))))
                .child(choice(tr("accounts_add_key_path"), tr("accounts_add_key_desc"),
                    Button::new("accounts-add-pick-key").outline().small().label(tr("accounts_add_pick"))
                        .on_click(cx.listener(|this, _, window, cx| this.go(AddStep::Catalog(false), window, cx))))),
            AddStep::Catalog(model) => div().flex().flex_col().children(keys::PROVIDERS.iter().map(|provider| {
                let id = provider.id;
                keys::provider_choice(provider, Button::new(SharedString::from(format!("accounts-add-provider-{id}"))).outline().small()
                    .icon(IconName::Plus).label(tr("accounts_add_connect"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.close_dialog(cx);
                        let _ = this.hangar.update(cx, |hangar, cx| { hangar.accounts.add = None; hangar.open_new_key(id, model, window, cx); });
                    })))
            })),
            AddStep::Subscriptions => div().flex().flex_col()
                .child(choice(tr("accounts_add_claude"), tr("accounts_add_claude_desc"),
                    Button::new("accounts-add-pick-claude").outline().small().icon(IconName::Plus).label(tr("accounts_add_connect"))
                        .on_click(cx.listener(|this, _, window, cx| this.go(AddStep::Claude, window, cx)))))
                .child(choice(tr("accounts_add_codex"), tr("accounts_add_codex_desc"),
                    Button::new("accounts-add-pick-codex").outline().small().icon(IconName::Plus).label(tr("accounts_add_connect"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            window.close_dialog(cx);
                            let _ = this.hangar.update(cx, |hangar, cx| { hangar.accounts.add = None; hangar.start_codex(None, false, window, cx); });
                        })))),
            AddStep::Claude => match self.base.clone() {
                Some((id, name)) => div().flex().flex_col().gap(px(14.))
                    .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal()
                        .child(tr("accounts_add_base_signed_out").replace("{name}", &name)))
                    .child(div().flex().justify_end().child(Button::new("accounts-add-base").primary().small()
                        .label(tr("accounts_add_base_enter").replace("{name}", &name))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.close_dialog(cx);
                            let id = id.clone();
                            let _ = this.hangar.update(cx, |hangar, cx| { hangar.accounts.add = None; hangar.start_sign_in(id, window, cx); });
                        })))),
                None => {
                    let hint = if self.slug.is_empty() { tr("accounts_add_name_hint") } else { tr("accounts_add_folder").replace("{slug}", &self.slug) };
                    div().flex().flex_col().gap(px(8.))
                        .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(tr("accounts_add_name")))
                        .child(Input::new(&self.name).disabled(saving).aria_label(tr("accounts_add_name")))
                        .child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(hint))
                        .child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(tr("accounts_add_login_later")))
                        .children(self.error.clone().map(|error| div().text_size(px(13.)).child(div().text_color(theme::danger()).whitespace_normal().child(error))))
                        .child(div().mt(px(6.)).flex().justify_end().child(Button::new("accounts-add-create").primary().small()
                            .label(tr(if saving { "accounts_add_creating" } else { "accounts_add_create" })).disabled(saving || self.slug.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx)))))
                }
            },
        };
        div().flex().flex_col().gap(px(10.))
            .when(self.step != AddStep::Choose, |el| el.child(div().flex().child(back)))
            .child(body)
    }
}

impl Hangar {
    pub(super) fn find_row(&self, id: &str) -> Option<&Row> { self.accounts.sections.iter().flat_map(|s| &s.rows).find(|r| r.id == id) }

    /// Alguma escrita de conta em voo (login, sair, remover, renomear): as outras esperam.
    pub(super) fn accounts_busy(&self) -> bool {
        self.accounts.sign_in.is_some() || self.accounts.change.is_some() || self.accounts.rename.as_ref().is_some_and(|r| r.saving)
            || self.accounts.form.as_ref().is_some_and(EngineForm::busy) || self.accounts.cookie.as_ref().is_some_and(|c| c.saving)
            || self.accounts.cookie_clearing.is_some() || self.accounts.codex.is_some() || self.accounts.reset.as_ref().is_some_and(|r| r.consuming)
    }

    pub(super) fn start_sign_in(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let Some(row) = self.find_row(&id) else { return };
        let (label, name) = (row.label.clone(), row.name.clone());
        let code = cx.new(|cx| InputState::new(window, cx).placeholder(tr("accounts_login_code_placeholder")));
        let subscription = cx.subscribe_in(&code, window, |this: &mut Hangar, input, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let filled = !input.read(cx).value().trim().is_empty();
                if let Some(s) = this.accounts.sign_in.as_mut() { s.has_code = filled; }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.send_code(cx),
            _ => {}
        });
        self.accounts.outcome = None;
        self.accounts.sign_in = Some(SignIn { label, name, attempt: 0, open: false, starting: false, sending: false, stopping: false,
            failed: false, url: None, copied: false, step_failed: false, error: None, done: None, code, has_code: false, poll: None,
            _subscription: subscription });
        self.begin_login(window, cx);
    }

    /// Começa (ou recomeça) a tentativa: o servidor abre a janela escondida do login.
    fn begin_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.accounts.login_attempt += 1;
        let attempt = self.accounts.login_attempt;
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        if let Some(poll) = s.poll.take() { poll.abort(); }
        (s.attempt, s.starting, s.failed, s.url, s.copied, s.step_failed, s.error, s.done, s.has_code) =
            (attempt, true, false, None, false, false, None, None, false);
        s.code.update(cx, |input, cx| input.set_value("", window, cx));
        let label = s.label.clone();
        let (done, turn) = (self.accounts_send_later(), login_turn(&api, &label));
        self.runtime.spawn(async move {
            let _turn = turn.lock().await;
            let result = api.server_post(&["conta-estado", &label, "login"], 30).await;
            let opened = result.as_ref().map_or_else(|e| e.uncertain, |_| true);
            let (owner, owned) = oneshot::channel();
            done(AccountsReply::Action(ActionReply::Started(attempt, result, owner))).await;
            if opened && !owned.await.unwrap_or(false) { cancel_unseen(&api, &label).await; }
        });
        cx.notify();
    }

    /// Lê o passo agora e a cada 2 s: o link aparece e, se a autorização voltar pelo navegador, o login conclui sozinho.
    fn poll_login(&mut self) {
        let Some(api) = self.api.clone() else { return };
        let done = self.accounts_send_later();
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        let (attempt, label) = (s.attempt, s.label.clone());
        s.poll = Some(self.runtime.spawn(async move {
            loop {
                let result = api.server_read(&["conta-estado", &label, "login", "passo"], &[], 10).await;
                done(AccountsReply::Action(ActionReply::Step(attempt, result))).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }));
    }

    fn send_code(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let Some(s) = self.accounts.sign_in.as_mut()
            .filter(|s| s.open && !s.starting && !s.sending && !s.stopping && !s.failed && s.done.is_none()) else { return };
        let code = s.code.read(cx).value().trim().to_owned();
        if code.is_empty() { return; }
        (s.sending, s.error) = (true, None);
        let (attempt, label) = (s.attempt, s.label.clone());
        let (done, turn) = (self.accounts_send_later(), login_turn(&api, &label));
        // O servidor segura a confirmação até o CLI reler a conta (até 300 s).
        self.runtime.spawn(async move {
            let _turn = turn.lock().await;
            let result = api.server_send(reqwest::Method::POST, &["conta-estado", &label, "login", "codigo"], Some(json!({"codigo": code})), 310).await;
            let entered = matches!(&result, Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true));
            let (owner, owned) = oneshot::channel();
            done(AccountsReply::Action(ActionReply::Code(attempt, result, owner))).await;
            // Sem tela, a resposta do código decide: entrou, nada a fechar; recusa, erro ou incerteza, fecha uma vez.
            if !entered && !owned.await.unwrap_or(false) { cancel_unseen(&api, &label).await; }
        });
        cx.notify();
    }

    /// Cancelar/Voltar (`restart` falso) ou Tentar de novo: fecha a tentativa aberta no servidor antes.
    fn stop_sign_in(&mut self, restart: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        if s.starting || s.sending || s.stopping { return; }
        if !s.open {
            if restart { self.begin_login(window, cx); } else { self.close_sign_in(window, cx); }
            return;
        }
        (s.stopping, s.error) = (true, None);
        let (attempt, label) = (s.attempt, s.label.clone());
        let (done, turn) = (self.accounts_send_later(), login_turn(&api, &label));
        self.runtime.spawn(async move {
            let _turn = turn.lock().await;
            let result = api.server_post(&["conta-estado", &label, "login", "cancelar"], 15).await;
            done(AccountsReply::Action(ActionReply::Stopped(attempt, restart, result))).await
        });
        cx.notify();
    }

    fn close_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.accounts.sign_in = None;
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    fn finish_login(&mut self, email: Option<String>, plan: Option<String>, cx: &mut Context<Self>) {
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        if let Some(poll) = s.poll.take() { poll.abort(); }
        (s.open, s.sending, s.failed, s.error, s.step_failed, s.done) = (false, false, false, None, false, Some((email, plan)));
        // A cota nova só vem com a leitura forçada; a falha dela aparece na tela de conectada.
        self.accounts.notice = None;
        self.load_accounts(true, cx);
    }

    /// A tentativa perdeu a tela (outra página, página fechada, outro servidor): o estado sai. Parada no meio, fecha
    /// aqui, no servidor atual, na vez da conta. Começando ou com o código em voo, a própria tarefa fecha pela resposta.
    pub(in crate::app) fn accounts_page_left(&mut self) {
        // O que estava sendo digitado sai com a página; uma gravação em voo termina sozinha e relê as listas. O painel do
        // Codex para de ler (como o web ao desmontar): a tentativa e a importação seguem no servidor. A redefinição
        // pedida fica com a chave dela: voltar e repetir tem de ser a mesma tentativa, e a resposta ainda acha quem pediu.
        (self.accounts.form, self.accounts.cookie, self.accounts.codex) = (None, None, None);
        let Some(s) = self.accounts.sign_in.take() else { return };
        let Some(api) = self.api.clone().filter(|_| s.open && !s.starting && !s.sending) else { return };
        let (label, turn) = (s.label.clone(), login_turn(&api, &s.label));
        self.runtime.spawn(async move { let _turn = turn.lock().await; cancel_unseen(&api, &label).await; });
    }

    /// Troca de servidor: a tentativa aberta é do servidor que sai, e é nele que se cancela.
    pub(in crate::app) fn leave_accounts(&mut self) { self.accounts_page_left(); self.accounts.reset = None; }

    pub(super) fn confirm_change(&mut self, id: String, kind: ChangeKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let Some(row) = self.find_row(&id) else { return };
        let (title, description, ok) = match kind {
            ChangeKind::SignOut => (tr("accounts_sign_out_title"), tr("accounts_sign_out_desc"), tr("accounts_sign_out_ok")),
            ChangeKind::Remove => (tr("accounts_remove_title"), tr(row.remove.as_ref().map_or("accounts_remove_desc_key", |r| r.2)), tr("accounts_remove_ok")),
        };
        let title = title.replace("{name}", &row.name);
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, title, description, ok, ButtonVariant::Danger,
            move |_, cx| { let _ = this.update(cx, |this, cx| this.start_change(id.clone(), kind, cx)); true });
    }

    fn start_change(&mut self, id: String, kind: ChangeKind, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.accounts_busy() { return; }
        // A linha pode ter sumido entre a pergunta e o sim: nada a fazer.
        let Some(row) = self.find_row(&id) else { return };
        let (path, seconds) = match kind {
            ChangeKind::SignOut => (vec!["claude-configs".to_owned(), row.label.clone(), "logout".to_owned()], 60),
            ChangeKind::Remove => match &row.remove { Some((path, seconds, _)) => (path.clone(), *seconds), None => return },
        };
        let name = row.name.clone();
        self.accounts.outcome = None;
        self.accounts.change = Some(Change { id: id.clone(), kind });
        let done = self.accounts_send_later();
        self.runtime.spawn(async move {
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let result = match kind {
                ChangeKind::SignOut => api.server_post(&path, seconds).await,
                ChangeKind::Remove => api.server_send(reqwest::Method::DELETE, &path, None, seconds).await,
            };
            done(AccountsReply::Action(ActionReply::Changed(id, name, kind, result))).await
        });
        cx.notify();
    }

    pub(super) fn open_add_account(&mut self, window: &mut Window, cx: &mut Context<Self>) { self.open_add_account_at(AddStep::Choose, window, cx); }

    pub(super) fn open_add_account_at(&mut self, step: AddStep, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let base = self.accounts.list.ok().and_then(|list| list.iter().find(|c| c.kind == "claude" && c.active && c.logged_in() == Some(false)))
            .map(|c| (c.id.clone(), c.name.clone()));
        let hangar = cx.entity().downgrade();
        let add = cx.new(|cx| AddAccount::new(hangar.clone(), step, base, window, cx));
        self.accounts.outcome = None;
        self.accounts.add = Some(add.clone());
        window.open_dialog(cx, move |dialog, _, cx| {
            let (saving, title) = { let add = add.read(cx); (add.saving, add.title()) };
            let (hangar, confirm) = (hangar.clone(), add.clone());
            // Criando: o diálogo não fecha, para a resposta ter onde aparecer. O Enter do diálogo cria a conta em vez
            // de fechá-lo: fechar sem criar parecia ter dado certo.
            dialog.w(px(540.)).title(title).child(add.clone()).keyboard(!saving).overlay_closable(!saving).close_button(!saving)
                .on_ok(move |_, window, cx| { confirm.update(cx, |add, cx| add.create(window, cx)); false })
                .on_close(move |_, _, cx| { let _ = hangar.update(cx, |this, _| this.accounts.add = None); })
        });
    }

    /// Falso quando não há conexão para mandar o pedido.
    fn create_account(&mut self, slug: String) -> bool {
        let Some(api) = self.api.clone() else { return false };
        let done = self.accounts_send_later();
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::POST, &["claude-configs"], Some(json!({"nome": slug})), 30).await;
            done(AccountsReply::Action(ActionReply::Created(slug, result))).await
        });
        true
    }

    pub(super) fn receive_action(&mut self, reply: ActionReply, window: &mut Window, cx: &mut Context<Self>) {
        let page_open = self.settings == Some(Page::Accounts);
        match reply {
            ActionReply::Started(attempt, result, owner) => {
                let s = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt && page_open);
                let _ = owner.send(s.is_some());
                let Some(s) = s else { return };
                s.starting = false;
                match result {
                    Ok(_) => { s.open = true; self.poll_login(); }
                    Err(error) if error.uncertain => (s.open, s.failed, s.error) = (true, true, Some(tr("accounts_login_start_uncertain"))),
                    Err(error) => s.error = Some(Self::failure(&error)),
                }
            }
            ActionReply::Step(attempt, result) => {
                if !page_open { self.accounts_page_left(); return; }
                let Some(s) = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt && s.open && !s.failed) else { return };
                match result {
                    Ok(step) if step.get("etapa").and_then(Value::as_str) == Some("concluido") => {
                        let field = |key: &str| step.get(key).and_then(Value::as_str).map(str::to_owned);
                        self.finish_login(field("email"), field("plano"), cx);
                    }
                    Ok(step) => {
                        s.step_failed = false;
                        if let Some(url) = step.get("url").and_then(Value::as_str).filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
                            s.url = Some(url.to_owned());
                        }
                    }
                    // O servidor desistiu da tentativa: perguntar de novo só mostraria "aguardando".
                    Err(error) if error.status == Some(409) => {
                        if let Some(poll) = s.poll.take() { poll.abort(); }
                        (s.failed, s.error) = (true, Some(Self::failure(&error)));
                    }
                    Err(_) => s.step_failed = true,
                }
            }
            ActionReply::Code(attempt, result, owner) => {
                let s = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt && s.done.is_none() && page_open);
                let _ = owner.send(s.is_some());
                let Some(s) = s else {
                    // Sem tela: a tarefa fecha a tentativa se precisar; a lista só muda se a conta entrou.
                    if matches!(&result, Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true)) { self.load_accounts(true, cx); }
                    return;
                };
                s.sending = false;
                match result {
                    Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true) => {
                        let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
                        self.finish_login(field("email"), field("plano"), cx);
                    }
                    other => {
                        if let Some(poll) = s.poll.take() { poll.abort(); }
                        s.failed = true;
                        s.error = Some(match other {
                            Err(error) if error.uncertain => tr("accounts_login_uncertain"),
                            Err(error) => Self::failure(&error),
                            Ok(_) => tr("accounts_login_not_confirmed"),
                        });
                        // A recusa não prova que a conta ficou de fora: a lista relida mostra como o servidor a vê.
                        self.load_accounts(false, cx);
                    }
                }
            }
            ActionReply::Stopped(attempt, restart, result) => {
                let Some(s) = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt) else { return };
                s.stopping = false;
                match result {
                    Ok(_) => {
                        if let Some(poll) = s.poll.take() { poll.abort(); }
                        s.open = false;
                        if restart { self.begin_login(window, cx); } else { self.close_sign_in(window, cx); }
                    }
                    // Sem confirmação de que fechou, a tentativa continua valendo e os botões continuam lá.
                    Err(error) => s.error = Some(if error.uncertain { tr("accounts_login_stop_uncertain") } else { Self::failure(&error) }),
                }
            }
            ActionReply::Changed(id, name, kind, result) => {
                self.accounts.change = None;
                let ok = result.is_ok();
                self.accounts.outcome = Some(match result {
                    Ok(_) => (tr(if kind == ChangeKind::SignOut { "accounts_signed_out" } else { "accounts_removed" }).replace("{name}", &name), false),
                    Err(error) if error.uncertain => (tr("accounts_change_uncertain").replace("{name}", &name), true),
                    Err(error) => (tr(if kind == ChangeKind::SignOut { "accounts_sign_out_failed" } else { "accounts_remove_failed" })
                        .replace("{name}", &name).replace("{reason}", &Self::failure(&error)), true),
                });
                // O servidor já confirmou: a linha muda agora, e a leitura seguinte traz o resto (a cota, os modelos).
                if ok && let Some(Ok(list)) = self.accounts.list.value.as_mut() {
                    match kind {
                        ChangeKind::SignOut => for c in list.iter_mut().filter(|c| c.id == id) {
                            c.login = Some(Login { state: "ok".into(), logged_in: Some(false), email: None, plan: None, reason: None, refresh_expires_at: None });
                            c.quota = None;
                        },
                        ChangeKind::Remove => list.retain(|c| c.id != id),
                    }
                }
                self.load_accounts(ok && kind == ChangeKind::SignOut, cx);
                if kind == ChangeKind::Remove { self.load_engines(cx); }
            }
            ActionReply::Created(slug, result) => {
                let add = self.accounts.add.clone();
                match (result, add) {
                    (Ok(_), add) => {
                        if add.is_some() { self.accounts.add = None; window.close_dialog(cx); }
                        self.accounts.outcome = Some((tr("accounts_created").replace("{name}", &slug), false));
                    }
                    (Err(error), Some(add)) => add.update(cx, |add, cx| {
                        add.saving = false;
                        add.error = Some(if error.uncertain { tr("accounts_create_uncertain") } else { Self::failure(&error) });
                        cx.notify();
                    }),
                    (Err(error), None) => self.accounts.outcome = Some((if error.uncertain { tr("accounts_create_uncertain") }
                        else { Self::failure(&error) }, true)),
                }
                self.load_accounts(false, cx);
            }
        }
    }

    /// A página no lugar da lista enquanto há uma tentativa de login: os passos do web, um por vez.
    pub(super) fn render_sign_in(&self, cx: &mut Context<Self>) -> Option<Div> {
        let s = self.accounts.sign_in.as_ref()?;
        let (color, _) = theme::provider("claude");
        let avatar = div().size(px(36.)).flex_shrink_0().rounded(px(10.)).bg(color.opacity(0.16)).flex().items_center().justify_center()
            .text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child("C");
        let title = if s.done.is_some() { tr("accounts_login_connected") } else { tr("accounts_login_title").replace("{name}", &s.name) };
        let head = div().flex().items_center().gap(px(12.)).child(avatar)
            .child(div().flex().flex_col().gap(px(2.)).min_w_0()
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("accounts_login_server").replace("{server}", &self.server_label(cx)))));
        let line = |text: String, color: Hsla| div().text_size(px(13.)).whitespace_normal().child(div().text_color(color).child(text));
        let footer = || div().mt(px(4.)).flex().justify_end().gap(px(8.));
        let mut panel = settings_box().mt(px(24.)).p(px(20.)).gap(px(16.)).child(head);
        if let Some((email, plan)) = &s.done {
            let fact = |label: &'static str, value: &Option<String>| value.clone().map(|value| div().flex().gap(px(12.)).text_size(px(13.))
                .child(div().w(px(72.)).text_color(theme::muted()).child(tr(label))).child(div().child(value)));
            panel = panel.child(div().flex().items_center().gap(px(8.)).text_size(px(13.))
                    .child(chrome::small_icon(IconName::Check, 15., theme::success()))
                    .child(div().child(tr("accounts_login_ready").replace("{name}", &s.name))))
                .children(fact("accounts_login_email", email)).children(fact("accounts_login_plan", plan))
                .children(self.accounts.notice.as_ref().map(|_| line(tr("accounts_login_refresh_failed"), theme::danger())))
                .child(footer().child(Button::new("accounts-login-done").primary().small().label(tr("accounts_login_done"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_sign_in(window, cx)))));
            return Some(panel);
        }
        panel = panel.children(s.error.clone().map(|error| line(error, theme::danger())))
            .when(s.step_failed, |el| el.child(line(tr("accounts_login_step_failed"), theme::danger())));
        let waiting = s.open && !s.failed;
        let locked = s.sending || s.stopping;
        if s.starting || (waiting && s.url.is_none()) {
            panel = panel.child(line(tr("accounts_login_preparing"), theme::muted()));
        } else if let (true, Some(url)) = (waiting, s.url.clone()) {
            let number = |n: &'static str| div().size(px(22.)).flex_shrink_0().rounded_full().border_1().border_color(theme::border_strong())
                .flex().items_center().justify_center().text_size(px(12.)).text_color(theme::muted()).child(n);
            let step = |n: &'static str, title: &'static str, help: &'static str, body: AnyElement| div().flex().gap(px(12.)).child(number(n))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(6.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(tr(title)))
                    .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr(help)))
                    .child(body));
            let open_url = url.clone();
            let links = div().flex().flex_col().gap(px(6.))
                .child(div().flex().gap(px(8.))
                    .child(Button::new("accounts-login-open").primary().small().icon(IconName::ExternalLink).label(tr("accounts_login_open")).disabled(locked)
                        .on_click(move |_, _, cx| cx.open_url(&open_url)))
                    .child(Button::new("accounts-login-copy").outline().small().icon(IconName::Copy)
                        .label(tr(if s.copied { "accounts_login_copied" } else { "accounts_login_copy" })).disabled(locked)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                            if let Some(s) = this.accounts.sign_in.as_mut() { s.copied = true; }
                            cx.notify();
                        }))))
                .child(div().min_w_0().truncate().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::faint())
                    .child(s.url.clone().unwrap_or_default()));
            let code = Input::new(&s.code).disabled(locked).aria_label(tr("accounts_login_code"));
            panel = panel.child(step("1", "accounts_login_authorize", "accounts_login_authorize_help", links.into_any_element()))
                .child(step("2", "accounts_login_code", "accounts_login_code_help", code.into_any_element()))
                .when(s.sending, |el| el.child(line(tr("accounts_login_confirming"), theme::muted())))
                .child(footer()
                    .child(Button::new("accounts-login-cancel").outline().small().label(tr("cancel")).disabled(locked)
                        .on_click(cx.listener(|this, _, window, cx| this.stop_sign_in(false, window, cx))))
                    .child(Button::new("accounts-login-confirm").primary().small()
                        .label(tr(if s.sending { "accounts_login_confirming_short" } else { "accounts_login_confirm" })).disabled(locked || !s.has_code)
                        .on_click(cx.listener(|this, _, _, cx| this.send_code(cx)))));
            return Some(panel);
        }
        let stuck = s.starting || s.stopping;
        Some(panel.child(footer()
            .child(Button::new("accounts-login-back").outline().small().label(tr("settings_back")).disabled(stuck)
                .on_click(cx.listener(|this, _, window, cx| this.stop_sign_in(false, window, cx))))
            .when(!s.starting && (!s.open || s.failed), |el| el.child(Button::new("accounts-login-retry").primary().small()
                .label(tr(if s.stopping { "accounts_login_stopping" } else { "accounts_retry" })).disabled(s.stopping)
                .on_click(cx.listener(|this, _, window, cx| this.stop_sign_in(true, window, cx)))))))
    }
}

#[cfg(test)]
mod tests {
    use super::account_slug;

    #[test]
    fn slug_is_what_the_server_accepts() {
        assert_eq!(account_slug("  Trabalho Ção 2 "), "trabalho-cao-2");
        assert_eq!(account_slug("--Nova//conta--"), "nova-conta");
        assert_eq!(account_slug("é"), "e");
        assert_eq!(account_slug("!!!"), "");
        assert_eq!(account_slug(&"a".repeat(40)).len(), 32);
    }
}
