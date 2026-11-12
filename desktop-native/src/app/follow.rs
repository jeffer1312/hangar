//! Rolagem da conversa: colada no fim, a lista desliza até o texto novo em vez de saltar; a roda
//! do mouse anda em passos animados; só gesto da pessoa solta o fim.
use super::*;
use super::panes::Area;

// Mola de velocidade no formato do use-stick-to-bottom, com as constantes medidas pelo Zeron.
const DAMPING: f32 = 0.7;
const STIFFNESS: f32 = 0.05;
const MASS: f32 = 1.25;
const FRAME_MS: f32 = 1000. / 60.;
const MAX_CATCHUP_FRAMES: f32 = 8.;
const GROWTH_EMA: f32 = 0.12;
const CHASE_MAX_LEAD: f32 = 32.;
/// Passo máximo por quadro de 60 Hz: bloco grande entrando desliza, não salta. 1200 px/s ainda
/// alcança qualquer texto; atraso maior que 2,5 janelas teleporta.
const MAX_STEP: f32 = 20.;
/// Parada há mais que isso, a mola esquece a velocidade do texto que chegava.
const SETTLE_GRACE: Duration = Duration::from_millis(500);
const AT_BOTTOM: f32 = 2.;
/// Distância do topo, em px, em que a rolagem para cima já busca o histórico anterior.
const OLDER_BAND: f32 = 300.;
/// Descendo por gesto a menos disso do fim, a lista volta a acompanhar.
const STICK_BAND: f32 = 70.;
/// Distância do fim a partir da qual a pílula "Ir para o fim" aparece, a do Zeron.
const JUMP_MIN: f32 = 320.;
/// Salto maior que isso (em alturas da janela) teleporta até essa distância e desliza o resto.
const GLIDE_MAX_VIEWPORTS: f32 = 2.5;
/// Mesmo passo por linha que a lista usa, para a roda andar a mesma distância, só que animada.
const WHEEL_LINE_PX: f32 = 20.;
const WHEEL_TAU_MS: f32 = 45.;

/// Passo da roda neste quadro e o que ainda falta andar. Descer zera o resto ao chegar no fim:
/// sem isso, um entalhe dado já no fim fica pendente e pede um quadro atrás do outro.
fn wheel_step(wheel: f32, ms: f32, distance: f32) -> (f32, f32) {
    let mut step = wheel * (1. - (-ms / WHEEL_TAU_MS).exp());
    if (wheel - step).abs() < 0.5 { step = wheel; }
    if wheel <= 0. { return (step, wheel - step); }
    step = step.min(distance);
    (step, if distance - step <= 0.5 { 0. } else { wheel - step })
}

/// Se a âncora da lista está no trecho trocado, o gpui a zera no começo do trecho e a tela pularia para lá.
fn splice_keeping_anchor(list: &ListState, old_ids: &[String], range: std::ops::Range<usize>, new_ids: &[String]) {
    let anchor = list.logical_scroll_top();
    let top = -f32::from(list.scroll_px_offset_for_scrollbar().y);
    list.splice(range.clone(), new_ids.len());
    if !range.contains(&anchor.item_ix) { return; }
    // A linha do topo só mudou de índice dentro do trecho: ancora nela. Pixel não serve, porque linha nunca desenhada
    // vale 0 px antes da troca e a altura real depois.
    if let Some(moved) = old_ids.get(anchor.item_ix).and_then(|id| new_ids.iter().position(|new| new == id)) {
        list.scroll_to(ListOffset { item_ix: range.start + moved, offset_in_item: anchor.offset_in_item });
        return;
    }
    // A própria linha foi trocada (prévia virando a resposta gravada): mesma altura, contada do começo do trecho.
    let start = -f32::from(list.scroll_px_offset_for_scrollbar().y);
    list.scroll_to(ListOffset { item_ix: range.start, offset_in_item: px((top - start).max(0.)) });
}

#[derive(Debug, Clone, Copy, Default)]
struct StickSpring { velocity: f32, target_vel: f32, last_target: Option<f32> }

impl StickSpring {
    /// Avança `frames` quadros de 60 Hz; nunca passa do alvo e encosta nele a 0,5 px.
    fn step(&mut self, mut pos: f32, target: f32, mut frames: f32) -> f32 {
        let grew = self.last_target.map_or(0., |last| target - last);
        self.last_target = Some(target);
        if grew < -1. {
            self.target_vel = 0.;
        } else {
            let observed = grew.max(0.) / frames.max(0.25);
            self.target_vel += GROWTH_EMA * (observed - self.target_vel);
        }
        let chase = target - (self.target_vel * 9.).min(CHASE_MAX_LEAD);
        let mut v = self.velocity;
        while frames > 0. {
            let h = frames.min(1.);
            frames -= h;
            let diff = (chase - pos).max(0.);
            v += h * ((DAMPING * v + STIFFNESS * diff) / MASS - v);
            pos = (pos + ((v + self.target_vel) * h).min(MAX_STEP * h)).min(target);
        }
        self.velocity = v;
        if target - pos <= 0.5 { target } else { pos }
    }
}

/// Colar no fim em vez de deslizar: sem animação pedida pelo sistema, ou na pele Terminal, onde o texto novo aparece
/// no lugar como no terminal do Claude Code.
fn snap_to_end(cx: &App) -> bool {
    cx.reduce_motion() || appearance::get().tool_look == appearance::ToolLook::Terminal
}

pub(super) struct Follow {
    pinned: bool,
    spring: StickSpring,
    tick: Option<Instant>,
    settled: Option<Instant>,
    kick: bool,
    scheduled: bool,
    /// Topo visível na última leitura: gesto se mede por ele, porque texto novo muda a distância sem mexer nele.
    last_top: f32,
    /// Pixels que a roda ainda vai andar; positivo desce.
    wheel: f32,
    wheel_tick: Option<Instant>,
}

impl Default for Follow {
    fn default() -> Self {
        Self { pinned: true, spring: StickSpring::default(), tick: None, settled: None, kick: false, scheduled: false,
            last_top: 0., wheel: 0., wheel_tick: None }
    }
}

impl Hangar {
    pub(super) fn watch_user_scroll(list: &ListState, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        // O handler roda com a lista emprestada: ler a posição só depois que ela soltar.
        list.set_scroll_handler(move |_, _, cx| {
            let view = view.clone();
            cx.defer(move |cx| { view.update(cx, |this, cx| this.user_scrolled(cx)).ok(); });
        });
    }

    fn distance_from_bottom(&self) -> f32 {
        let max = f32::from(self.list_state.max_offset_for_scrollbar().y);
        (max + f32::from(self.list_state.scroll_px_offset_for_scrollbar().y)).max(0.)
    }

    /// Solta do fim e longe dele: a pílula "Ir para o fim" aparece. Perto do fim ela cobriria o próprio texto que falta.
    pub(super) fn follow_detached(&self) -> bool { !self.follow.pinned && self.distance_from_bottom() > JUMP_MIN }

    fn visible_top(&self) -> f32 { f32::from(self.list_state.max_offset_for_scrollbar().y) - self.distance_from_bottom() }

    /// Troca as linhas `range` de `row_ids` por `new_ids` mantendo o texto da tela parado. Chamar antes de gravar
    /// `row_ids`.
    pub(super) fn splice_rows(&self, range: std::ops::Range<usize>, new_ids: &[String]) {
        splice_keeping_anchor(&self.list_state, &self.row_ids, range, new_ids);
    }

    /// Conversa aberta ou recarregada: nasce no fim, sem deslizar.
    pub(super) fn follow_reset(&mut self) { self.follow = Follow::default(); }

    /// Histórico antigo entrou em cima com a lista colada no fim: continua no fim. Sem isto, a troca que alcança a
    /// âncora (as linhas fixas do fim mudam com mais eventos) a repunha contada do topo, no meio da conversa antiga.
    pub(super) fn follow_keep_end(&mut self) {
        if self.follow.pinned { self.list_state.scroll_to_end(); }
    }

    /// Chamar antes de a altura das linhas mudar. Colada no fim, a lista saltaria para o fim novo
    /// no próximo layout; ancorada num pixel logo acima dele, ela fica parada e a mola desliza.
    pub(super) fn follow_content_changed(&mut self, cx: &App) {
        if !self.follow.pinned { return; }
        // Sem animação, colada acompanha colada, como o `FollowMode::Tail` fazia. Colar aqui, e não
        // no render, deixa o gesto de subir chegar ao handler antes: no render ele seria desfeito.
        if snap_to_end(cx) { self.list_state.scroll_to_end(); return; }
        self.follow.kick = true;
        self.unglue(0.75);
    }

    /// Colada no fim, a posição lógica fica além do último item e o `scroll_by` parte da altura
    /// total, não do topo visível. Troca pela posição real (menos `lift`), que o layout mantém.
    fn unglue(&self, lift: f32) {
        if self.list_state.logical_scroll_top().item_ix < self.list_state.item_count() { return; }
        self.list_state.scroll_to_end();
        let content = -f32::from(self.list_state.scroll_px_offset_for_scrollbar().y);
        let max = f32::from(self.list_state.max_offset_for_scrollbar().y);
        if max > 0. { self.list_state.scroll_by(px(max - lift - content)); }
    }

    /// "Ir para o fim" e envio: volta a acompanhar deslizando.
    pub(super) fn follow_engage(&mut self, cx: &mut Context<Self>) {
        self.follow.pinned = true;
        self.follow.wheel = 0.;
        if snap_to_end(cx) { self.list_state.scroll_to_end(); self.redraw(Area::Conversation, cx); return; }
        // Mais de uma janela longe do fim, deslizar vira espera: salta direto.
        let viewport = f32::from(self.list_state.viewport_bounds().size.height);
        if viewport > 0. && self.distance_from_bottom() > viewport {
            self.list_state.scroll_to_end();
            self.follow.spring = StickSpring::default();
            self.redraw(Area::Conversation, cx);
            return;
        }
        self.follow.kick = true;
        self.redraw(Area::Conversation, cx);
    }

    /// Marcador lateral: solta do fim e põe a linha no topo. O `scroll_to` da lista vale para linha ainda não medida.
    pub(super) fn jump_to_row(&mut self, row: usize, cx: &mut Context<Self>) {
        self.release();
        self.follow.wheel = 0.;
        self.list_state.scroll_to(ListOffset { item_ix: row, offset_in_item: px(0.) });
        self.redraw(Area::Conversation, cx);
    }

    fn release(&mut self) {
        self.follow.pinned = false;
        self.follow.spring = StickSpring::default();
        self.follow.tick = None;
        self.follow.settled = None;
    }

    /// Gesto que a própria lista tratou (trackpad, toque): afastar do fim solta; voltar perto dele cola.
    fn user_scrolled(&mut self, cx: &mut Context<Self>) {
        self.follow.wheel = 0.;
        let distance = self.distance_from_bottom();
        let top = self.visible_top();
        let previous = std::mem::replace(&mut self.follow.last_top, top);
        if top < previous - 1. && distance > AT_BOTTOM {
            self.release();
        } else if !self.follow.pinned && (distance <= AT_BOTTOM || distance <= STICK_BAND && top > previous) {
            self.follow.pinned = true;
            self.follow.kick = true;
        }
        self.older_near_top(top < previous, cx);
        self.redraw(Area::Conversation, cx);
    }

    /// Subindo perto do topo e o servidor tem mais: busca sozinho, como o web, sem esperar o botão. Vale para todo
    /// caminho que rola (gesto, roda, tecla), não só o gesto que a lista trata.
    fn older_near_top(&mut self, up: bool, cx: &mut Context<Self>) {
        if up && self.visible_top() < OLDER_BAND && self.has_older && !self.loading { self.load_older(cx); }
    }

    /// Entalhe da roda do mouse: vira distância a percorrer em alguns quadros, não um salto.
    fn wheel_lines(&mut self, lines: f32, cx: &mut Context<Self>) {
        let down = -lines * WHEEL_LINE_PX;
        // Conversa que cabe na janela não rola: soltar ali deixaria o texto acompanhar aos saltos.
        if down < 0. && f32::from(self.list_state.max_offset_for_scrollbar().y) > 0. { self.release(); }
        if self.follow.wheel != 0. && self.follow.wheel.signum() != down.signum() { self.follow.wheel = 0.; }
        if self.follow.wheel == 0. { self.follow.wheel_tick = None; }
        self.follow.wheel += down;
        self.redraw(Area::Conversation, cx);
    }

    /// As teclas de página valem só com a conversa na tela: sem página, busca, seletor ou diálogo por cima. O seletor de
    /// modelo tem lista longa própria, que precisa delas.
    pub(super) fn chat_keys_apply(&self, window: &mut Window, cx: &mut App) -> bool {
        let page_open = self.page_open();
        !page_open && !self.search.open && !self.connection_dialog && !self.controls.picker_open() && self.selected.is_some()
            && !window.has_active_dialog(cx)
    }

    /// Page Up/Down rolam uma janela da conversa, mesmo com o cursor no compositor, que não usa essas teclas. Home e End
    /// vão ao começo e ao fim só fora de campo de texto: dentro dele movem o cursor.
    pub(super) fn chat_page_key(&mut self, key: &str, window: &Window, cx: &mut Context<Self>) -> bool {
        let viewport = f32::from(self.list_state.viewport_bounds().size.height);
        if viewport <= 0. { return false; }
        let page = (viewport - 60.).max(viewport * 0.5);
        match key {
            "pageup" | "pagedown" => {
                let up = key == "pageup";
                if cx.reduce_motion() {
                    if up { self.release(); self.unglue(0.); self.list_state.scroll_by(px(-page)); }
                    else if self.distance_from_bottom() - page <= STICK_BAND { self.follow_engage(cx); return true; }
                    else { self.list_state.scroll_by(px(page)); }
                    self.follow.last_top = self.visible_top();
                    self.older_near_top(up, cx);
                    self.redraw(Area::Conversation, cx);
                } else {
                    self.wheel_lines(if up { page } else { -page } / WHEEL_LINE_PX, cx);
                }
                true
            }
            "home" if !window.text_input_focused() => {
                self.jump_to_row(0, cx);
                if self.has_older && !self.loading { self.load_older(cx); }
                true
            }
            // Salta e cola: depois do Home as linhas de baixo ainda não têm medida, e a distância que a mola usaria sai curta.
            "end" if !window.text_input_focused() => {
                self.follow.pinned = true;
                self.follow.wheel = 0.;
                self.follow.spring = StickSpring::default();
                self.list_state.scroll_to_end();
                self.redraw(Area::Conversation, cx);
                true
            }
            _ => false,
        }
    }

    /// Camada sobre a lista que pega a roda antes dela; trackpad (pixels) segue direto para a lista.
    pub(super) fn wheel_layer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.weak_entity();
        canvas(|bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal), move |_, hitbox, window, _| {
            window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                let ScrollDelta::Lines(lines) = event.delta else { return };
                if phase != DispatchPhase::Capture || lines.y == 0. || cx.reduce_motion() || !hitbox.should_handle_scroll(window) { return; }
                cx.stop_propagation();
                view.update(cx, |this, cx| this.wheel_lines(lines.y, cx)).ok();
            });
        }).absolute().inset_0()
    }

    /// Chamado no render: um quadro por vez enquanto houver mola ou roda andando.
    pub(super) fn schedule_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follow.last_top = self.visible_top();
        if cx.reduce_motion() { self.follow.wheel = 0.; return; }
        if self.follow.scheduled { return; }
        // Na pele Terminal o fim cola sem mola; só a roda anda em quadros.
        let spring = !snap_to_end(cx) && self.follow.pinned && (self.follow.kick || self.distance_from_bottom() > 0.5);
        if !spring && self.follow.wheel == 0. { return; }
        self.follow.scheduled = true;
        let view = cx.weak_entity();
        window.on_next_frame(move |_, cx| { view.update(cx, |this, cx| this.scroll_frame(cx)).ok(); });
    }

    fn scroll_frame(&mut self, cx: &mut Context<Self>) {
        self.follow.scheduled = false;
        self.follow.kick = false;
        let detached = self.follow_detached();
        let now = Instant::now();
        let wheel_up = self.follow.wheel < 0.;
        if self.follow.wheel != 0. { self.wheel_frame(now); self.older_near_top(wheel_up, cx); }
        if self.follow.pinned && self.follow.wheel == 0. { self.spring_frame(now); }
        // O último passo da roda não pede quadro; se ele cruzou a distância da pílula, a conversa redesenha para mostrá-la.
        if self.follow.wheel != 0. || self.follow.pinned && self.distance_from_bottom() > 0.5 || detached != self.follow_detached() {
            self.redraw(Area::Conversation, cx);
        }
    }

    fn wheel_frame(&mut self, now: Instant) {
        let ms = self.follow.wheel_tick.map_or(FRAME_MS, |t| now.duration_since(t).as_secs_f32() * 1000.).min(4. * FRAME_MS);
        self.follow.wheel_tick = Some(now);
        let down = self.follow.wheel > 0.;
        let step;
        (step, self.follow.wheel) = wheel_step(self.follow.wheel, ms, self.distance_from_bottom());
        if step < 0. { self.unglue(0.); }
        if step != 0. { self.list_state.scroll_by(px(step)); }
        let distance = self.distance_from_bottom();
        self.follow.last_top = self.visible_top();
        if down && !self.follow.pinned && distance <= STICK_BAND {
            self.follow.pinned = true;
            self.follow.wheel = 0.;
        }
        if self.follow.wheel == 0. { self.follow.wheel_tick = None; }
    }

    fn spring_frame(&mut self, now: Instant) {
        if self.follow.settled.is_some_and(|t| now.duration_since(t) >= SETTLE_GRACE) {
            self.follow.spring = StickSpring::default();
            self.follow.tick = None;
            self.follow.settled = None;
        }
        let frames = self.follow.tick.map_or(1., |t| now.duration_since(t).as_secs_f32() * 1000. / FRAME_MS).min(MAX_CATCHUP_FRAMES);
        self.follow.tick = Some(now);
        let target = f32::from(self.list_state.max_offset_for_scrollbar().y);
        let mut distance = self.distance_from_bottom();
        let glide_max = GLIDE_MAX_VIEWPORTS * f32::from(self.list_state.viewport_bounds().size.height);
        if glide_max > 0. && distance > glide_max {
            self.list_state.scroll_by(px(distance - glide_max));
            distance = glide_max;
        }
        let pos = target - distance;
        let next = self.follow.spring.step(pos, target, frames);
        if next > pos { self.list_state.scroll_by(px(next - pos)); }
        self.follow.last_top = next;
        if target - next <= 0.5 {
            // Pousa num pixel, sem colar no fim: linha nova que entrou antes deste layout saltaria colada.
            self.follow.settled.get_or_insert(now);
            // Parada, o próximo crescimento parte de um quadro, não do tempo inteiro em que ficou parada.
            self.follow.tick = None;
        } else {
            self.follow.settled = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StickSpring;

    #[test]
    fn spring_glides_monotonically_and_lands_on_the_target() {
        let mut spring = StickSpring::default();
        let (mut pos, target) = (0., 300.);
        let mut frames = 0;
        while pos < target {
            let next = spring.step(pos, target, 1.);
            assert!(next > pos && next <= target, "{pos} -> {next}");
            assert!(next - pos <= super::MAX_STEP, "salto de {} px num quadro", next - pos);
            pos = next;
            frames += 1;
            assert!(frames < 120, "não pousou em 2 s");
        }
        assert!(frames > 6, "pousou em {frames} quadros: salto, não deslize");
    }

    #[test]
    fn spring_follows_steady_growth_without_falling_behind() {
        let mut spring = StickSpring::default();
        let (mut pos, mut target) = (0., 0.);
        for _ in 0..240 {
            target += 4.;
            pos = spring.step(pos, target, 1.);
            assert!(pos <= target);
        }
        assert!(target - pos < 40., "ficou {} px atrás do texto", target - pos);
    }

    #[test]
    fn late_frames_move_proportionally_but_never_past_the_target() {
        let mut spring = StickSpring::default();
        let next = spring.step(0., 2000., 4.);
        assert!(next <= 4. * super::MAX_STEP + 0.01, "{next} px em 4 quadros");
        let mut spring = StickSpring::default();
        assert_eq!(spring.step(29.8, 30., 8.), 30.);
    }

    /// Agente de fundo que termina volta do fim para o meio: o trecho trocado vai dele até o fim e engole a linha do
    /// topo da tela, que só mudou de índice.
    #[test]
    fn splice_keeps_the_anchor_row_that_moved_inside_the_range() {
        use gpui_kit::{ListAlignment, ListOffset, ListState, px};
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let old = ids(&["a", "b", "c", "d", "e", "pin"]);
        let list = ListState::new(old.len(), ListAlignment::Bottom, px(300.));
        list.scroll_to(ListOffset { item_ix: 3, offset_in_item: px(12.) });
        super::splice_keeping_anchor(&list, &old, 1..6, &ids(&["agent", "b", "c", "d", "e"]));
        let top = list.logical_scroll_top();
        assert_eq!((top.item_ix, top.offset_in_item), (4, px(12.)));
    }

    #[test]
    fn wheel_down_at_the_bottom_leaves_nothing_pending() {
        assert_eq!(super::wheel_step(780., super::FRAME_MS, 0.), (0., 0.));
        let (step, left) = super::wheel_step(100., super::FRAME_MS, 10.);
        assert_eq!((step, left), (10., 0.));
        let (step, left) = super::wheel_step(-100., super::FRAME_MS, 0.);
        assert!(step < 0. && left < 0. && left > -100.);
    }
}
