//! Parear um celular (AcessoSettings.svelte, parte "parear"): o código e o QR vêm prontos do servidor e só aparecem a pedido, porque
//! quem fotografa a tela entra. O QR embute um endereço que respondeu na medida de agora.
use super::*;

#[derive(Default)]
enum Phase {
    #[default]
    Hidden,
    Loading,
    Shown { url: String, qr: Arc<Image> },
    Failed(String),
}

#[derive(Default)]
pub(in crate::app) struct Pair {
    open: bool,
    /// Descarta a resposta de um pedido de antes da troca de endereço, do Esconder ou do diálogo fechado.
    seq: u64,
    choice: Option<Kind>,
    phase: Phase,
    /// Revelar leva o foco à escolha do endereço; esconder, de volta ao botão que revelou. Quem foca é a transição.
    focus: Option<Focuses>,
}

/// Os focos do dono: "Mostrar código" e um por endereço, que o botão da escolha usa quando aparece.
struct Focuses { show: FocusHandle, choices: Vec<(Kind, FocusHandle)> }

impl Focuses {
    fn new(cx: &mut App) -> Self {
        Self { show: cx.focus_handle(), choices: [Kind::Lan, Kind::Tailscale, Kind::Public].map(|k| (k, cx.focus_handle())).into() }
    }
    fn choice(&self, kind: Kind) -> Option<FocusHandle> { self.choices.iter().find(|(k, _)| *k == kind).map(|(_, f)| f.clone()) }
}

impl Reach {
    /// O que pode ir no QR: os endereços que responderam, fora "nesta máquina" (de fora ela não alcança).
    fn pairable(&self) -> Vec<&Address> { self.addresses.iter().filter(|a| a.status == Status::Ok && a.kind != Kind::Here).collect() }
}

/// O `Button` do kit só usa o foco que ele mesmo guarda no estado com chave pelo id, e não aceita outro. Registrado ali antes do
/// primeiro desenho dele, o foco do dono vira o do botão; o caminho na árvore é o do `FocusOnClick`.
#[derive(IntoElement)]
struct OwnFocus {
    /// O mesmo id dado ao `Button`.
    id: ElementId,
    button: Button,
    focus: Option<FocusHandle>,
}

impl RenderOnce for OwnFocus {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if let Some(focus) = self.focus {
            window.with_id(std::any::type_name::<Button>(), |window| { window.use_keyed_state(self.id, cx, move |_, _| focus); });
        }
        self.button
    }
}

impl Hangar {
    pub(super) fn open_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.api.is_none() { return; }
        let pair = &mut self.machines.pair;
        *pair = Pair { open: true, seq: pair.seq + 1, focus: Some(Focuses::new(cx)), ..Pair::default() };
        // O QR embute um endereço que respondeu: a medida é a de agora, como o web ao montar.
        self.load_reach(cx);
        let hangar = cx.entity();
        let body = cx.new(|cx| PairBody { _observe: cx.observe(&hangar, |_, _, cx| cx.notify()), hangar: hangar.downgrade() });
        let weak = hangar.downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            dialog.w(px(560.)).child(body.clone()).on_ok(enter_to_focused)
                .on_close(move |_, _, cx| { let _ = weak.update(cx, |this, _| {
                    let pair = &mut this.machines.pair;
                    (pair.open, pair.seq) = (false, pair.seq + 1);
                }); })
        });
    }

    /// O endereço do QR: o escolhido, se ainda respondeu; sem escolha, o mais rápido (é o que a frase "respondeu mais rápido" diz).
    fn pair_kind(&self) -> Option<Kind> {
        let m = &self.machines;
        let list = m.reach.ok().filter(|_| !m.reach.loading)?.pairable();
        match m.pair.choice {
            Some(kind) if list.iter().any(|a| a.kind == kind) => Some(kind),
            Some(_) => list.first().map(|a| a.kind),
            None => Reach::fastest(list.into_iter()).map(|a| a.kind),
        }
    }

    fn reveal_pair(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(kind)) = (self.api.clone(), self.pair_kind()) else { return };
        let pair = &mut self.machines.pair;
        (pair.choice, pair.phase, pair.seq) = (Some(kind), Phase::Loading, pair.seq + 1);
        let (seq, done) = (pair.seq, self.machines_send_later());
        self.runtime.spawn(async move {
            done(MachinesReply::Paired(seq, api.server_read(&["alcance", "pareamento"], &[("endereco", kind.raw())], 15).await)).await
        });
        cx.notify();
    }

    pub(super) fn paired(&mut self, seq: u64, result: Result<Value, Failure>, window: &mut Window) {
        let pair = &mut self.machines.pair;
        if !pair.open || seq != pair.seq { return; }
        pair.phase = match result {
            Ok(value) => match (value.get("url").and_then(Value::as_str), value.get("qr_svg").and_then(Value::as_str)) {
                (Some(url), Some(svg)) => {
                    // O botão da escolha só aparece no desenho seguinte.
                    if let Some(focus) = pair.choice.zip(pair.focus.as_ref()).and_then(|(kind, f)| f.choice(kind)) {
                        window.on_next_frame(move |window, cx| focus.focus(window, cx));
                    }
                    Phase::Shown { url: url.to_owned(), qr: Arc::new(Image::from_bytes(ImageFormat::Svg, svg.as_bytes().to_vec())) }
                }
                _ => Phase::Failed(tr("invalid_response")),
            },
            // Queda de rede não tem texto traduzível: só a frase da casa. O motivo do servidor vem inteiro.
            Err(error) if error.status.is_none() => Phase::Failed(tr("connection_failed")),
            Err(error) => Phase::Failed(format!("{}: {}", tr("connection_failed"), Self::fetch_failure(&error))),
        };
    }

    fn choose_pair(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let pair = &mut self.machines.pair;
        if pair.choice == Some(kind) && matches!(pair.phase, Phase::Shown { .. }) { return; }
        pair.choice = Some(kind);
        // Revelado, o QR acompanha: o seletor diria um endereço e o QR mostraria outro.
        if matches!(pair.phase, Phase::Shown { .. } | Phase::Failed(_)) { self.reveal_pair(cx); } else { cx.notify(); }
    }

    fn hide_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pair = &mut self.machines.pair;
        (pair.phase, pair.seq) = (Phase::Hidden, pair.seq + 1);
        if let Some(focus) = pair.focus.as_ref().map(|f| f.show.clone()) { window.on_next_frame(move |window, cx| focus.focus(window, cx)); }
        cx.notify();
    }

    fn render_pair(&self, cx: &mut Context<Self>) -> Div {
        let kind = self.pair_kind();
        let m = &self.machines;
        let focus = m.pair.focus.as_ref();
        let reach = m.reach.ok().filter(|_| !m.reach.loading);
        let reach_error = m.reach.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.reach.loading).cloned();
        let list = reach.map(|r| r.pairable().into_iter().map(|a| a.kind).collect::<Vec<_>>()).unwrap_or_default();
        let fastest = reach.and_then(|r| Reach::fastest(r.pairable().into_iter()).map(|a| a.kind));
        let muted = |text: String| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text);
        let panel = || div().p(px(14.)).rounded(px(12.)).border_1().border_color(theme::border()).bg(theme::inset()).flex().flex_col().gap(px(10.));

        let block = if let Some(error) = reach_error {
            div().child(div().id("machines-pair-reach-error").role(Role::Alert).flex().items_center().gap(px(10.))
                .child(div().flex_1().text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error))
                .child(Button::new("machines-pair-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_reach(cx)))))
        } else if reach.is_some() && list.is_empty() {
            // Nada respondeu: não há o que revelar. A lista do detalhe deste servidor é onde se conserta.
            panel().child(muted(tr("machines_pair_no_candidate")))
        } else if matches!(m.pair.phase, Phase::Hidden) {
            let show = Button::new("machines-pair-show").primary().small().label(tr("machines_pair_show")).disabled(kind.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.reveal_pair(cx)));
            panel().child(div().text_size(px(13.)).whitespace_normal().child(tr("machines_pair_warning")))
                .child(div().flex().child(OwnFocus { id: "machines-pair-show".into(), button: show, focus: focus.map(|f| f.show.clone()) }))
        } else {
            let (url, qr) = match &m.pair.phase { Phase::Shown { url, qr } => (url.clone(), Some(qr.clone())), _ => (String::new(), None) };
            let failed = match &m.pair.phase { Phase::Failed(error) => Some(error.clone()), _ => None };
            let shown = qr.is_some();
            // No erro o quadrado do QR não é desenhado: branco com "Testando…" para sempre seria um beco.
            let square = failed.is_none().then(|| div().flex_shrink_0().size(px(176.)).p(px(8.)).rounded(px(10.)).bg(gpui_kit::white())
                .flex().items_center().justify_center()
                .child(match qr {
                    Some(qr) => img(qr).size_full().into_any_element(),
                    None => div().text_size(px(12.5)).text_color(gpui_kit::black()).child(tr("machines_testing")).into_any_element(),
                }));
            let choices = div().flex().flex_col().gap(px(6.))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("machines_pair_address")))
                .child(div().flex().flex_wrap().gap(px(6.)).children(list.iter().map(|&k| {
                    let id = SharedString::from(format!("machines-pair-{}", k.raw()));
                    let button = Button::new(id.clone()).outline().small().label(k.name()).selected(kind == Some(k))
                        // `selected` só pinta; o leitor de tela sabe qual endereço está no QR pelo estado de alternância.
                        .toggled(kind == Some(k))
                        .accessibility_label(format!("{}: {}", tr("machines_pair_address"), k.name()))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose_pair(k, cx)));
                    OwnFocus { id: id.into(), button, focus: focus.and_then(|f| f.choice(k)) }
                })))
                .child(muted(tr("machines_pair_switch_note")))
                .when(kind.is_some() && kind == fastest, |el| el.child(muted(tr("machines_pair_chosen")
                    .replace("{rede}", &kind.map(Kind::name).unwrap_or_default()))));
            let copy_url = url.clone();
            let column = div().flex_1().min_w_0().flex().flex_col().gap(px(12.))
                .child(div().flex().flex_col().gap(px(4.)).child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("machines_pair_code")))
                    .child(div().id("machines-pair-code").font_family(theme::MONO).text_size(px(12.5)).whitespace_normal().child(url)))
                .map(|el| match (&failed, shown) {
                    (Some(error), _) => el.child(div().id("machines-pair-error").role(Role::Alert).text_size(px(12.5)).text_color(theme::danger())
                        .whitespace_normal().child(error.clone())),
                    (None, true) => el.child(choices),
                    (None, false) => el.child(div().id("machines-pair-loading").role(Role::Status).child(muted(tr("machines_testing")))),
                })
                // Esconder nos três estados: o erro não pode ser beco sem saída.
                .child(div().flex().flex_wrap().gap(px(8.))
                    .when(shown, |el| el.child(Button::new("machines-pair-copy").outline().small().icon(IconName::Copy).label(tr("machines_pair_copy"))
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy_url.clone())))))
                    .child(Button::new("machines-pair-hide").ghost().small().label(tr("machines_pair_hide"))
                        .on_click(cx.listener(|this, _, window, cx| this.hide_pair(window, cx)))));
            div().flex().items_start().gap(px(16.)).children(square).child(column)
        };

        div().flex().flex_col().gap(px(14.)).pb(px(8.))
            .child(div().pr(px(28.)).flex().flex_col().gap(px(2.))
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(tr("machines_pair")))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(self.server_label(cx))))
            .child(muted(tr("machines_pair_legend")))
            .child(block)
    }
}

/// O corpo do diálogo, no mesmo desenho do `MachineDetail`.
struct PairBody {
    hangar: WeakEntity<Hangar>,
    _observe: Subscription,
}

impl Render for PairBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.hangar.update(cx, |hangar, cx| hangar.render_pair(cx)).unwrap_or_else(|_| div())
    }
}
