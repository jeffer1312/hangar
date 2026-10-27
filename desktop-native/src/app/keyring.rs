//! Chaveiro `login` trancado numa máquina: o aviso com a senha e o Destrancar, um por servidor conectado.
use super::*;

/// Evento `keyring` da lista: `Some(true)` trancado, `Some(false)` aberto ou sem chaveiro, `None` corpo que não se lê.
pub(super) fn locked(data: &serde_json::Value) -> Option<bool> {
    match data.get("state")?.as_str()? {
        "locked" => Some(true),
        "unlocked" | "absent" => Some(false),
        _ => None,
    }
}

/// O aviso de uma máquina com o chaveiro trancado. Existe só enquanto o servidor diz `locked`.
pub(super) struct Prompt {
    input: Entity<InputState>,
    sending: bool,
    error: Option<String>,
    _enter: Subscription,
}

fn unlock_error(error: &Failure) -> String {
    if error.code.as_deref() == Some("keyring_wrong_password") { return tr_shared("keyring_wrong_password", &[]); }
    // Sem status a falha é de rede, com a frase local; com status, a frase do envelope do servidor.
    let reason = if error.status.is_none() { tr(&error.detail) } else { error.detail.clone() };
    tr_shared("keyring_unlock_failed", &[("error", &reason)])
}

impl Hangar {
    /// Estado novo do chaveiro de uma máquina (evento `keyring`; `false` também na queda do stream). Devolve se mudou.
    pub(super) fn set_keyring(&mut self, key: &str, locked: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !locked { return self.keyring.remove(key).is_some(); }
        if self.keyring.contains_key(key) { return false; }
        let input = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(tr_shared("keyring_password_placeholder", &[])));
        let server = key.to_owned();
        let _enter = cx.subscribe_in(&input, window, move |this: &mut Hangar, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event { this.unlock_keyring(server.clone(), window, cx); }
        });
        self.keyring.insert(key.to_owned(), Prompt { input, sending: false, error: None, _enter });
        true
    }

    fn unlock_keyring(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let api = if key == self.active_key() { self.api.clone() } else { self.remote.get(&key).and_then(|l| l.api.clone()) };
        let Some(prompt) = self.keyring.get_mut(&key).filter(|p| !p.sending) else { return };
        let password = prompt.input.read(cx).value().to_string();
        if password.is_empty() { return; }
        // A senha não fica no campo depois do envio, dê certo ou não.
        prompt.input.update(cx, |input, cx| input.set_value("", window, cx));
        let Some(api) = api else {
            prompt.error = Some(tr_shared("keyring_unlock_failed", &[("error", &tr("network_error"))]));
            self.redraw(panes::Area::Nav, cx);
            cx.notify();
            return;
        };
        (prompt.sending, prompt.error) = (true, None);
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            // O servidor pode reiniciar o daemon do chaveiro antes de responder.
            let result = api.server_send(reqwest::Method::POST, &["keyring", "unlock"], Some(serde_json::json!({"password": password})), 45).await;
            let _ = tx.send(Envelope { connection: 0, selection: None, payload: Payload::Keyring(key, result) }).await;
        });
        self.redraw(panes::Area::Nav, cx);
        cx.notify();
    }

    /// Sucesso não esconde o aviso: quem o tira é o evento `keyring` que o servidor manda na mudança.
    pub(super) fn receive_keyring(&mut self, key: String, result: Result<serde_json::Value, Failure>, cx: &mut Context<Self>) {
        let Some(prompt) = self.keyring.get_mut(&key) else { return };
        prompt.sending = false;
        prompt.error = result.err().map(|error| unlock_error(&error));
        self.redraw(panes::Area::Nav, cx);
        cx.notify();
    }

    /// Máquinas com o chaveiro trancado: a ativa primeiro, depois as outras em ordem estável.
    pub(super) fn keyring_keys(&self) -> Vec<String> {
        let active = self.active_key();
        let mut keys: Vec<String> = self.keyring.keys().cloned().collect();
        keys.sort_by_key(|key| (*key != active, key.clone()));
        keys
    }

    /// Rótulo da máquina no aviso: o da lista de servidores, ou o host.
    fn keyring_label(&self, key: &str, cx: &App) -> String {
        match self.server_entry(key) {
            Some(entry) if !entry.label.is_empty() => entry.label.clone(),
            Some(entry) => servers::default_label(&entry.address),
            None => self.server_label(cx),
        }
    }

    /// Aviso de uma máquina, se o chaveiro dela estiver trancado. `compact`: uma linha só, para a barra das abas.
    pub(super) fn render_keyring_notice(&self, key: &str, compact: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prompt = self.keyring.get(key)?;
        let title = tr_shared("keyring_locked_title", &[("label", &self.keyring_label(key, cx))]);
        let server = key.to_owned();
        let button = Button::new(SharedString::from(format!("keyring-unlock-{key}"))).xsmall().flex_shrink_0().primary()
            .label(tr_shared(if prompt.sending { "keyring_unlocking" } else { "keyring_unlock" }, &[]))
            .loading(prompt.sending).disabled(prompt.sending)
            .on_click(cx.listener(move |this, _, window, cx| this.unlock_keyring(server.clone(), window, cx)));
        let field = Input::new(&prompt.input).xsmall().mask_toggle().disabled(prompt.sending).aria_label(tr_shared("keyring_password_placeholder", &[]));
        let error = prompt.error.clone().map(|text| div().text_xs().text_color(theme::danger()).child(text));
        let id = SharedString::from(format!("keyring-{key}"));
        Some(if compact {
            // Na linha só, a falha da última tentativa toma o lugar do título até a próxima.
            let (text, color) = match &prompt.error { Some(error) => (format!("{title} · {error}"), theme::danger()), None => (title, theme::warning()) };
            div().id(id).flex_shrink_0().flex().items_center().gap_1().max_w(px(420.))
                .child(div().min_w_0().truncate().text_xs().text_color(color).child(text))
                .child(div().w(px(140.)).flex_shrink_0().child(field))
                .child(button)
                .into_any_element()
        } else {
            div().id(id).flex_shrink_0().mx(px(8.)).my(px(4.)).p_2().rounded(px(8.)).bg(theme::inset()).flex().flex_col().gap_1()
                .role(Role::Group).aria_label(title.clone())
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::warning()).whitespace_normal().child(title))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr_shared("keyring_locked_hint", &[])))
                .child(field)
                .children(error)
                .child(button.w_full())
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{locked, unlock_error};
    use crate::api::Failure;
    use serde_json::json;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn keyring_event_reads_locked_only_from_the_locked_state() {
        assert_eq!(locked(&json!({"state": "locked"})), Some(true));
        assert_eq!(locked(&json!({"state": "unlocked"})), Some(false));
        assert_eq!(locked(&json!({"state": "absent"})), Some(false));
        assert_eq!(locked(&json!({"state": "unknown"})), None);
        assert_eq!(locked(&json!({"state": 3})), None);
        assert_eq!(locked(&json!([])), None);
    }

    #[test]
    fn wrong_password_has_its_own_sentence() {
        let failure = |code: &str| Failure { status: Some(403), detail: "x".into(), retry_after: None, uncertain: false, code: Some(code.into()) };
        assert_eq!(unlock_error(&failure("keyring_wrong_password")), crate::i18n::tr_shared("keyring_wrong_password", &[]));
        assert!(unlock_error(&failure("keyring_no_daemon")).contains('x'));
    }
}
