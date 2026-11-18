//! Revisão do trabalho da sessão ligada a um ticket (`work_review/v1`): aba Revisão do painel e chip nas linhas. Tudo vem
//! das rotas da sessão no servidor dela; o texto da revisão é do conector, e o genérico, das chaves `work_review_*`.
use super::*;
use super::device::Remote;
use crate::api::dto::{WorkReview, WorkReviewChoice, WorkReviewDecision, WorkReviewResponse, WorkReviewSide, WorkReviewState, WorkReviewSummary, WorkReviewTone};
use gpui_kit::component::{link::Link, progress::Progress};
use model::{Choices, Picked, StepStatus, Tone};

mod compare;
mod model;

pub(super) use model::store;

/// Altura dos quadros com zoom, como no desenho.
const FRAME_H: f32 = 200.;
/// Miniatura da tira das corrigidas.
const PAIR_W: f32 = 150.;
const PAIR_H: f32 = 84.;
const PAIR_GAP: f32 = 8.;
/// Escolher sem motivo leva à próxima decisão depois deste respiro.
const ADVANCE: Duration = Duration::from_millis(250);
/// Teto da imagem decodificada dos quadros e da cortina; a tira usa a miniatura.
const FULL_SIDE: u32 = 4096;
/// ponytail: teto das imagens decodificadas da revisão (~8 capturas de 1920×1080); a desenhada no quadro não sai.
const IMAGE_BUDGET: usize = 64 * 1024 * 1024;

/// Imagem guardada por (asset, inteira): o asset é imutável dentro da rodada, e trocar de rodada ou sessão esvazia tudo.
type AssetKey = (String, bool);

pub(super) struct Loaded { pub(super) via_branch: bool, pub(super) review: Arc<WorkReview> }

pub(super) struct Reason { input: Entity<TextareaState>, _changed: Subscription }

pub(super) struct Panel {
    owner: Option<SessionKey>,
    /// Resumo do evento `work_reviews` na hora da última leitura: outro estado, rodada ou hora pede releitura.
    seen: Option<WorkReviewSummary>,
    /// `Ok(None)` = a seção não aparece (sem vínculo, conector sem a capacidade, sem revisão ou servidor sem a rota).
    read: Remote<Option<Loaded>>,
    /// Quem pediu a leitura em voo: a falha que `model::shows_failure` não mostra vai ao log, e a tela fica com o que tinha.
    reading: model::Read,
    choices: Choices,
    reasons: HashMap<String, Reason>,
    index: usize,
    sending: bool,
    send_error: Option<String>,
    resolved_closed: bool,
    kept_open: bool,
    /// Buscando ou ilegível, o quadro mostra o marcador no lugar.
    images: media::MediaCache<AssetKey>,
    /// Sobe a cada `clear_round`: a imagem que chega de uma rodada ou sessão anterior não entra.
    round: u64,
    advance_seq: u64,
    strip: ScrollHandle,
    /// Rolagem do corpo da aba: sem ela o GPUI guardaria pelo id a posição da revisão ou sessão anterior.
    pub(super) scroll: ScrollHandle,
}

impl Default for Panel {
    fn default() -> Self {
        Self { owner: None, seen: None, read: Remote::default(), reading: model::Read::Open, choices: Choices::new(), reasons: HashMap::new(), index: 0,
            sending: false, send_error: None, resolved_closed: false, kept_open: false, images: media::MediaCache::with_budget(IMAGE_BUDGET), round: 0,
            advance_seq: 0, strip: ScrollHandle::new(), scroll: ScrollHandle::new() }
    }
}

impl Panel {
    fn review(&self) -> Option<&WorkReview> { self.read.ok()?.as_ref().map(|l| &*l.review) }

    /// Ler marca a imagem como desenhada neste quadro: o teto não a expulsa.
    pub(super) fn image(&mut self, asset: &str, full: bool) -> Option<Arc<RenderImage>> {
        match self.images.get(&(asset.to_owned(), full)) { Some(media::MediaState::Image(image)) => Some(image.clone()), _ => None }
    }

    /// Rodada ou sessão nova: escolhas, caixas e imagens eram da anterior.
    fn clear_round(&mut self, window: &mut Window, cx: &mut App) {
        for image in self.images.clear() { cx.drop_image(image, Some(window)); }
        self.round += 1;
        self.choices.clear();
        self.reasons.clear();
        self.index = 0;
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    fn outcome(&self) -> model::Outcome {
        match &self.read.value {
            None => model::Outcome::Pending,
            Some(Ok(None)) => model::Outcome::Absent,
            Some(Ok(Some(_))) => model::Outcome::Present,
            Some(Err(_)) => model::Outcome::Failed,
        }
    }
}

fn tone_color(tone: Tone) -> Hsla {
    match tone { Tone::Accent => theme::accent(), Tone::Warning => theme::warning(), Tone::Success => theme::success() }
}

fn badge_color(tone: WorkReviewTone) -> Hsla {
    match tone { WorkReviewTone::Warning => theme::warning_text(), WorkReviewTone::Success => theme::success_text(), WorkReviewTone::Info => theme::accent_text() }
}

fn badge_tone(tone: WorkReviewTone) -> Hsla {
    match tone { WorkReviewTone::Warning => theme::warning(), WorkReviewTone::Success => theme::success(), WorkReviewTone::Info => theme::accent() }
}

fn badge_fill(tone: WorkReviewTone) -> Hsla { badge_tone(tone).opacity(0.16) }

fn tab_dot(color: Hsla) -> Div { div().flex_shrink_0().size(px(6.)).rounded_full().bg(color) }

pub(crate) fn tab_pill(text: String, tone: WorkReviewTone) -> AnyElement {
    div().flex_shrink_0().px(px(5.)).rounded_full().bg(badge_fill(tone)).font_family(theme::MONO).text_size(px(10.5)).line_height(px(15.))
        .text_color(badge_color(tone)).child(text).into_any_element()
}

/// Chip do card da sessão: o texto do selo na cor do tom.
pub(super) fn chip(text: String, tone: WorkReviewTone) -> Div {
    div().flex_shrink_0().max_w(px(160.)).truncate().px(px(7.)).py(px(1.)).rounded_full().bg(badge_fill(tone))
        .text_size(px(10.5)).font_weight(FontWeight::MEDIUM).text_color(badge_color(tone)).child(text)
}

fn present(response: WorkReviewResponse) -> Option<Loaded> {
    if !response.linked || !response.supported { return None; }
    Some(Loaded { via_branch: response.via.as_deref() == Some("branch"), review: Arc::new(response.review?) })
}

fn pending_text(pending: usize) -> String {
    if pending == 1 { tr_shared("work_review_pending_one", &[]) } else { tr_shared("work_review_pending", &[("count", &pending.to_string())]) }
}

/// "há N min" pela hora da revisão; hora ilegível não mostra nada.
fn updated_ago(at: &str) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    Some(super::side::ago(super::side::now_seconds() - at.timestamp() as f64))
}

impl Hangar {
    /// Resumo do evento para a linha `server`/`name` (chave `servers::norm`).
    pub(super) fn work_review_badge(&self, server: &str, name: &str) -> Option<(String, WorkReviewTone)> {
        let badge = self.work_reviews.get(server)?.get(name)?.badge.as_ref()?;
        Some((badge.text.clone(), badge.tone))
    }

    pub(super) fn work_review_chip(&self, server: &str, name: &str) -> Option<Div> {
        self.work_review_badge(server, name).map(|(text, tone)| chip(text, tone))
    }

    /// Chip que abre a sessão da linha já na aba Revisão. Ele para o pressionar e o clique: a linha não abre de novo nem
    /// começa a renomear. `popups`: a linha mora num popup que se fecha ao abrir a sessão.
    pub(super) fn work_review_chip_button(&self, target: &super::sidebar::Target, popups: bool, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let (text, tone) = self.work_review_badge(&target.server, &target.name)?;
        let label = format!("{} · {text}", tr_shared("work_review_tab", &[]));
        let target = target.clone();
        Some(chip(text, tone).id(SharedString::from(format!("work-review-chip-{}", target.id()))).cursor_pointer()
            .role(Role::Button).aria_label(label)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if popups { this.close_popups(); }
                this.open_work_review(&target, window, cx);
            })))
    }

    fn open_work_review(&mut self, target: &super::sidebar::Target, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_hover();
        if self.select_target(target, window, cx).is_none() { return; }
        if !self.side.open { self.toggle_side(cx); }
        self.choose_side_tab(crate::appearance::SideTab::Review, window, cx);
    }

    fn loaded_review(&self, key: &SessionKey) -> Option<&WorkReview> {
        (self.work_review.owner.as_ref() == Some(key)).then(|| self.work_review.review()).flatten()
    }

    /// A aba Revisão: só o dono, fora do orquestrador, e com revisão dita pela lista ou já lida para esta sessão.
    pub(super) fn work_review_tab_shown(&self) -> bool {
        let Some(session) = self.selected.as_ref() else { return false };
        if !session.readable() || session.read_only() || session.orq() { return false; }
        let Some(key) = self.selected_key() else { return false };
        let read = if self.work_review.owner.as_ref() == Some(&key) { self.work_review.outcome() } else { model::Outcome::Pending };
        model::tab_shown(self.work_reviews.get(&self.open_server()).is_some_and(|m| m.contains_key(&key.name)), read)
    }

    /// Depois de guardar o evento `work_reviews` da máquina `server`: aba, selo e corpo moram no painel lateral, que o
    /// redesenho da lista não alcança.
    pub(super) fn work_reviews_changed(&self, server: &str, cx: &mut Context<Self>) {
        if self.side.open && server == self.open_server() { self.redraw(super::panes::Area::Side, cx); }
    }

    /// Selo da aba: esperando, quantas decisões faltam aqui (um ponto até a leitura chegar); rodando, ponto pulsando;
    /// concluída, ✓; aplicando, ponto no tom do selo da lista.
    pub(super) fn work_review_tab_badge(&self, cx: &App) -> Option<AnyElement> {
        let key = self.selected_key()?;
        let summary = self.work_reviews.get(&self.open_server()).and_then(|m| m.get(&key.name));
        let loaded = self.loaded_review(&key);
        let state = loaded.map(|r| r.state).or(summary.map(|s| s.state))?;
        Some(match state {
            WorkReviewState::Waiting => match loaded.map(|r| model::pending_count(r, &self.work_review.choices)) {
                Some(pending) if pending > 0 => tab_pill(pending.to_string(), WorkReviewTone::Warning),
                _ => tab_dot(theme::warning()).into_any_element(),
            },
            WorkReviewState::Running if !cx.reduce_motion() => tab_dot(theme::accent()).with_animation("work-review-tab-pulse",
                Animation::new(Duration::from_millis(1200)).repeat(), |el, t| el.opacity(0.4 + 0.6 * (0.5 + 0.5 * (t * std::f32::consts::TAU).cos()))).into_any_element(),
            WorkReviewState::Running => tab_dot(theme::accent()).into_any_element(),
            WorkReviewState::Done => tab_pill("✓".into(), WorkReviewTone::Success),
            WorkReviewState::Applying => tab_dot(summary.and_then(|s| s.badge.as_ref()).map_or_else(theme::success, |b| badge_tone(b.tone))).into_any_element(),
        })
    }

    /// Lê de novo ao abrir outra sessão e quando o evento da lista diz que a revisão andou. Roda no desenho do painel da
    /// sessão do dono, em qualquer aba.
    pub(super) fn sync_work_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return };
        let summary = self.work_reviews.get(&self.open_server()).and_then(|m| m.get(&key.name)).cloned();
        if self.work_review.owner.as_ref() != Some(&key) {
            self.reset_work_review(Some(key), window, cx);
            self.work_review.seen = summary;
            self.load_work_review(model::Read::Open, window, cx);
        } else if model::moved(self.work_review.seen.as_ref(), summary.as_ref()) {
            self.work_review.seen = summary;
            self.load_work_review(model::Read::Background, window, cx);
        }
    }

    fn reset_work_review(&mut self, owner: Option<SessionKey>, window: &mut Window, cx: &mut Context<Self>) {
        let panel = &mut self.work_review;
        panel.clear_round(window, cx);
        panel.read.reset();
        (panel.owner, panel.seen, panel.reading, panel.sending, panel.send_error) = (owner, None, model::Read::Open, false, None);
        panel.advance_seq += 1;
    }

    fn load_work_review(&mut self, reading: model::Read, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.work_review.owner.clone() else { return };
        let Some(api) = self.api_for(&key.server) else { return };
        let seq = self.work_review.read.start();
        self.work_review.reading = reading;
        let name = key.name.clone();
        let request = async move {
            let value = api.read(&name, &["work-review"], &[], 30).await?;
            serde_json::from_value::<WorkReviewResponse>(value).map_err(|_| Failure::local("invalid_response"))
        };
        run(&self.runtime, request, window, cx, move |this: &mut Hangar, result, window, cx| this.receive_work_review(key, seq, result, window, cx));
    }

    fn receive_work_review(&mut self, key: SessionKey, seq: u64, result: Result<WorkReviewResponse, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        if self.work_review.owner.as_ref() != Some(&key) || self.work_review.read.seq != seq { return; }
        let loaded = match result {
            Ok(response) => present(response),
            // Servidor sem a rota (anterior a ela) ou sem acesso a ela: a seção não aparece.
            Err(error) if matches!(error.status, Some(403 | 404 | 405)) => None,
            // Sessão sem revisão conhecida (busca pela branch ou conector fora) e releitura de fundo: nada na tela.
            Err(error) if !model::shows_failure(self.work_review.reading, self.work_review.seen.is_some()) => {
                crate::log_line(&format!("work_review.read_failed read={:?} status={:?} code={}", self.work_review.reading, error.status, error.code.as_deref().unwrap_or("-")));
                self.work_review.read.loading = false;
                return;
            }
            Err(error) => {
                let msg = Self::read_failure(&error);
                self.work_review.read.finish(seq, Err(tr_shared("work_review_load_error", &[("msg", &msg)])));
                return;
            }
        };
        let panel = &mut self.work_review;
        let before = panel.review().map(|r| r.id.clone());
        let now = loaded.as_ref().map(|l| l.review.id.clone());
        if before != now { panel.clear_round(window, cx); }
        if let Some(loaded) = &loaded {
            panel.choices = model::carry(&panel.choices, &loaded.review);
            panel.reasons.retain(|id, _| panel.choices.contains_key(id));
            panel.index = panel.index.min(loaded.review.decisions.len().saturating_sub(1));
        }
        panel.read.finish(seq, Ok(loaded));
    }

    /// Busca o asset uma vez por rodada; `full` decodifica no tamanho dos quadros e da cortina, senão a miniatura.
    pub(super) fn want_review_image(&mut self, asset: &str, full: bool, window: &mut Window, cx: &mut Context<Self>) {
        let panel = &self.work_review;
        let (Some(owner), Some(review)) = (panel.owner.as_ref(), panel.review()) else { return };
        let slot = (asset.to_owned(), full);
        if panel.images.contains(&slot) { return; }
        let (name, review_id, round) = (owner.name.clone(), review.id.clone(), panel.round);
        let Some(api) = self.api_for(&owner.server) else { return };
        self.work_review.images.start(slot.clone());
        let asset = slot.0.clone();
        let request = async move {
            let bytes = api.session_bytes(&name, &["work-review", "assets", &asset], &[("review_id", &review_id)], 60).await?;
            Ok(tokio::task::spawn_blocking(move || if full { media::decode(&bytes, FULL_SIDE, FULL_SIDE, None) } else { media::thumbnail(&bytes) })
                .await.ok().flatten())
        };
        run(&self.runtime, request, window, cx, move |this: &mut Hangar, result: Result<Option<Arc<RenderImage>>, Failure>, window, cx| {
            // A rodada ou a sessão mudou no meio: a imagem não tem mais onde ficar.
            if this.work_review.round != round {
                if let Ok(Some(image)) = result { cx.drop_image(image, Some(window)); }
                return;
            }
            match result {
                Ok(image) => {
                    let state = image.map_or_else(|| media::MediaState::Failed(String::new()), media::MediaState::Image);
                    for old in this.work_review.images.insert(slot, state) { cx.drop_image(old, Some(window)); }
                }
                Err(error) if error.code.as_deref() == Some("stale_review") => this.load_work_review(model::Read::Background, window, cx),
                Err(error) => crate::log_line(&format!("work_review.asset_failed status={:?} code={}", error.status, error.code.as_deref().unwrap_or("-"))),
            }
        });
    }

    fn pick_review_choice(&mut self, decision: String, choice: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = self.work_review.review() else { return };
        let Some(option) = review.decisions.iter().find(|d| d.id == decision).and_then(|d| d.choices.iter().find(|c| c.id == choice)).cloned() else { return };
        let total = review.decisions.len();
        let panel = &mut self.work_review;
        panel.send_error = None;
        panel.advance_seq += 1;
        if !option.requires_reason {
            panel.choices.insert(decision.clone(), Picked { choice, reason: String::new() });
            panel.reasons.remove(&decision);
            let (seq, from) = (panel.advance_seq, panel.index);
            if from + 1 < total {
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(ADVANCE).await;
                    let _ = this.update(cx, |this, cx| {
                        let panel = &mut this.work_review;
                        if panel.advance_seq == seq && panel.index == from { panel.index += 1; cx.notify(); }
                    });
                }).detach();
            }
            cx.notify();
            return;
        }
        // Voltar à mesma escolha mantém o que já foi escrito.
        let kept = panel.choices.get(&decision).filter(|p| p.choice == choice).map(|p| p.reason.clone());
        let reason = kept.unwrap_or_else(|| model::initial_reason(&option));
        panel.choices.insert(decision.clone(), Picked { choice, reason: reason.clone() });
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 6));
        input.update(cx, |input, cx| { input.set_value(reason, window, cx); input.focus(window, cx); });
        let id = decision.clone();
        let changed = cx.subscribe(&input, move |this: &mut Hangar, input, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) { return; }
            let value = input.read(cx).value().to_string();
            if let Some(picked) = this.work_review.choices.get_mut(&id) { picked.reason = value; }
            cx.notify();
        });
        self.work_review.reasons.insert(decision, Reason { input, _changed: changed });
        cx.notify();
    }

    fn step_review(&mut self, forward: bool, cx: &mut Context<Self>) {
        let total = self.work_review.review().map_or(0, |r| r.decisions.len());
        let panel = &mut self.work_review;
        panel.advance_seq += 1;
        panel.index = if forward { (panel.index + 1).min(total.saturating_sub(1)) } else { panel.index.saturating_sub(1) };
        cx.notify();
    }

    fn send_work_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = &self.work_review;
        if panel.sending { return; }
        let (Some(key), Some(review)) = (panel.owner.clone(), panel.review()) else { return };
        let Some(body) = model::decisions_payload(review, &panel.choices) else { return };
        let Some(api) = self.api_for(&key.server) else { return };
        (self.work_review.sending, self.work_review.send_error) = (true, None);
        let name = key.name.clone();
        let request = async move { api.act(&name, &["work-review", "decisions"], Some(body), false, 60).await };
        run(&self.runtime, request, window, cx, move |this: &mut Hangar, result, window, cx| {
            if this.work_review.owner.as_ref() != Some(&key) { return; }
            this.work_review.sending = false;
            match result {
                Ok(_) => {}
                Err(error) if error.code.as_deref() == Some("stale_review") => this.work_review.send_error = Some(tr_shared("work_review_stale", &[])),
                Err(error) => {
                    let msg = Self::read_failure(&error);
                    this.work_review.send_error = Some(tr_shared("work_review_send_error", &[("msg", &msg)]));
                    return;
                }
            }
            this.load_work_review(model::Read::Requested, window, cx);
        });
        cx.notify();
    }

    /// O corpo da aba, ou `None` quando a sessão não tem revisão para mostrar. `width` é a do painel.
    pub(super) fn render_work_review(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = &self.work_review;
        let Some(value) = panel.read.value.as_ref() else {
            // Primeira leitura: o marcador só aparece quando a lista já disse que há revisão; sem isso a seção some calada.
            return panel.seen.is_some().then(|| div().flex().items_center().gap_2().text_xs().text_color(theme::muted())
                .child(chrome::Spinner::new("work-review-loading", IconName::LoaderCircle, px(13.), theme::accent())).child(tr("loading")).into_any_element());
        };
        let loaded = match value {
            Err(error) => return Some(div().text_xs().text_color(theme::warning()).whitespace_normal().child(error.clone()).into_any_element()),
            Ok(None) => return None,
            Ok(Some(loaded)) => loaded,
        };
        let review = loaded.review.clone();
        let via_branch = loaded.via_branch;
        // O que for lido daqui em diante conta como desenhado neste quadro.
        self.work_review.images.next_frame();
        // Largura útil: o painel menos o recuo das seções.
        let inner = (width - 32.).max(120.);
        let mut body = div().flex().flex_col().child(self.render_review_header(&review, via_branch));
        if let Some(steps) = &review.steps { body = body.child(render_steps(steps, review.state, inner, cx)); }
        let pending = model::pending_count(&review, &self.work_review.choices);
        if let Some(band) = render_band(&review, pending) { body = body.child(band); }
        // O host só aceita `progress` rodando e com total acima de zero.
        if let Some(progress) = &review.progress {
            let percent = progress.done as f32 * 100. / progress.total as f32;
            body = body.child(div().mt(px(12.)).flex().flex_col().gap(px(6.))
                .child(div().flex().text_xs().text_color(theme::muted()).child(div().flex_1().min_w_0().truncate().child(progress.label.clone()))
                    .child(div().flex_shrink_0().font_family(theme::MONO).child(tr_shared("work_review_position",
                        &[("index", &progress.done.to_string()), ("total", &progress.total.to_string())]))))
                .child(Progress::new("work-review-progress").accessibility_label(progress.label.clone()).value(percent)));
        }
        if review.state == WorkReviewState::Waiting && !review.decisions.is_empty() {
            body = body.child(self.render_decision(&review, pending, inner, window, cx));
        }
        // Rodando, as corrigidas ficam para depois, como no desenho: a faixa já diz quantas vão.
        if review.state != WorkReviewState::Running
            && let Some(resolved) = review.resolved.as_ref().filter(|r| !r.items.is_empty() || r.count.is_some_and(|n| n > 0)) {
            body = body.child(self.render_resolved(resolved, window, cx));
        }
        if let Some(kept) = review.kept.as_ref().filter(|k| !k.items.is_empty() || k.count.is_some_and(|n| n > 0)) {
            body = body.child(self.render_kept(kept, cx));
        }
        if !review.links.is_empty() { body = body.child(self.render_review_links(&review)); }
        Some(body.into_any_element())
    }

    fn render_review_header(&self, review: &WorkReview, via_branch: bool) -> Div {
        div().flex().flex_col().gap(px(2.))
            .child(div().flex().items_baseline().gap_2().min_w_0()
                .child(div().min_w_0().truncate().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(review.title.clone()))
                .children(review.subject.clone().map(|s| div().min_w_0().truncate().text_xs().text_color(theme::muted()).child(s)))
                .child(div().flex_1())
                .children(updated_ago(&review.updated_at).map(|ago| div().flex_shrink_0().font_family(theme::MONO).text_size(px(11.))
                    .text_color(theme::muted()).child(ago))))
            .when(via_branch, |el| el.child(div().text_size(px(11.)).text_color(theme::faint()).child(tr_shared("work_review_found_by_branch", &[]))))
    }

    fn render_decision(&mut self, review: &WorkReview, pending: usize, inner: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let total = review.decisions.len();
        let index = self.work_review.index.min(total - 1);
        let decision = &review.decisions[index];
        for side in decision.left.iter().chain(decision.right.iter()) { self.want_review_image(&side.asset, true, window, cx); }
        let frames = decision.left.as_ref().map(|left| {
            let w = decision_frame_width(inner, decision.right.is_some());
            let left = self.render_side_frame("left", decision, left, w, cx);
            frames_row(left, decision.right.as_ref().map(|right| self.render_side_frame("right", decision, right, w, cx)))
        });
        let panel = &self.work_review;
        let picked = panel.choices.get(&decision.id).cloned();
        let done = review.decisions.iter().map(|d| model::decided(d, panel.choices.get(&d.id))).collect::<Vec<_>>();
        let choices = decision.choices.iter().map(|choice| {
            let selected = picked.as_ref().is_some_and(|p| p.choice == choice.id);
            let (d, c) = (decision.id.clone(), choice.id.clone());
            choice_button(&decision.id, choice, selected, cx)
                .on_click(cx.listener(move |this, _, window, cx| this.pick_review_choice(d.clone(), c.clone(), window, cx)))
        }).collect::<Vec<_>>();
        let required =picked.as_ref().and_then(|p| decision.choices.iter().find(|c| c.id == p.choice).filter(|c| c.requires_reason).map(|c| (c.clone(), p.reason.clone())));
        let reason_box = required.and_then(|(choice, text)| {
            let input = panel.reasons.get(&decision.id)?.input.clone();
            let label = choice.reason_prompt.clone().unwrap_or_else(|| choice.label.clone());
            Some(div().mt(px(8.)).flex().flex_col().gap(px(4.))
                .child(div().text_xs().text_color(theme::muted()).child(label.clone()))
                .child(Textarea::new(&input).aria_label(label))
                .when(text.trim().is_empty(), |el| el.child(div().id("work-review-reason-error").role(Role::Alert).text_xs().text_color(theme::danger()).child(tr_shared("work_review_reason_required", &[])))))
        });
        let ready = model::ready_to_send(review, &panel.choices);
        let (previous, next, send) = decision_nav(index, total, pending, ready, panel.sending);
        let nav = decision_nav_row(previous.on_click(cx.listener(|this, _, _, cx| this.step_review(false, cx))),
            next.on_click(cx.listener(|this, _, _, cx| this.step_review(true, cx))),
            send.on_click(cx.listener(|this, _, window, cx| this.send_work_review(window, cx))));
        decision_card(decision, &done, index, frames, choices)
            .children(reason_box)
            .child(nav)
            .children(panel.send_error.clone().map(|e| div().id("work-review-send-error").mt(px(6.)).role(Role::Alert).text_xs().text_color(theme::danger()).whitespace_normal().child(e)))
            .into_any_element()
    }

    /// Um lado da decisão com a captura da rodada; o clique abre a cortina.
    fn render_side_frame(&mut self, slot: &'static str, decision: &WorkReviewDecision, side: &WorkReviewSide, w: f32, cx: &mut Context<Self>) -> Div {
        let image = self.work_review.image(&side.asset, true);
        let decision_id = decision.id.clone();
        let quadro = div().id(SharedString::from(format!("work-review-frame-{slot}-{}", decision.id)))
            .cursor(CursorStyle::PointingHand)
            .role(Role::Button).aria_label(format!("{} · {}", decision.title, side.label))
            .on_click(cx.listener(move |this, _, window, cx| this.open_decision_compare(decision_id.clone(), window, cx)));
        side_frame(quadro, side, w, FRAME_H, image.map(|image| img(image).size_full().into_any_element()))
    }

    fn open_decision_compare(&mut self, decision: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.work_review.review().and_then(|r| r.decisions.iter().find(|x| x.id == decision)).cloned() else { return };
        let Some(left) = d.left.clone() else { return };
        // Sem o lado direito, a cortina mostra só o esquerdo.
        let right = d.right.clone().unwrap_or_else(|| left.clone());
        let title = tr_shared("work_review_compare_title", &[("title", &d.title), ("left", &left.label), ("right", &right.label)]);
        let pair = compare::Pair { title, before: left.asset.clone(), after: right.asset.clone(), size: Some((left.width, left.height)) };
        self.open_compare(vec![pair], 0, (left.label, right.label), true, window, cx);
    }

    fn open_resolved_compare(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(resolved) = self.work_review.review().and_then(|r| r.resolved.clone()) else { return };
        let pairs: Vec<compare::Pair> = resolved.items.iter().filter_map(|item| Some(compare::Pair {
            title: item.title.clone(), before: item.before.clone()?, after: item.after.clone()?, size: None,
        })).collect();
        let Some(at) = resolved.items.iter().take(index + 1).filter(|i| i.before.is_some() && i.after.is_some()).count().checked_sub(1) else { return };
        self.open_compare(pairs, at, (tr_shared("work_review_before", &[]), tr_shared("work_review_after", &[])), false, window, cx);
    }

    fn open_compare(&mut self, pairs: Vec<compare::Pair>, index: usize, labels: (String, String), single: bool, window: &mut Window, cx: &mut Context<Self>) {
        let pair = &pairs[index];
        self.want_review_image(&pair.before, true, window, cx);
        self.want_review_image(&pair.after, true, window, cx);
        compare::open(pairs, index, labels, single, window, cx);
    }

    fn render_resolved(&mut self, resolved: &crate::api::dto::WorkReviewResolved, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let open = !self.work_review.resolved_closed;
        let count = resolved.count.unwrap_or(resolved.items.len() as u64);
        let head = collapsible_head("work-review-resolved", open, resolved.label.clone(), count, Some(tr_shared("work_review_drag_hint", &[])))
            .on_click(cx.listener(|this, _, _, cx| { this.work_review.resolved_closed = !this.work_review.resolved_closed; cx.notify(); }));
        let mut section = div().mt(px(14.)).pt(px(10.)).border_t_1().border_color(theme::border()).flex().flex_col().child(head);
        if open {
            // Só as miniaturas que a rolagem mostra são pedidas e montadas.
            let handle = self.work_review.strip.clone();
            let (offset, view) = (-f32::from(handle.offset().x), f32::from(handle.bounds().size.width));
            let step = PAIR_W + PAIR_GAP;
            let len = resolved.items.len();
            let first = ((offset / step).floor().max(0.) as usize).min(len);
            let last = (((offset + view.max(step * 3.)) / step).ceil() as usize + 1).clamp(first, len);
            let visible = &resolved.items[first..last];
            for item in visible {
                for asset in item.before.iter().chain(item.after.iter()) { self.want_review_image(asset, false, window, cx); }
            }
            // Fora da janela, um vão da mesma largura mantém a rolagem do tamanho da tira inteira.
            let spacer = |n: usize| (n > 0).then(|| div().flex_shrink_0().w(px(n as f32 * step - PAIR_GAP)));
            let tiles = visible.iter().enumerate().map(|(k, item)| {
                let n = first + k;
                let (before, after) = (item.before.as_deref().and_then(|a| self.work_review.image(a, false)), item.after.as_deref().and_then(|a| self.work_review.image(a, false)));
                let comparable = item.before.is_some() && item.after.is_some();
                div().id(SharedString::from(format!("work-review-pair-{n}"))).flex_shrink_0().w(px(PAIR_W)).flex().flex_col().gap(px(4.))
                    .when(comparable, |el| el.cursor_pointer().on_click(cx.listener(move |this, _, window, cx| this.open_resolved_compare(n, window, cx))))
                    .role(Role::Button).aria_label(item.title.clone())
                    .child(pair_preview(before, after))
                    .child(div().truncate().text_size(px(11.)).text_color(theme::muted()).child(item.title.clone()))
            }).collect::<Vec<_>>();
            let more = count.saturating_sub(resolved.items.len() as u64);
            section = section.child(div().id("work-review-strip").track_scroll(&handle).overflow_x_scroll().pt(px(10.)).pb(px(4.)).flex().gap(px(PAIR_GAP))
                .children(spacer(first))
                .children(tiles)
                .children(spacer(len - last))
                .when(more > 0, |el| el.child(div().flex_shrink_0().w(px(PAIR_W)).h(px(PAIR_H)).rounded(px(6.)).border_1().border_dashed().border_color(theme::border_strong())
                    .flex().items_center().justify_center().text_xs().text_color(theme::muted()).child(tr_shared("work_review_more", &[("count", &more.to_string())])))));
        }
        section.into_any_element()
    }

    fn render_kept(&self, kept: &crate::api::dto::WorkReviewKept, cx: &mut Context<Self>) -> AnyElement {
        let open = self.work_review.kept_open;
        let count = kept.count.unwrap_or(kept.items.len() as u64);
        let head = collapsible_head("work-review-kept", open, kept.label.clone(), count, None)
            .on_click(cx.listener(|this, _, _, cx| { this.work_review.kept_open = !this.work_review.kept_open; cx.notify(); }));
        div().mt(px(14.)).pt(px(10.)).border_t_1().border_color(theme::border()).flex().flex_col().child(head)
            .when(open, |el| el.child(div().pt(px(8.)).flex().flex_col().gap(px(4.)).children(kept.items.iter().map(|item| {
                div().flex().items_start().gap_2().py(px(5.)).border_b_1().border_dashed().border_color(theme::border()).text_xs()
                    .child(div().flex_1().min_w_0().whitespace_normal().child(item.title.clone()))
                    .child(div().max_w(relative(0.5)).text_color(theme::muted()).whitespace_normal().child(item.reason.clone()))
            }))))
            .into_any_element()
    }

    /// Links do rodapé; o `local` só abre quando o servidor da sessão é esta máquina.
    fn render_review_links(&self, review: &WorkReview) -> Div {
        let here = self.work_review.owner.as_ref().and_then(|k| self.api_for(&k.server)).is_some_and(|api| api.is_loopback());
        div().mt(px(14.)).flex().flex_wrap().gap_x(px(14.)).gap_y(px(4.)).text_xs()
            .children(review.links.iter().enumerate().map(|(n, link)| {
                if link.local && !here {
                    return div().flex().items_center().gap_1().child(div().text_color(theme::muted()).child(link.label.clone()))
                        .child(div().text_size(px(11.)).text_color(theme::faint()).child(format!("· {}", tr_shared("work_review_local_link", &[])))).into_any_element();
                }
                let url = link.url.clone();
                Link::new(SharedString::from(format!("work-review-link-{n}"))).child(link.label.clone())
                    .on_click(move |_, window, cx| super::links::open_link(&url, false, window, cx)).into_any_element()
            }))
    }
}

/// Largura de cada quadro: a útil do painel menos a borda do cartão (12 + 1 de cada lado) e, em par, o vão entre eles.
pub(crate) fn decision_frame_width(inner: f32, pair: bool) -> f32 {
    let card = inner - 26.;
    if pair { ((card - 8.) / 2.).max(60.) } else { card.max(60.) }
}

pub(crate) fn frames_row(left: Div, right: Option<Div>) -> Div { div().flex().gap(px(8.)).child(left).children(right) }

/// Um lado da decisão: rótulo e valor em cima e o quadro `w`×`h` com zoom no trecho e a moldura vermelha. `fill` ocupa o
/// lugar da imagem no quadro; sem ele, o marcador de imagem. `quadro` traz id, clique e acessibilidade de quem chama.
pub(crate) fn side_frame(quadro: Stateful<Div>, side: &WorkReviewSide, w: f32, h: f32, fill: Option<AnyElement>) -> Div {
    let framing = model::frame((w, h), (side.width, side.height), side.frame);
    let mut quadro = quadro.relative().w(px(w)).h(px(h))
        // Fundo branco como o do desenho: o zoom perto da borda da captura não mostra uma faixa escura.
        .overflow_hidden().rounded(px(6.)).border_1().border_color(theme::border()).bg(if fill.is_some() { gpui::white() } else { theme::inset() });
    quadro = match fill {
        Some(fill) => quadro.child(div().absolute().left(px(framing.image.x)).top(px(framing.image.y)).w(px(framing.image.w)).h(px(framing.image.h)).child(fill)),
        None => quadro.flex().items_center().justify_center().child(chrome::small_icon(IconName::Image, 20., theme::faint())),
    };
    if let Some(mark) = framing.mark {
        // O lado de fora do trecho escurece em quatro faixas: a sombra gigante do web não é recortada aqui.
        let dim = theme::scrim().opacity(0.32);
        let (right, bottom) = (mark.x + mark.w, mark.y + mark.h);
        quadro = quadro
            .child(div().absolute().left_0().top_0().w(px(w)).h(px(mark.y.max(0.))).bg(dim))
            .child(div().absolute().left_0().top(px(bottom)).w(px(w)).h(px((h - bottom).max(0.))).bg(dim))
            .child(div().absolute().left_0().top(px(mark.y)).w(px(mark.x.max(0.))).h(px(mark.h)).bg(dim))
            .child(div().absolute().left(px(right)).top(px(mark.y)).w(px((w - right).max(0.))).h(px(mark.h)).bg(dim))
            .child(div().absolute().left(px(mark.x)).top(px(mark.y)).w(px(mark.w)).h(px(mark.h)).border_2().border_color(theme::danger()).rounded(px(3.)));
    }
    div().w(px(w)).flex().flex_col().gap(px(4.))
        .child(div().flex().items_center().gap_2().text_size(px(11.)).text_color(theme::muted())
            .child(div().flex_1().min_w_0().truncate().child(side.label.clone()))
            .children(side.value.clone().map(|v| div().flex_shrink_0().font_family(theme::MONO).font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(v))))
        .child(quadro)
}

/// Uma escolha da decisão; quem chama põe o clique. Escolhida, ganha o tom (âmbar quando pede motivo).
pub(crate) fn choice_button(decision: &str, choice: &WorkReviewChoice, selected: bool, cx: &App) -> Button {
    let tone = if choice.requires_reason { theme::warning() } else { theme::accent() };
    let button = Button::new(SharedString::from(format!("work-review-choice-{decision}-{}", choice.id))).w_full().h_auto().py(px(8.)).px(px(12.))
        .child(div().w_full().flex().flex_col().items_start().gap(px(1.))
            .child(div().text_size(px(13.)).child(choice.label.clone()))
            .children(choice.hint.clone().map(|hint| div().text_size(px(11.)).text_color(theme::muted()).whitespace_normal().child(hint))));
    if selected {
        button.custom(ButtonCustomVariant::new(cx).color(tone.opacity(0.16)).foreground(theme::text()).hover(tone.opacity(0.22)).active(tone.opacity(0.28)))
            .border_1().border_color(tone).selected(true)
    } else { button.outline() }
}

/// O cartão da decisão até as escolhas: posição (um ponto por decisão, verde quando `done`), título, porquê, quadros e
/// escolhas. Quem chama acrescenta o motivo, a navegação e o erro.
pub(crate) fn decision_card(decision: &WorkReviewDecision, done: &[bool], index: usize, frames: Option<Div>, choices: Vec<Button>) -> Div {
    let dots = done.iter().enumerate().map(|(n, done)| {
        div().size(px(7.)).rounded_full().bg(if *done { theme::success() } else if n == index { theme::warning() } else { theme::border_strong() })
    });
    let position = tr_shared("work_review_position", &[("index", &(index + 1).to_string()), ("total", &done.len().to_string())]);
    let top = div().flex().items_center().gap_2().text_xs().text_color(theme::muted())
        .children(decision.tag.clone().map(|tag| div().min_w_0().truncate().child(tag)))
        .child(div().flex_1())
        .child(div().flex_shrink_0().flex().items_center().gap(px(4.)).children(dots).child(div().ml(px(4.)).child(position)));
    let reason = decision.reason.clone().map(|reason| {
        let line = div().text_xs().text_color(theme::muted()).whitespace_normal();
        match decision.reason_label.clone() {
            Some(label) => line.child(format!("{label}: ")).child(div().text_color(theme::text()).font_weight(FontWeight::MEDIUM).child(reason)).flex().flex_wrap(),
            None => line.child(div().text_color(theme::text()).child(reason)),
        }
    });
    let columns = choices.len().clamp(1, 2) as u16;
    div().mt(px(10.)).p(px(12.)).rounded(px(12.)).border_1().border_color(theme::warning().opacity(0.5)).flex().flex_col()
        .child(top)
        .child(div().mt(px(6.)).mb(px(2.)).text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(decision.title.clone()))
        .children(reason)
        .children(frames.map(|f| f.my(px(10.))))
        .child(div().when(decision.left.is_none(), |el| el.mt(px(10.))).grid().grid_cols(columns).gap(px(8.)).children(choices))
}

/// Anterior, Próxima e Enviar; quem chama põe os cliques. Enviar diz quantas faltam até não faltar nenhuma.
pub(crate) fn decision_nav(index: usize, total: usize, pending: usize, ready: bool, sending: bool) -> (Button, Button, Button) {
    let send_label = if pending > 0 { pending_text(pending) } else { tr_shared("work_review_send", &[]) };
    (Button::new("work-review-previous").outline().xsmall().label(tr_shared("work_review_previous", &[])).disabled(index == 0),
        Button::new("work-review-next").outline().xsmall().label(tr_shared("work_review_next", &[])).disabled(index + 1 >= total),
        Button::new("work-review-send").primary().small().label(send_label).disabled(!ready || sending).loading(sending))
}

pub(crate) fn decision_nav_row(previous: Button, next: Button, send: Button) -> Div {
    div().mt(px(10.)).flex().items_center().gap_2().child(previous).child(next).child(div().flex_1()).child(send)
}

fn collapsible_head(id: &'static str, open: bool, label: String, count: u64, hint: Option<String>) -> Stateful<Div> {
    div().id(id).flex().items_center().gap_2().cursor_pointer().text_size(px(13.)).role(Role::Button).aria_expanded(open)
        .child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., theme::muted()))
        .child(div().min_w_0().truncate().child(label))
        .child(div().flex_shrink_0().font_family(theme::MONO).text_xs().text_color(theme::muted()).child(count.to_string()))
        .child(div().flex_1())
        .children(hint.map(|h| div().min_w_0().truncate().text_size(px(11.)).text_color(theme::muted()).child(h)))
}

/// Miniatura da corrigida: o antes inteiro e o depois só na metade direita, com a divisória no meio.
fn pair_preview(before: Option<Arc<RenderImage>>, after: Option<Arc<RenderImage>>) -> Div {
    let mut frame = div().relative().w(px(PAIR_W)).h(px(PAIR_H)).overflow_hidden().rounded(px(6.)).border_1().border_color(theme::border()).bg(theme::inset());
    if before.is_none() && after.is_none() {
        return frame.flex().items_center().justify_center().child(chrome::small_icon(IconName::Image, 18., theme::faint()));
    }
    if let Some(before) = before { frame = frame.child(cover(before, PAIR_W, PAIR_H, 0.)); }
    if let Some(after) = after {
        frame = frame.child(div().absolute().top_0().left(px(PAIR_W / 2.)).w(px(PAIR_W / 2.)).h(px(PAIR_H)).overflow_hidden().child(cover(after, PAIR_W, PAIR_H, -PAIR_W / 2.)));
    }
    frame.child(div().absolute().top_0().bottom_0().left(px(PAIR_W / 2. - 1.)).w(px(2.)).bg(theme::accent()))
}

/// A imagem cobrindo `w`×`h` presa ao canto de cima à esquerda (`object-position: top left` do desenho), deslocada `dx`.
pub(super) fn cover(image: Arc<RenderImage>, w: f32, h: f32, dx: f32) -> AnyElement {
    let size = image.size(0);
    let (iw, ih) = (size.width.0.max(1) as f32, size.height.0.max(1) as f32);
    let scale = (w / iw).max(h / ih);
    img(image).absolute().top_0().left(px(dx)).w(px(iw * scale)).h(px(ih * scale)).into_any_element()
}

/// Trilha das etapas: feitas em verde, a atual no tom do estado e pulsando quando roda.
fn render_steps(steps: &crate::api::dto::WorkReviewSteps, state: WorkReviewState, inner: f32, cx: &App) -> Div {
    let n = steps.labels.len().max(1);
    let width = ((inner - (n as f32 - 1.) * 8.) / n as f32).clamp(36., 70.);
    let still = cx.reduce_motion();
    let mut trail = div().flex().items_start().mt(px(12.)).mb(px(14.));
    for (k, label) in steps.labels.iter().enumerate() {
        let step = model::step_status(steps, state, k);
        if k > 0 {
            let lit = step.status != StepStatus::Pending;
            trail = trail.child(div().flex_1().min_w(px(4.)).h(px(2.)).mt(px(9.)).bg(if lit { theme::success() } else { theme::border_strong() }));
        }
        let color = step.tone.map(tone_color);
        let dot = div().size(px(12.)).rounded_full().border_2()
            .border_color(color.unwrap_or_else(theme::border_strong)).bg(color.unwrap_or(transparent_black()));
        let current = step.status == StepStatus::Current;
        let ring = div().size(px(20.)).rounded_full().flex().items_center().justify_center()
            .when_some(color.filter(|_| current), |el, c| el.bg(c.opacity(0.22))).child(dot);
        let ring = if step.pulse && !still {
            ring.with_animation(SharedString::from(format!("work-review-step-{k}")), Animation::new(Duration::from_millis(1200)).repeat(),
                |el, t| el.opacity(0.55 + 0.45 * (0.5 + 0.5 * (t * std::f32::consts::TAU).cos()))).into_any_element()
        } else { ring.into_any_element() };
        let text = match (current, state) {
            (true, WorkReviewState::Waiting) => theme::warning(),
            (true, _) => theme::text(),
            _ => theme::muted(),
        };
        trail = trail.child(div().flex_shrink_0().w(px(width)).flex().flex_col().items_center().gap(px(3.))
            .child(ring)
            .child(div().w_full().text_center().text_size(px(10.5)).text_color(text).whitespace_normal()
                .when(current, |el| el.font_weight(FontWeight::SEMIBOLD)).child(label.clone())));
    }
    trail
}

/// Faixa de chamada no tom do estado; esperando, o número é o que ainda falta decidir aqui.
pub(crate) fn render_band(review: &WorkReview, pending: usize) -> Option<Div> {
    let model::Band { value, text, detail } = model::band(review, pending)?;
    let text = text.map(|t| match t {
        model::BandText::Ready => tr_shared("work_review_ready", &[]),
        model::BandText::Waiting(1) => tr_shared("work_review_waiting_one", &[]),
        model::BandText::Waiting(_) => tr_shared("work_review_waiting", &[]),
        model::BandText::Connector(t) => t,
    });
    let tone = model::state_tone(review.state);
    let (fill, number) = match tone {
        Tone::Accent => (theme::accent_dim(), theme::text()),
        Tone::Warning => (theme::warning().opacity(0.16), theme::warning()),
        Tone::Success => (theme::success().opacity(0.14), theme::success()),
    };
    Some(div().rounded(px(12.)).px(px(14.)).py(px(12.)).flex().items_center().gap(px(12.)).bg(fill)
        .child(div().flex_shrink_0().font_family(theme::MONO).text_size(px(26.)).font_weight(FontWeight::BOLD).line_height(px(26.)).text_color(number).child(value))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .children(text.map(|text| div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(text)))
            .children(detail.map(|d| div().text_xs().text_color(theme::muted()).whitespace_normal().child(d)))))
}
