//! Página Máquinas (web: "Servidores"): o servidor conectado vira um cartão, e o detalhe dele traz identificador, endereços,
//! reinício do serviço e o Avançado. Porta de `MaquinasSettings.svelte` e da parte "detalhe" de `AcessoSettings.svelte`.
//! O nativo fala com um servidor só: o que depende de guardar outras máquinas neste aparelho chega depois.
mod add;
mod pair;

use super::*;
use std::{collections::HashMap, rc::Rc};
use super::device::Remote;
use add::{AddMachine, Found};
use pair::Pair;
use super::server_config::chip;
use super::settings::{settings_box, Page};
use gpui_kit::component::{WindowExt, switch::Switch, tooltip::Tooltip};

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
    /// O nome do tipo nas rotas do servidor (`/api/alcance/pareamento?endereco=`).
    fn raw(self) -> &'static str {
        match self { Kind::Here => "nesta_maquina", Kind::Lan => "rede_local", Kind::Tailscale => "tailscale", Kind::Public => "publico" }
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

/// Uma outra máquina do `peers.json` deste servidor, como `/api/peers` a devolve. O token vem mascarado e não é lido: o nativo não
/// guarda token de outra máquina, e toda linha é a "só no servidor" do web (`navegador: null`).
#[derive(Clone, Debug)]
struct Peer { id: String, url: String, enabled: bool }

fn parse_peers(value: &Value) -> Option<Vec<Peer>> {
    value.as_array()?.iter().map(|p| Some(Peer { id: p.get("id")?.as_str()?.to_owned(), url: p.get("base_url")?.as_str()?.to_owned(),
        enabled: p.get("enabled").and_then(Value::as_bool).unwrap_or(true) })).collect()
}

/// A ida (este servidor → ela), medida pelo servidor em `/api/peers/check`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Way { Ok, Other, Failed, Unset }

#[derive(Clone, Debug)]
pub(super) struct Going {
    way: Way,
    answered_as: String,
    ms: Option<i64>,
    /// O pedido do teste falhou neste servidor (não é a outra máquina que não respondeu): a frase dele vai no cartão.
    error: Option<String>,
}

fn parse_going(value: &Value) -> Going {
    let way = match value.get("estado").and_then(Value::as_str) {
        Some("ok") => Way::Ok, Some("estranho") => Way::Other, Some("falhou" | "recusou") => Way::Failed, _ => Way::Unset,
    };
    Going { way, answered_as: value.get("identificador").and_then(Value::as_str).unwrap_or_default().to_owned(),
        ms: value.get("tempo_ms").and_then(Value::as_f64).map(|ms| ms.round() as i64), error: None }
}

/// Medição de uma máquina, só em memória: cada abertura da página mede de novo.
#[derive(Default)]
struct Check { seq: u64, testing: bool, going: Option<Going>, at: Option<chrono::DateTime<chrono::Local>> }

impl Check {
    fn done(&self) -> Option<&Going> { self.going.as_ref().filter(|_| !self.testing) }
}

/// O que a linha da lista diz (`sessionsState` do web para quem não tem entrada no aparelho).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row { Testing, NoToken, Off, Silent }

fn row_state(peer: &Peer, check: Option<&Check>) -> Row {
    if !peer.enabled { return Row::Off; }
    match check.and_then(Check::done) { None => Row::Testing, Some(g) if g.way == Way::Ok => Row::NoToken, Some(_) => Row::Silent }
}

/// Desligada no servidor ou sem resposta: vai para "Não respondem", como no web (`recolhida`).
fn collapsed(row: Row) -> bool { matches!(row, Row::Off | Row::Silent) }

/// O cartão dos recados (`recadosCard` do web). Sem token no aparelho a volta nunca é medida, então "ok" não acontece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Card { Testing, MissingToken, GoingFailed, GoingOther, Paused }

fn card_state(peer: &Peer, check: Option<&Check>) -> Card {
    if !peer.enabled { return Card::Paused; }
    match check.and_then(Check::done).map(|g| g.way) {
        None => Card::Testing, Some(Way::Other) => Card::GoingOther, Some(Way::Failed) => Card::GoingFailed, Some(_) => Card::MissingToken,
    }
}

/// De onde saiu uma gravação de outra máquina: a falha aparece só junto desse controle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot { Row, Messages, TurnOn, Scan, Footer }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PeerWrite { Enabled(bool), Removed }

/// Como o reinício terminou, lido do estado que o motor grava (`fase: pronto`, casado pelo pid do pedido).
pub(super) enum RestartEnd { Done(Option<String>), Failed(Option<String>), Unconfirmed }

pub(super) enum MachinesReply {
    Reach(u64, Result<Value, Failure>),
    Id(u64, Result<Value, Failure>),
    IdSaved(u64, Result<Value, Failure>),
    Restart(u64, Result<Value, Failure>),
    RestartEnd(u64, RestartEnd),
    Peers(u64, Result<Value, Failure>),
    PeerCheck(String, u64, Result<Value, Failure>),
    PeerSaved(u64, PeerWrite, Result<Value, Failure>),
    /// Do diálogo Adicionar, pela entidade dele: a resposta de um diálogo já fechado não acha dono.
    Discovered(EntityId, u64, Result<Value, Failure>),
    Probed(EntityId, u64, Result<Found, String>),
    Registered(EntityId, u64, Result<(Going, Going), String>),
    Paired(u64, Result<Value, Failure>),
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
    /// As outras máquinas que este servidor conhece, e a medição de cada uma pelo id.
    peers: Remote<Vec<Peer>>,
    checks: HashMap<String, Check>,
    check_seq: u64,
    /// Uma gravação por vez: a máquina e o controle que a pediu. O `seq` descarta resposta de pedido velho.
    peer_busy: Option<(String, Spot)>,
    peer_seq: u64,
    peer_error: Option<(String, Spot, String)>,
    silent_open: bool,
    /// A máquina do detalhe aberto e o Avançado dele.
    peer_open: Option<String>,
    peer_advanced: bool,
    /// O diálogo Adicionar aberto.
    add: Option<Entity<AddMachine>>,
    pair: Pair,
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
        (m.id_saved, m.leave_error, m.peer_error) = (false, None, None);
        // "Não respondem" nasce fechado, como o `<details>` do web remontado.
        m.silent_open = false;
        // Medições só em memória: cada abertura mede de novo, e a resposta de um teste de antes cai pelo `seq`.
        m.checks.clear();
        if !m.id_saving { self.load_machine_id(cx); }
        self.load_reach(cx);
        self.load_peers(cx);
    }

    fn load_peers(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.machines.peers.start();
        let done = self.machines_send_later();
        self.runtime.spawn(async move { done(MachinesReply::Peers(seq, api.server_read(&["peers"], &[], 15).await)).await });
        cx.notify();
    }

    /// Mede a ida de uma máquina de novo. Uma medição em voo não é refeita (clique duplo, abrir o detalhe enquanto a lista mede).
    fn check_peer(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let m = &mut self.machines;
        let Some(url) = m.peers.ok().and_then(|list| list.iter().find(|p| p.id == id && p.enabled)).map(|p| p.url.clone()) else { return };
        if m.checks.get(id).is_some_and(|c| c.testing) { return; }
        m.check_seq += 1;
        let seq = m.check_seq;
        let check = m.checks.entry(id.to_owned()).or_default();
        (check.seq, check.testing) = (seq, true);
        let (id, done) = (id.to_owned(), self.machines_send_later());
        self.runtime.spawn(async move {
            let result = api.server_read(&["peers", "check"], &[("url", &url), ("id", &id)], 30).await;
            done(MachinesReply::PeerCheck(id, seq, result)).await
        });
        cx.notify();
    }

    /// PUT `/api/peers/{id}/enabled` ou DELETE `/api/peers/{id}`; as duas respondem a lista nova.
    fn write_peer(&mut self, id: String, spot: Spot, write: PeerWrite, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let m = &mut self.machines;
        if m.peer_busy.is_some() { return; }
        m.peer_seq += 1;
        (m.peer_busy, m.peer_error) = (Some((id.clone(), spot)), None);
        let (seq, done) = (m.peer_seq, self.machines_send_later());
        self.runtime.spawn(async move {
            let result = match write {
                PeerWrite::Enabled(on) => api.server_send(reqwest::Method::PUT, &["peers", &id, "enabled"], Some(json!({"enabled": on})), 15).await,
                PeerWrite::Removed => api.server_send(reqwest::Method::DELETE, &["peers", &id], None, 15).await,
            };
            done(MachinesReply::PeerSaved(seq, write, result)).await
        });
        cx.notify();
    }

    /// Remover pergunta antes, como o web; a pergunta diz que o lado de lá fica, porque este aparelho não tem o token dele.
    fn confirm_peer_removal(&mut self, id: String, spot: Spot, window: &mut Window, cx: &mut Context<Self>) {
        let title = tr("machines_peer_remove_title").replace("{nome}", &id);
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, title, tr("machines_peer_here_only"), tr("machines_peer_remove"), ButtonVariant::Danger,
            move |_, cx| {
                let _ = this.update(cx, |this, cx| this.write_peer(id.clone(), spot, PeerWrite::Removed, cx));
                true
            });
    }

    fn open_peer_detail(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        // Abrir o detalhe mede de novo: o resultado de antes era de outra hora.
        self.check_peer(&id, cx);
        let m = &mut self.machines;
        (m.peer_open, m.peer_advanced) = (Some(id.clone()), false);
        if m.peer_error.as_ref().is_some_and(|(_, spot, _)| *spot != Spot::Row) { m.peer_error = None; }
        let hangar = cx.entity();
        let detail = cx.new(|cx| PeerDetail { _observe: cx.observe(&hangar, |_, _, cx| cx.notify()), hangar: hangar.downgrade() });
        let weak = hangar.downgrade();
        // Fechado pela pessoa, o detalhe deixa de ser o diálogo do topo: um Remover que volta depois não fecha outro diálogo.
        window.open_dialog(cx, move |dialog, _, _| {
            let (weak, id) = (weak.clone(), id.clone());
            dialog.w(px(600.)).child(detail.clone()).on_ok(enter_to_focused)
                .on_close(move |_, _, cx| { let _ = weak.update(cx, |this, _| {
                    if this.machines.peer_open.as_deref() == Some(id.as_str()) { this.machines.peer_open = None; }
                }); })
        });
    }

    fn peer_error_at(&self, id: &str, spot: Spot) -> Option<String> {
        self.machines.peer_error.as_ref().filter(|(i, s, _)| i == id && *s == spot).map(|(.., error)| error.clone())
    }

    fn peer_busy_at(&self, id: &str, spot: Spot) -> bool {
        self.machines.peer_busy.as_ref().is_some_and(|(i, s)| i == id && *s == spot)
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
            MachinesReply::Peers(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).and_then(|v| parse_peers(&v).ok_or_else(|| tr("invalid_response")));
                if !self.machines.peers.finish(seq, parsed) { return; }
                self.peers_arrived(cx);
            }
            MachinesReply::PeerCheck(id, seq, result) => {
                let Some(check) = self.machines.checks.get_mut(&id).filter(|c| c.seq == seq) else { return };
                // Falha do próprio pedido é "não chegou", como o web faz com o erro do `checkPeer`.
                let going = result.map(|v| parse_going(&v)).unwrap_or_else(|error| Going { way: Way::Failed, answered_as: String::new(), ms: None,
                    error: Some(Self::fetch_failure(&error)) });
                (check.testing, check.going, check.at) = (false, Some(going), Some(chrono::Local::now()));
            }
            MachinesReply::PeerSaved(seq, write, result) => {
                let m = &mut self.machines;
                if seq != m.peer_seq { return; }
                let Some((id, spot)) = m.peer_busy.take() else { return };
                let parsed = result.map_err(|e| Self::fetch_failure(&e)).and_then(|v| parse_peers(&v).ok_or_else(|| tr("invalid_response")));
                match parsed {
                    Ok(list) => {
                        // A resposta é a lista nova do servidor: mais nova que qualquer leitura em voo.
                        let newest = m.peers.start();
                        m.peers.finish(newest, Ok(list));
                        // Religada: a medição de antes de desligar não vale para agora (sem isto o web fica em "Testando…").
                        if write == PeerWrite::Enabled(true) { m.checks.remove(&id); }
                        if write == PeerWrite::Removed {
                            m.checks.remove(&id);
                            // Saiu pelo detalhe (rodapé ou recados): o detalhe dela fecha, como no web.
                            if matches!(spot, Spot::Footer | Spot::Messages) && m.peer_open.as_deref() == Some(id.as_str()) {
                                m.peer_open = None;
                                window.close_dialog(cx);
                                // O kit devolve o foco à linha que abriu o detalhe, e ela acabou de sair: sem isto, com a
                                // resposta rápida o foco fica fora da árvore e o Esc não chega à página.
                                if !window.has_active_dialog(cx) { self.root_focus.focus(window, cx); }
                            }
                        }
                        self.peers_arrived(cx);
                    }
                    Err(error) => m.peer_error = Some((id, spot, error)),
                }
            }
            MachinesReply::Discovered(dialog, seq, result) => {
                let parsed = result.map_err(|e| Self::fetch_failure(&e)).and_then(|v| add::parse_discovered(&v).ok_or_else(|| tr("invalid_response")));
                if let Some(add) = self.add_dialog(dialog) { add.update(cx, |add, cx| add.discovered(seq, parsed, cx)); }
            }
            MachinesReply::Probed(dialog, seq, result) => {
                if let Some(add) = self.add_dialog(dialog) { add.update(cx, |add, cx| add.probed(seq, result, window, cx)); }
            }
            MachinesReply::Registered(dialog, seq, result) => {
                // Gravou aqui: a lista já tem a máquina, mesmo que o outro lado tenha falhado.
                if result.is_ok() { self.load_peers(cx); }
                let Some(add) = self.add_dialog(dialog).filter(|add| add.read(cx).waiting(seq)) else { return };
                if result.as_ref().is_ok_and(|(going, back)| going.way == Way::Ok && back.way == Way::Ok) {
                    // Em voo o diálogo não fecha nem tem outro por cima: ele é o do topo.
                    self.machines.add = None;
                    window.close_dialog(cx);
                } else {
                    add.update(cx, |add, cx| add.registered(seq, result, cx));
                }
            }
            MachinesReply::Paired(seq, result) => self.paired(seq, result, window),
        }
        cx.notify();
    }

    fn add_dialog(&self, dialog: EntityId) -> Option<Entity<AddMachine>> {
        self.machines.add.clone().filter(|add| add::owns(Some(add.entity_id()), dialog))
    }

    /// Foco que ficou sem dono na página (a linha saiu da lista, o botão sumiu): vai ao ancestral focável mais próximo que
    /// sobrou ou à raiz, onde o Esc fecha a página — o `fallbackFocus` do web.
    pub(super) fn machines_focus_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings != Some(Page::Servers) { return; }
        window.focus_lost_restore_target(cx).unwrap_or_else(|| self.root_focus.clone()).focus(window, cx);
    }

    /// Lista nova: some a medição de quem saiu, e toda máquina ligada sem medição (ou religada agora) é medida.
    fn peers_arrived(&mut self, cx: &mut Context<Self>) {
        let list = self.machines.peers.ok().cloned().unwrap_or_default();
        self.machines.checks.retain(|id, _| list.iter().any(|p| &p.id == id));
        for peer in list.iter().filter(|p| p.enabled) {
            if self.machines.checks.get(&peer.id).is_none_or(|c| c.going.is_none() && !c.testing) { self.check_peer(&peer.id, cx); }
        }
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
        chrome::confirm_alert(window, cx, title, description, ok, ButtonVariant::Danger, move |window, cx| {
            // Saiu: o detalhe e esta pergunta fecham juntos. Não saiu: o aviso aparece onde se clicou.
            let left = this.update(cx, |this, cx| this.forget_connection(leave, window, cx)).unwrap_or(false);
            if left { window.close_all_dialogs(cx); }
            !left
        });
    }

    fn open_machine_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hangar = cx.entity();
        let detail = cx.new(|cx| MachineDetail { _observe: cx.observe(&hangar, |_, _, cx| cx.notify()), hangar: hangar.downgrade() });
        // Enter no diálogo é o "confirmar" do kit, que fecharia o detalhe; aqui Enter só salva o identificador (no campo dele).
        window.open_dialog(cx, move |dialog, _, _| dialog.w(px(600.)).child(detail.clone()).on_ok(enter_to_focused));
    }

    pub(super) fn render_machines(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.machines;
        let offline = self.api.is_none();
        // Os dois abrem diálogo: o foco volta a eles no Esc.
        let this = cx.entity().downgrade();
        let add = FocusOnClick { id: "machines-add".into(), button: Button::new("machines-add").outline().small().icon(IconName::Plus)
            .label(tr("machines_add_device")).disabled(offline), open: Rc::new(move |window, cx| {
                let _ = this.update(cx, |this, cx| this.open_add_machine(window, cx));
            }) };
        let this = cx.entity().downgrade();
        let pair = FocusOnClick { id: "machines-pair".into(), button: Button::new("machines-pair").primary().small().icon(IconName::Smartphone)
            .label(tr("machines_pair")).disabled(offline), open: Rc::new(move |window, cx| {
                let _ = this.update(cx, |this, cx| this.open_pair(window, cx));
            }) };
        let top = div().flex().items_center().gap(px(8.))
            .child(div().flex_1().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Servers.title()))
            .child(self.mark(div().rounded(px(8.)).child(add), "machines_search_tailscale"))
            .child(pair);
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
        let card = FocusOnClick { id: "machines-this".into(), button: card, open: Rc::new(move |window, cx| {
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
            .child(self.render_peers(cx))
            .child(actions)
            .into_any_element()
    }

    /// "Máquinas neste aparelho": as outras máquinas deste servidor, e as que não respondem recolhidas embaixo (ListaMaquinas.svelte).
    fn render_peers(&mut self, cx: &mut Context<Self>) -> Div {
        let m = &self.machines;
        let rows = m.peers.ok().cloned().unwrap_or_default().into_iter()
            .map(|p| { let row = row_state(&p, m.checks.get(&p.id)); (p, row) }).collect::<Vec<_>>();
        let (silent, shown): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(_, row)| collapsed(*row));
        let error = m.peers.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.peers.loading).cloned();
        let note = |text: String| div().px_4().py(px(12.)).text_size(px(12.5)).text_color(theme::muted()).child(text);
        let list = settings_box().map(|el| match error {
            Some(error) => el.child(div().id("machines-peers-error").role(Role::Alert).flex().items_center().gap(px(10.)).px_4().py(px(12.))
                .child(div().flex_1().text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error))
                .child(Button::new("machines-peers-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_peers(cx))))),
            None if shown.is_empty() => el.child(note(if m.peers.loading { tr("server_loading") } else if !silent.is_empty() {
                tr("machines_peers_none_answering") } else { tr("machines_peers_empty") })),
            None => el.children(shown.iter().enumerate().map(|(n, (peer, row))| div().when(n > 0, |el| el.border_t_1().border_color(theme::border()))
                .child(self.peer_row(peer, *row, false, cx))).collect::<Vec<_>>()),
        });
        let open = m.silent_open;
        let this = cx.entity().downgrade();
        let silent_block = (!silent.is_empty()).then(|| div().mt(px(14.)).flex().flex_col().gap(px(8.))
            .child(div().flex().child(super::settings::Disclosure::new("machines-silent", open,
                tr("machines_peers_silent").replace("{n}", &silent.len().to_string()), false)
                .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.machines.silent_open = open; cx.notify(); }); })))
            .when(open, |el| el.child(settings_box().children(silent.iter().enumerate()
                .map(|(n, (peer, row))| div().when(n > 0, |el| el.border_t_1().border_color(theme::border())).child(self.peer_row(peer, *row, true, cx)))
                .collect::<Vec<_>>()))));
        div().flex().flex_col().child(list).children(silent_block)
    }

    /// Uma linha da lista: abre o detalhe. Recolhida ("não respondem"), ganha o Remover ao lado, como no web.
    fn peer_row(&self, peer: &Peer, row: Row, silent: bool, cx: &mut Context<Self>) -> Div {
        let (glyph, color, phrase) = match row {
            Row::Testing => ("◌", theme::muted(), tr("machines_testing")),
            Row::NoToken => ("●", theme::warning(), tr("machines_peer_no_token")),
            // O "·" do web some no desenho do app: o neutro é o mesmo ○ do farol deste servidor.
            Row::Off => (Light::Neutral.glyph(), theme::muted(), tr("machines_peer_off")),
            Row::Silent => (Light::Neutral.glyph(), theme::muted(), tr("machines_peer_silent")),
        };
        let id = peer.id.clone();
        let key = SharedString::from(format!("machines-peer-{id}"));
        let button = Button::new(key.clone())
            .custom(ButtonCustomVariant::new(cx).color(theme::boxed()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
            .w_full().h_auto().min_h(px(56.)).px_4().py(px(10.)).rounded(px(0.))
            .accessibility_label(format!("{}. {phrase}", tr("machines_open").replace("{nome}", &id)))
            .child(div().w_full().flex().items_center().gap(px(12.))
                .child(div().w(px(16.)).flex_shrink_0().text_center().text_size(px(14.)).text_color(color).child(glyph))
                .child(div().flex_1().min_w_0().flex().flex_col().items_start().gap(px(2.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(id.clone()))
                    .child(div().text_size(px(12.5)).text_color(if row == Row::NoToken { theme::warning() } else { theme::muted() })
                        .whitespace_normal().child(phrase)))
                .when(!silent, |el| el.child(chrome::small_icon(IconName::ChevronRight, 16., theme::muted()))));
        let this = cx.entity().downgrade();
        let open_id = id.clone();
        let line = FocusOnClick { id: key.into(), button, open: Rc::new(move |window, cx| {
            let _ = this.update(cx, |this, cx| this.open_peer_detail(open_id.clone(), window, cx));
        }) };
        if !silent { return div().child(line); }
        let key = SharedString::from(format!("machines-peer-{id}-remove"));
        let removing = self.peer_busy_at(&id, Spot::Row);
        let remove = Button::new(key.clone()).ghost().small().label(tr(if removing { "machines_peer_removing" } else { "machines_peer_remove" }))
            .text_color(theme::danger())
            .accessibility_label(if removing { tr("machines_peer_removing") } else { tr("machines_peer_remove_aria").replace("{nome}", &id) })
            // Gravando a própria remoção: carregando, não desligado. O botão desligado larga o foco, e o Esc não sobe mais.
            .loading(removing).disabled(self.machines.peer_busy.is_some() && !removing);
        let this = cx.entity().downgrade();
        let remove_id = id.clone();
        let remove = FocusOnClick { id: key.into(), button: remove, open: Rc::new(move |window, cx| {
            let _ = this.update(cx, |this, cx| this.confirm_peer_removal(remove_id.clone(), Spot::Row, window, cx));
        }) };
        div().flex().flex_col()
            .child(div().flex().items_center().child(div().flex_1().min_w_0().child(line)).child(div().pr(px(12.)).flex_shrink_0().child(remove)))
            .when_some(self.peer_error_at(&id, Spot::Row), |el, error| el.child(div().id(SharedString::from(format!("machines-peer-{id}-error")))
                .role(Role::Alert).px_4().pb(px(10.)).text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error)))
    }

    /// Detalhe de outra máquina (DetalheServidor.svelte, no caso "só no servidor"): o que o aparelho guarda dela, os recados e o avançado.
    fn render_peer_detail(&mut self, cx: &mut Context<Self>) -> Div {
        let m = &self.machines;
        let Some(peer) = m.peer_open.as_ref().and_then(|id| m.peers.ok()?.iter().find(|p| &p.id == id)).cloned() else { return div() };
        let check = m.checks.get(&peer.id);
        let (card, row) = (card_state(&peer, check), row_state(&peer, check));
        let (here, name) = (self.server_label(cx), peer.id.clone());
        let fill = |key: &str| tr(key).replace("{este}", &here).replace("{nome}", &name);
        let tested = if check.is_some_and(|c| c.testing) || card == Card::Testing || row == Row::Testing { tr("machines_testing") } else {
            match check.and_then(|c| c.at) {
                Some(at) => tr("machines_peer_tested_at").replace("{hora}", &at.format("%H:%M").to_string()),
                None => tr("machines_peer_tested_now"),
            }
        };
        let busy = m.peer_busy.is_some();
        let next = tr("settings_next_version");
        let section = |title: String| div().mt(px(20.)).mb(px(8.)).flex().items_center().gap(px(8.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD)
            .child(title);
        let muted = |text: String| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text);
        let setting = |title: String, help: String| div().flex().items_center().gap(px(14.)).px_4().py(px(12.))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(div().font_weight(FontWeight::MEDIUM).child(title)).child(muted(help)));
        let error = |id: &str, error: Option<String>| error.map(|error| div().id(SharedString::from(id.to_owned())).role(Role::Alert).px_4().pb(px(12.))
            .text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error));
        let scope = || chip(tr("server_scope"), theme::muted(), theme::raised());

        // Neste aparelho: acompanhar as sessões dela exige guardar o token dela aqui, que chega na próxima versão.
        let follow = div().id("machines-peer-follow").child(Switch::new("machines-peer-follow-switch").checked(false).disabled(true)
            .accessibility_label(format!("{}. {next}", tr("machines_peer_show_sessions"))))
            .tooltip({ let next = next.clone(); move |window, cx| Tooltip::new(next.clone()).build(window, cx) });
        let device = settings_box().child(setting(tr("machines_peer_show_sessions"), tr("machines_peer_no_token_here"))
            .child(div().flex_shrink_0().child(follow)));

        // Recados: desligar é remover o registro, com a mesma pergunta do Remover.
        let has_id = m.id.ok().is_some_and(|id| !id.is_empty());
        let peer_id = peer.id.clone();
        let messages_switch = Switch::new("machines-peer-messages").checked(true).disabled(!has_id || busy)
            // Desligado sem identificador: o motivo vai no nome e embaixo da legenda.
            .accessibility_label(if has_id { fill("machines_peer_messages_title") } else {
                format!("{}. {}", fill("machines_peer_messages_title"), tr("machines_no_id_short")) })
            .on_click(cx.listener(move |this, on: &bool, window, cx| if !*on { this.confirm_peer_removal(peer_id.clone(), Spot::Messages, window, cx) }));
        let tone = match card { Card::Testing => theme::border(), Card::MissingToken | Card::Paused => theme::warning(), _ => theme::danger() };
        let phrase = |text: String| div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(text);
        let going = check.and_then(Check::done).cloned();
        let peer_id = peer.id.clone();
        let result = div().id("machines-peer-result").role(Role::Status).p(px(12.)).rounded(px(10.)).border_1().border_color(tone).bg(theme::inset())
            .flex().flex_col().gap(px(8.))
            .map(|el| match card {
                Card::Testing => el.child(phrase(format!("◌ {}", tr("machines_peer_testing_both")))),
                Card::MissingToken => el.child(phrase(fill("machines_peer_missing_token"))).child(muted(fill("machines_peer_missing_token_p")))
                    .child(div().flex().flex_wrap().items_center().gap(px(8.))
                        .child(Button::new("machines-peer-use-token").primary().small().label(fill("machines_peer_use_token")).disabled(true)
                            .accessibility_label(format!("{}. {next}", fill("machines_peer_use_token"))))
                        .child(muted(next.clone()))),
                Card::GoingFailed => el.child(phrase(fill("machines_peer_going_failed"))).child(muted(tr("machines_peer_going_failed_p")))
                    .when_some(going.as_ref().and_then(|g| g.error.clone()), |el, error| el.child(div().id("machines-peer-check-error")
                        .text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error))),
                Card::GoingOther => el.child(phrase(fill("machines_peer_going_other"))).child(muted(fill("machines_peer_going_other_p")
                    .replace("{endereco}", &peer.url).replace("{outro}", going.as_ref().map(|g| g.answered_as.as_str()).unwrap_or_default()))),
                Card::Paused => el.child(phrase(tr("machines_peer_off"))).child(muted(tr("machines_peer_scan_legend")))
                    .child(div().flex().child(Button::new("machines-peer-turn-on").primary().small().label(tr("machines_peer_turn_on"))
                        .loading(self.peer_busy_at(&peer.id, Spot::TurnOn)).disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.write_peer(peer_id.clone(), Spot::TurnOn, PeerWrite::Enabled(true), cx)))))
                    .when_some(self.peer_error_at(&peer.id, Spot::TurnOn), |el, error| el.child(div().id("machines-peer-turn-on-error")
                        .role(Role::Alert).text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error))),
            })
            // Desligada não é testada: a medida de antes de desligar não é o estado de agora.
            .when_some(going.filter(|g| g.way == Way::Ok && card != Card::Paused).and_then(|g| g.ms), |el, ms| el.child(muted(tr("machines_peer_measure")
                .replace("{de}", &here).replace("{para}", &name).replace("{ms}", &ms.to_string()))))
            .when(matches!(card, Card::Testing | Card::GoingFailed | Card::GoingOther), |el| {
                let id = peer.id.clone();
                el.child(div().flex().child(Button::new("machines-peer-test").outline().small().label(tr("machines_peer_test_again"))
                    .disabled(card == Card::Testing).on_click(cx.listener(move |this, _, _, cx| this.check_peer(&id, cx)))))
            });
        let messages = settings_box()
            .child(setting(fill("machines_peer_messages_title"), fill("machines_peer_messages_legend")).child(div().flex_shrink_0().child(messages_switch)))
            .when(!has_id, |el| el.child(div().px_4().pb(px(12.)).text_size(px(12.5)).text_color(theme::warning()).whitespace_normal()
                .child(tr("machines_no_id_short"))))
            .children(error("machines-peer-messages-error", self.peer_error_at(&peer.id, Spot::Messages)))
            .child(div().border_t_1().border_color(theme::border()).p(px(12.)).child(result));

        // Avançado: tirar da varredura grava no servidor; o endereço de ida é o que ele guardou.
        let this = cx.entity().downgrade();
        let advanced_open = m.peer_advanced;
        let peer_id = peer.id.clone();
        let advanced = div().mt(px(18.)).flex().flex_col().gap(px(10.))
            .child(div().flex().child(super::settings::Disclosure::new("machines-peer-advanced", advanced_open, tr("machines_advanced"), false)
                .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.machines.peer_advanced = open; cx.notify(); }); })))
            .when(advanced_open, |el| el.child(settings_box()
                .child(div().flex().items_center().gap(px(14.)).px_4().py(px(12.))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                        .child(div().flex().flex_wrap().items_center().gap(px(8.)).child(div().font_weight(FontWeight::MEDIUM).child(tr("machines_peer_scan")))
                            .child(scope()))
                        .child(muted(tr("machines_peer_scan_legend"))))
                    .child(div().flex_shrink_0().child(Switch::new("machines-peer-scan").checked(peer.enabled).disabled(busy)
                        .accessibility_label(tr("machines_peer_scan"))
                        .on_click(cx.listener(move |this, on: &bool, _, cx| this.write_peer(peer_id.clone(), Spot::Scan, PeerWrite::Enabled(*on), cx))))))
                .children(error("machines-peer-scan-error", self.peer_error_at(&peer.id, Spot::Scan)))
                .child(div().border_t_1().border_color(theme::border()).px_4().py(px(12.)).flex().flex_col().gap(px(2.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(fill("machines_peer_going_url")))
                    .child(div().font_family(theme::MONO).text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(peer.url.clone())))));

        let key = SharedString::from("machines-peer-remove");
        let removing = self.peer_busy_at(&peer.id, Spot::Footer);
        let remove = Button::new(key.clone()).ghost().small().text_color(theme::danger())
            .child(div().flex().items_center().gap(px(8.))
                .child(tr(if removing { "machines_peer_removing" } else { "machines_peer_remove_machine" })).child(scope()))
            .accessibility_label(if removing { tr("machines_peer_removing") } else { fill("machines_peer_remove_machine_aria") })
            // Carregando, não desligado: com o foco nele o Esc tem de continuar chegando ao diálogo.
            .loading(removing).disabled(busy && !removing);
        let this = cx.entity().downgrade();
        let peer_id = peer.id.clone();
        let remove = FocusOnClick { id: key.into(), button: remove, open: Rc::new(move |window, cx| {
            let _ = this.update(cx, |this, cx| this.confirm_peer_removal(peer_id.clone(), Spot::Footer, window, cx));
        }) };
        let footer = div().mt(px(22.)).flex().flex_col().items_end().gap(px(6.))
            .children(self.peer_error_at(&peer.id, Spot::Footer).map(|error| div().id("machines-peer-remove-error").role(Role::Alert)
                .text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error)))
            .child(remove);

        div().flex().flex_col().pb(px(8.))
            .child(div().pr(px(28.)).mb(px(4.)).flex().flex_col().gap(px(2.))
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tested)))
            .child(section(tr("machines_peer_on_device")))
            .child(device)
            .child(section(tr("machines_peer_messages")).child(scope()))
            .child(messages)
            .child(advanced)
            .child(footer)
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

/// Diálogo sem ação principal: o Enter vira o "confirmar" do kit, que sem `propagate` para a tecla ali e o botão focado nunca
/// recebe o clique de teclado. Seguindo, o botão focado clica ao soltar a tecla, e o diálogo não fecha.
pub(super) fn enter_to_focused(_: &ClickEvent, _: &mut Window, cx: &mut App) -> bool {
    cx.propagate();
    false
}

/// O `Button` do kit não toma foco no clique, e o diálogo devolve ao fechar o foco de quem o abriu: sem focar o cartão antes,
/// o Esc deixaria o foco na página. O foco é o que o próprio botão guarda, achado pelo mesmo caminho na árvore (o gpui põe
/// o nome do tipo do componente no caminho antes de desenhá-lo).
#[derive(IntoElement)]
pub(super) struct FocusOnClick {
    /// O mesmo id dado ao `Button`.
    pub(super) id: ElementId,
    pub(super) button: Button,
    pub(super) open: Rc<dyn Fn(&mut Window, &mut App)>,
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

/// O corpo do diálogo de outra máquina, no mesmo desenho do `MachineDetail`.
struct PeerDetail {
    hangar: WeakEntity<Hangar>,
    _observe: Subscription,
}

impl Render for PeerDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.hangar.update(cx, |hangar, cx| hangar.render_peer_detail(cx)).unwrap_or_else(|_| div())
    }
}

#[cfg(test)]
mod tests {
    use super::{Card, Check, Kind, Light, Peer, Reach, Row, card_state, collapsed, parse_going, parse_peers, parse_reach, row_state, valid_id};
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
    fn peer_row_and_card_follow_the_web_without_a_token_here() {
        let peer = |enabled| Peer { id: "casa".into(), url: "https://casa.test".into(), enabled };
        let check = |estado: &str| Check { going: Some(parse_going(&serde_json::json!({"estado": estado}))), ..Check::default() };
        // Sem medição (ou medindo): testando nos dois lugares.
        assert_eq!((row_state(&peer(true), None), card_state(&peer(true), None)), (Row::Testing, Card::Testing));
        let testing = Check { testing: true, ..check("ok") };
        assert_eq!(card_state(&peer(true), Some(&testing)), Card::Testing);
        // Ida ok: aparece na lista pedindo o token; a volta não dá pra conferir.
        assert_eq!((row_state(&peer(true), Some(&check("ok"))), card_state(&peer(true), Some(&check("ok")))), (Row::NoToken, Card::MissingToken));
        // Falha e outra máquina vão para "Não respondem", cada uma com o seu cartão.
        assert_eq!(card_state(&peer(true), Some(&check("recusou"))), Card::GoingFailed);
        assert_eq!(card_state(&peer(true), Some(&check("estranho"))), Card::GoingOther);
        assert!(collapsed(row_state(&peer(true), Some(&check("falhou")))));
        // Desligada no servidor: pausada, recolhida, qualquer que seja a medição antiga.
        assert_eq!((row_state(&peer(false), Some(&check("ok"))), card_state(&peer(false), Some(&check("ok")))), (Row::Off, Card::Paused));
        assert!(collapsed(Row::Off) && !collapsed(Row::NoToken) && !collapsed(Row::Testing));
        // Lista do servidor: `enabled` ausente é ligada, como no backend.
        let list = parse_peers(&serde_json::json!([{"id": "a", "base_url": "http://a"}, {"id": "b", "base_url": "http://b", "enabled": false}]))
            .expect("formato do /api/peers");
        assert_eq!(list.iter().map(|p| p.enabled).collect::<Vec<_>>(), [true, false]);
    }

    #[test]
    fn identifier_follows_the_backend_rule() {
        for ok in ["casa", "notebook-2", "a", "x_y", &"a".repeat(32)] { assert!(valid_id(ok), "{ok}"); }
        for bad in ["", "Casa", "-casa", "_x", "casa nova", "ção", &"a".repeat(33)] { assert!(!valid_id(bad), "{bad}"); }
    }
}
