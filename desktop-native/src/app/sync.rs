//! Sincronização deste servidor: estado lido da rota de setup e ações que preservam a conta.
use super::*;
use super::device::Remote;
use super::server_config::chip;
use super::settings::Page;
use base64::{Engine as _, engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}};
use ring::{aead, hkdf, pbkdf2, rand::{SecureRandom, SystemRandom}};
use std::num::NonZeroU32;

const PBKDF2_ITERATIONS: u32 = 600_000;

fn registration_body(user: String, password: String, base: String, label: String, token: String) -> Result<Value, Failure> {
    let rng = SystemRandom::new();
    let (mut salt, mut iv, mut id) = ([0u8; 16], [0u8; 12], [0u8; 8]);
    for bytes in [&mut salt[..], &mut iv[..], &mut id[..]] { rng.fill(bytes).map_err(|_| Failure::local("sync_config_erro"))?; }
    let mut master = [0u8; 32];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(PBKDF2_ITERATIONS).unwrap(), &salt, password.as_bytes(), &mut master);
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(&master);
    let (mut auth, mut key) = ([0u8; 32], [0u8; 32]);
    prk.expand(&[b"cp-auth"], hkdf::HKDF_SHA256).and_then(|okm| okm.fill(&mut auth)).map_err(|_| Failure::local("sync_config_erro"))?;
    prk.expand(&[b"cp-enc"], hkdf::HKDF_SHA256).and_then(|okm| okm.fill(&mut key)).map_err(|_| Failure::local("sync_config_erro"))?;
    let server = json!([{"id": format!("srv-{}", URL_SAFE_NO_PAD.encode(id)), "label": label, "baseUrl": base, "token": token}]);
    let mut data = serde_json::to_vec(&server).map_err(|_| Failure::local("sync_config_erro"))?;
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &key).map_err(|_| Failure::local("sync_config_erro"))?);
    key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(iv), aead::Aad::empty(), &mut data)
        .map_err(|_| Failure::local("sync_config_erro"))?;
    Ok(json!({"user": user, "salt": STANDARD.encode(salt), "auth_hash": STANDARD.encode(auth),
        "enc_blob": {"iv": STANDARD.encode(iv), "data": STANDARD.encode(data)}}))
}

fn secure_setup(address: &str) -> bool {
    url::Url::parse(address).ok().is_some_and(|url| url.scheme() == "https" || match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    })
}

fn sync_field(label: String, input: &Entity<InputState>, saving: bool) -> Div {
    div().flex().flex_col().gap(px(6.)).child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label.clone()))
        .child(Input::new(input).disabled(saving).aria_label(label))
}

struct SyncForm {
    user: Entity<InputState>,
    password: Entity<InputState>,
    confirmation: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Debug)]
struct Setup { enabled: bool, registered: bool, user: Option<String> }

fn parse_setup(value: Value) -> Result<Setup, String> {
    let enabled = value.get("enabled").and_then(Value::as_bool).ok_or_else(|| tr("invalid_response"))?;
    let registered = value.get("registered").and_then(Value::as_bool).ok_or_else(|| tr("invalid_response"))?;
    let user = match value.get("user") { None | Some(Value::Null) => None, Some(Value::String(user)) => Some(user.clone()), _ => return Err(tr("invalid_response")) };
    Ok(Setup { enabled, registered, user })
}

#[derive(Default)]
pub(in crate::app) struct Sync {
    setup: Remote<Setup>,
    saving: bool,
    save_seq: u64,
    error: Option<String>,
    reconcile: Option<Write>,
    activated: bool,
    deactivated: bool,
    copied: bool,
    form: Option<SyncForm>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Write { Enable, Disable, Register }

pub(super) enum SyncReply {
    Loaded(u64, Result<Value, Failure>),
    Saved(u64, Write, Result<Value, Failure>),
}

impl Hangar {
    fn sync_send_later(&self) -> impl Fn(SyncReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Sync(reply) }).await; })
        }
    }

    pub(super) fn sync_opened(&mut self, cx: &mut Context<Self>) {
        let (seq, save_seq, saving) = (self.sync.setup.seq, self.sync.save_seq, self.sync.saving);
        self.sync = Sync { save_seq, saving, ..Sync::default() };
        self.sync.setup.seq = seq;
        self.load_sync(cx);
    }

    pub(super) fn sync_page_left(&mut self) { self.sync.form = None; }

    fn sync_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sync.form.is_some() || self.settings != Some(Page::Sync) { return; }
        let user = cx.new(|cx| InputState::new(window, cx));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let confirmation = cx.new(|cx| InputState::new(window, cx).masked(true));
        let subscriptions = [&user, &password, &confirmation].map(|input| cx.subscribe_in(input, window,
            |this: &mut Hangar, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) { this.register_sync(cx); }
            }));
        self.sync.form = Some(SyncForm { user, password, confirmation, _subscriptions: subscriptions.into() });
    }

    fn load_sync(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.sync.setup.start();
        let done = self.sync_send_later();
        self.runtime.spawn(async move { done(SyncReply::Loaded(seq, api.server_read(&["sync", "setup"], &[], 10).await)).await });
        cx.notify();
    }

    fn write_sync(&mut self, write: Write, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let Some(setup) = self.sync.setup.ok() else { return };
        if self.sync.saving || match write { Write::Enable => !setup.registered || setup.enabled, Write::Disable => !setup.enabled, Write::Register => true } { return; }
        self.sync.save_seq += 1;
        let seq = self.sync.save_seq;
        (self.sync.saving, self.sync.error) = (true, None);
        let done = self.sync_send_later();
        self.runtime.spawn(async move {
            let result = match write {
                Write::Enable => api.server_send(reqwest::Method::POST, &["sync", "setup"], Some(json!({})), 10).await,
                Write::Disable => api.server_post(&["sync", "setup", "disable"], 10).await,
                Write::Register => unreachable!(),
            };
            done(SyncReply::Saved(seq, write, result)).await
        });
        cx.notify();
    }

    fn register_sync(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(form), Some(setup)) = (self.api.clone(), self.sync.form.as_ref(), self.sync.setup.ok()) else { return };
        if self.sync.saving || self.sync.setup.loading || setup.registered { return; }
        let user = form.user.read(cx).value().trim().to_string();
        let password = form.password.read(cx).value().to_string();
        let confirmation = form.confirmation.read(cx).value().to_string();
        self.sync.error = if !secure_setup(&api.identity()) { Some(tr("sync_config_https")) }
            else if user.is_empty() { Some(tr("sync_username_required")) }
            else if user.chars().count() > 100 { Some(tr("sync_username_too_long")) }
            else if password.chars().count() < 8 { Some(tr("sync_password_min")) }
            else if password != confirmation { Some(tr("sync_config_senhas_diferentes")) }
            else { None };
        if self.sync.error.is_some() { cx.notify(); return; }
        self.sync.save_seq += 1;
        let seq = self.sync.save_seq;
        self.sync.saving = true;
        let (base, label, token) = (api.identity().trim_end_matches('/').to_string(), self.server_label(cx), self.token.read(cx).value().trim().to_string());
        let done = self.sync_send_later();
        self.runtime.spawn(async move {
            let result = match tokio::task::spawn_blocking(move || registration_body(user, password, base, label, token)).await {
                Ok(Ok(body)) => api.server_send(reqwest::Method::POST, &["sync", "setup"], Some(body), 30).await,
                _ => Err(Failure::local("sync_config_erro")),
            };
            done(SyncReply::Saved(seq, Write::Register, result)).await
        });
        cx.notify();
    }

    fn confirm_disable_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("sync_config_desativar"), tr("sync_config_desativar_aviso"),
            tr("sync_config_desativar"), ButtonVariant::Danger, move |_, cx| {
                let _ = this.update(cx, |this, cx| this.write_sync(Write::Disable, cx));
                true
            });
    }

    pub(super) fn receive_sync(&mut self, reply: SyncReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            SyncReply::Loaded(seq, result) => {
                if seq != self.sync.setup.seq { return; }
                let result = result.map_err(|error| Self::fetch_failure(&error)).and_then(parse_setup);
                if let (Some(write), Ok(setup)) = (self.sync.reconcile, &result) {
                    if setup.enabled == matches!(write, Write::Enable | Write::Register) && (write != Write::Register || setup.registered) {
                        self.sync.error = None;
                        (self.sync.activated, self.sync.deactivated) = (matches!(write, Write::Enable | Write::Register), matches!(write, Write::Disable));
                    }
                    self.sync.reconcile = None;
                }
                if !self.sync.setup.finish(seq, result) { return; }
                if self.sync.setup.ok().is_some_and(|setup| !setup.registered) { self.sync_form(window, cx); }
                else if self.sync.setup.ok().is_some_and(|setup| setup.registered) { self.sync.form = None; }
            }
            SyncReply::Saved(seq, write, result) => {
                if seq != self.sync.save_seq { return; }
                self.sync.saving = false;
                match result.map_err(|error| Self::fetch_failure(&error)).and_then(parse_setup) {
                    Ok(setup) => {
                        self.sync.setup.seq += 1;
                        self.sync.setup.loading = false;
                        self.sync.setup.value = Some(Ok(setup));
                        (self.sync.activated, self.sync.deactivated) = (matches!(write, Write::Enable | Write::Register), matches!(write, Write::Disable));
                        if write == Write::Register { self.sync.form = None; }
                    }
                    Err(error) => {
                        self.sync.error = Some(error);
                        self.sync.reconcile = Some(write);
                        // A gravação pode ter chegado antes da queda: reler evita repetir uma ação já feita.
                        self.load_sync(cx);
                    }
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_sync(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let title = div().flex().items_center().gap_2()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Sync.title()))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()));
        let mut page = div().flex().flex_col().gap_4().child(title)
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("sync_config_ganho")))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr("sync_config_principal").replace("{servidor}", &self.server_label(cx))));
        if self.api.is_none() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).into_any_element();
        }
        if let Some(error) = &self.sync.error {
            page = page.child(div().id("sync-write-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone()));
        }
        if self.sync.setup.loading {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))).into_any_element();
        }
        let setup = match &self.sync.setup.value {
            Some(Ok(setup)) => setup,
            Some(Err(error)) => return page.child(div().id("sync-load-error").role(Role::Alert).text_sm().text_color(theme::danger()).child(error.clone()))
                .child(Button::new("sync-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| { this.sync.error = None; this.load_sync(cx); }))).into_any_element(),
            None => return page.into_any_element(),
        };
        if setup.enabled && setup.registered {
            let mut url = self.api.as_ref().and_then(|api| url::Url::parse(&api.identity()).ok());
            if let Some(url) = &mut url { url.set_path("/"); }
            let address = url.map(|url| url.to_string()).unwrap_or_default();
            let copy = address.clone();
            page = page.child(self.mark(div().rounded(px(6.)).font_weight(FontWeight::SEMIBOLD)
                .child(tr(if self.sync.activated { "sync_config_ativada" } else { "sync_config_ativa" })), "sync_config_ativa"))
                .when_some(setup.user.as_ref(), |el, user| el.child(div().text_sm().child(tr("sync_config_usuario_atual").replace("{usuario}", user))))
                .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("sync_config_como_entrar")))
                .child(self.mark(div().rounded(px(8.)).flex().items_center().gap_2()
                    .child(div().flex_1().min_w_0().font_family(theme::MONO).text_sm().truncate().child(address.clone()))
                    .child(Button::new("sync-copy").outline().small().icon(IconName::Copy)
                        .label(tr(if self.sync.copied { "sync_config_copiado" } else { "sync_config_copiar" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            this.sync.copied = true;
                            cx.notify();
                        }))), "sync_config_copiar"))
                .child(div().flex().items_center().gap_2()
                    .child(Button::new("sync-open").outline().small().label(tr("sync_config_abrir"))
                        .on_click(move |_, _, cx| cx.open_url(&address)))
                    .child(self.mark(div().rounded(px(8.)).child(Button::new("sync-disable").outline().small().label(tr("sync_config_desativar"))
                        .disabled(self.sync.saving).on_click(cx.listener(|this, _, window, cx| this.confirm_disable_sync(window, cx)))), "sync_config_desativar")))
        } else {
            if !setup.enabled {
                page = page.child(self.mark(div().rounded(px(6.)).text_sm().text_color(theme::muted()).whitespace_normal()
                    .child(tr("sync_config_direta")), "sync_config_ativar"));
            } else if !setup.registered {
                page = page.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("sync_no_account")));
            }
            if self.sync.deactivated { page = page.child(div().text_sm().child(tr("sync_config_desativada"))); }
            if setup.registered {
                page = page.child(div().text_sm().text_color(theme::muted()).whitespace_normal()
                    .child(tr("sync_config_conta_existente").replace("{usuario}", setup.user.as_deref().unwrap_or(""))))
                    .child(Button::new("sync-enable").primary().small()
                    .label(tr(if self.sync.saving { "sync_config_ativando" } else { "sync_config_reativar" }))
                        .disabled(self.sync.saving).on_click(cx.listener(|this, _, _, cx| this.write_sync(Write::Enable, cx))));
            } else {
                let secure = self.api.as_ref().is_some_and(|api| secure_setup(&api.identity()));
                if !secure { page = page.child(div().id("sync-insecure").role(Role::Alert).text_sm().text_color(theme::danger()).child(tr("sync_config_https"))); }
                if let Some(form) = &self.sync.form {
                    page = page.child(div().flex().flex_col().gap_3()
                        .child(sync_field(tr("login_usuario"), &form.user, self.sync.saving))
                        .child(sync_field(tr("login_senha"), &form.password, self.sync.saving))
                        .child(sync_field(tr("sync_config_confirmar_senha"), &form.confirmation, self.sync.saving))
                        .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("sync_config_password_help")))
                        .child(Button::new("sync-register").primary().small()
                            .label(tr(if self.sync.saving { "sync_config_ativando" } else { "sync_config_ativar" }))
                            .disabled(self.sync.saving || !secure)
                            .on_click(cx.listener(|this, _, _, cx| this.register_sync(cx)))));
                }
            }
            if setup.enabled {
                page = page.child(self.mark(div().rounded(px(8.)).child(Button::new("sync-disable").outline().small().label(tr("sync_config_desativar"))
                    .disabled(self.sync.saving).on_click(cx.listener(|this, _, window, cx| this.confirm_disable_sync(window, cx)))), "sync_config_desativar"));
            }
        }
        page.into_any_element()
    }
}
