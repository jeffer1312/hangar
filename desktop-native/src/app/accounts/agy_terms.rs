//! Termos do Google do agy aceitos no painel da página de contas (`/api/antigravity-contas/default/termos`). O servidor
//! conduz o agy num terminal escondido e publica a etapa; o painel só desenha a etapa e manda a opção que a pessoa tocou.
use super::*;

/// A etapa da tentativa, como o GET dos Termos manda.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TermsStage {
    Starting,
    /// A pergunta do agy, com os links dos Termos.
    Terms(String),
    Answering,
    /// O agy pediu login antes dos Termos.
    Login,
    Done,
    /// O código do servidor (`termos_prazo`…); `None` quando o pedido nem chegou a uma tentativa.
    Failed(Option<String>),
}

/// O que o painel oferece em cada etapa.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TermsAction {
    /// A `opcao` do agy: 1 aceita e compartilha os dados de uso, 2 aceita sem compartilhar.
    Accept(u8),
    Cancel,
    SignIn,
    OpenTerminal,
    Back,
}

impl TermsStage {
    /// A resposta do GET (e a do POST que começa). Etapa desconhecida, ou Termos sem o texto, é resposta inválida.
    pub(super) fn read(value: &Value) -> Option<Self> {
        Some(match value.get("estado")?.as_str()? {
            "iniciando" => Self::Starting,
            "termos" => Self::Terms(value.get("texto")?.as_str()?.to_owned()),
            "respondendo" => Self::Answering,
            "login" => Self::Login,
            "concluido" => Self::Done,
            "falhou" => Self::Failed(value.get("codigo").and_then(Value::as_str).map(str::to_owned)),
            _ => return None,
        })
    }

    /// O servidor ainda anda sozinho: a leitura de 1 s segue.
    pub(super) fn polls(&self) -> bool { matches!(self, Self::Starting | Self::Terms(_) | Self::Answering) }

    /// Leitura que saiu antes de a resposta chegar ao servidor ainda mostra os Termos: a resposta já mandada vale.
    pub(super) fn then(&self, read: Self) -> Self {
        if *self == Self::Answering && matches!(read, Self::Terms(_)) { Self::Answering } else { read }
    }

    pub(crate) fn actions(&self) -> Vec<TermsAction> {
        use TermsAction::*;
        match self {
            Self::Starting => vec![Cancel],
            Self::Terms(_) => vec![Accept(1), Accept(2), Cancel],
            Self::Login => vec![SignIn, Cancel],
            Self::Failed(_) => vec![OpenTerminal, Back],
            Self::Answering | Self::Done => Vec::new(),
        }
    }
}

/// A frase do código de falha do servidor; código que o cliente não conhece fica com a genérica.
pub(super) fn failed_text(code: Option<&str>) -> String {
    terms_tr(match code {
        Some("termos_prazo") => "error_deadline",
        Some("termos_tela_desconhecida") => "error_screen_gone",
        Some("termos_sem_confirmacao") => "error_unconfirmed",
        Some("termos_nao_convergiu") => "error_no_converge",
        Some("termos_terminal_falhou") => "error_terminal",
        Some("termos_expirou") => "error_expired",
        _ => "failed",
    })
}

/// As frases do painel são as do Expo (`mobile_antigravity_terms_*`): uma frase, um dicionário.
pub(super) fn terms_tr(key: &str) -> String { crate::i18n::tr_shared(&format!("mobile_antigravity_terms_{key}"), &[]) }

const TERMS: [&str; 3] = ["antigravity-contas", "default", "termos"];

/// O painel no lugar da lista enquanto existe. Só a conta padrão: o `agy` com terminal só conhece a pasta dela.
pub(super) struct AgyTermsFlow {
    attempt: Option<String>,
    stage: TermsStage,
    /// Falha de um pedido (começar, responder, ler), mostrada junto da etapa.
    error: Option<String>,
    /// Começando ou mandando a resposta.
    busy: bool,
    /// O erro na tela é de uma leitura que não veio: a próxima leitura boa o tira. O de uma ação fica até a etapa mudar.
    read_failed: bool,
    /// O DELETE do Cancelar falhou com a janela aberta: só o Cancelar fica, e ele manda o DELETE de novo.
    closing: bool,
    turn: u64,
    task: Option<JoinHandle<()>>,
}

impl AgyTermsFlow {
    pub(super) fn actions(&self) -> Vec<TermsAction> { if self.closing { vec![TermsAction::Cancel] } else { self.stage.actions() } }

    /// A resposta do DELETE do Cancelar; `true` fecha o painel. Sem reler depois do erro: o servidor pode já ter
    /// esquecido a tentativa, e a leitura trocaria o erro por "tentativa perdida".
    pub(super) fn cancelled(&mut self, result: Result<Value, Failure>) -> bool {
        self.busy = false;
        let Err(error) = result else { return true };
        self.closing = true;
        self.error = Some(if error.code.as_deref() == Some("erro_termos_janela_aberta") { terms_tr("error_window_open") }
            else { Hangar::setting_failure(&error) });
        false
    }
}

impl Drop for AgyTermsFlow {
    // Tarefa do tokio não morre com o handle: sem isto a leitura seguiria depois do painel fechado.
    fn drop(&mut self) { if let Some(task) = self.task.take() { task.abort(); } }
}

pub(in crate::app) enum TermsReply {
    Started(u64, Result<Value, Failure>),
    Read(u64, Result<Value, Failure>),
    /// A resposta da opção, pela tentativa a que ela foi.
    Answered(String, Result<Value, Failure>),
    Cancelled(u64, Result<Value, Failure>),
}

/// Fecha no servidor o terminal escondido da tentativa; quem já saiu da tela não tem onde mostrar a falha.
fn close_attempt(runtime: &Runtime, api: Api, attempt: String) {
    runtime.spawn(async move {
        if let Err(error) = api.server_delete(&TERMS, &[("attempt_id", &attempt)], 15).await {
            eprintln!("Termos do Antigravity: o terminal escondido não foi fechado no servidor: {}", error.detail);
        }
    });
}

impl Hangar {
    /// Aceitar os Termos: o painel abre e o servidor começa a conduzir o agy.
    pub(super) fn open_agy_terms(&mut self, cx: &mut Context<Self>) {
        if self.api.is_none() || self.accounts.terms.is_some() { return; }
        self.accounts.outcome = None;
        self.accounts.terms = Some(AgyTermsFlow { attempt: None, stage: TermsStage::Starting, error: None, busy: true, read_failed: false, closing: false, turn: 0, task: None });
        let Some((turn, api)) = self.terms_turn() else { return };
        let done = self.accounts_send_later();
        if let Some(flow) = self.accounts.terms.as_mut() {
            flow.task = Some(self.runtime.spawn(async move {
                done(AccountsReply::AgyTerms(TermsReply::Started(turn, api.server_post(&TERMS, 30).await))).await
            }));
        }
        cx.notify();
    }

    /// Nova vez do painel: a tarefa da vez anterior para, e só respostas desta vez valem.
    fn terms_turn(&mut self) -> Option<(u64, Api)> {
        let api = self.api.clone()?;
        self.accounts.keys_seq += 1;
        let turn = self.accounts.keys_seq;
        let flow = self.accounts.terms.as_mut()?;
        if let Some(task) = flow.task.take() { task.abort(); }
        flow.turn = turn;
        Some((turn, api))
    }

    /// Lê a etapa a cada segundo enquanto o servidor anda sozinho. Leitura que não veio (rede, 5xx) tenta de novo.
    fn agy_terms_poll(&mut self, cx: &mut Context<Self>) {
        let Some(attempt) = self.accounts.terms.as_ref().and_then(|f| f.attempt.clone()) else { return };
        let Some((turn, api)) = self.terms_turn() else { return };
        let done = self.accounts_send_later();
        let Some(flow) = self.accounts.terms.as_mut() else { return };
        flow.task = Some(self.runtime.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let result = api.server_read(&TERMS, &[("attempt_id", &attempt)], 10).await;
                let going = match &result {
                    Ok(value) => TermsStage::read(value).is_some_and(|s| s.polls()),
                    Err(error) => error.status.is_none_or(|s| s >= 500),
                };
                done(AccountsReply::AgyTerms(TermsReply::Read(turn, result))).await;
                if !going { break; }
            }
        }));
        cx.notify();
    }

    /// A opção tocada vai ao servidor, que aperta a tecla dela; a leitura de 1 s segue e vê o aceite.
    fn agy_terms_answer(&mut self, option: u8, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let done = self.accounts_send_later();
        let Some(flow) = self.accounts.terms.as_mut().filter(|f| !f.busy && matches!(f.stage, TermsStage::Terms(_))) else { return };
        let Some(attempt) = flow.attempt.clone() else { return };
        (flow.busy, flow.error, flow.read_failed) = (true, None, false);
        self.runtime.spawn(async move {
            let body = json!({"attempt_id": attempt, "opcao": option});
            let path = ["antigravity-contas", "default", "termos", "resposta"];
            done(AccountsReply::AgyTerms(TermsReply::Answered(attempt, api.server_send(reqwest::Method::POST, &path, Some(body), 30).await))).await
        });
        cx.notify();
    }

    /// O painel sai (Cancelar, Voltar, Entrar, outra página): a leitura para e o terminal escondido fecha no servidor.
    pub(super) fn agy_terms_close(&mut self) {
        let Some(flow) = self.accounts.terms.take() else { return };
        if let (Some(attempt), Some(api)) = (flow.attempt.clone(), self.api.clone()) { close_attempt(&self.runtime, api, attempt); }
    }

    /// Cancelar espera o DELETE: com a janela ainda aberta no servidor o painel fica com o erro, e Cancelar tenta de novo.
    fn agy_terms_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(attempt) = self.accounts.terms.as_ref().and_then(|f| f.attempt.clone()) else { return self.agy_terms_back(window, cx) };
        let Some((turn, api)) = self.terms_turn() else { return };
        let done = self.accounts_send_later();
        if let Some(flow) = self.accounts.terms.as_mut() { (flow.busy, flow.error, flow.read_failed) = (true, None, false); }
        self.runtime.spawn(async move {
            done(AccountsReply::AgyTerms(TermsReply::Cancelled(turn, api.server_delete(&TERMS, &[("attempt_id", &attempt)], 15).await))).await
        });
        cx.notify();
    }

    fn agy_terms_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agy_terms_close();
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    /// O agy pediu login: a tentativa fecha e o login de sempre abre; concluído, os Termos voltam pela lista relida.
    fn agy_terms_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agy_terms_close();
        self.start_agy(Some("default".into()), window, cx);
    }

    /// A casa da máquina, onde a sessão do agy abre: a da linha padrão com Termos pendentes.
    fn agy_terms_home(&self) -> Option<String> { self.accounts.sections.iter().flat_map(|s| &s.rows).find_map(|r| r.agy_terms.clone()) }

    /// Abrir no terminal: o caminho de antes, a sessão `agy-termos` com o cartão dos Termos no chat.
    fn agy_terms_open_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(home) = self.agy_terms_home() else { return };
        self.agy_terms_close();
        self.start_agy_terms(home, window, cx);
    }

    /// `false` quando nada da tela mudou (resposta de outra vez, leitura com a mesma etapa): não redesenha.
    pub(super) fn receive_agy_terms(&mut self, reply: TermsReply, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match reply {
            TermsReply::Started(turn, result) => {
                let Some(flow) = self.accounts.terms.as_mut().filter(|f| f.turn == turn) else {
                    // O painel fechou com o pedido em voo: a tentativa que o servidor abriu fecha aqui.
                    if let (Ok(value), Some(api)) = (&result, self.api.clone())
                        && let Some(attempt) = value.get("attempt_id").and_then(Value::as_str) {
                        close_attempt(&self.runtime, api, attempt.to_owned());
                    }
                    return false;
                };
                flow.busy = false;
                match result {
                    Ok(value) => match (value.get("attempt_id").and_then(Value::as_str), TermsStage::read(&value)) {
                        (Some(attempt), Some(stage)) => {
                            (flow.attempt, flow.stage) = (Some(attempt.to_owned()), stage);
                            if flow.stage.polls() { self.agy_terms_poll(cx); }
                        }
                        _ => (flow.stage, flow.error) = (TermsStage::Failed(None), Some(tr("invalid_response"))),
                    },
                    Err(error) => (flow.stage, flow.error) = (TermsStage::Failed(None), Some(super::antigravity::agy_failure(&error))),
                }
            }
            TermsReply::Read(turn, result) => {
                let Some(flow) = self.accounts.terms.as_mut().filter(|f| f.turn == turn) else { return false };
                match result {
                    Ok(value) => match TermsStage::read(&value) {
                        Some(stage) => {
                            let next = flow.stage.then(stage);
                            if next == flow.stage && !flow.read_failed { return false; }
                            (flow.stage, flow.error, flow.read_failed) = (next, None, false);
                        }
                        None => (flow.stage, flow.error) = (TermsStage::Failed(None), Some(tr("invalid_response"))),
                    },
                    // O servidor não tem mais a tentativa (reiniciou, ou outra a trocou): esperar por ela não adianta.
                    Err(error) if error.status == Some(404) =>
                        (flow.stage, flow.error) = (TermsStage::Failed(None), Some(terms_tr("attempt_lost"))),
                    Err(error) if error.status.is_some_and(|s| s < 500) =>
                        (flow.stage, flow.error) = (TermsStage::Failed(None), Some(Self::setting_failure(&error))),
                    Err(error) => (flow.error, flow.read_failed) = (Some(Self::failure(&error)), true),
                }
                if flow.stage == TermsStage::Done {
                    self.accounts.terms = None;
                    self.accounts.outcome = Some((terms_tr("done"), false));
                    self.load_accounts(true, cx);
                    self.root_focus.focus(window, cx);
                }
            }
            TermsReply::Cancelled(turn, result) => {
                let Some(flow) = self.accounts.terms.as_mut().filter(|f| f.turn == turn) else { return false };
                if flow.cancelled(result) {
                    self.accounts.terms = None;
                    self.root_focus.focus(window, cx);
                }
            }
            TermsReply::Answered(attempt, result) => {
                let Some(flow) = self.accounts.terms.as_mut().filter(|f| f.attempt.as_deref() == Some(attempt.as_str())) else { return false };
                flow.busy = false;
                match result {
                    Ok(_) => if matches!(flow.stage, TermsStage::Terms(_)) { flow.stage = TermsStage::Answering; },
                    // Sem resposta a opção pode ter chegado: a leitura diz, e mandar de novo fora da tela responde 409.
                    Err(error) if error.uncertain => flow.error = Some(tr("delivery_uncertain")),
                    Err(error) => flow.error = Some(Self::setting_failure(&error)),
                }
            }
        }
        true
    }

    /// O painel no lugar da lista: a etapa que o servidor publicou e os botões dela.
    pub(super) fn render_agy_terms(&self, cx: &mut Context<Self>) -> Option<Div> {
        let f = self.accounts.terms.as_ref()?;
        let buttons = terms_buttons(f.actions(), f.busy, self.agy_terms_home().is_some(), |action, button| match action {
            TermsAction::Accept(option) => button.on_click(cx.listener(move |this, _, _, cx| this.agy_terms_answer(option, cx))),
            TermsAction::Cancel => button.on_click(cx.listener(|this, _, window, cx| this.agy_terms_cancel(window, cx))),
            TermsAction::Back => button.on_click(cx.listener(|this, _, window, cx| this.agy_terms_back(window, cx))),
            TermsAction::SignIn => button.on_click(cx.listener(|this, _, window, cx| this.agy_terms_sign_in(window, cx))),
            TermsAction::OpenTerminal => button.on_click(cx.listener(|this, _, window, cx| this.agy_terms_open_terminal(window, cx))),
        });
        Some(terms_panel(&f.stage, f.error.clone(), &self.server_label(cx), buttons).mt(px(24.)))
    }
}

/// Os botões da etapa, na ordem da tela: Cancelar e Voltar à esquerda, a ação da etapa por último. `wire` liga cada um
/// à sua ação; a animação da novidade não liga nenhum.
pub(crate) fn terms_buttons(actions: Vec<TermsAction>, busy: bool, home: bool, mut wire: impl FnMut(TermsAction, Button) -> Button) -> Vec<Button> {
    actions.into_iter().rev().map(|action| {
        let button = match action {
            TermsAction::Accept(option) => Button::new(SharedString::from(format!("accounts-agy-terms-accept-{option}"))).primary().small()
                .label(terms_tr(if option == 1 { "accept_share" } else { "accept_private" })).disabled(busy),
            TermsAction::Cancel => Button::new("accounts-agy-terms-cancel").outline().small().label(terms_tr("cancel")),
            TermsAction::Back => Button::new("accounts-agy-terms-back").outline().small().label(tr("settings_back")),
            TermsAction::SignIn => Button::new("accounts-agy-terms-sign-in").primary().small().label(tr("accounts_agy_sign_in")),
            TermsAction::OpenTerminal => Button::new("accounts-agy-terms-terminal").primary().small().icon(IconName::SquareTerminal)
                .label(terms_tr("open_terminal")).disabled(!home),
        };
        wire(action, button)
    }).collect()
}

/// O painel dos Termos numa etapa, com os botões já ligados. `server` é o nome da máquina onde o agy roda.
pub(crate) fn terms_panel(stage: &TermsStage, error: Option<String>, server: &str, buttons: Vec<Button>) -> Div {
    let head = super::antigravity::agy_panel_head(terms_tr("title"), server);
    let line = |text: String| div().text_size(px(13.)).whitespace_normal().child(text);
    let waiting = |id: &'static str, text: String| div().flex().items_center().gap(px(8.)).text_size(px(13.)).text_color(theme::muted())
        .child(chrome::Spinner::new(id, IconName::LoaderCircle, px(14.), theme::accent())).child(text);
    let mut panel = settings_box().p(px(20.)).gap(px(16.)).child(head);
    panel = match stage {
        TermsStage::Starting => panel.child(waiting("accounts-agy-terms-starting", terms_tr("starting"))),
        TermsStage::Terms(text) => {
            // Os links dos Termos abrem fora do app, pelo mesmo leitor do cartão de opções.
            let links = crate::interaction::question_links(text).into_iter().enumerate().map(|(i, url)|
                Button::new(SharedString::from(format!("accounts-agy-terms-link-{i}"))).small().ghost().icon(IconName::ExternalLink)
                    .label(url.clone()).on_click(move |_, _, cx| cx.open_url(&url)));
            panel.child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(terms_tr("intro")))
                .child(div().p(px(12.)).rounded(px(8.)).border_1().border_color(theme::border()).bg(theme::inset())
                    .text_size(px(12.5)).whitespace_normal().child(text.clone()))
                .child(div().flex().flex_col().items_start().gap(px(4.)).children(links))
        }
        TermsStage::Answering => panel.child(waiting("accounts-agy-terms-answering", terms_tr("answering"))),
        TermsStage::Login => panel.child(line(terms_tr("login"))),
        TermsStage::Failed(code) => panel.child(line(failed_text(code.as_deref())).text_color(theme::danger())),
        TermsStage::Done => panel,
    };
    panel.children(error.map(|e| line(e).text_color(theme::danger())))
        .child(div().mt(px(4.)).flex().flex_wrap().justify_end().gap(px(8.)).children(buttons))
}

#[cfg(test)]
mod tests {
    use super::{AgyTermsFlow, TermsAction::*, TermsStage, failed_text, terms_tr};
    use crate::api::Failure;
    use serde_json::json;

    #[test]
    fn the_get_of_the_terms_reads_each_stage_of_the_contract() {
        let read = |v| TermsStage::read(&v);
        assert_eq!(read(json!({"attempt_id": "a", "estado": "iniciando"})), Some(TermsStage::Starting));
        assert_eq!(read(json!({"estado": "termos", "texto": "Leia https://policies.google.com/terms"})),
            Some(TermsStage::Terms("Leia https://policies.google.com/terms".into())));
        assert_eq!(read(json!({"estado": "respondendo"})), Some(TermsStage::Answering));
        assert_eq!(read(json!({"estado": "login"})), Some(TermsStage::Login));
        assert_eq!(read(json!({"estado": "concluido"})), Some(TermsStage::Done));
        assert_eq!(read(json!({"estado": "falhou", "codigo": "termos_prazo"})), Some(TermsStage::Failed(Some("termos_prazo".into()))));
        assert_eq!(read(json!({"estado": "falhou"})), Some(TermsStage::Failed(None)));
        // Etapa que o cliente não conhece, ou Termos sem o texto, não viram tela inventada.
        assert_eq!(read(json!({"estado": "outra"})), None);
        assert_eq!(read(json!({"estado": "termos"})), None);
    }

    #[test]
    fn polling_goes_on_only_while_the_server_still_moves_by_itself() {
        assert!(TermsStage::Starting.polls() && TermsStage::Terms("t".into()).polls() && TermsStage::Answering.polls());
        assert!(!TermsStage::Login.polls() && !TermsStage::Done.polls() && !TermsStage::Failed(None).polls());
    }

    #[test]
    fn a_late_read_of_the_terms_screen_does_not_undo_the_answer_already_sent() {
        assert_eq!(TermsStage::Answering.then(TermsStage::Terms("t".into())), TermsStage::Answering);
        assert_eq!(TermsStage::Answering.then(TermsStage::Done), TermsStage::Done);
        assert_eq!(TermsStage::Starting.then(TermsStage::Terms("t".into())), TermsStage::Terms("t".into()));
    }

    #[test]
    fn each_stage_offers_its_buttons() {
        assert_eq!(TermsStage::Starting.actions(), [Cancel]);
        assert_eq!(TermsStage::Terms("t".into()).actions(), [Accept(1), Accept(2), Cancel]);
        assert!(TermsStage::Answering.actions().is_empty());
        assert_eq!(TermsStage::Login.actions(), [SignIn, Cancel]);
        assert_eq!(TermsStage::Failed(Some("termos_prazo".into())).actions(), [OpenTerminal, Back]);
        assert!(TermsStage::Done.actions().is_empty());
    }

    #[test]
    fn each_failure_code_has_its_sentence_and_an_unknown_one_the_generic() {
        assert_eq!(failed_text(Some("termos_prazo")), terms_tr("error_deadline"));
        assert_eq!(failed_text(Some("termos_tela_desconhecida")), terms_tr("error_screen_gone"));
        assert_eq!(failed_text(Some("termos_sem_confirmacao")), terms_tr("error_unconfirmed"));
        assert_eq!(failed_text(Some("termos_nao_convergiu")), terms_tr("error_no_converge"));
        assert_eq!(failed_text(Some("termos_terminal_falhou")), terms_tr("error_terminal"));
        assert_eq!(failed_text(Some("termos_expirou")), terms_tr("error_expired"));
        assert_eq!(failed_text(Some("termos_novo")), terms_tr("failed"));
        assert_eq!(failed_text(None), terms_tr("failed"));
    }

    // `tr_shared` devolve a chave crua quando ela não existe, e a trava do i18n só confere `tr`.
    #[test]
    fn every_terms_sentence_exists_in_the_expo_dictionary() {
        for key in ["title", "starting", "intro", "accept_share", "accept_private", "cancel", "answering", "login", "done", "failed",
            "open_terminal", "error_deadline", "error_screen_gone", "error_unconfirmed", "error_no_converge", "error_terminal",
            "error_expired", "error_window_open", "attempt_lost", "pending"] {
            assert_ne!(terms_tr(key), format!("mobile_antigravity_terms_{key}"), "{key}");
        }
    }

    #[test]
    fn a_cancel_whose_window_stayed_open_keeps_the_panel_with_the_error_and_cancel_tries_again() {
        let mut flow = AgyTermsFlow { attempt: Some("t1".into()), stage: TermsStage::Terms("t".into()), error: None, busy: true,
            read_failed: false, closing: false, turn: 1, task: None };
        let open = Failure { status: Some(500), detail: "a janela escondida dos Termos não fechou".into(), retry_after: None, uncertain: false,
            code: Some("erro_termos_janela_aberta".into()) };
        assert!(!flow.cancelled(Err(open)), "o painel fica");
        assert_eq!(flow.error, Some(terms_tr("error_window_open")));
        assert_eq!(flow.actions(), [Cancel], "só o Cancelar, sem Aceitar de uma tentativa que o servidor já soltou");
        assert_eq!(flow.attempt.as_deref(), Some("t1"), "o segundo Cancelar vai à mesma tentativa");
        assert!(flow.cancelled(Ok(serde_json::json!({"ok": true}))), "o DELETE que deu certo fecha");
    }
}
