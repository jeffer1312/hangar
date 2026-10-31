//! Página Cofre do servidor conectado: as senhas da máquina dona do cofre e onde o cofre fica. A página e o formulário
//! são entidades próprias: o `open_dialog` desenha na hora, com o `Hangar` em atualização, e as respostas voltam a elas.
//! A senha revelada só vive na memória da página enquanto ela está aberta; nunca vai a log nem a disco.
use super::*;
use super::settings::{Page, section_head, settings_box};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Policy { Ask, Free }

impl Policy {
    pub(super) fn label(self) -> String { tr(match self { Policy::Ask => "vault_policy_ask", Policy::Free => "vault_policy_free" }) }
}

#[derive(Clone, Debug, serde::Deserialize)]
struct Entry {
    id: String,
    name: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    policy: Policy,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct Status {
    role: String,
    #[serde(default)]
    owner: Option<String>,
    /// Endereço da dona no `peers.json`, quando esta máquina aponta para outra.
    #[serde(default)]
    owner_url: Option<String>,
    state: String,
    #[serde(default)]
    detail: Option<String>,
    /// Só na dona com máquina de cópia escolhida.
    #[serde(default)]
    backup: Option<Backup>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct Backup {
    peer: String,
    #[serde(default)]
    last_ok_at: Option<f64>,
    #[serde(default)]
    last_error: Option<String>,
}

/// Mínimo da senha de recuperação: ela é a única defesa da cópia guardada fora.
const RECOVERY_MIN: usize = 12;

#[derive(Clone, Debug)]
struct Peer { id: String }

/// Campos da cópia de segurança e da restauração; nascem com a página e somem com ela.
struct BackupFields {
    recovery: Entity<InputState>,
    confirm: Entity<InputState>,
    restore_password: Entity<InputState>,
    restore_server: Entity<InputState>,
}

/// Frase de uma recusa do cofre pelo código; código que esta versão não conhece fica com a frase do servidor.
pub(super) fn vault_failure(error: &Failure) -> String {
    // Servidor de antes do cofre: a rota não existe e o 404 vem sem código.
    if error.status == Some(404) && error.code.is_none() { return tr("vault_error_sem_cofre"); }
    if let Some(code) = error.code.as_deref() {
        let key = format!("vault_error_{}", code.trim_start_matches("erro_cofre_").trim_start_matches("erro_"));
        let text = tr(&key);
        if text != key { return text; }
    }
    Hangar::fetch_failure(error)
}

/// Título da página e a linha abaixo dele, como o `page_top` das outras páginas.
fn top(lead: String) -> Div {
    div().flex().flex_col()
        .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Vault.title()))
        .child(div().mt(px(6.)).text_color(theme::muted()).whitespace_normal().child(lead))
}

fn muted(text: String) -> Div { div().text_sm().text_color(theme::muted()).whitespace_normal().child(text) }

fn alert(id: &'static str, text: String) -> Stateful<Div> {
    div().id(id).role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(text)
}

fn policy_chip(policy: Policy) -> Div {
    div().flex_shrink_0().px(px(6.)).py(px(1.)).rounded(px(4.)).bg(theme::inset()).text_xs()
        .text_color(if policy == Policy::Free { theme::warning() } else { theme::muted() }).child(policy.label())
}

/// Pedido no runtime do tokio; a volta cai na entidade, se ela ainda existir.
fn run<V: 'static, F>(runtime: &Runtime, request: F, window: &mut Window, cx: &mut Context<V>,
    then: impl FnOnce(&mut V, Result<Value, Failure>, &mut Window, &mut Context<V>) + 'static)
where F: Future<Output = Result<Value, Failure>> + Send + 'static {
    let (done, result) = tokio::sync::oneshot::channel();
    runtime.spawn(async move { let _ = done.send(request.await); });
    cx.spawn_in(window, async move |this, cx| {
        let result = result.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
        let _ = this.update_in(cx, |this, window, cx| { then(this, result, window, cx); cx.notify(); });
    }).detach();
}

pub(super) struct VaultPage {
    api: Api,
    runtime: Arc<Runtime>,
    hangar: WeakEntity<Hangar>,
    /// Número da leitura: resposta de uma leitura anterior não pisa na nova.
    seq: u64,
    status: Option<Result<Status, String>>,
    entries: Option<Result<Vec<Entry>, String>>,
    peers: Vec<Peer>,
    peers_error: Option<String>,
    /// Quem mostra e muda as senhas: só a dona atende; a máquina que aponta para ela repassa apenas a lista.
    writer: Option<Api>,
    revealed: HashMap<String, String>,
    copied: Option<String>,
    busy: HashSet<String>,
    config_busy: bool,
    error: Option<String>,
    fields: BackupFields,
    recovery_busy: bool,
    recovery_saved: bool,
    recovery_error: Option<String>,
    restore_peer: Option<String>,
    restore_busy: bool,
    restore_error: Option<String>,
}

impl VaultPage {
    fn new(api: Api, runtime: Arc<Runtime>, hangar: WeakEntity<Hangar>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let masked = |hint: String, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(hint));
        let fields = BackupFields {
            recovery: masked(tr("vault_recovery_new"), window, cx),
            confirm: masked(tr("vault_recovery_confirm"), window, cx),
            restore_password: masked(tr("vault_recovery_password"), window, cx),
            restore_server: cx.new(|cx| InputState::new(window, cx).placeholder(tr("vault_restore_server_hint"))),
        };
        let mut page = Self { api, runtime, hangar, seq: 0, status: None, entries: None, peers: Vec::new(), peers_error: None, writer: None,
            revealed: HashMap::new(), copied: None, busy: HashSet::new(), config_busy: false, error: None, fields, recovery_busy: false,
            recovery_saved: false, recovery_error: None, restore_peer: None, restore_busy: false, restore_error: None };
        page.load(window, cx);
        page
    }

    fn status_ok(&self) -> Option<&Status> { self.status.as_ref().and_then(|s| s.as_ref().ok()) }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.seq += 1;
        let seq = self.seq;
        (self.status, self.entries, self.writer, self.error, self.peers_error) = (None, None, None, None, None);
        self.peers.clear();
        self.revealed.clear();
        let api = self.api.clone();
        // A dona pode esperar o chaveiro antes de responder.
        run(&self.runtime, async move { api.server_read(&["vault", "status"], &[], 45).await }, window, cx, move |this: &mut Self, result, window, cx| {
            if seq != this.seq { return; }
            let status = result.map_err(|e| vault_failure(&e))
                .and_then(|v| serde_json::from_value::<Status>(v).map_err(|_| tr("invalid_response")));
            if let Ok(status) = &status {
                this.writer = match (status.role.as_str(), &status.owner_url) {
                    ("owner", _) => Some(this.api.clone()),
                    ("remote", Some(url)) => this.hangar.upgrade().and_then(|h| h.read(cx).vault_owner_api(url)),
                    _ => None,
                };
                if status.state == "open" { this.load_entries(window, cx); }
            }
            this.status = Some(status);
        });
        // As outras máquinas só servem às listas de escolha (dona, cópia, restauração): pedidas junto com o estado.
        self.load_peers(window, cx);
        cx.notify();
    }

    fn load_entries(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (seq, api) = (self.seq, self.api.clone());
        run(&self.runtime, async move { api.server_read(&["vault", "entries"], &[], 45).await }, window, cx, move |this: &mut Self, result, _, _| {
            if seq != this.seq { return; }
            this.entries = Some(result.map_err(|e| vault_failure(&e))
                .and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response"))));
        });
    }

    fn load_peers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (seq, api) = (self.seq, self.api.clone());
        run(&self.runtime, async move { api.server_read(&["peers"], &[], 15).await }, window, cx, move |this: &mut Self, result, _, _| {
            if seq != this.seq { return; }
            match result {
                Ok(value) => match value.as_array() {
                    Some(list) => this.peers = list.iter().filter_map(|p| Some(Peer { id: p.get("id")?.as_str()?.to_owned() })).collect(),
                    None => this.peers_error = Some(tr("invalid_response")),
                },
                Err(error) => this.peers_error = Some(vault_failure(&error)),
            }
        });
    }

    fn set_owner(&mut self, owner: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_busy { return; }
        (self.config_busy, self.error) = (true, None);
        let api = self.api.clone();
        run(&self.runtime, async move { api.server_send(reqwest::Method::PUT, &["vault", "config"], Some(json!({"owner": owner})), 45).await },
            window, cx, |this: &mut Self, result, window, cx| {
                this.config_busy = false;
                match result {
                    Ok(_) => this.load(window, cx),
                    Err(error) => this.error = Some(vault_failure(&error)),
                }
            });
        cx.notify();
    }

    /// Estado que a rota devolveu no lugar do atual, sem reler a lista.
    fn take_status(&mut self, value: Value) {
        match serde_json::from_value::<Status>(value) {
            Ok(status) => self.status = Some(Ok(status)),
            Err(_) => self.error = Some(tr("invalid_response")),
        }
    }

    fn set_backup_peer(&mut self, peer: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_busy { return; }
        (self.config_busy, self.error) = (true, None);
        let api = self.api.clone();
        run(&self.runtime, async move { api.server_send(reqwest::Method::PUT, &["vault", "config"], Some(json!({"backup_peer": peer})), 45).await },
            window, cx, |this: &mut Self, result, _, _| {
                this.config_busy = false;
                match result {
                    Ok(value) => this.take_status(value),
                    Err(error) => this.error = Some(vault_failure(&error)),
                }
            });
        cx.notify();
    }

    fn set_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recovery_busy { return; }
        let password = self.fields.recovery.read(cx).value().to_string();
        let confirm = self.fields.confirm.read(cx).value().to_string();
        let problem = if password.chars().count() < RECOVERY_MIN { Some(tr("vault_error_recuperacao_curta")) }
            else if password != confirm { Some(tr("vault_recovery_mismatch")) } else { None };
        self.recovery_saved = false;
        if let Some(problem) = problem { self.recovery_error = Some(problem); cx.notify(); return; }
        (self.recovery_busy, self.recovery_error) = (true, None);
        let api = self.api.clone();
        run(&self.runtime, async move { api.server_send(reqwest::Method::POST, &["vault", "recovery"], Some(json!({"password": password})), 45).await },
            window, cx, |this: &mut Self, result, window, cx| {
                this.recovery_busy = false;
                match result {
                    Ok(value) => {
                        for input in [this.fields.recovery.clone(), this.fields.confirm.clone()] { input.update(cx, |i, cx| i.set_value("", window, cx)); }
                        this.recovery_saved = true;
                        this.take_status(value);
                    }
                    Err(error) => this.recovery_error = Some(vault_failure(&error)),
                }
            });
        cx.notify();
    }

    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.restore_busy { return; }
        let Some(peer) = self.restore_peer.clone() else { self.restore_error = Some(tr("vault_restore_pick_peer")); cx.notify(); return };
        let password = self.fields.restore_password.read(cx).value().to_string();
        if password.is_empty() { self.restore_error = Some(tr("vault_password_required")); cx.notify(); return; }
        let mut body = json!({"password": password, "from_peer": peer});
        let server = self.fields.restore_server.read(cx).value().trim().to_owned();
        if !server.is_empty() { body["server_id"] = json!(server); }
        (self.restore_busy, self.restore_error) = (true, None);
        let api = self.api.clone();
        // Buscar a cópia na outra máquina e gravar a chave nova pode levar o tempo do chaveiro.
        run(&self.runtime, async move { api.server_send(reqwest::Method::POST, &["vault", "restore"], Some(body), 90).await },
            window, cx, |this: &mut Self, result, window, cx| {
                this.restore_busy = false;
                match result {
                    Ok(_) => {
                        this.fields.restore_password.update(cx, |i, cx| i.set_value("", window, cx));
                        this.load(window, cx);
                    }
                    // O cofre voltou; só a senha de recuperação precisa ser definida de novo, na seção da cópia.
                    Err(error) if error.code.as_deref() == Some("erro_cofre_recuperacao_nao_gravada") => {
                        this.load(window, cx);
                        this.recovery_error = Some(vault_failure(&error));
                    }
                    Err(error) => this.restore_error = Some(vault_failure(&error)),
                }
            });
        cx.notify();
    }

    /// Mostrar alterna; copiar usa a revelada ou pede a senha só para a área de transferência, sem guardá-la.
    fn reveal(&mut self, id: String, copy: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(password) = self.revealed.get(&id).cloned() {
            if copy { self.copy(id, password, cx); } else { self.revealed.remove(&id); }
            cx.notify();
            return;
        }
        let Some(api) = self.writer.clone() else { return };
        if !self.busy.insert(id.clone()) { return; }
        (self.error, self.copied) = (None, None);
        let (seq, path) = (self.seq, id.clone());
        run(&self.runtime, async move { api.server_read(&["vault", "entries", &path, "password"], &[], 45).await }, window, cx,
            move |this: &mut Self, result, _, cx| {
                this.busy.remove(&id);
                if seq != this.seq { return; }
                match result.map(|v| v.get("password").and_then(Value::as_str).map(str::to_owned)) {
                    Ok(Some(password)) if copy => this.copy(id, password, cx),
                    Ok(Some(password)) => { this.revealed.insert(id, password); }
                    Ok(None) => this.error = Some(tr("invalid_response")),
                    Err(error) => this.error = Some(vault_failure(&error)),
                }
            });
        cx.notify();
    }

    fn copy(&mut self, id: String, password: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(password));
        self.copied = Some(id.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| if this.copied.as_ref() == Some(&id) { this.copied = None; cx.notify(); });
        }).detach();
    }

    fn confirm_delete(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let title = tr("vault_delete_title").replace("{name}", &entry.name);
        chrome::confirm_alert(window, cx, title, tr("vault_delete_body"), tr("vault_delete"), ButtonVariant::Danger, move |window, cx| {
            let _ = page.update(cx, |page, cx| page.delete(entry.id.clone(), window, cx));
            true
        });
    }

    fn delete(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.writer.clone() else { return };
        if !self.busy.insert(id.clone()) { return; }
        self.error = None;
        let path = id.clone();
        run(&self.runtime, async move { api.server_send(reqwest::Method::DELETE, &["vault", "entries", &path], None, 45).await }, window, cx,
            move |this: &mut Self, result, window, cx| {
                this.busy.remove(&id);
                match result {
                    Ok(_) => { this.revealed.remove(&id); this.load_entries(window, cx); }
                    Err(error) => this.error = Some(vault_failure(&error)),
                }
            });
        cx.notify();
    }

    fn open_form(&mut self, entry: Option<Entry>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.writer.clone() else { return };
        let title = match &entry { Some(e) => tr("vault_edit_title").replace("{name}", &e.name), None => tr("vault_add_title") };
        let (runtime, page) = (self.runtime.clone(), cx.entity().downgrade());
        let form = cx.new(|cx| VaultForm::new(api, runtime, page, entry, window, cx));
        let first = form.read(cx).name.clone();
        window.open_dialog(cx, move |dialog, _, _| popup::dialog(dialog).w(px(480.)).title(title.clone()).child(form.clone()));
        window.on_next_frame(move |window, cx| first.update(cx, |input, cx| input.focus(window, cx)));
    }

    /// Formulário gravado: a lista volta da dona, sem a senha.
    fn saved(&mut self, id: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = id { self.revealed.remove(id); }
        self.load_entries(window, cx);
    }

    fn retry(&self, id: &'static str, cx: &mut Context<Self>) -> Button {
        Button::new(id).outline().small().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| this.load(window, cx)))
    }

    fn render_entries(&self, cx: &mut Context<Self>) -> AnyElement {
        let read_only = self.writer.is_none();
        let add = Button::new("vault-add").primary().small().icon(IconName::Plus).label(tr("vault_add")).disabled(read_only)
            .on_click(cx.listener(|this, _, window, cx| this.open_form(None, window, cx)));
        let head = section_head(IconName::KeyRound, tr("vault_entries"), Some(tr("vault_entries_hint")), Some(add.into_any_element()), px(16.));
        let mut section = div().flex().flex_col().gap_2().child(head);
        if read_only {
            let owner = self.status_ok().and_then(|s| s.owner.clone()).unwrap_or_default();
            section = section.child(muted(tr("vault_read_only").replace("{owner}", &owner)));
        }
        let entries = match &self.entries {
            None => return section.child(div().id("vault-entries-loading").role(Role::Status).child(muted(tr("loading")))).into_any_element(),
            Some(Err(error)) => return section.child(alert("vault-entries-error", error.clone()))
                .child(div().child(Button::new("vault-entries-retry").outline().small().label(tr("retry"))
                    .on_click(cx.listener(|this, _, window, cx| this.load_entries(window, cx))))).into_any_element(),
            Some(Ok(entries)) => entries,
        };
        if entries.is_empty() {
            return section.child(settings_box().px_4().py(px(18.)).child(muted(tr("vault_empty")))).into_any_element();
        }
        let rows = entries.iter().map(|entry| self.render_entry(entry, cx));
        section.child(settings_box().id("vault-entries").role(Role::List).aria_label(tr("vault_entries")).children(rows)).into_any_element()
    }

    fn render_entry(&self, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let id = entry.id.clone();
        let enabled = self.writer.is_some() && !self.busy.contains(&id);
        let shown = self.revealed.get(&id);
        let copied = self.copied.as_ref() == Some(&id);
        let detail: Vec<&str> = [entry.username.as_deref(), entry.url.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
        let named = |key: &str| format!("{} · {}", tr(key), entry.name);
        let button = |suffix: &str, icon: IconName, label: String| Button::new(SharedString::from(format!("vault-{suffix}-{id}"))).ghost().xsmall()
            .icon(icon).tooltip(label.clone()).accessibility_label(label).disabled(!enabled);
        let (reveal_id, copy_id) = (id.clone(), id.clone());
        let (edit_entry, delete_entry) = (entry.clone(), entry.clone());
        let actions = div().flex_shrink_0().flex().items_center().gap_1()
            .child(button("reveal", if shown.is_some() { IconName::EyeOff } else { IconName::Eye },
                named(if shown.is_some() { "vault_hide" } else { "vault_reveal" }))
                .on_click(cx.listener(move |this, _, window, cx| this.reveal(reveal_id.clone(), false, window, cx))))
            .child(button("copy", if copied { IconName::Check } else { IconName::Copy }, named(if copied { "vault_copied" } else { "vault_copy" }))
                .on_click(cx.listener(move |this, _, window, cx| this.reveal(copy_id.clone(), true, window, cx))))
            .child(button("edit", IconName::Pencil, named("vault_edit"))
                .on_click(cx.listener(move |this, _, window, cx| this.open_form(Some(edit_entry.clone()), window, cx))))
            .child(button("delete", IconName::Trash, named("vault_delete"))
                .on_click(cx.listener(move |this, _, window, cx| this.confirm_delete(delete_entry.clone(), window, cx))));
        div().id(SharedString::from(format!("vault-entry-{id}"))).role(Role::ListItem).aria_label(entry.name.clone())
            .mt(px(-1.)).border_t_1().border_color(theme::border()).px_4().py(px(12.)).flex().items_center().gap(px(14.))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().flex().items_center().gap_2()
                    .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(entry.name.clone()))
                    .child(policy_chip(entry.policy)))
                .when(!detail.is_empty(), |el| el.child(div().truncate().text_size(px(12.5)).text_color(theme::muted()).child(detail.join(" · "))))
                .when_some(entry.notes.clone().filter(|n| !n.is_empty()), |el, notes| el.child(div().text_size(px(12.5)).text_color(theme::faint())
                    .whitespace_normal().child(notes)))
                .when_some(shown.cloned(), |el, password| el.child(div().font_family(theme::MONO).text_size(px(12.5)).text_color(theme::text())
                    .whitespace_normal().child(password))))
            .child(actions)
            .into_any_element()
    }

    /// Cofre que não abre: trancado, chave perdida, dona fora do ar ou ainda sem cofre.
    fn render_closed(&self, status: &Status, cx: &mut Context<Self>) -> AnyElement {
        let owner = status.owner.clone().unwrap_or_default();
        let (text, retry) = match (status.state.as_str(), status.role.as_str()) {
            ("locked", _) => (tr("vault_state_locked"), true),
            ("unavailable", _) => (tr("vault_error_chaveiro_indisponivel"), true),
            ("key_lost", _) => (tr("vault_state_key_lost"), false),
            ("offline", _) => (tr("vault_state_offline").replace("{owner}", &owner), true),
            ("absent", "remote") => (tr("vault_state_owner_empty").replace("{owner}", &owner), true),
            ("absent", _) => (tr("vault_state_none"), false),
            _ => (tr("invalid_response"), true),
        };
        let warn = status.state != "absent" || status.role == "remote";
        div().id("vault-closed").role(if warn { Role::Alert } else { Role::Status }).flex().flex_col().gap_2()
            .child(settings_box().px_4().py(px(14.)).gap_1()
                .child(div().text_sm().whitespace_normal().text_color(if warn { theme::warning() } else { theme::text() }).child(text))
                .when_some(status.detail.clone(), |el, detail| el.child(muted(detail))))
            .when(retry, |el| el.child(div().child(self.retry("vault-closed-retry", cx))))
            .into_any_element()
    }

    /// Onde o cofre fica. Criar aqui só sem dona: com dona, as senhas dela ficariam fora do alcance.
    /// Uma opção por máquina do `peers.json`, com a escolhida marcada.
    fn peer_choices(&self, prefix: &'static str, chosen: Option<&str>, disabled: bool,
        pick: fn(&mut Self, String, &mut Window, &mut Context<Self>), cx: &mut Context<Self>) -> Vec<Button> {
        self.peers.iter().map(|p| {
            let id = p.id.clone();
            Button::new(SharedString::from(format!("{prefix}-{id}"))).outline().small().label(id.clone())
                .selected(chosen == Some(id.as_str())).disabled(disabled)
                .on_click(cx.listener(move |this, _, window, cx| pick(this, id.clone(), window, cx)))
        }).collect()
    }

    /// As opções de máquina; sem nenhuma, a falha da leitura ou o aviso de que a lista é mesmo vazia.
    fn peers_or_note(&self, peers: Vec<Button>) -> Div {
        if !peers.is_empty() { return div().flex().flex_wrap().gap_2().children(peers); }
        match &self.peers_error {
            Some(error) => div().text_sm().text_color(theme::danger()).whitespace_normal().child(tr("vault_peers_failed").replace("{error}", error)),
            None => muted(tr("vault_backup_no_peers")),
        }
    }

    /// Cópia diária para outra máquina: quem guarda, como foi a última e a senha que abre a cópia.
    fn render_backup(&self, status: &Status, cx: &mut Context<Self>) -> AnyElement {
        let label = |key: &str| div().text_sm().font_weight(FontWeight::MEDIUM).child(tr(key));
        let chosen = status.backup.as_ref().map(|b| b.peer.as_str());
        let peers = self.peer_choices("vault-backup-peer", chosen, self.config_busy, |this, id, window, cx| this.set_backup_peer(id, window, cx), cx);
        let last = match &status.backup {
            None => muted(tr("vault_backup_off")),
            Some(Backup { last_error: Some(error), .. }) => div().text_sm().text_color(theme::danger()).whitespace_normal()
                .child(tr("vault_backup_failed").replace("{error}", error)),
            Some(Backup { last_ok_at: Some(at), .. }) => muted(tr("vault_backup_last")
                .replace("{when}", &super::side::ago(chrono::Local::now().timestamp() as f64 - at))),
            Some(_) => muted(tr("vault_backup_never")),
        };
        let off = self.recovery_busy;
        let status_line = if self.recovery_saved { Some((tr("vault_recovery_saved"), theme::success())) } else { None };
        settings_box().id("vault-backup")
            .child(section_head(IconName::RotateCcwClock, tr("vault_backup"), Some(tr("vault_backup_hint")), None, px(16.)))
            .child(div().px_4().pb(px(16.)).flex().flex_col().gap_3()
                .child(div().flex().flex_col().gap_2().child(label("vault_backup_peer"))
                    .child(self.peers_or_note(peers))
                    .child(div().id("vault-backup-last").role(Role::Status).child(last)))
                .child(div().flex().flex_col().gap_2().child(label("vault_recovery"))
                    .child(div().text_sm().text_color(theme::warning()).whitespace_normal().child(tr("vault_recovery_hint")))
                    .child(Input::new(&self.fields.recovery).small().mask_toggle().disabled(off).aria_label(tr("vault_recovery_new")))
                    .child(Input::new(&self.fields.confirm).small().mask_toggle().disabled(off).aria_label(tr("vault_recovery_confirm")))
                    .children(self.recovery_error.clone().map(|error| alert("vault-recovery-error", error)))
                    .child(div().flex().items_center().gap_3()
                        .child(Button::new("vault-recovery-save").primary().small().label(tr("vault_recovery_save")).loading(off).disabled(off)
                            .on_click(cx.listener(|this, _, window, cx| this.set_recovery(window, cx))))
                        .children(status_line.map(|(text, color)| div().id("vault-recovery-status").role(Role::Status).text_sm().text_color(color).child(text))))))
            .into_any_element()
    }

    /// Restaurar só entra num cofre vazio: as senhas voltam da cópia que outra máquina guarda.
    fn render_restore(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = |key: &str| div().text_sm().font_weight(FontWeight::MEDIUM).child(tr(key));
        let off = self.restore_busy;
        let peers = self.peer_choices("vault-restore-peer", self.restore_peer.as_deref(), off,
            |this, id, _, cx| { this.restore_peer = Some(id); cx.notify(); }, cx);
        settings_box().id("vault-restore")
            .child(section_head(IconName::RefreshCw, tr("vault_restore"), Some(tr("vault_restore_hint")), None, px(16.)))
            .child(div().px_4().pb(px(16.)).flex().flex_col().gap_2()
                .child(label("vault_restore_peer"))
                .child(self.peers_or_note(peers))
                .child(label("vault_restore_server"))
                .child(Input::new(&self.fields.restore_server).small().disabled(off).aria_label(tr("vault_restore_server")))
                .child(label("vault_recovery_password"))
                .child(Input::new(&self.fields.restore_password).small().mask_toggle().disabled(off).aria_label(tr("vault_recovery_password")))
                .children(self.restore_error.clone().map(|error| alert("vault-restore-error", error)))
                .child(div().child(Button::new("vault-restore-go").primary().small().label(tr("vault_restore_go")).loading(off).disabled(off)
                    .on_click(cx.listener(|this, _, window, cx| this.restore(window, cx))))))
            .into_any_element()
    }

    fn render_where(&self, status: &Status, cx: &mut Context<Self>) -> AnyElement {
        let owner = status.owner.clone().unwrap_or_default();
        let text = match status.role.as_str() {
            "owner" => tr("vault_where_here"),
            "remote" => tr("vault_where_remote").replace("{owner}", &owner),
            _ => tr("vault_where_none"),
        };
        let busy = self.config_busy;
        let make_here = (status.role == "none").then(|| Button::new("vault-make-here").outline().small().label(tr("vault_make_here"))
            .loading(busy).disabled(busy).on_click(cx.listener(|this, _, window, cx| this.set_owner("self".into(), window, cx))));
        let peers: Vec<Button> = if status.role == "owner" { Vec::new() } else {
            self.peers.iter().filter(|p| p.id != owner).map(|p| {
                let id = p.id.clone();
                Button::new(SharedString::from(format!("vault-owner-{id}"))).outline().small().label(tr("vault_use_peer").replace("{peer}", &id))
                    .disabled(busy).on_click(cx.listener(move |this, _, window, cx| this.set_owner(id.clone(), window, cx)))
            }).collect()
        };
        let choices = make_here.is_some() || !peers.is_empty();
        settings_box()
            .child(section_head(IconName::Server, tr("vault_where"), Some(text), None, px(16.)))
            .when(choices, |el| el.child(div().px_4().pb(px(14.)).flex().flex_wrap().gap_2().children(make_here).children(peers)))
            // Na dona e sem dona, a falha já aparece na lista da cópia ou da restauração.
            .when_some(self.peers_error.clone().filter(|_| status.role == "remote"), |el, error| el.child(div().px_4().pb(px(14.))
                .child(alert("vault-peers-error", tr("vault_peers_failed").replace("{error}", &error)))))
            .into_any_element()
    }
}

impl Render for VaultPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = div().flex().flex_col().gap_4().child(top(tr("vault_intro")));
        if let Some(error) = self.error.clone() { page = page.child(alert("vault-error", error)); }
        let status = match &self.status {
            None => return page.child(div().id("vault-loading").role(Role::Status).child(muted(tr("loading")))),
            Some(Err(error)) => { let error = error.clone(); return page.child(alert("vault-status-error", error)).child(div().child(self.retry("vault-retry", cx))); }
            Some(Ok(status)) => status.clone(),
        };
        let body = if status.state == "open" { self.render_entries(cx) } else { self.render_closed(&status, cx) };
        let empty = matches!(&self.entries, Some(Ok(list)) if list.is_empty());
        let backup = (status.role == "owner" && status.state == "open").then(|| self.render_backup(&status, cx));
        // Chave perdida também restaura: o servidor guarda o arquivo velho de lado e cria uma chave nova.
        let restore = (status.role == "none" || (status.role == "owner" && (empty || status.state == "key_lost"))).then(|| self.render_restore(cx));
        page.child(body).children(backup).children(restore).child(self.render_where(&status, cx))
    }
}

/// Cadastro ou edição de uma senha, num diálogo. Na edição, senha vazia mantém a gravada.
pub(super) struct VaultForm {
    api: Api,
    runtime: Arc<Runtime>,
    page: WeakEntity<VaultPage>,
    id: Option<String>,
    name: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    url: Entity<InputState>,
    notes: Entity<TextareaState>,
    policy: Policy,
    saving: bool,
    error: Option<String>,
    _enter: Vec<Subscription>,
}

impl VaultForm {
    fn new(api: Api, runtime: Arc<Runtime>, page: WeakEntity<VaultPage>, entry: Option<Entry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let value = |v: Option<&String>| v.cloned().unwrap_or_default();
        let field = |v: String, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).default_value(v));
        let name = field(entry.as_ref().map(|e| e.name.clone()).unwrap_or_default(), window, cx);
        let username = field(value(entry.as_ref().and_then(|e| e.username.as_ref())), window, cx);
        let url = field(value(entry.as_ref().and_then(|e| e.url.as_ref())), window, cx);
        let keep = entry.is_some();
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(if keep { tr("vault_password_keep") } else { String::new() }));
        let notes_text = value(entry.as_ref().and_then(|e| e.notes.as_ref()));
        let notes = cx.new(|cx| TextareaState::new(window, cx).rows(3).default_value(notes_text));
        let _enter = [&name, &username, &password, &url].into_iter().map(|input| cx.subscribe_in(input, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event { this.save(window, cx); }
        })).collect();
        Self { api, runtime, page, id: entry.as_ref().map(|e| e.id.clone()), name, username, password, url, notes,
            policy: entry.map_or(Policy::Ask, |e| e.policy), saving: false, error: None, _enter }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving { return; }
        let text = |input: &Entity<InputState>, cx: &App| input.read(cx).value().trim().to_owned();
        let optional = |s: String| if s.is_empty() { Value::Null } else { Value::String(s) };
        let name = text(&self.name, cx);
        // A senha vai como foi digitada: espaço nas pontas pode ser parte dela.
        let password = self.password.read(cx).value().to_string();
        let missing = if name.is_empty() { Some("vault_name_required") } else if self.id.is_none() && password.is_empty() { Some("vault_password_required") } else { None };
        if let Some(key) = missing { self.error = Some(tr(key)); cx.notify(); return; }
        let mut body = json!({"name": name, "username": optional(text(&self.username, cx)), "url": optional(text(&self.url, cx)),
            "notes": optional(self.notes.read(cx).value().trim().to_owned()), "policy": self.policy});
        if !password.is_empty() { body["password"] = json!(password); }
        (self.saving, self.error) = (true, None);
        let (api, id) = (self.api.clone(), self.id.clone());
        run(&self.runtime, async move {
            match id {
                None => api.server_send(reqwest::Method::POST, &["vault", "entries"], Some(body), 45).await,
                Some(id) => api.server_send(reqwest::Method::PATCH, &["vault", "entries", &id], Some(body), 45).await,
            }
        }, window, cx, |this: &mut Self, result, window, cx| {
            this.saving = false;
            match result {
                Ok(_) => {
                    let id = this.id.clone();
                    let _ = this.page.update(cx, |page, cx| page.saved(id.as_deref(), window, cx));
                    window.close_dialog(cx);
                }
                Err(error) => this.error = Some(vault_failure(&error)),
            }
        });
        cx.notify();
    }
}

impl Render for VaultForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let off = self.saving;
        let field = |label: String, control: AnyElement| div().flex().flex_col().gap(px(6.))
            .child(div().font_weight(FontWeight::MEDIUM).child(label)).child(control);
        let input = |state: &Entity<InputState>, key: &str| Input::new(state).small().disabled(off).aria_label(tr(key)).into_any_element();
        let radio = |id: &'static str, policy: Policy, cx: &mut Context<Self>| Radio::new(id).label(policy.label()).checked(self.policy == policy).disabled(off)
            .on_change(cx.listener(move |this, _, _, cx| { this.policy = policy; cx.notify(); }));
        div().id("vault-form").flex().flex_col().gap(px(12.)).text_size(px(13.))
            .child(field(tr("vault_field_name"), input(&self.name, "vault_field_name")))
            .child(field(tr("vault_field_username"), input(&self.username, "vault_field_username")))
            .child(field(tr("vault_field_password"), Input::new(&self.password).small().mask_toggle().disabled(off)
                .aria_label(tr("vault_field_password")).into_any_element()))
            .child(muted(tr("vault_token_hint")).text_size(px(12.5)))
            .child(field(tr("vault_field_url"), input(&self.url, "vault_field_url")))
            .child(field(tr("vault_field_notes"), Textarea::new(&self.notes).disabled(off).aria_label(tr("vault_field_notes")).into_any_element()))
            .child(div().id("vault-form-policy").role(Role::RadioGroup).aria_label(tr("vault_field_policy")).flex().flex_col().gap(px(6.))
                .child(div().font_weight(FontWeight::MEDIUM).child(tr("vault_field_policy")))
                .child(radio("vault-policy-ask", Policy::Ask, cx))
                .child(radio("vault-policy-free", Policy::Free, cx))
                .child(muted(tr("vault_policy_hint")).text_size(px(12.5))))
            .children(self.error.clone().map(|error| alert("vault-form-error", error)))
            .child(div().flex().items_center().justify_end().gap(px(10.))
                .child(Button::new("vault-form-cancel").ghost().small().label(tr("cancel")).disabled(off)
                    .on_click(|_, window, cx| window.close_dialog(cx)))
                .child(Button::new("vault-form-save").primary().small().label(tr("vault_save")).loading(off).disabled(off)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx)))))
    }
}

impl Hangar {
    /// A página é refeita a cada abertura e a cada conexão: nada revelado sobrevive à saída.
    pub(super) fn render_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(api) = self.api.clone() else {
            return top(tr("settings_offline")).into_any_element();
        };
        if self.vault_page.as_ref().is_none_or(|(connection, _)| *connection != self.connection) {
            let (runtime, hangar) = (self.runtime.clone(), cx.entity().downgrade());
            let page = cx.new(|cx| VaultPage::new(api, runtime, hangar, window, cx));
            self.vault_page = Some((self.connection, page));
        }
        self.vault_page.as_ref().map(|(_, page)| page.clone().into_any_element()).unwrap_or_else(|| div().into_any_element())
    }

    /// A máquina deste aparelho que é a dona, pelo endereço que o `peers.json` do servidor conectado dá a ela.
    pub(super) fn vault_owner_api(&self, url: &str) -> Option<Api> {
        let want = machines::host_key(url);
        // Pelo endereço salvo ou pelo da rede local aprendido: o `peers.json` pode dar qualquer um dos dois.
        self.servers.iter().filter(|s| !s.invite && !s.disabled)
            .find(|s| std::iter::once(s.address.as_str()).chain(s.lan.as_ref().map(|l| l.url.as_str())).any(|u| machines::host_key(u) == want))
            .and_then(|s| self.machine_api(&servers::norm(&s.address)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn vault_codes_have_their_own_sentence_and_unknown_codes_keep_the_server_one() {
        let failure = |code: &str| Failure { status: Some(409), detail: "frase do servidor".into(), retry_after: None, uncertain: false, code: Some(code.into()) };
        assert_eq!(vault_failure(&failure("erro_cofre_trancado")), tr("vault_error_trancado"));
        assert_eq!(vault_failure(&failure("erro_interacao_desatualizada")), tr("vault_error_interacao_desatualizada"));
        assert_eq!(vault_failure(&failure("erro_cofre_novo_codigo")), "frase do servidor");
        let old_server = Failure { status: Some(404), detail: "Not Found".into(), retry_after: None, uncertain: false, code: None };
        assert_eq!(vault_failure(&old_server), tr("vault_error_sem_cofre"));
    }

    #[test]
    fn entry_reads_without_optional_fields() {
        let entry: Entry = serde_json::from_value(json!({"id": "a", "name": "GitHub", "policy": "free", "updated_at": 1.0})).unwrap();
        assert_eq!((entry.policy, entry.username, entry.url), (Policy::Free, None, None));
        assert!(serde_json::from_value::<Entry>(json!({"id": "a", "name": "x", "policy": "other"})).is_err());
    }
}
