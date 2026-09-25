//! Cartão do kick-off `[hangar: passagem de bastão]` (`BastaoCard.svelte`): o recado que a sessão sucessora recebe, em
//! passos e avisos, com o resumo gravado num diálogo e o recado cru num bloco fechado, porque o cartão pode ler errado e
//! o recado não.
use super::*;
use super::device::Remote;
use super::machines::FocusOnClick;
use crate::cards::Baton;
use std::rc::Rc;

/// O resumo do bastão, lido uma vez por tela (`GET …/bastao/dossie`): o arquivo é gravado antes de a sessão nascer e não
/// muda depois. Trocar de sessão descarta este estado, e a resposta de um pedido velho não acha mais a quem entregar.
pub(super) struct Dossier { key: SessionKey, remote: Remote<Option<Entity<TextViewState>>> }

impl Render for Dossier {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // "Não consegui ler" e "o resumo está vazio" são respostas diferentes: a falha nunca vira caixa vazia.
        match self.remote.value.as_ref().filter(|_| !self.remote.loading) {
            None => div().id("dossier-loading").role(Role::Status).text_sm().text_color(theme::muted()).child(web("comum_carregando", &[])).into_any_element(),
            Some(Err(error)) => div().id("dossier-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal()
                .child(error.clone()).into_any_element(),
            Some(Ok(None)) => div().text_sm().text_color(theme::muted()).child(tr("create_baton_preview_empty")).into_any_element(),
            Some(Ok(Some(view))) => TextView::new(view).selectable(true).scrollable(false).text_sm().on_link_click(open_web_link).into_any_element(),
        }
    }
}

fn web(key: &str, params: &[(&str, &str)]) -> String {
    let params = params.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
    crate::i18n::tr_web(key, &params).unwrap_or_else(|| key.to_owned())
}

/// O único 404 desta rota é o do resumo que não está mais no disco, e a frase do web já diz tudo; o resto diz o que
/// falhou e o motivo.
fn dossier_failure(error: &Failure) -> String {
    if error.status == Some(404) { return web("erro_bastao_sem_dossie", &[]); }
    format!("{} {}", tr("create_baton_preview_failed"), Hangar::fetch_failure(error))
}

impl Hangar {
    pub(super) fn render_baton_card(&mut self, id: &str, event: usize, card: Baton, cx: &mut Context<Self>) -> AnyElement {
        let raw_key = format!("{id}#raw");
        let open = self.expanded.contains(&raw_key);
        let event = &self.chat.events[event];
        let body = event.body();
        let raw_text = body.trim();
        // O cru só aberto, e com o mesmo teto dos detalhes das chamadas.
        let raw = open.then(|| {
            let (shown, clipped) = conversation::clip(raw_text, DETAIL_MAX);
            let note = clipped.then(|| tr("clipped").replace("{shown}", &DETAIL_MAX.to_string())
                .replace("{total}", &raw_text.chars().count().to_string()));
            (shown.to_owned(), note)
        });
        let lines = raw_text.lines().count().to_string();
        let time = clock(event.ts);
        let origin = card.origin.as_str();
        // Selo de seta sobre a marca: a passagem é do Hangar; o anel separa os dois, que encostados viram uma mancha só.
        let seal = div().relative().size(px(21.)).flex_shrink_0()
            .child(chrome::hangar_mark(19., theme::accent()))
            .child(div().absolute().right(px(-3.)).bottom(px(-2.)).size(px(13.)).rounded_full().bg(theme::elevated())
                .border_1().border_color(theme::raised()).flex().items_center().justify_center()
                .child(Icon::new(IconName::ArrowRight).size(px(9.)).text_color(theme::accent())));
        let header = div().flex().items_center().gap_2().px_3().py_2().bg(theme::inset())
            .child(seal)
            .child(div().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text())
                .child(web("bastao_card_titulo", &[("nome", origin)])))
            .when_some(time, |el, time| el.child(div().ml_auto().flex_shrink_0().text_size(px(10.5)).text_color(theme::muted()).child(time)));
        let step = |n: &'static str, key: &str| div().flex().gap(px(7.)).text_size(px(12.5)).text_color(theme::muted())
            .child(div().mt(px(1.)).size(px(16.)).flex_shrink_0().rounded_full().bg(theme::accent_dim()).flex().items_center().justify_center()
                .text_size(px(10.)).font_weight(FontWeight::BOLD).text_color(theme::accent_text()).child(n))
            .child(div().flex_1().min_w_0().whitespace_normal().child(web(key, &[])));
        let warning = |text: String| div().flex().gap(px(6.)).text_xs().text_color(theme::muted())
            .child(div().flex_shrink_0().text_color(theme::warning()).child("▲"))
            .child(div().flex_1().min_w_0().whitespace_normal().child(text));
        // Preenchimento pela cor da linha (translúcida): aparece sobre o cartão nos dois temas, onde `inset` e `raised` empatam no claro.
        let chip = |text: String| div().px(px(8.)).py(px(2.)).rounded_full().bg(theme::border()).text_size(px(11.)).text_color(theme::muted()).child(text);
        // As duas ações em pílula cheia, como no web: a do resumo em destaque, a da origem neutra. Contorno fino e texto solto
        // não liam como botão sobre o cartão.
        let pill = |button: Button, main: bool| button.small().rounded_full().custom(ButtonCustomVariant::new(cx)
            .color(if main { theme::accent_dim() } else { theme::border() })
            .foreground(if main { theme::accent_text() } else { theme::text() })
            .hover(if main { theme::accent_focus() } else { theme::border_strong() })
            .active(if main { theme::accent_focus() } else { theme::border_strong() }))
            // A cor do variante só pinta no hover; o fundo parado vem daqui, como em `create::choice`.
            .bg(if main { theme::accent_dim() } else { theme::border() });
        let chips = (!card.account.is_empty() || !card.model.is_empty()).then(|| div().flex().flex_wrap().gap(px(6.))
            .when(!card.account.is_empty(), |el| el.child(chip(web("bastao_card_de_conta", &[("conta", &card.account)]))))
            .when(!card.model.is_empty(), |el| el.child(chip(card.model.clone()))));
        // O diálogo devolve o foco a quem o tinha ao abrir: o botão pega o foco antes, e o Esc volta a ele.
        let this = cx.entity().downgrade();
        let dossier_origin = card.origin.clone();
        let dossier_id: ElementId = SharedString::from(format!("baton-dossier-{id}")).into();
        let dossier = FocusOnClick { id: dossier_id.clone(), button: pill(Button::new(dossier_id), true).label(web("bastao_card_abrir_dossie", &[])),
            open: Rc::new(move |window, cx| {
                let _ = this.update(cx, |this, cx| this.open_dossier(dossier_origin.clone(), window, cx));
            }) };
        let target = card.origin.clone();
        let origin_button = pill(Button::new(SharedString::from(format!("baton-origin-{id}"))), false)
            .label(web("hangar_cmd_abrir", &[("nome", origin)]))
            .on_click(cx.listener(move |this, _, window, cx| this.open_origin(&target, window, cx)));
        let toggle_key = raw_key.clone();
        let original = self.disclosure(&raw_key, open)
            .child(div().flex_1().text_xs().text_color(theme::muted()).child(web("bastao_card_recado_original", &[("n", &lines)])))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let body = div().flex().flex_col().gap_2().px_3().pt_2().pb_3()
            // Destaque pelo fundo inteiro, sem faixa lateral.
            .child(div().p_2().rounded_md().bg(theme::accent_dim()).text_sm().text_color(theme::text()).whitespace_normal().child(web("bastao_card_resumo", &[])))
            .child(div().flex().flex_col().gap(px(6.)).child(step("1", "bastao_card_passo_dossie")).child(step("2", "bastao_card_passo_plano")))
            .child(div().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::muted()).whitespace_normal().child(card.dossier.clone()))
            .child(div().flex().flex_col().gap(px(5.))
                .child(warning(web("bastao_card_aviso_viva", &[("nome", origin)])))
                .child(warning(web("bastao_card_aviso_par", &[]))))
            .children(chips)
            .child(div().flex().flex_wrap().gap_2().child(dossier).child(origin_button))
            .child(div().flex().flex_col().pt_1().border_t_1().border_color(theme::border()).child(original)
                .when_some(raw, |el, (raw, note)| el.child(div().px_2().pt_1().font_family(theme::MONO).text_xs().text_color(theme::muted()).child(raw))
                    .when_some(note, |el, note| el.child(div().px_2().pt_1().text_xs().text_color(theme::muted()).child(note)))));
        div().max_w(relative(0.8)).min_w(px(280.)).flex().flex_col().overflow_hidden().rounded_md().bg(theme::raised())
            .border_1().border_color(theme::border()).child(header).child(body).into_any_element()
    }

    /// Abre o resumo da sessão desta tela; lê só na primeira abertura, ou de novo depois de uma falha.
    fn open_dossier(&mut self, origin: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return };
        let dossier = match self.dossier.clone().filter(|d| d.read(cx).key == key) {
            Some(dossier) => dossier,
            None => { let dossier = cx.new(|_| Dossier { key: key.clone(), remote: Remote::default() }); self.dossier = Some(dossier.clone()); dossier }
        };
        let fetch = dossier.update(cx, |d, cx| {
            if d.remote.loading || d.remote.ok().is_some() { return None; }
            cx.notify();
            Some(d.remote.start())
        });
        if let Some(seq) = fetch {
            let (id, tx, connection) = (dossier.entity_id(), self.tx.clone(), self.connection);
            self.runtime.spawn(async move {
                let result = api.server_bytes(&["sessions", &key.name, "bastao", "dossie"], 30).await
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Dossier(id, seq, result) }).await;
            });
        }
        let title = web("bastao_card_titulo", &[("nome", &origin)]);
        window.open_dialog(cx, move |dialog, _, _| dialog.w(px(720.)).title(title.clone()).child(dossier.clone()));
    }

    /// Só o pedido mais novo do resumo à vista entra: o de uma sessão anterior não acha mais o seu estado.
    pub(super) fn receive_dossier(&mut self, id: EntityId, seq: u64, result: Result<String, Failure>, cx: &mut Context<Self>) {
        let Some(dossier) = self.dossier.clone().filter(|d| d.entity_id() == id) else { return };
        let value = result.map_err(|error| dossier_failure(&error))
            .map(|text| (!text.trim().is_empty()).then(|| cx.new(|cx| TextViewState::markdown(&safe_markdown(&text), cx))));
        dossier.update(cx, |d, cx| if d.remote.finish(seq, value) { cx.notify(); });
    }

    /// A origem continua viva: abre o chat dela. Fora da lista deste servidor, um aviso.
    fn open_origin(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.sessions.iter().find(|s| s.name == name).cloned() {
            Some(session) => self.select(session, window, cx),
            None => window.push_notification(Notification::warning(tr("baton_origin_missing").replace("{nome}", name)), cx),
        }
    }
}
