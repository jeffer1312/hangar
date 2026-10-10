//! "No PC" / "Fora": com o modo "pc" e esta janela à vista (sinal de vida a cada 30 s), o servidor retém o push do celular.
//! O modo é de cada máquina do dono; o botão da barra troca em todas, e o sinal traz de volta o que o celular mudou.
use super::*;

/// O servidor dá o app por vivo por 75 s: um sinal a cada 30 s aguenta uma perda.
const BEAT_EVERY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct Presence {
    /// O modo de cada máquina pela última resposta dela.
    modes: HashMap<String, String>,
    /// Servidor anterior ao recurso (404/405): fica calado.
    unsupported: HashSet<String>,
    /// As conexões do último sinal: no encerramento a tela pode já ter sumido, e o aviso de saída ainda sai.
    leaving: Arc<std::sync::Mutex<Vec<Api>>>,
}

async fn leave_all(apis: Vec<Api>) {
    futures::future::join_all(apis.iter().map(|api| api.server_send(reqwest::Method::POST, &["presence", "heartbeat"],
        Some(serde_json::json!({"leaving": true})), 2))).await;
}

impl Hangar {
    pub(super) fn start_presence(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(BEAT_EVERY).await;
            if this.update(cx, |this, _| this.presence_beat()).is_err() { break; }
        }).detach();
        let (runtime, leaving) = (self.runtime.clone(), self.presence.leaving.clone());
        // Registrado no app, não na tela: fechar sem bandeja solta a tela antes do encerramento.
        App::on_app_quit(cx, move |_| {
            let apis = leaving.lock().map(|list| list.clone()).unwrap_or_default();
            let task = runtime.spawn(leave_all(apis));
            async move { let _ = task.await; }
        }).detach();
    }

    /// As máquinas do dono com conexão: a ativa e as ligadas da lista, nunca convite nem par externo.
    fn owned_presence(&self) -> Vec<(String, Api)> {
        let mut seen = HashSet::new();
        std::iter::once(self.active_key()).chain(self.servers.iter().filter(|s| !s.disabled).map(|s| servers::norm(&s.address)))
            .filter(|key| !key.is_empty() && seen.insert(key.clone()) && !self.server_entry(key).is_some_and(|s| s.invite))
            .filter_map(|key| self.machine_api(&key).map(|api| (key, api))).collect()
    }

    fn send_presence(&self, key: String, api: Api, path: &'static [&'static str], body: Value, switched: bool) {
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::POST, path, Some(body), 10).await;
            let _ = tx.send(Envelope { connection: 0, selection: None, payload: Payload::Presence(key, switched, result) }).await;
        });
    }

    /// Sinal de vida para todas as máquinas do dono; com a janela na bandeja não sai nada.
    pub(super) fn presence_beat(&mut self) {
        if self.window_tray.hidden { return; }
        let owned = self.owned_presence();
        if let Ok(mut list) = self.presence.leaving.lock() { *list = owned.iter().map(|(_, api)| api.clone()).collect(); }
        for (key, api) in owned { self.send_presence(key, api, &["presence", "heartbeat"], serde_json::json!({}), false); }
    }

    /// A janela saiu de vista: o celular volta a receber na hora, sem esperar o prazo do servidor.
    pub(super) fn presence_leave(&self) {
        drop(self.runtime.spawn(leave_all(self.owned_presence().into_iter().map(|(_, api)| api).collect())));
    }

    /// O modo da máquina da conversa aberta; sem resposta dela, o da ativa ou o de qualquer outra.
    fn presence_mode(&self) -> Option<&str> {
        let modes = &self.presence.modes;
        modes.get(&self.open_server()).or_else(|| modes.get(&self.active_key())).or_else(|| modes.values().next()).map(String::as_str)
    }

    fn toggle_presence(&mut self) {
        let next = if self.presence_mode() == Some("pc") { "away" } else { "pc" };
        for (key, api) in self.owned_presence() {
            if self.presence.unsupported.contains(&key) { continue; }
            self.send_presence(key, api, &["presence"], serde_json::json!({"mode": next}), true);
        }
    }

    pub(super) fn receive_presence(&mut self, key: String, switched: bool, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            Ok(value) => {
                self.presence.unsupported.remove(&key);
                if let Some(mode) = value.get("mode").and_then(Value::as_str) { self.presence.modes.insert(key, mode.to_owned()); }
            }
            Err(error) if matches!(error.status, Some(404 | 405)) => { self.presence.modes.remove(&key); self.presence.unsupported.insert(key); }
            Err(error) if switched => {
                let server = self.server_entry(&key).map_or_else(|| key.clone(), |s| s.label.clone());
                let text = tr("presence_failed").replace("{server}", &server).replace("{erro}", &Self::failure(&error));
                window.push_notification(Notification::error(text), cx);
            }
            // Sinal perdido (máquina fora do ar): o próximo tenta de novo, e a lista dela já mostra a queda.
            Err(_) => return,
        }
        self.redraw(panes::Area::Top, cx);
    }

    /// Sem modo lido, só aparece (desligado, com o motivo) quando alguma máquina respondeu que não tem o recurso.
    pub(super) fn render_presence_button(&self, cx: &mut Context<Self>) -> Option<Button> {
        let Some(mode) = self.presence_mode() else {
            if self.presence.unsupported.is_empty() { return None; }
            let tip = tr("presence_unsupported");
            return Some(Button::new("topbar-presence").ghost().small().h(px(26.)).px(px(8.)).rounded_full().disabled(true)
                .child(div().flex().items_center().gap(px(6.)).text_size(px(12.5)).child(chrome::small_icon(IconName::Monitor, 14., theme::faint()))
                    .child(tr("presence_pc")))
                .accessibility_label(tip.clone()).tooltip(tip));
        };
        let pc = mode == "pc";
        let (label, tip) = if pc { (tr("presence_pc"), tr("presence_tip_pc")) } else { (tr("presence_away"), tr("presence_tip_away")) };
        Some(Button::new("topbar-presence").ghost().small().h(px(26.)).px(px(8.)).rounded_full()
            .child(div().flex().items_center().gap(px(6.)).text_size(px(12.5))
                .child(chrome::small_icon(if pc { IconName::Monitor } else { IconName::Smartphone }, 14., theme::muted()))
                .child(div().text_color(theme::muted()).child(label.clone())))
            .accessibility_label(format!("{label}. {tip}")).tooltip(tip)
            .on_click(cx.listener(|this, _, _, _| this.toggle_presence())))
    }
}
