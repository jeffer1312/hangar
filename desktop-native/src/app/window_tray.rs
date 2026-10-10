//! A janela e a bandeja: o ícone segue a opção de Configurações → Geral, e fechar esconde em vez de encerrar.
use super::*;
use crate::tray::{Tray, TrayEvent};

pub(super) struct WindowTray {
    events: async_channel::Sender<TrayEvent>,
    pub(super) icon: Option<Tray>,
    /// O ícone está sendo criado em outra thread.
    starting: bool,
    pub(super) error: Option<String>,
    /// A janela está escondida na bandeja.
    pub(super) hidden: bool,
    last_toggle: Option<Instant>,
}

impl WindowTray {
    pub(super) fn new(events: async_channel::Sender<TrayEvent>) -> Self { Self { events, icon: None, starting: false, error: None, hidden: false, last_toggle: None } }
}

/// O segundo clique de um duplo clique no ícone chega como outro pedido: sem isto a janela aparecia e sumia.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

fn is_bounce(since_last: Option<Duration>) -> bool { since_last.is_some_and(|gap| gap < DOUBLE_CLICK) }

/// Fechar só esconde com o ícone de pé e uma bandeja que o mostre: sem isso o app ficaria vivo e invisível.
pub(super) fn hides_on_close(keep: bool, icon_online: Option<bool>) -> bool { keep && icon_online == Some(true) }

impl Hangar {
    /// Põe o ícone de acordo com a opção: cria quando liga, remove quando desliga.
    pub(super) fn sync_tray(&mut self, cx: &mut Context<Self>) {
        if !(crate::tray::SUPPORTED && appearance::get().keep_in_tray) {
            self.window_tray.icon = None;
            self.window_tray.error = None;
            cx.notify();
            return;
        }
        if self.window_tray.icon.is_some() || self.window_tray.starting { return; }
        (self.window_tray.starting, self.window_tray.error) = (true, None);
        let events = self.window_tray.events.clone();
        let task = self.runtime.spawn_blocking(move || crate::tray::start(events));
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.window_tray.starting = false;
                match result {
                    // Desligada enquanto o ícone subia: o que chegou é solto, e some.
                    Ok(icon) if appearance::get().keep_in_tray => { this.window_tray.icon = Some(icon); this.window_tray.error = None; }
                    Err(reason) if appearance::get().keep_in_tray => { eprintln!("tray: {reason}"); this.window_tray.error = Some(reason); }
                    _ => {}
                }
                cx.notify();
            });
        }).detach();
    }

    pub(super) fn closes_to_tray(&self) -> bool {
        hides_on_close(appearance::get().keep_in_tray, self.window_tray.icon.as_ref().map(Tray::online))
    }

    pub(super) fn hide_to_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_hidden(true);
        self.window_tray.hidden = true;
        self.presence_leave();
        let (title, body) = (tr("tray_notice_title"), tr("tray_notice_body"));
        self.runtime.spawn_blocking(move || if appearance::take_tray_notice() { show_system_notification(&title, &body) });
        cx.notify();
    }

    pub(super) fn show_from_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.window_tray.hidden {
            window.set_hidden(false);
            self.window_tray.hidden = false;
            self.presence_beat();
        }
        window.activate_window();
        cx.notify();
    }

    fn restart_from_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let updater = cx.global::<crate::update::Handle>().0.clone();
        let task = match updater.update(cx, |updater, cx| updater.restart_desktop(cx)) {
            Ok(task) => task,
            Err(reason) => {
                self.show_from_tray(window, cx);
                window.push_notification(Notification::error(reason), cx);
                return;
            }
        };
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let outcome = task.await.unwrap_or_else(|error| {
                let reason = format!("tarefa do reinício: {error}");
                crate::log_line(&reason);
                Err(reason)
            });
            let reached = handle.update(cx, |_, window, cx| this.update(cx, |this, cx| {
                match &outcome {
                    Ok(()) => {
                        this.window_tray.icon = None;
                        cx.quit();
                    }
                    Err(reason) => {
                        updater.update(cx, |updater, cx| updater.finish_desktop_restart(cx));
                        this.show_from_tray(window, cx);
                        window.push_notification(Notification::error(format!("{} ({reason})", tr("app_restart_failed"))), cx);
                    }
                }
            }).is_ok()).unwrap_or(false);
            // Sem a janela, o desfecho ainda vale: preso em DesktopRestart, o app recusaria atualizar até reabrir.
            if !reached {
                crate::log_line(&format!("reinício pela bandeja concluído sem a janela: {}", outcome.as_ref().err().map_or("app novo de pé", String::as_str)));
                match outcome {
                    Ok(()) => {
                        let _ = this.update(cx, |this, _| this.window_tray.icon = None);
                        cx.update(|cx| cx.quit());
                    }
                    Err(_) => { updater.update(cx, |updater, cx| updater.finish_desktop_restart(cx)); }
                }
            }
        }).detach();
    }

    pub(super) fn on_tray_event(&mut self, event: TrayEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event == TrayEvent::Toggle {
            let now = Instant::now();
            if is_bounce(self.window_tray.last_toggle.map(|last| now.duration_since(last))) { return; }
            self.window_tray.last_toggle = Some(now);
        }
        match event {
            // Minimizada conta como fora da tela: o clique a traz de volta em vez de escondê-la.
            TrayEvent::Toggle if !self.window_tray.hidden && window.is_visible() && self.closes_to_tray() => self.hide_to_tray(window, cx),
            TrayEvent::Toggle | TrayEvent::Show => self.show_from_tray(window, cx),
            TrayEvent::Restart => self.restart_from_tray(window, cx),
            // O ícone sai antes: encerrar com ele de pé deixa um ícone morto na bandeja do Windows.
            TrayEvent::Quit => { self.window_tray.icon = None; cx.quit(); }
            TrayEvent::Host(online) => {
                // A bandeja sumiu com a janela escondida: sem ícone não haveria como voltar.
                if !online && self.window_tray.hidden { self.show_from_tray(window, cx); }
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{hides_on_close, is_bounce};
    use core::prelude::v1::test;
    use std::time::Duration;

    #[test]
    fn the_second_click_of_a_double_click_is_ignored() {
        assert!(!is_bounce(None));
        assert!(is_bounce(Some(Duration::from_millis(120))));
        assert!(!is_bounce(Some(Duration::from_millis(900))));
    }

    #[test]
    fn closing_only_hides_with_the_option_on_and_a_tray_showing_the_icon() {
        assert!(hides_on_close(true, Some(true)));
        // Opção desligada, ícone que ainda não subiu (ou falhou) e bandeja ausente: fechar encerra.
        assert!(!hides_on_close(false, Some(true)));
        assert!(!hides_on_close(true, None));
        assert!(!hides_on_close(true, Some(false)));
    }
}
