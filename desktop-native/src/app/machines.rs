//! Página Máquinas (web: "Servidores"): o servidor conectado vira um cartão, e o detalhe dele traz identificador, endereços,
//! reinício do serviço e o Avançado. Porta de `MaquinasSettings.svelte` e da parte "detalhe" de `AcessoSettings.svelte`.
//! O nativo fala com um servidor só: o que depende de guardar outras máquinas neste aparelho chega depois.
use super::*;
use std::rc::Rc;
use super::device::Remote;
use super::settings::{settings_box, Page};
use gpui_kit::component::{WindowExt, dialog::DialogButtonProps, tooltip::Tooltip};

/// O web espera o serviço voltar por até 2 minutos, perguntando a cada 2 segundos.
const RESTART_WAIT: Duration = Duration::from_secs(120);
const RESTART_POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind { Here, Lan, Tailscale, Public }

impl Kind {
    fn parse(raw: &str) -> Option<Self> {
        match raw { "nesta_maquina" => Some(Kind::Here), "rede_local" => Some(Kind::Lan), "tailscale" => Some(Kind::Tailscale),
            "publico" => Some(Kind::Public), _ => None }
    }
    fn name(self) -> String {
        tr(match self { Kind::Here => "machines_kind_here", Kind::Lan => "machines_kind_lan", Kind::Tailscale => "machines_kind_tailscale",
            Kind::Public => "machines_kind_public" })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status { Ok, Failed, Testing, Unset }

#[derive(Clone, Debug)]
struct Address { kind: Kind, url: String, status: Status, ms: Option<i64> }

/// `/api/alcance`: por onde o servidor responde, medido por ele mesmo.
#[derive(Clone, Debug, Default)]
struct Reach { loopback: bool, bind: String, addresses: Vec<Address> }

/// Farol de uma linha ou do cartão: a frase ao lado diz o mesmo, a cor nunca vai sozinha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Light { Ok, No, Test, Neutral }

impl Light {
    fn glyph(self) -> &'static str { match self { Light::Ok | Light::No => "●", Light::Test => "◌", Light::Neutral => "○" } }
    fn color(self) -> Hsla { match self { Light::Ok => theme::success(), Light::No => theme::danger(), _ => theme::muted() } }
}

fn parse_reach(value: &Value) -> Option<Reach> {
    // Uma linha fora do formato derruba a leitura inteira: sumir com ela mudaria o veredito sem aviso.
    let addresses = value.get("enderecos")?.as_array()?.iter().map(|e| Some(Address {
        kind: Kind::parse(e.get("tipo")?.as_str()?)?,
        url: e.get("url").and_then(Value::as_str).unwrap_or_default().to_owned(),
        status: match e.get("estado").and_then(Value::as_str)? { "ok" => Status::Ok, "falhou" => Status::Failed, "nao_configurado" => Status::Unset,
            _ => Status::Testing },
        ms: e.get("tempo_ms").and_then(Value::as_f64).map(|ms| ms.round() as i64),
    })).collect::<Option<Vec<_>>>()?;
    Some(Reach { loopback: value.get("loopback").and_then(Value::as_bool).unwrap_or(false),
        bind: value.get("bind").and_then(Value::as_str).unwrap_or_default().to_owned(), addresses })
}

impl Reach {
    /// Com CP_PUBLIC_URL apontando para o nome do Tailscale, as duas linhas são o mesmo endereço.
    fn public_same(&self) -> bool {
        let tailscale = self.addresses.iter().find(|a| a.kind == Kind::Tailscale).map(|a| a.url.as_str()).unwrap_or_default();
        !tailscale.is_empty() && self.addresses.iter().any(|a| a.kind == Kind::Public && a.url == tailscale)
    }
    fn main(&self, a: &Address) -> bool {
        matches!(a.kind, Kind::Lan | Kind::Tailscale) || (a.kind == Kind::Public && !self.public_same() && a.status != Status::Unset)
    }
    fn extras(&self) -> Vec<&Address> {
        self.addresses.iter().filter(|a| !self.main(a) && !(a.kind == Kind::Public && self.public_same())).collect()
    }
    fn fastest<'a>(mut list: impl Iterator<Item = &'a Address>) -> Option<&'a Address> {
        let first = list.next()?;
        Some(list.fold(first, |best, a| if a.ms.unwrap_or(0) < best.ms.unwrap_or(0) { a } else { best }))
    }
    /// O caminho de fora de casa mais rápido que respondeu.
    fn outside(&self) -> Option<&Address> {
        let same = self.public_same();
        Self::fastest(self.addresses.iter().filter(|a| a.status == Status::Ok && (a.kind == Kind::Tailscale || (a.kind == Kind::Public && !same))))
    }
    fn lan(&self) -> Option<&Address> { self.addresses.iter().find(|a| a.kind == Kind::Lan && a.status == Status::Ok) }
    /// Resumo do cartão: o endereço que responde mais rápido, fora "nesta máquina".
    fn summary(&self) -> (String, Light) {
        match Self::fastest(self.addresses.iter().filter(|a| a.status == Status::Ok && a.kind != Kind::Here)) {
            Some(best) => (format!("{} · {} ms", best.kind.name(), best.ms.unwrap_or(0)), Light::Ok),
            None => (tr("machines_loopback_short"), Light::No),
        }
    }
    /// A rede local fechada com o serviço escutando só em loopback é escolha da máquina, não defeito.
    fn by_choice(&self, a: &Address) -> bool { a.kind == Kind::Lan && a.status == Status::Failed && self.loopback }
    fn light(&self, a: &Address) -> Light {
        if self.by_choice(a) { return Light::Neutral; }
        match a.status { Status::Ok => Light::Ok, Status::Failed => Light::No, Status::Testing => Light::Test, Status::Unset => Light::Neutral }
    }
    fn phrase(&self, a: &Address) -> String {
        let time = format!("{} ms", a.ms.unwrap_or(0));
        match (a.status, a.kind) {
            (Status::Unset, _) => tr("machines_public_empty"),
            (Status::Testing, _) => tr("machines_testing"),
            (Status::Failed, Kind::Lan) if self.loopback && !self.bind.is_empty() => tr("machines_closed_loopback").replace("{endereco}", &self.bind),
            (Status::Failed, _) => tr("machines_failed"),
            (Status::Ok, Kind::Lan) => tr("machines_ok_wifi").replace("{tempo}", &time),
            (Status::Ok, Kind::Here) => tr("machines_ok_local"),
            (Status::Ok, _) => tr("machines_ok_4g").replace("{tempo}", &time),
        }
    }
}

/// Mesma regra do backend (`peers.validar_id`): minúsculas, números, hífen e sublinhado, até 32.
fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.len() <= 32 && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn id_hint() -> String { tr("machines_id_hint").replace("{exemplos}", "casa, notebook") }

/// Como o reinício terminou, lido do estado que o motor grava (`fase: pronto`, casado pelo pid do pedido).
pub(super) enum RestartEnd { Done(Option<String>), Failed(Option<String>), Unconfirmed }

pub(super) enum MachinesReply {
    Reach(u64, Result<Value, Failure>),
    Id(u64, Result<Value, Failure>),
    IdSaved(u64, Result<Value, Failure>),
    Restart(u64, Result<Value, Failure>),
    RestartEnd(u64, RestartEnd),
}

#[derive(Default)]
struct Restart {
    seq: u64,
    asking: bool,
    waiting: bool,
    /// Hora em que o serviço novo respondeu.
    at: Option<String>,
    error: Option<String>,
    /// 409: o servidor respondeu recusando, com o motivo dele; não é travamento.
    refused: bool,
    task: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leave { SignOut, Remove }

#[derive(Default)]
pub(in crate::app) struct Machines {
    reach: Remote<Reach>,
    /// Identificador lido do servidor; sem leitura boa o campo fica desligado e nada é gravado.
    id: Remote<String>,
    id_input: Option<(Entity<InputState>, Subscription)>,
    id_saving: bool,
    id_seq: u64,
    id_error: Option<String>,
    id_saved: bool,
    restart: Restart,
    advanced: bool,
    /// O arquivo da conexão não saiu do disco: a ação não aconteceu. Cada ação mostra a sua falha junto do próprio botão.
    leave_error: Option<(Leave, String)>,
}

impl Drop for Machines {
    fn drop(&mut self) { if let Some(task) = self.restart.task.take() { task.abort(); } }
}

impl Machines {
    fn id_value(&self, cx: &App) -> String { self.id_input.as_ref().map(|(i, _)| i.read(cx).value().trim().to_owned()).unwrap_or_default() }
    fn id_changed(&self, cx: &App) -> bool { self.id.ok().is_some_and(|loaded| *loaded != self.id_value(cx)) }
    /// O identificador salvo no servidor (vazio enquanto não leu).
    fn id_loaded(&self) -> &str { self.id.ok().map(String::as_str).unwrap_or_default() }
}

impl Hangar {
    fn machines_send_later(&self) -> impl Fn(MachinesReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Machines(reply) }).await; })
        }
    }

    /// Página aberta: relê identificador e alcance. Como o web remonta a tela, o resultado de um reinício anterior sai.
    pub(super) fn machines_opened(&mut self, cx: &mut Context<Self>) {
        let m = &mut self.machines;
        if let Some(task) = m.restart.task.take() { task.abort(); }
        m.restart = Restart { seq: m.restart.seq + 1, ..Restart::default() };
        (m.id_saved, m.leave_error) = (false, None);
        if !m.id_saving { self.load_machine_id(cx); }
        self.load_reach(cx);
    }

    fn load_reach(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.machines.reach.start();
        let done = self.machines_send_later();
        // O servidor testa cada endereço antes de responder.
        self.runtime.spawn(async move { done(MachinesReply::Reach(seq, api.server_read(&["alcance"], &[], 30).await)).await });
        cx.notify();
    }

    fn load_machine_id(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.machines.id.start();
        self.machines.id_error = None;
        let done = self.machines_send_later();
        self.runtime.spawn(async move { done(MachinesReply::Id(seq, api.server_read(&["peers", "identificador"], &[], 15).await)).await });
        cx.notify();
    }

    /// Salvar é um botão, não sair do campo: o nome é como as outras máquinas chegam aqui, e vai para o .env.
    fn save_machine_id(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let value = self.machines.id_value(cx);
        let m = &mut self.machines;
        if m.id_saving || !m.id_changed(cx) { return; }
        if !value.is_empty() && !valid_id(&value) { m.id_error = Some(id_hint()); cx.notify(); return; }
        m.id_seq += 1;
        (m.id_saving, m.id_error, m.id_saved) = (true, None, false);
        let (seq, done) = (m.id_seq, self.machines_send_later());
        self.runtime.spawn(async move {
            let body = json!({"identificador": value});
            done(MachinesReply::IdSaved(seq, api.server_send(reqwest::Method::PUT, &["peers", "identificador"], Some(body), 15).await)).await
        });
        cx.notify();
    }

    fn undo_machine_id(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let loaded = self.machines.id_loaded().to_owned();
        if let Some((input, _)) = &self.machines.id_input { input.update(cx, |state, cx| state.set_value(loaded, window, cx)); }
        self.machines.id_error = None;
        cx.notify();
    }

    fn restart_service(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let r = &mut self.machines.restart;
        if r.asking || r.waiting { return; }
        r.seq += 1;
        (r.asking, r.at, r.error, r.refused) = (true, None, None, false);
        let (seq, done) = (r.seq, self.machines_send_later());
        self.runtime.spawn(async move { done(MachinesReply::Restart(seq, api.server_post(&["atualizacao", "reiniciar"], 30).await)).await });
        cx.notify();
    }

    /// Pedir não é reiniciar: quem diz que o serviço voltou é o estado gravado pelo motor que este pedido lançou.
    fn wait_restart(&mut self, pid: Option<i64>) {
        let Some(api) = self.api.clone() else {
            let r = &mut self.machines.restart;
            (r.waiting, r.error) = (false, Some(tr("machines_restart_unconfirmed")));
            return;
        };
        let (seq, done) = (self.machines.restart.seq, self.machines_send_later());
        self.machines.restart.task = Some(self.runtime.spawn(async move {
            let deadline = Instant::now() + RESTART_WAIT;
            let end = loop {
                if Instant::now() >= deadline { break RestartEnd::Unconfirmed; }
                tokio::time::sleep(RESTART_POLL).await;
                // Servidor caído no meio do reinício é o esperado: pergunta de novo.
                let Ok(value) = api.server_read(&["atualizacao"], &[], 10).await else { continue };
                let state = &value["estado"];
                if state["pid"].as_i64() != pid || state["fase"].as_str() != Some("pronto") { continue; }
                let text = |k: &str| state[k].as_str().filter(|t| !t.is_empty()).map(str::to_owned);
                break if state["ok"].as_bool() == Some(true) { RestartEnd::Done(text("ts")) }
                    else { RestartEnd::Failed(text("reinicio_erro").or_else(|| text("erro"))) };
            };
            done(MachinesReply::RestartEnd(seq, end)).await
        }));
    }

    pub(super) fn receive_machines(&mut self, reply: MachinesReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            MachinesReply::Reach(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).and_then(|v| parse_reach(&v).ok_or_else(|| tr("invalid_response")));
                self.machines.reach.finish(seq, parsed);
            }
            MachinesReply::Id(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e))
                    .and_then(|v| v.get("identificador").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| tr("invalid_response")));
                let loaded = parsed.as_ref().ok().cloned();
                if !self.machines.id.finish(seq, parsed) { return; }
                if let Some(loaded) = loaded { self.show_machine_id(loaded, window, cx); }
            }
            MachinesReply::IdSaved(seq, result) => {
                let m = &mut self.machines;
                if seq != m.id_seq { return; }
                m.id_saving = false;
                // O motivo do servidor, como o web; o texto de "entrega incerta" é do envio de mensagens.
                match result.map_err(|e| Self::fetch_failure(&e))
                    .and_then(|v| v.get("identificador").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| tr("invalid_response"))) {
                    Ok(saved) => {
                        // Mais novo que qualquer leitura em voo: ela passa a ser descartada.
                        let newest = m.id.start();
                        m.id.finish(newest, Ok(saved.clone()));
                        m.id_saved = true;
                        self.show_machine_id(saved, window, cx);
                    }
                    // A recusa do backend ("identificador invalido…") chega como veio; o digitado fica no campo.
                    Err(error) => m.id_error = Some(error),
                }
            }
            MachinesReply::Restart(seq, result) => {
                let r = &mut self.machines.restart;
                if seq != r.seq { return; }
                r.asking = false;
                match result {
                    Ok(value) => {
                        r.waiting = true;
                        self.wait_restart(value.get("pid").and_then(Value::as_i64));
                    }
                    Err(error) => {
                        r.refused = error.status == Some(409);
                        r.error = Some(Self::fetch_failure(&error));
                    }
                }
            }
            MachinesReply::RestartEnd(seq, end) => {
                let r = &mut self.machines.restart;
                if seq != r.seq { return; }
                (r.waiting, r.task) = (false, None);
                match end {
                    RestartEnd::Done(ts) => {
                        let local = ts.and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok()).map(|t| t.with_timezone(&chrono::Local))
                            .unwrap_or_else(chrono::Local::now);
                        r.at = Some(local.format("%H:%M:%S").to_string());
                    }
                    RestartEnd::Failed(error) => r.error = Some(error.unwrap_or_else(|| tr("machines_restart_failed"))),
                    RestartEnd::Unconfirmed => r.error = Some(tr("machines_restart_unconfirmed")),
                }
            }
        }
        cx.notify();
    }

    /// O campo do identificador com o valor salvo no servidor.
    fn show_machine_id(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.machines.id_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("machines_id_placeholder")));
            let sub = cx.subscribe_in(&input, window, |this: &mut Hangar, _, event: &InputEvent, _, cx| match event {
                // Só o que a pessoa digita apaga o "salvo" e o erro; o valor que chegou do servidor não.
                InputEvent::Change if this.machines.id_changed(cx) => { (this.machines.id_error, this.machines.id_saved) = (None, false); cx.notify(); }
                InputEvent::PressEnter { .. } => this.save_machine_id(cx),
                _ => {}
            });
            self.machines.id_input = Some((input, sub));
        }
        if let Some((input, _)) = &self.machines.id_input { input.update(cx, |state, cx| state.set_value(value, window, cx)); }
        cx.notify();
    }

    /// Sair e remover a última máquina: o endereço e o token guardados saem do disco e a conexão acaba. Falso quando o arquivo
    /// ficou; aí nada muda na tela além do aviso.
    fn forget_connection(&mut self, leave: Leave, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.machines.leave_error = None;
        let removed = saved_connection_path().map_or(Ok(()), |path| match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        });
        if removed.is_err() {
            let key = match leave { Leave::SignOut => "machines_sign_out_error", Leave::Remove => "machines_remove_error" };
            self.machines.leave_error = Some((leave, tr(key)));
            cx.notify();
            return false;
        }
        self.drop_connection(window, cx);
        (self.api, self.server, self.unsaved_connection) = (None, None, None);
        self.reset_device(cx);
        self.server_config.reconnected(String::new());
        // Voltar pede o token de novo, como no web.
        self.token.update(cx, |input, cx| input.set_value("", window, cx));
        self.close_settings(window, cx);
        self.open_connection(window, cx);
        true
    }

    fn confirm_leave(&mut self, leave: Leave, window: &mut Window, cx: &mut Context<Self>) {
        let (title, description, ok) = if leave == Leave::Remove {
            (tr("machines_remove_title").replace("{nome}", &self.server_label(cx)),
                format!("{} {}", tr("machines_remove_token"), tr("machines_back_needs")), tr("machines_remove_ok"))
        } else {
            (tr("machines_sign_out_title"), tr("machines_back_needs"), tr("machines_sign_out"))
        };
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert.title(SharedString::from(title.clone())).description(SharedString::from(description.clone()))
                .button_props(DialogButtonProps::default().show_cancel(true).ok_text(ok.clone()).ok_variant(ButtonVariant::Danger)
                    .cancel_text(tr("cancel")))
                .on_ok(move |_, window, cx| {
                    // Saiu: o detalhe e esta pergunta fecham juntos. Não saiu: o aviso aparece onde se clicou.
                    let left = this.update(cx, |this, cx| this.forget_connection(leave, window, cx)).unwrap_or(false);
                    if left { window.close_all_dialogs(cx); }
                    !left
                })
        });
    }

    fn open_machine_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hangar = cx.entity();
        let detail = cx.new(|cx| MachineDetail { _observe: cx.observe(&hangar, |_, _, cx| cx.notify()), hangar: hangar.downgrade() });
        // Enter no diálogo é o "confirmar" do kit, que fecharia o detalhe; aqui Enter só salva o identificador (no campo dele).
        window.open_dialog(cx, move |dialog, _, _| dialog.w(px(600.)).child(detail.clone()).on_ok(|_, _, _| false));
    }

    pub(super) fn render_machines(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.machines;
        let next = tr("settings_next_version");
        let soon = |id: &'static str, label: String, icon: IconName, primary: bool| {
            let button = Button::new(id).small().icon(icon).label(label.clone()).disabled(true).accessibility_label(format!("{label}. {next}"));
            let next = next.clone();
            div().id(SharedString::from(format!("{id}-wrap"))).child(if primary { button.primary() } else { button.outline() })
                .tooltip(move |window, cx| Tooltip::new(next.clone()).build(window, cx))
        };
        let top = div().flex().items_center().gap(px(8.))
            .child(div().flex_1().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Servers.title()))
            .child(self.mark(div().rounded(px(8.)).child(soon("machines-add", tr("machines_add_device"), IconName::Plus, false)),
                "machines_search_tailscale"))
            .child(soon("machines-pair", tr("machines_pair"), IconName::Smartphone, true));
        if self.api.is_none() {
            return div().flex().flex_col().child(top).child(div().mt_4().text_sm().text_color(theme::muted()).child(tr("settings_offline")))
                .into_any_element();
        }
        let (summary, light) = match &m.reach.value {
            _ if m.reach.loading => (tr("machines_testing"), Light::Test),
            Some(Ok(reach)) => reach.summary(),
            Some(Err(error)) => (error.clone(), Light::No),
            None => (tr("machines_testing"), Light::Test),
        };
        let id = m.id_loaded().to_owned();
        let name = self.server_label(cx);
        let no_id = m.id.ok().is_some_and(String::is_empty);
        // O nome do botão substitui o conteúdo para o leitor de tela: o estado que o farol pinta vai junto.
        let spoken = [Some(tr("machines_open").replace("{nome}", &name)), (!id.is_empty()).then(|| id.clone()),
            Some(format!("{} · {summary}", tr("machines_this_server"))), no_id.then(|| tr("machines_no_id_short"))]
            .into_iter().flatten().collect::<Vec<_>>().join(". ");
        // Buscar um ajuste: identificador e origens moram no detalhe, e a busca (como no web) só abre a página e aponta o cartão.
        let card_hit = self.search_hit("machines_id") || self.search_hit("server_term_origins");
        let card = Button::new("machines-this")
            .custom(ButtonCustomVariant::new(cx).color(if card_hit { theme::accent_dim() } else { theme::boxed() }).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
            .w_full().h_auto().min_h(px(60.)).px_4().py(px(12.)).rounded(px(14.)).border_1().border_color(theme::border())
            .accessibility_label(spoken)
            .child(div().w_full().flex().items_center().gap(px(12.))
                .child(div().w(px(16.)).flex_shrink_0().text_center().text_size(px(14.)).text_color(light.color()).child(light.glyph()))
                .child(div().flex_1().min_w_0().flex().flex_col().items_start().gap(px(2.))
                    .child(div().flex().items_center().gap(px(8.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                        .when(!id.is_empty(), |el| el.child(div().px(px(8.)).rounded_full().border_1().border_color(theme::border())
                            .font_family(theme::MONO).text_size(px(11.)).text_color(theme::muted()).child(id.clone()))))
                    .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
                        .child(format!("{} · {summary}", tr("machines_this_server"))))
                    .when(no_id, |el| el.child(div().text_size(px(12.5)).text_color(theme::warning())
                        .child(tr("machines_no_id_short")))))
                .child(chrome::small_icon(IconName::ChevronRight, 16., theme::muted())));
        let this = cx.entity().downgrade();
        let card = FocusOnClick { id: "machines-this", button: card, open: Rc::new(move |window, cx| {
            let _ = this.update(cx, |this, cx| this.open_machine_detail(window, cx));
        }) };
        let card = self.mark(self.mark(div().rounded(px(14.)).child(card), "machines_id"), "server_term_origins");
        let others = self.mark(div().mt(px(28.)).mb(px(10.)).rounded(px(6.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD)
            .child(tr("machines_others")), "machines_others");
        let sign_out_error = m.leave_error.clone().filter(|(leave, _)| *leave == Leave::SignOut).map(|(_, error)| error);
        let actions = div().mt(px(20.)).flex().flex_col().gap(px(8.))
            .when_some(sign_out_error, |el, error| el.child(div().id("machines-sign-out-error").role(Role::Alert)
                .text_size(px(12.5)).text_color(theme::danger()).child(error)))
            .child(div().flex().items_center().justify_between()
                .child(self.mark(div().rounded(px(8.)).child(Button::new("machines-reconnect").ghost().small().icon(IconName::RefreshCw)
                    .label(tr("machines_reconnect")).on_click(cx.listener(|this, _, window, cx| this.connect(window, cx)))), "machines_reconnect"))
                .child(self.mark(div().rounded(px(8.)).child(Button::new("machines-sign-out").ghost().small().icon(IconName::LogOut)
                    .label(tr("machines_sign_out")).text_color(theme::danger())
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_leave(Leave::SignOut, window, cx)))), "machines_sign_out_title")));
        div().flex().flex_col().child(top)
            .child(div().mt(px(24.)).child(card))
            .when_some(m.id.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.id.loading).cloned(), |el, error| el.child(div().id("machines-id-read-error")
                .role(Role::Alert).mt(px(8.)).text_size(px(12.5)).text_color(theme::danger()).child(error)))
            .child(others)
            .child(settings_box().child(div().px_4().py(px(12.)).text_sm().text_color(theme::muted()).child(next.clone())))
            .child(actions)
            .into_any_element()
    }

    fn render_machine_detail(&mut self, cx: &mut Context<Self>) -> Div {
        let m = &self.machines;
        let id = m.id_loaded().to_owned();
        let section = |key: &str| div().mt(px(22.)).mb(px(8.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr(key));
        let muted = |text: String| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text);

        // Identificador: o CP_SERVER_ID do .env. Vazio, os outros servidores não conseguem registrar este.
        let changed = m.id_changed(cx);
        let id_ready = m.id.ok().is_some() && !m.id.loading;
        // O campo não tem descrição no kit: por que está desligado, ou o erro dele, vai no nome.
        let id_label = match (id_ready, m.id_saving, &m.id_error) {
            (false, ..) => format!("{}. {}", tr("machines_id"), tr("server_loading")),
            (_, true, _) => format!("{}. {}", tr("machines_id"), tr("server_saving")),
            (_, _, Some(error)) => format!("{}. {error}", tr("machines_id")),
            _ => tr("machines_id"),
        };
        let id_row = div().flex().items_center().gap(px(12.))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().flex().items_center().gap(px(8.)).child(div().font_weight(FontWeight::MEDIUM).child(tr("machines_id")))
                    .child(div().px(px(6.)).rounded_full().bg(theme::raised()).text_size(px(10.5)).font_weight(FontWeight::BOLD)
                        .text_color(theme::muted()).child(tr("machines_scope_env"))))
                .child(muted(if id.is_empty() { id_hint() } else { tr("machines_id_set").replace("{nome}", &id) })))
            .children(m.id_input.as_ref().map(|(input, _)| div().w(px(200.)).flex_shrink_0()
                .child(Input::new(input).small().font_family(theme::MONO).disabled(!id_ready || m.id_saving).aria_label(id_label))));
        let id_state = if changed || m.id_saving {
            Some(div().flex().items_center().gap(px(8.))
                .child(Button::new("machines-id-save").primary().small().label(tr(if m.id_saving { "server_saving" } else { "server_save" }))
                    .loading(m.id_saving).disabled(m.id_saving).on_click(cx.listener(|this, _, _, cx| this.save_machine_id(cx))))
                .child(Button::new("machines-id-undo").ghost().small().label(tr("server_undo")).disabled(m.id_saving)
                    .on_click(cx.listener(|this, _, window, cx| this.undo_machine_id(window, cx))))
                .into_any_element())
        } else if m.id_saved {
            Some(div().id("machines-id-saved").role(Role::Status).text_size(px(12.5)).text_color(theme::success()).child(tr("machines_id_saved"))
                .into_any_element())
        } else { None };
        let id_read_error = m.id.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.id.loading).cloned();
        let identifier = div().flex().flex_col().gap(px(8.))
            .when(id_ready && id.is_empty(), |el| el.child(muted(tr("machines_id_legend")))
                .child(div().text_size(px(12.5)).text_color(theme::warning()).whitespace_normal().child(tr("machines_id_unset"))))
            .child(id_row)
            .children(id_state)
            .when_some(m.id_error.clone(), |el, error| el.child(div().id("machines-id-error").role(Role::Alert).text_size(px(12.5))
                .text_color(theme::danger()).whitespace_normal().child(error)))
            // Sem leitura boa o campo não grava: o vazio de uma falha apagaria o nome que o servidor tem.
            .when_some(id_read_error, |el, error| el.child(div().flex().items_center().gap(px(10.))
                .child(div().id("machines-id-load-error").role(Role::Alert).flex_1().text_size(px(12.5)).text_color(theme::danger())
                    .whitespace_normal().child(error))
                .child(Button::new("machines-id-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_machine_id(cx))))));

        // Endereços medidos pelo servidor; enquanto a medida não chega, as duas linhas que todo servidor tem aparecem testando.
        let line = |reach: &Reach, a: &Address, n: usize| {
            let light = reach.light(a);
            let text_color = match light { Light::Ok => theme::success(), Light::No => theme::danger(), _ => theme::muted() };
            let copy = (a.status == Status::Ok && a.kind != Kind::Here).then(|| {
                let url = a.url.clone();
                Button::new(SharedString::from(format!("machines-copy-{n}"))).ghost().small().icon(IconName::Copy).label(tr("machines_copy"))
                    .accessibility_label(format!("{} {}", tr("machines_copy"), a.kind.name()))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(url.clone())))
            });
            div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_start().gap(px(12.)).px_4().py(px(12.))
                .child(div().w(px(16.)).flex_shrink_0().text_center().text_size(px(14.)).text_color(light.color()).child(light.glyph()))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(a.kind.name()))
                    .child(div().font_family(theme::MONO).text_size(px(12.5)).text_color(theme::muted()).truncate()
                        .child(if a.status == Status::Unset { tr("machines_unset") } else { a.url.clone() }))
                    .child(div().text_size(px(12.5)).text_color(text_color).whitespace_normal().child(reach.phrase(a)))
                    .when(a.kind == Kind::Tailscale && reach.public_same(), |el| el.child(muted(tr("machines_public_same")))))
                .children(copy.map(|c| div().flex_shrink_0().child(c)))
        };
        let testing = Reach { addresses: [Kind::Lan, Kind::Public].map(|kind| Address { kind, url: String::new(), status: Status::Testing, ms: None })
            .to_vec(), ..Reach::default() };
        let reach = m.reach.ok().filter(|_| !m.reach.loading).cloned();
        let reach_error = m.reach.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.reach.loading).cloned();
        let addresses = settings_box().map(|el| match (&reach, &reach_error) {
            (_, Some(error)) => el.child(div().id("machines-reach-error").role(Role::Alert).flex().items_center().gap(px(10.)).px_4().py(px(12.))
                .child(div().flex_1().text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error.clone()))
                .child(Button::new("machines-reach-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_reach(cx))))),
            (Some(reach), None) => el.children(reach.addresses.iter().filter(|a| reach.main(a)).enumerate().map(|(n, a)| line(reach, a, n))
                .collect::<Vec<_>>()),
            (None, None) => el.children(testing.addresses.iter().enumerate().map(|(n, a)| line(&testing, a, n)).collect::<Vec<_>>()),
        });
        let verdict = reach.as_ref().map(|reach| {
            let outside = reach.outside();
            let lan = reach.lan();
            let point = |light: Light, bold: String, rest: String| div().flex().items_start().gap(px(8.)).text_size(px(13.))
                .child(div().w(px(14.)).flex_shrink_0().text_color(light.color()).child(light.glyph()))
                .child(div().flex_1().min_w_0().whitespace_normal().child(div().font_weight(FontWeight::SEMIBOLD).child(bold))
                    .when(!rest.is_empty(), |el| el.child(div().text_color(theme::muted()).child(rest))));
            let isolated = outside.is_none() && lan.is_none();
            let body = if isolated {
                div().flex().flex_col().gap(px(6.))
                    .child(point(Light::No, tr("machines_verdict_none"), tr("machines_verdict_none_why").replace("{endereco}", &reach.bind)))
                    .child(muted(tr("machines_verdict_way_out").replace("{variavel}", "CP_LAN_BIND_IP").replace("{valor}", "auto")))
            } else {
                let (out_bold, out_rest) = match outside {
                    Some(a) => (tr("machines_verdict_out_ok"), tr("machines_verdict_out_how").replace("{rede}", &a.kind.name())
                        .replace("{tempo}", &format!("{} ms", a.ms.unwrap_or(0)))),
                    None => (tr("machines_verdict_out_no"), tr("machines_verdict_out_no_why")),
                };
                let (lan_bold, lan_rest) = match (lan, outside) {
                    (Some(_), _) => (tr("machines_verdict_lan_ok"), String::new()),
                    (None, Some(a)) => (tr("machines_verdict_lan_no"), tr("machines_verdict_lan_no_ok").replace("{rede}", &a.kind.name())),
                    (None, None) => (tr("machines_verdict_lan_no"), String::new()),
                };
                div().flex().flex_col().gap(px(6.))
                    .child(point(if outside.is_some() { Light::Ok } else { Light::Neutral }, out_bold, out_rest))
                    .child(point(if lan.is_some() { Light::Ok } else { Light::Neutral }, lan_bold, lan_rest))
                    .when(lan.is_none() && reach.loopback, |el| el.child(muted(tr("machines_verdict_want_lan")
                        .replace("{variavel}", "CP_LAN_BIND_IP").replace("{valor}", "auto"))))
            };
            div().p(px(14.)).rounded(px(12.)).border_1().border_color(if isolated { theme::danger() } else { theme::border() }).bg(theme::inset())
                .flex().flex_col().gap(px(8.))
                .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr("machines_verdict")))
                .child(body)
        });

        // Reiniciar não é avançado: é o gesto que faz valer o identificador e as outras chaves do .env.
        let r = &m.restart;
        let busy = r.asking || r.waiting;
        // A ponte do app do computador só existe no Electron; o web a mostra para o servidor desta máquina, depois de um erro
        // que não é recusa.
        let local = self.address.read(cx).value().trim().parse::<url::Url>().ok()
            .and_then(|u| u.host_str().map(|h| matches!(h, "localhost" | "[::1]" | "::1") || h.starts_with("127.")))
            .unwrap_or(false);
        let desktop_note = tr("settings_next_version");
        let service = div().flex().flex_col().gap(px(8.))
            .child(muted(tr("machines_service_help")))
            .child(div().flex().flex_wrap().items_center().gap(px(10.))
                .child(Button::new("machines-restart").primary().small().icon(IconName::RotateCw)
                    .label(tr(if busy { "machines_restarting" } else { "machines_restart" })).loading(busy).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.restart_service(cx))))
                .when(r.waiting, |el| el.child(div().id("machines-restart-waiting").role(Role::Status).text_size(px(12.5))
                    .text_color(theme::muted()).child(tr("machines_restart_waiting"))))
                .when_some(r.at.clone(), |el, at| el.child(div().id("machines-restarted").role(Role::Status).text_size(px(12.5))
                    .text_color(theme::success()).child(tr("machines_restarted").replace("{hora}", &at)))))
            .when_some(r.error.clone(), |el, error| el.child(div().id("machines-restart-error").role(Role::Alert).text_size(px(12.5))
                    .text_color(theme::danger()).whitespace_normal().child(error))
                .when(!r.refused, |el| el.child(muted(tr("machines_restart_stuck"))))
                .when(!r.refused && local, |el| el.child(div().flex().items_center().gap(px(8.))
                    .child(Button::new("machines-restart-desktop").outline().small().label(tr("machines_restart_desktop")).disabled(true)
                        .accessibility_label(format!("{}. {desktop_note}", tr("machines_restart_desktop"))))
                    .child(muted(desktop_note.clone())))));

        let this = cx.entity().downgrade();
        let advanced_open = m.advanced;
        let advanced = div().mt(px(18.)).flex().flex_col().gap(px(10.))
            .child(div().flex().child(super::settings::Disclosure::new("machines-advanced", advanced_open, tr("machines_advanced"), false)
                .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.machines.advanced = open; cx.notify(); }); })))
            .when(advanced_open, |el| {
                let extras = reach.as_ref().map(|reach| reach.extras().into_iter().cloned().collect::<Vec<_>>()).unwrap_or_default();
                let bind = reach.as_ref().map(|r| r.bind.clone()).filter(|b| !b.is_empty());
                el.when(!extras.is_empty(), |el| {
                    let reach = reach.clone().unwrap_or_default();
                    el.child(settings_box().children(extras.iter().enumerate().map(|(n, a)| line(&reach, a, 100 + n)).collect::<Vec<_>>()))
                })
                .when_some(bind, |el, bind| el.child(muted(tr("machines_listening").replace("{ip}", &bind))))
                .child(self.term_origins_block(cx))
            });

        let remove_error = m.leave_error.clone().filter(|(leave, _)| *leave == Leave::Remove).map(|(_, error)| error);
        let remove = div().mt(px(22.)).flex().flex_col().items_end().gap(px(6.))
            .when_some(remove_error, |el, error| el.child(div().id("machines-remove-error").role(Role::Alert)
                .text_size(px(12.5)).text_color(theme::danger()).child(error)))
            .child(Button::new("machines-remove").ghost().small().label(tr("machines_remove_here")).text_color(theme::danger())
                .on_click(cx.listener(|this, _, window, cx| this.confirm_leave(Leave::Remove, window, cx))));

        let head = div().pr(px(28.)).mb(px(16.)).flex().flex_col().gap(px(2.))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(self.server_label(cx)))
            .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("machines_this_server")));
        div().flex().flex_col().pb(px(8.))
            .child(head)
            .child(identifier)
            .children(verdict.map(|v| div().mt(px(18.)).child(v)))
            .child(section("machines_addresses"))
            .child(addresses)
            .child(section("machines_service"))
            .child(service)
            .child(advanced)
            .child(remove)
    }
}

/// O `Button` do kit não toma foco no clique, e o diálogo devolve ao fechar o foco de quem o abriu: sem focar o cartão antes,
/// o Esc deixaria o foco na página. O foco é o que o próprio botão guarda, achado pelo mesmo caminho na árvore (o gpui põe
/// o nome do tipo do componente no caminho antes de desenhá-lo).
#[derive(IntoElement)]
struct FocusOnClick {
    id: &'static str,
    button: Button,
    open: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl RenderOnce for FocusOnClick {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window.with_id(std::any::type_name::<Button>(), |window| {
            window.use_keyed_state(self.id, cx, |_, cx| cx.focus_handle()).read(cx).clone()
        });
        let open = self.open;
        self.button.on_click(move |_, window, cx| {
            focus.focus(window, cx);
            open(window, cx);
        })
    }
}

/// O corpo do diálogo do detalhe. O kit chama o construtor do diálogo na hora de abrir, com o `Hangar` ainda em
/// atualização; esta vista só lê o `Hangar` ao desenhar, e redesenha quando ele muda.
struct MachineDetail {
    hangar: WeakEntity<Hangar>,
    _observe: Subscription,
}

impl Render for MachineDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.hangar.update(cx, |hangar, cx| hangar.render_machine_detail(cx)).unwrap_or_else(|_| div())
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, Light, Reach, parse_reach, valid_id};
    use crate::i18n::tr;

    fn reach(text: &str) -> Reach { parse_reach(&serde_json::from_str(text).expect("JSON")).expect("formato do /api/alcance") }

    #[test]
    fn summary_and_verdict_follow_the_measure_like_the_web() {
        let r = reach(r#"{"loopback": false, "bind": "0.0.0.0", "enderecos": [
            {"tipo": "rede_local", "url": "http://192.168.0.2:8765", "estado": "ok", "tempo_ms": 9},
            {"tipo": "tailscale", "url": "https://casa.ts.net", "estado": "ok", "tempo_ms": 3.4},
            {"tipo": "publico", "url": "https://casa.ts.net", "estado": "ok", "tempo_ms": 2}]}"#);
        // Público igual ao Tailscale não conta duas vezes nem vira caminho próprio.
        assert!(r.public_same());
        assert_eq!(r.outside().map(|a| a.kind), Some(Kind::Tailscale));
        assert_eq!(r.addresses.iter().filter(|a| r.main(a)).map(|a| a.kind).collect::<Vec<_>>(), [Kind::Lan, Kind::Tailscale]);
        assert!(r.extras().is_empty());
        // O cartão resume pelo mais rápido que respondeu, e o público repetido entra na conta como no web.
        assert_eq!(r.summary().1, Light::Ok);

        let isolated = reach(r#"{"loopback": true, "bind": "127.0.0.1", "enderecos": [
            {"tipo": "nesta_maquina", "url": "http://127.0.0.1:8765", "estado": "ok", "tempo_ms": 1},
            {"tipo": "rede_local", "url": "http://192.168.0.2:8765", "estado": "falhou", "tempo_ms": 3},
            {"tipo": "publico", "url": "", "estado": "nao_configurado", "tempo_ms": null}]}"#);
        assert!(isolated.outside().is_none() && isolated.lan().is_none());
        assert_eq!(isolated.summary(), (tr("machines_loopback_short"), Light::No));
        // Rede local fechada por escolha (loopback) é neutra; "nesta máquina" e o público vazio vão para o Avançado.
        let lan = &isolated.addresses[1];
        assert_eq!(isolated.light(lan), Light::Neutral);
        assert_eq!(isolated.extras().iter().map(|a| a.kind).collect::<Vec<_>>(), [Kind::Here, Kind::Public]);
    }

    #[test]
    fn identifier_follows_the_backend_rule() {
        for ok in ["casa", "notebook-2", "a", "x_y", &"a".repeat(32)] { assert!(valid_id(ok), "{ok}"); }
        for bad in ["", "Casa", "-casa", "_x", "casa nova", "ção", &"a".repeat(33)] { assert!(!valid_id(bad), "{bad}"); }
    }
}
