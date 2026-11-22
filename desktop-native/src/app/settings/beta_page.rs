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
        let conversation = segmented("conversation-beta", &[tr("settings_conversation_beta_off"), tr("settings_conversation_beta_on")],
            a.conversation_beta as usize, true,
            |this: &mut Hangar, index, _: &mut Window, cx| {
                let mut next = appearance::get();
                next.conversation_beta = index == 1;
                this.apply_appearance(next, true, cx);
                // O desenho da conversa é deste computador; o padrão da abertura é do servidor.
                let code_mode = this.settings_ui.session_options.and_then(|(_, options)| options).map_or(next.claude_code_mode, |o| o.code_mode);
                this.save_session_options(crate::api::SessionOptions { conversation_beta: index == 1, code_mode }, cx);
            }, cx);
        // Ligado pelos ajustes do Claude: travado, mas com "Ligado" marcado. Sem a leitura do servidor, travado também.
        let code_mode = segments("code-mode", &[tr("settings_code_mode_off"), tr("settings_code_mode_on")], (server.is_some_and(|o| o.code_mode) || from_settings) as usize,
            if a.conversation_beta { 2 } else { 0 }, from_settings || server.is_none(), tr("settings_next_version"),
            |this: &mut Hangar, index, _: &mut Window, cx| {
                // A cópia local acompanha: é ela que vai a um servidor que ainda não tem a escolha.
                let mut next = appearance::get();
                next.claude_code_mode = index == 1;
                this.apply_appearance(next, true, cx);
                this.save_session_options(crate::api::SessionOptions { conversation_beta: next.conversation_beta, code_mode: index == 1 }, cx);
            }, cx);
        let hint = self.settings_ui.session_options_error.clone()
            .unwrap_or_else(|| tr(if from_settings { "settings_code_mode_settings" } else { "settings_code_mode_hint" }));
        div().flex().flex_col()
            .child(self.page_top("settings_page_beta", tr("settings_beta_lead")))
            .child(settings_box().mt(px(20.))
                .child(self.row(IconName::MessageSquare, "settings_conversation_beta", Some(tr("settings_conversation_beta_hint")), true, conversation))
                .child(self.row(IconName::Code, "settings_code_mode", Some(hint), a.conversation_beta, code_mode)))
            .into_any_element()
    }

    /// Uma leitura por conexão das opções que o servidor aplica a toda abertura.
    fn ask_session_options(&mut self, cx: &mut Context<Self>) {
        if self.settings_ui.session_options.is_some_and(|(connection, _)| connection == self.connection) { return; }
        let Some(api) = self.api.clone() else { return };
        let connection = self.connection;
        self.settings_ui.session_options = Some((connection, None));
        let task = self.runtime.spawn(async move { api.read_session_options().await });
        self.options_reply(connection, task, cx);
    }

    fn options_reply(&self, connection: u64, task: tokio::task::JoinHandle<Result<crate::api::SessionOptions, crate::api::Failure>>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let Ok(result) = task.await else { return };
            let _ = this.update(cx, |this, cx| if this.connection == connection {
                match result {
                    Ok(options) => (this.settings_ui.session_options, this.settings_ui.session_options_error) = (Some((connection, Some(options))), None),
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

    /// Grava no servidor; a resposta dele é o que fica marcado, e a falha aparece na linha.
    fn save_session_options(&mut self, next: crate::api::SessionOptions, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let connection = self.connection;
        let task = self.runtime.spawn(async move { api.save_session_options(next, false).await });
        self.options_reply(connection, task, cx);
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
