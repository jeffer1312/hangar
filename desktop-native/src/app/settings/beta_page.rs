//! Página Beta: os recursos em teste ficam juntos aqui, cada um desligado até a pessoa ligar.
use super::*;

impl Hangar {
    pub(in crate::app) fn render_beta(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let a = appearance::get();
        // O Code Mode já ligado pelo settings.json do Claude vale em toda sessão: a opção aparece marcada e travada.
        self.ask_code_mode_settings(cx);
        self.ask_session_options(cx);
        let from_settings = self.settings_ui.code_mode_settings.is_some_and(|(_, on)| on);
        let server = self.settings_ui.session_options.and_then(|(_, options)| options);
        // Sem a leitura do servidor, ou com uma gravação no ar, o Code Mode fica travado.
        let busy = server.is_none() || self.settings_ui.session_options_saving == Some(self.connection);
        // O desenho da conversa é deste computador e muda mesmo com o servidor fora; o servidor recebe a escolha
        // quando puder.
        let conversation = segments("conversation-beta", &[tr("settings_conversation_beta_off"), tr("settings_conversation_beta_on")],
            a.conversation_beta as usize, 2, false, tr("settings_next_version"),
            |this: &mut Hangar, index, _: &mut Window, cx| this.choose_conversation(index == 1, cx), cx);
        // A abertura só liga o Code Mode com a Conversa nova do servidor ligada; antes da leitura vale a cópia daqui.
        let server_beta = server.map_or(a.conversation_beta, |o| o.conversation_beta);
        // Ligado pelos ajustes do Claude: travado, mas com "Ligado" marcado.
        let code_mode = segments("code-mode", &[tr("settings_code_mode_off"), tr("settings_code_mode_on")],
            (server.map_or(a.claude_code_mode, |o| o.code_mode) || from_settings) as usize,
            if server_beta { 2 } else { 0 }, from_settings || busy, tr("settings_next_version"),
            |this: &mut Hangar, index, _: &mut Window, cx| this.change_session_options(BetaChange::CodeMode(index == 1), cx), cx);
        let hint = self.settings_ui.session_options_error.clone()
            .unwrap_or_else(|| tr(if from_settings { "settings_code_mode_settings" } else { "settings_code_mode_hint" }));
        div().flex().flex_col()
            .child(self.page_top("settings_page_beta", tr("settings_beta_lead")))
            .child(settings_box().mt(px(20.))
                .child(self.row(IconName::MessageSquare, "settings_conversation_beta", Some(tr("settings_conversation_beta_hint")), true, conversation))
                .child(self.row(IconName::Code, "settings_code_mode", Some(hint), server_beta, code_mode)))
            .into_any_element()
    }

    /// Uma leitura por conexão das opções que o servidor aplica a toda abertura.
    fn ask_session_options(&mut self, cx: &mut Context<Self>) {
        if self.settings_ui.session_options.is_some_and(|(connection, _)| connection == self.connection) { return; }
        let Some(api) = self.api.clone() else { return };
        let connection = self.connection;
        self.settings_ui.session_options = Some((connection, None));
        let look = appearance::get();
        let local = crate::api::SessionOptions { conversation_beta: look.conversation_beta, code_mode: look.claude_code_mode };
        let task = self.runtime.spawn(async move { api.read_session_options(local).await });
        self.options_reply(connection, task, false, cx);
    }

    fn options_reply(&self, connection: u64, task: tokio::task::JoinHandle<Result<crate::api::SessionOptions, crate::api::Failure>>, save: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // A tarefa que some é falha como outra: com aviso e nova tentativa, nunca a linha travada calada.
            let result = task.await.unwrap_or_else(|error| Err(crate::api::Failure::local(error.to_string())));
            let _ = this.update(cx, |this, cx| {
                if save && this.settings_ui.session_options_saving == Some(connection) { this.settings_ui.session_options_saving = None; }
                if this.connection != connection { cx.notify(); return; }
                match result {
                    Ok(options) => {
                        (this.settings_ui.session_options, this.settings_ui.session_options_error) = (Some((connection, Some(options))), None);
                        // A cópia do Code Mode é a que vai a um servidor que ainda não tem a escolha: acompanha a
                        // gravação confirmada. O desenho da conversa nunca muda pelo servidor.
                        let mut next = appearance::get();
                        if save && next.claude_code_mode != options.code_mode {
                            next.claude_code_mode = options.code_mode;
                            this.apply_appearance(next, true, cx);
                        }
                        this.flush_conversation(cx);
                    }
                    Err(error) => {
                        this.settings_ui.session_options_error = Some(Hangar::fetch_failure(&error));
                        // Sem leitura boa a linha fica travada: libera nova leitura depois de uma espera.
                        if this.settings_ui.session_options == Some((connection, None)) {
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(Duration::from_secs(30)).await;
                                this.update(cx, |this, cx| if this.settings_ui.session_options == Some((connection, None)) {
                                    this.settings_ui.session_options = None;
                                    cx.notify();
                                }).ok();
                            }).detach();
                        }
                    }
                }
                cx.notify();
            });
        }).detach();
    }

    fn choose_conversation(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut next = appearance::get();
        next.conversation_beta = on;
        self.apply_appearance(next, true, cx);
        self.settings_ui.conversation_pending = Some((self.connection, on));
        self.flush_conversation(cx);
    }

    /// A Conversa nova escolhida aqui vai ao servidor assim que ele tiver leitura boa e nenhuma gravação no ar, e
    /// só deixa de estar pendente quando o servidor confirma: gravação que falha volta na próxima resposta boa.
    fn flush_conversation(&mut self, cx: &mut Context<Self>) {
        // Só no servidor da conexão em que a pessoa escolheu: outro servidor tem a escolha dele.
        let Some((_, on)) = self.settings_ui.conversation_pending.filter(|(owner, _)| *owner == self.connection) else { return };
        // Com uma gravação no ar o valor do servidor ainda vai mudar: compara de novo quando ela responder.
        if self.settings_ui.session_options_saving == Some(self.connection) { return; }
        let server = self.settings_ui.session_options.filter(|(connection, _)| *connection == self.connection).and_then(|(_, options)| options);
        if server.is_some_and(|o| o.conversation_beta == on) {
            self.settings_ui.conversation_pending = None;
            return;
        }
        self.change_session_options(BetaChange::Conversation(on), cx);
    }

    /// Grava no servidor; a resposta dele é o que fica marcado, e a falha aparece na linha.
    fn change_session_options(&mut self, change: BetaChange, cx: &mut Context<Self>) {
        let server = self.settings_ui.session_options.filter(|(connection, _)| *connection == self.connection).and_then(|(_, options)| options);
        let Some(next) = options_to_save(server, self.settings_ui.session_options_saving == Some(self.connection), change) else { return };
        let Some(api) = self.api.clone() else { return };
        let connection = self.connection;
        self.settings_ui.session_options_saving = Some(connection);
        cx.notify();
        let task = self.runtime.spawn(async move { api.save_session_options(next, false).await });
        self.options_reply(connection, task, true, cx);
    }

    /// Uma consulta por conexão: o settings.json do servidor diz se o Code Mode já está ligado lá. Até a resposta, e
    /// com falha, a opção fica livre; a falha não é resposta e libera nova consulta depois de uma espera.
    fn ask_code_mode_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_ui.code_mode_settings.is_some_and(|(connection, _)| connection == self.connection) { return; }
        let connection = self.connection;
        self.settings_ui.code_mode_settings = Some((connection, false));
        self.server_get(vec!["claude".into(), "code-mode".into()], Vec::new(), 10, cx, move |this, result, cx| {
            // 404 é servidor sem a rota (versão antiga): resposta definitiva, não falha a repetir.
            let answer = match result {
                Ok(value) => value["settings"].as_bool(),
                Err(failure) => (failure.status == Some(404)).then_some(false),
            };
            match answer {
                Some(on) => this.settings_ui.code_mode_settings = Some((connection, on)),
                None => cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(30)).await;
                    this.update(cx, |this, cx| if this.settings_ui.code_mode_settings == Some((connection, false)) {
                        this.settings_ui.code_mode_settings = None;
                        cx.notify();
                    }).ok();
                }).detach(),
            }
            cx.notify();
        });
    }
}

#[derive(Clone, Copy)]
enum BetaChange { Conversation(bool), CodeMode(bool) }

/// A gravação manda o objeto inteiro: só parte do que o servidor confirmou, nunca de uma leitura pendente
/// nem por cima de outra gravação ainda sem resposta.
fn options_to_save(server: Option<crate::api::SessionOptions>, saving: bool, change: BetaChange) -> Option<crate::api::SessionOptions> {
    if saving { return None; }
    let mut next = server?;
    match change {
        BetaChange::Conversation(on) => next.conversation_beta = on,
        BetaChange::CodeMode(on) => next.code_mode = on,
    }
    Some(next)
}

#[cfg(test)]
mod beta_tests {
    use super::{options_to_save, BetaChange};
    use crate::api::SessionOptions;

    #[test]
    fn a_change_waits_for_the_server_read_and_for_the_save_in_flight() {
        let server = SessionOptions { conversation_beta: true, code_mode: true };
        assert_eq!(options_to_save(None, false, BetaChange::Conversation(false)), None);
        assert_eq!(options_to_save(Some(server), true, BetaChange::Conversation(false)), None);
        assert_eq!(options_to_save(Some(server), false, BetaChange::Conversation(false)),
            Some(SessionOptions { conversation_beta: false, code_mode: true }));
        assert_eq!(options_to_save(Some(server), false, BetaChange::CodeMode(false)),
            Some(SessionOptions { conversation_beta: true, code_mode: false }));
    }
}
