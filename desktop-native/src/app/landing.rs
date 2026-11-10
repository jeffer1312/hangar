//! A primeira mensagem da tela sem sessão. Começa no Enviar: o compositor desce do meio ao pé da tela, a mensagem aparece
//! na conversa já como enviada e, embaixo dela, "Abrindo a sessão…" até a sessão nascer; as pílulas da tela inicial
//! saem. Quando a sessão abre, a mesma bolha segue na conversa dela até a mensagem real chegar ao transcript. Tempos e
//! curvas do kit de movimento (`motion.rs`): `fade-in` para o que entra e para a descida do compositor, `splash-out` para
//! o que sai. Toda a animação do envio mora aqui; `render` e `pane_element` só aplicam o quadro.
use super::*;
use super::create::NewSession;

/// translateY do `fade-in` (entra de 4 px abaixo) e do `splash-out` (sai 6 px para cima).
const RISE: f32 = 4.;
const LIFT: f32 = 6.;
/// Folga sob o compositor no pé da faixa de baixo (`pb` do `#composer`).
const COMPOSER_GAP: f32 = 10.;
/// Linha da mensagem enviada da tela sem sessão, antes de o transcript trazer a real.
pub(super) const OPENING: &str = "__opening__";

/// A chegada em curso. `travel`: quantos pixels o compositor desce; `ghost`: a tela inicial que sai, com o lugar das
/// pílulas de cima e de baixo no último desenho dela.
pub(super) struct Landing { start: Instant, travel: f32, ghost: Entity<NewSession>, top: Option<Bounds<Pixels>>, bottom: Option<Bounds<Pixels>> }

/// A mensagem mandada da tela sem sessão. `key` fica vazia até a sessão nascer. `text` é o do campo; `sent`, o que o
/// transcript vai trazer (com os caminhos dos anexos), conhecido quando a sessão nasce; `files`, os nomes dos anexos.
#[derive(Clone)]
pub(super) struct Opening { pub text: String, pub sent: String, pub files: Vec<String>, pub name: String, pub provider: String, pub key: Option<SessionKey> }

/// Um quadro: o quanto o compositor ainda está acima do lugar (`drop`), e a opacidade e o deslocamento de quem entra.
pub(super) struct Frame { pub drop: f32, pub shown: f32, pub rise: f32 }

impl Frame {
    const REST: Frame = Frame { drop: 0., shown: 1., rise: 0. };
}

/// Âncoras das duas fileiras de pílulas da tela inicial (`render_new_chat`).
pub(super) const TOP: &str = "new-chat-top";
pub(super) const BOTTOM: &str = "new-chat-bottom";

impl Hangar {
    /// Começa a chegada; a tela inicial que sai (`ghost`) ainda desenha as pílulas dela enquanto somem.
    pub(super) fn start_landing(&mut self, ghost: Entity<NewSession>, window: &Window) {
        let travel = popup::anchor_bounds("composer")
            .map_or(0., |b| f32::from(window.viewport_size().height - b.bottom()) - COMPOSER_GAP).max(0.);
        self.landing = Some(Landing { start: Instant::now(), travel, ghost,
            top: popup::anchor_bounds(TOP), bottom: popup::anchor_bounds(BOTTOM) });
    }

    pub(super) fn landing_active(&self) -> bool { self.landing.is_some() }

    /// O quadro de agora, sem pedir outro: para as áreas, que desenham dentro do quadro que a raiz pediu.
    pub(super) fn landing_now(&self) -> Frame {
        let Some(landing) = self.landing.as_ref() else { return Frame::REST };
        let t = motion::FADE_IN.ease(motion::FADE_IN.raw(landing.start));
        Frame { drop: landing.travel * (1. - t), shown: t, rise: RISE * (1. - t) }
    }

    /// O quadro da raiz, que pede o seguinte enquanto a chegada anda. Sem sessão e sem envio em curso, acabou.
    pub(super) fn landing_frame(&mut self, window: &mut Window, cx: &App) -> Frame {
        if self.landing.is_some() && self.selected.is_none() && self.opening.is_none() { self.landing = None; }
        if self.landing.is_none() { return Frame::REST; }
        motion::request_frame(window, cx);
        self.landing_now()
    }

    /// As pílulas da tela inicial saindo no lugar em que estavam; nada de clique chega a elas.
    pub(super) fn render_landing_ghost(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let landing = self.landing.as_ref()?;
        let t = motion::SPLASH_OUT.ease(motion::SPLASH_OUT.raw(landing.start));
        if t >= 1. { return None; }
        let (top, bottom, ghost) = (landing.top, landing.bottom, landing.ghost.clone());
        let (top_row, bottom_row) = ghost.update(cx, |view, cx| (view.render_top_pills(None, cx), view.render_bottom_pills(cx)));
        let place = |bounds: Bounds<Pixels>, row: Div| div().absolute().left(bounds.origin.x).top(bounds.origin.y - px(LIFT * t))
            .w(bounds.size.width).h(bounds.size.height).opacity(1. - t).occlude().child(row);
        Some(div().absolute().inset_0()
            .children(top.map(|b| place(b, top_row)))
            .children(bottom.map(|b| place(b, bottom_row)))
            .into_any_element())
    }

    /// Terminou: as áreas voltam a ser guardadas, redesenhadas uma vez, porque a última cópia guardada delas é de antes da
    /// tela sem sessão.
    pub(super) fn finish_landing(&mut self, window: &mut Window) {
        if self.landing.as_ref().is_none_or(|l| l.start.elapsed() < motion::FADE_IN.total().max(motion::SPLASH_OUT.total())) { return; }
        self.landing = None;
        let panes = [self.panes.conversation.clone(), self.panes.bottom.clone(), self.panes.side.clone()];
        window.on_next_frame(move |_, cx| for pane in &panes { pane.update(cx, |_, cx| cx.notify()); });
    }

    /// Enviar na tela sem sessão, com a criação já pedida: a mensagem sai do campo e vai para a conversa, e a chegada
    /// começa agora, sem esperar a sessão.
    pub(super) fn begin_opening(&mut self, home: Entity<NewSession>, text: String, files: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (name, provider) = { let view = home.read(cx); (view.opening_name(), view.provider().to_owned()) };
        self.opening = Some(Opening { sent: text.clone(), text, files, name, provider, key: None });
        self.composer.update(cx, |input, cx| input.set_value("", window, cx));
        if !cx.reduce_motion() { self.start_landing(home, window); }
    }

    /// A criação falhou: volta a tela sem sessão com o erro dela e a mensagem no campo, sem perder nada.
    pub(super) fn fail_opening(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(opening) = self.opening.take() else { return };
        self.landing = None;
        if self.composer.read(cx).value().is_empty() { self.composer.update(cx, |input, cx| input.set_value(opening.text, window, cx)); }
        self.composer.update(cx, |input, cx| input.focus(window, cx));
    }

    /// A bolha fica na conversa da sessão nova enquanto o transcript não traz a mensagem; chegou, sai sem duplicar.
    pub(super) fn opening_row_shown(&mut self) -> bool {
        let Some(opening) = self.opening.as_ref() else { return false };
        if opening.key.is_none() || opening.key != self.selected_key() { return false; }
        let arrived = self.chat.events.iter().rev()
            .any(|event| event.kind == "user_msg" && crate::delivery::matches_real(&display_body(event), &opening.sent));
        if arrived { self.opening = None; }
        !arrived
    }

    /// Antes de a sessão nascer, o painel direito aberto já ocupa o lugar dele: a conversa não muda de largura quando a
    /// sessão abre.
    pub(super) fn opening_side_width(&self, window: &Window) -> Option<f32> {
        self.opening.as_ref()?;
        self.new_chat_side_width(window)
    }

    /// O lugar que o painel direito aberto vai tomar quando a sessão nascer. A tela sem sessão o deixa vazio, para o
    /// compositor ficar no meio da mesma área da conversa, e não da janela inteira.
    pub(super) fn new_chat_side_width(&self, window: &Window) -> Option<f32> {
        if self.selected.is_some() || !self.side.open { return None; }
        self.side.fitted(f32::from(window.viewport_size().width), theme::is_floating(), self.nav_width(), false)
    }

    /// O painel ainda sem a sessão: o cabeçalho dele e linhas carregando.
    pub(super) fn render_opening_side(&self, width: f32) -> AnyElement {
        let floating = theme::is_floating();
        div().w(px(width)).h_full().flex_shrink_0().child(chrome::glass_panel(div().size_full().flex().flex_col().bg(theme::chrome())
            .overflow_hidden()
            .map(|el| if floating { el.rounded(px(theme::PANEL_RADIUS)).border_1().border_color(theme::border()).shadow(theme::panel_shadow()) }
                else { el.border_l_1().border_color(theme::border()) })
            .child(div().flex_shrink_0().h(px(44.)).pl_4().pr(px(12.)).flex().items_center().font_weight(FontWeight::SEMIBOLD)
                .child(tr("side_context")))
            .child(div().px_4().py(px(14.)).flex().flex_col().gap(px(10.))
                .children((0..3usize).map(|n| chrome::Skeleton::new(("opening-side", n)).row(n).h(px(12.)).rounded(px(4.))))),
            px(if floating { theme::PANEL_RADIUS } else { 0. }))).into_any_element()
    }

    /// A mensagem enviada, na bolha do usuário e com o mesmo recuo da linha real, para a troca não mexer nada.
    pub(super) fn render_opening_bubble(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // Os anexos ainda não subiram: a bolha diz os nomes, a miniatura vem com a mensagem real.
        let text = self.opening.as_ref().map(|o| std::iter::once(safe_markdown(&o.text)).filter(|t| !t.trim().is_empty())
            .chain(o.files.iter().map(|name| format!("📎 {}", safe_markdown(name)))).collect::<Vec<_>>().join("\n\n")).unwrap_or_default();
        let view = self.text_view(OPENING, OPENING, text, cx);
        div().w_full().flex().flex_col().gap_2().items_end()
            .child(user_bubble(conversation_text(div().flex().flex_col().gap_2(), true).child(chat_text(&view, cx))))
            // A faixa de hora e copiar da linha real, vazia: a altura não muda quando ela chega.
            .child(div().h(px(24.)))
            .into_any_element()
    }

    /// A tela sem sessão depois do Enviar: a conversa por vir (a mensagem e "Abrindo a sessão…") e o compositor no pé,
    /// descendo do meio. Mesmas medidas da conversa e da faixa de baixo que vêm depois.
    pub(super) fn render_opening(&mut self, home: Entity<NewSession>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Frame { drop, shown, rise } = self.landing_now();
        let (step, seconds) = home.read(cx).progress();
        let step = if step.is_empty() { tr("create_step_opening") } else { step };
        let line = div().id("opening-status").h(px(38.)).flex().items_center().gap(px(8.)).role(Role::Status)
            .child(chrome::WorkingMark::new("opening-mark", 14., theme::accent()))
            .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted())
                .child(tr("create_step_time").replace("{passo}", &step).replace("{segundos}", &seconds.to_string())));
        let bubble = self.render_opening_bubble(cx);
        let composer = self.render_composer(false, false, false, 0, false, false, window, cx);
        div().id("new-chat").size_full().flex().flex_col()
            .child(div().flex_1().min_h_0().flex().flex_col().justify_end().relative().opacity(shown).top(px(rise))
                .child(row_frame(bubble, true)).child(row_frame(line.into_any_element(), false)))
            .child(div().relative().top(px(-drop)).child(composer))
            .into_any_element()
    }
}
