//! O cartão de entrada e a ligação do assistente com o `Hangar` (abrir, fechar, conectar no fim).
use super::*;

impl Hangar {
    /// Fim do assistente: o app entra no Hangar desta máquina sem digitar nada; o `connect` grava a conexão quando a lista chega.
    pub(in crate::app) fn setup_connect(&mut self, address: String, token: String, window: &mut Window, cx: &mut Context<Self>) {
        self.address.update(cx, |input, cx| input.set_value(address, window, cx));
        self.token.update(cx, |input, cx| input.set_value(token, window, cx));
        self.entry = None;
        self.connect(window, cx);
    }

    /// Fechar o assistente sem nada rodando: larga a entidade (e com ela o canal da senha de administrador).
    pub(in crate::app) fn close_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.setup = None;
        self.setup_hidden = None;
        if self.api.is_none() { self.open_connection(window, cx); }
        cx.notify();
    }

    /// Fechar com o script ainda rodando: a entidade sai da tela mas fica viva, para o canal da senha e o acompanhamento
    /// continuarem. Reabrir pelo menu reaproveita `setup_hidden`.
    pub(in crate::app) fn hide_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.setup_hidden = self.setup.take();
        // O foco ficaria no assistente escondido: as teclas do app não chegariam.
        if self.api.is_none() { self.open_connection(window, cx); } else { self.root_focus.focus(window, cx); }
        cx.notify();
    }

    /// Sem conexão salva: acha a pasta pela unit/tarefa e testa `/api/sessions` antes de mostrar qualquer cartão.
    pub(in crate::app) fn start_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entry = Some(Entry::Probing);
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let install = tokio::task::spawn_blocking(local::find_install).await.ok().flatten();
            let _ = done.send(local::probe(install).await);
        });
        cx.spawn_in(window, async move |this, cx| {
            // A procura morreu: o cartão de sempre, nunca o "procurando" para sempre.
            let found = result.await.ok();
            let _ = this.update_in(cx, |this, window, cx| match found {
                Some(found) => this.entry_found(found, window, cx),
                None => this.entry_other(window, cx),
            });
        }).detach();
        cx.notify();
    }

    fn entry_found(&mut self, found: local::Found, window: &mut Window, cx: &mut Context<Self>) {
        // Conectou por outro caminho enquanto procurava (importar do Electron): a entrada não vale mais.
        if self.api.is_some() { self.entry = None; return; }
        match found {
            // É um Hangar, mas sem token legível: o cartão de sempre, com o endereço preenchido.
            local::Found::NeedsToken { address, .. } => {
                self.address.update(cx, |input, cx| input.set_value(address, window, cx));
                self.entry = None;
                self.token.update(cx, |input, cx| input.focus(window, cx));
            }
            found => self.entry = Some(Entry::Found(found)),
        }
        cx.notify();
    }

    fn entry_other(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entry = None;
        self.address.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(in crate::app) fn open_setup(&mut self, origin: Origin, window: &mut Window, cx: &mut Context<Self>) {
        // Lido já: uma volta antecipada não deixa a marca para a próxima abertura pelo menu.
        let at_launch = super::take_launch();
        if self.setup.is_some() || !supported() { return; }
        // O escondido volta como estava: nunca dois assistentes nem duas execuções.
        if let Some(hidden) = self.setup_hidden.take() {
            hidden.update(cx, |wizard, cx| wizard.focus.focus(window, cx));
            self.setup = Some(hidden);
            cx.notify();
            return;
        }
        let (hangar, runtime) = (cx.entity().downgrade(), self.runtime.handle().clone());
        self.setup = Some(cx.new(|cx| SetupWizard::new(hangar, runtime, origin, at_launch, window, cx)));
        cx.notify();
    }

    /// Pelo menu: retoma o que estava rodando; senão procura a pasta de novo, nunca um segundo clone em `~/hangar`.
    pub(in crate::app) fn open_setup_from_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let origin = match run::load_state() { Some(state) => Origin::Resume(state), None => Origin::Menu };
        self.open_setup(origin, window, cx);
    }

    /// O conteúdo do cartão de conexão enquanto há entrada; `None` mostra o cartão de endereço + token de sempre.
    pub(in crate::app) fn render_entry(&self, cx: &mut Context<Self>) -> Option<Div> {
        let entry = self.entry.as_ref()?;
        let title = |text: String| div().text_base().font_weight(FontWeight::SEMIBOLD).child(text);
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        let other = Button::new("entry-other").outline().label(tr("setup_entry_other"))
            .on_click(cx.listener(|this, _, window, cx| this.entry_other(window, cx)));
        Some(match entry {
            Entry::Probing => div().id("entry-probing").role(Role::Status).flex().items_center().gap_2()
                .child(chrome::Spinner::new("entry-probing-spin", IconName::LoaderCircle, px(14.), theme::muted()))
                .child(muted(tr("setup_entry_probing"))).into_any_element(),
            Entry::Found(local::Found::Ready { install, address, token }) => {
                let (address_owned, token_owned) = (address.clone(), token.clone());
                div().flex().flex_col().gap_3()
                    .child(title(tr("setup_entry_found_title")))
                    .child(muted(tr("setup_entry_found_hint").replace("{address}", address)))
                    .child(div().text_xs().font_family(theme::MONO).text_color(theme::faint()).whitespace_normal()
                        .child(tr("setup_entry_folder").replace("{path}", &install.dir.display().to_string())))
                    .child(div().flex().flex_wrap().justify_end().gap_2().child(other)
                        .child(Button::new("entry-connect-here").primary().label(tr("setup_entry_connect_here"))
                            .on_click(cx.listener(move |this, _, window, cx| this.setup_connect(address_owned.clone(), token_owned.clone(), window, cx)))))
                    .into_any_element()
            }
            Entry::Found(local::Found::Silent { install } | local::Found::NeedsToken { install, .. }) => {
                let install = install.clone();
                div().flex().flex_col().gap_3()
                    .child(title(tr("setup_entry_none_title")))
                    .child(muted(tr("setup_entry_none_hint")))
                    .child(div().flex().flex_wrap().justify_end().gap_2().child(other)
                        .child(Button::new("entry-install").primary().icon(IconName::Wrench).label(tr("setup_entry_install"))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_setup(Origin::Entry(install.clone()), window, cx)))))
                    .into_any_element()
            }
        }).map(|content| div().child(content))
    }
}
