// Kit de movimento: o catálogo do Zeron (tempos, curvas e o jeito de cada entrada) sobre a GPUI do gpui-kit.
//
// Adaptado de `crates/ui/src/motion.rs` do Zeron (github.com/zeronsh, revisão c338a6e), sob esta licença:
//
//   MIT License
//
//   Copyright (c) 2026 Wing
//
//   Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
//   documentation files (the "Software"), to deal in the Software without restriction, including without limitation
//   the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and
//   to permit persons to whom the Software is furnished to do so, subject to the following conditions:
//
//   The above copyright notice and this permission notice shall be included in all copies or substantial portions of
//   the Software.
//
//   THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO
//   THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//   AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
//   CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
//   IN THE SOFTWARE.
//
// O `cubic-bezier` exato é o do gpui-base, o mesmo algoritmo do Zeron. A GPUI não escala `div`: `menu-in` e `dialog-in`
// fazem a escala do Zeron com opacidade e deslocamento. O deslocamento é o `top` relativo, que não empurra os vizinhos.
//
// Quem pede o quadro seguinte importa. Uma animação na raiz que avisasse a view da raiz acordaria todas as áreas, que a
// observam (`panes.rs`); aqui a raiz pede o quadro pelo `ticker`, uma view vazia dentro dela, e só ela redesenha. Dentro
// de uma área, o quadro é só dessa área.
use gpui_kit::{base::animation::cubic_bezier, *};
use std::time::{Duration, Instant};

/// Uma entrada do catálogo: duração e curva `cubic-bezier`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec { ms: u64, delay: u64, curve: [f32; 4] }

const EASE: [f32; 4] = [0.25, 0.1, 0.25, 1.];
const EASE_OUT: [f32; 4] = [0., 0., 0.58, 1.];
const EASE_OUT_EXPO: [f32; 4] = [0.16, 1., 0.3, 1.];
const EASE_OUT_QUINT: [f32; 4] = [0.22, 1., 0.36, 1.];
/// `--ease-out` do web (`cubic-bezier(0.23, 1, 0.32, 1)`), a curva da marca e da linha "trabalhando".
const EASE_OUT_WEB: [f32; 4] = [0.23, 1., 0.32, 1.];

/// `fade-in`: entradas, 0,5 s, opacidade e 4 px subindo.
pub const FADE_IN: Spec = Spec::new(500, EASE_OUT_EXPO);
/// `fade-quick`: troca de tela ou aba, 0,15 s só de opacidade.
pub const FADE_QUICK: Spec = Spec::new(150, EASE);
/// `menu-in`: popover abrindo, 0,14 s.
pub const MENU_IN: Spec = Spec::new(140, EASE);
/// O cartão de contas abrindo: mais longo que o `menu-in`, para a descida se ver.
pub const CARD_OPEN: Spec = Spec::new(380, EASE_OUT_EXPO);
/// As barras de limite do cartão subindo do zero, a partir do meio da abertura.
pub const BAR_FILL: Spec = Spec::new(700, EASE_OUT_EXPO).after(190);
/// Popover fechando: 0,1 s, mais curto que a entrada.
pub const MENU_OUT: Spec = Spec::new(100, EASE);
/// `dialog-in`: 0,18 s.
pub const DIALOG_IN: Spec = Spec::new(180, EASE);
/// Painéis abrindo e fechando: 200 ms `ease-out`.
pub const RESIZE: Spec = Spec::new(200, EASE_OUT);
/// `splash-out`: a tela que sai, 0,5 s depois de 0,15 s parada, opacidade e 6 px subindo.
pub const SPLASH_OUT: Spec = Spec::new(500, EASE).after(150);
/// A linha "trabalhando" e a marca dela entrando (tempo e curva do web).
pub const WORKING: Spec = Spec::new(200, EASE_OUT_WEB);
/// A barra de sessões recolhendo ao trilho e voltando: o `transition: width 160ms var(--ease-out)` do web.
pub const NAV_FOLD: Spec = Spec::new(160, EASE_OUT_WEB);
/// Ferramenta nova entrando no grupo da Árvore: altura, opacidade e 4 px subindo.
pub const TOOL_REVEAL: Spec = Spec::new(360, EASE_OUT_EXPO);
/// Intervalo entre ferramentas que chegam juntas, para entrarem uma de cada vez.
pub const TOOL_STAGGER: Duration = Duration::from_millis(65);
/// Grupo da Árvore abrindo e fechando.
pub const TOOL_FOLD: Spec = Spec::new(140, EASE_OUT);
/// O traço da árvore descendo até a ponta da ferramenta nova: rápido no começo, assentando na ponta.
pub const TOOL_CONNECTOR: Spec = Spec::new(480, EASE_OUT_QUINT);
/// Uma passada do brilho no título do grupo que ainda roda.
pub const TOOL_SHIMMER: Duration = Duration::from_millis(3400);
/// O brilho do "Pensando", o `live-activity-focus` do T3: uma passada a cada 2,2 s, em 30 degraus.
pub const THINKING_FOCUS: Duration = Duration::from_millis(2200);
pub const THINKING_FOCUS_STEPS: u32 = 30;
/// Largura da faixa: os 4,5rem do T3.
pub const THINKING_FOCUS_WIDTH: f32 = 72.;
/// O `mask-image` do T3 em px desde a borda esquerda da faixa: (posição, alfa).
const FOCUS_STOPS: [(f32, f32); 7] = [(0., 0.), (10.8, 0.12), (25.2, 0.55), (36., 1.), (46.8, 0.55), (61.2, 0.12), (72., 0.)];
/// Largura de cada recorte da faixa: 24 recortes cobrem os 72 px sem degrau visível no texto.
const FOCUS_STRIP: f32 = 3.;
/// Linha trocando de lugar na lista (FLIP): desliza da posição antiga para a nova, e a que muda de seção acende.
pub const REORDER: Spec = Spec::new(320, EASE_OUT_QUINT);
/// A mensagem enviada indo do campo (ou do painel "Na fila") até o lugar dela; o painel fecha no mesmo tempo.
pub const FLIGHT: Spec = Spec::new(320, EASE_OUT_EXPO);
/// O painel "Na fila" abrindo para a mensagem que chega.
pub const QUEUE_OPEN: Spec = Spec::new(240, EASE_OUT_EXPO);
/// `zeron-pulse`: período dos esqueletos de carregando.
pub const PULSE: Duration = Duration::from_millis(2400);

impl Spec {
    const fn new(ms: u64, curve: [f32; 4]) -> Self { Self { ms, delay: 0, curve } }

    pub const fn after(mut self, delay: u64) -> Self { self.delay = delay; self }

    pub const fn duration(self) -> Duration { Duration::from_millis(self.ms) }

    /// Espera mais duração: quando a linha do tempo inteira acaba.
    pub fn total(self) -> Duration { Duration::from_millis(self.delay + self.ms).mul_f32(scale()) }

    /// Progresso com a curva para `raw` de 0 a 1 do tempo; fora disso, preso às pontas.
    pub fn ease(self, raw: f32) -> f32 {
        let [x1, y1, x2, y2] = self.curve;
        cubic_bezier(x1, y1, x2, y2)(raw)
    }

    /// Quanto do tempo passou desde `start`, de 0 a 1, sem curva; 0 durante a espera.
    pub fn raw(self, start: Instant) -> f32 { self.raw_at(start, Instant::now()) }

    pub fn raw_at(self, start: Instant, now: Instant) -> f32 {
        let (delay, span) = (self.delay as f32 / 1000. * scale(), self.duration().as_secs_f32() * scale());
        ((now.saturating_duration_since(start).as_secs_f32() - delay) / span).clamp(0., 1.)
    }
}

/// Só para medir e provar: `HANGAR_NATIVE_MOTION_SCALE=8` estica 8 vezes cada linha do tempo do catálogo, para uma
/// sequência de capturas pegar os quadros do meio. Sem a variável, 1.
fn scale() -> f32 {
    static SCALE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| std::env::var("HANGAR_NATIVE_MOTION_SCALE").ok().and_then(|v| v.parse::<f32>().ok())
        .filter(|s| s.is_finite()).map_or(1., |s| s.clamp(0.1, 50.)))
}

/// A curva `--ease-out` do web para quem monta os próprios trechos (a marca "trabalhando").
pub fn ease_out(x: f32) -> f32 { WORKING.ease(x) }

/// Onda do pulso: 0 no começo do ciclo, 1 no meio, 0 no fim.
pub fn pulse_wave(phase: f32) -> f32 { 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos() }

// Os desenhos de cada entrada, para `t` já com a curva (0 = começo, 1 = no lugar).

pub fn fade_in<E: Styled>(el: E, t: f32) -> E { el.relative().opacity(t).top(px(4. * (1. - t))) }

/// A tela sem sessão chegando: desce 10 px até o lugar, como um objeto só.
pub fn settle_down<E: Styled>(el: E, t: f32) -> E { el.relative().opacity(t).top(px(-10. * (1. - t))) }

pub fn fade_quick<E: Styled>(el: E, t: f32) -> E { el.opacity(t) }

pub fn menu_in<E: Styled>(el: E, t: f32) -> E { el.relative().opacity(0.3 + 0.7 * t).top(px(-2. * (1. - t))) }

pub fn card_open<E: Styled>(el: E, t: f32) -> E { el.relative().opacity(t).top(px(-14. * (1. - t))) }

/// Saída do popover; `t` vai de 0 (inteiro) a 1 (sumido).
pub fn menu_out<E: Styled>(el: E, t: f32) -> E { el.relative().opacity(1. - t).top(px(-2. * t)) }

/// Um traço da árvore chegando, de `progress` (0 a 1, com curva): quanto do tronco que desce até a linha e quanto do
/// ramo até ela estão desenhados. Com uma linha antes, o tronco dela desce primeiro (`connector_continuation`) e só
/// depois o desta; o ramo começa um pouco antes do fim do tronco, sem quadro parado na curva.
pub fn connector_parts(progress: f32, has_predecessor: bool) -> (f32, f32) {
    let progress = progress.clamp(0., 1.);
    let (start, end, branch) = if has_predecessor { (0.45, 0.72, 0.68) } else { (0., 0.62, 0.58) };
    (((progress - start) / (end - start)).clamp(0., 1.), ((progress - branch) / (1. - branch)).clamp(0., 1.))
}

/// O tronco que sai da linha de cima até a próxima: pertence à de cima, mas anda no tempo da que chega.
pub fn connector_continuation(next_progress: Option<f32>) -> f32 {
    next_progress.map_or(1., |progress| (progress / 0.45).clamp(0., 1.))
}

/// Quanto brilha o ponto `x` (0 a 1 da largura do título) na fase `phase` da passada: faixa de 0,36 título de meia
/// largura, cópias a três títulos uma da outra, como no Zeron.
pub fn shimmer_amount(x: f32, phase: f32) -> f32 {
    let center = -2.5 + phase.clamp(0., 1.) * 6.;
    (-2..=2).map(|copy| center + copy as f32 * 3.).map(|c| (1. - (x - c).abs() / 0.36).clamp(0., 1.)).fold(0., f32::max)
}

/// Alfa do gradiente da faixa a `u` px da borda esquerda dela; fora da faixa, 0.
pub fn focus_mask(u: f32) -> f32 {
    if !(0. ..=THINKING_FOCUS_WIDTH).contains(&u) { return 0.; }
    FOCUS_STOPS.windows(2).find(|pair| u <= pair[1].0)
        .map_or(0., |pair| {
            let ((x0, a0), (x1, a1)) = (pair[0], pair[1]);
            a0 + (a1 - a0) * (u - x0) / (x1 - x0)
        })
}

/// Fase da passada em degraus, como o `steps(30)` do CSS: anda de 1/30 em 1/30 e volta a 0 a cada 2,2 s.
pub fn focus_phase(elapsed: Duration) -> f32 {
    let period = THINKING_FOCUS.as_secs_f64();
    let steps = THINKING_FOCUS_STEPS as f64;
    ((elapsed.as_secs_f64() % period / period * steps).floor() / steps) as f32
}

/// Borda esquerda da faixa: entra toda por fora à esquerda e sai toda por fora à direita, como o overlay do T3,
/// que tem a largura da linha mais duas faixas.
pub fn focus_band_left(phase: f32, width: f32) -> f32 {
    -THINKING_FOCUS_WIDTH + phase * (width + 2. * THINKING_FOCUS_WIDTH)
}

/// Os recortes da faixa que caem na linha: posição e largura em px desde o começo dela, e o alfa do gradiente no
/// meio do recorte. A GPUI não tem máscara em gradiente; recortar a cópia acesa em fatias faz o mesmo desenho.
pub fn focus_strips(phase: f32, width: f32) -> Vec<(f32, f32, f32)> {
    let left = focus_band_left(phase, width);
    let count = (THINKING_FOCUS_WIDTH / FOCUS_STRIP) as usize;
    (0..count).filter_map(|n| {
        let start = n as f32 * FOCUS_STRIP;
        // Metade da direita lê o alfa no recorte espelhado: o mesmo valor, bit a bit, e a linha moldada se reaproveita.
        let alpha = focus_mask(start.min(THINKING_FOCUS_WIDTH - FOCUS_STRIP - start) + FOCUS_STRIP / 2.);
        let (x0, x1) = ((left + start).max(0.), (left + start + FOCUS_STRIP).min(width));
        (x1 > x0 && alpha > 0.).then_some((x0, x1 - x0, alpha))
    }).collect()
}

/// Progresso com curva da entrada do elemento `key`, contado do primeiro desenho dele; pede quadros até terminar. O
/// relógio vive enquanto o elemento é desenhado em quadros seguidos: sumiu e voltou, entra de novo. Com movimento
/// reduzido, já chega inteiro.
pub fn enter(key: impl Into<ElementId>, spec: Spec, window: &mut Window, cx: &App) -> f32 {
    if cx.reduce_motion() { return 1.; }
    let start = window.with_global_id(key.into(), |id, window| window.with_element_state(id, |start: Option<Instant>, _| {
        let start = start.unwrap_or_else(Instant::now);
        (start, start)
    }));
    if start.elapsed() < spec.total() { request_frame(window, cx); }
    spec.ease(spec.raw(start))
}

/// A view vazia que a raiz monta para pedir quadros só para si.
struct Tick;

impl Render for Tick {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement { div() }
}

struct Ticker { view: Entity<Tick>, root: EntityId }

impl Global for Ticker {}

/// Monta o relógio da raiz: chamar uma vez no desenho da view de cima, em todo quadro.
pub fn ticker(window: &mut Window, cx: &mut App) -> AnyElement {
    let root = window.current_view();
    if !cx.has_global::<Ticker>() {
        let view = cx.new(|_| Tick);
        cx.set_global(Ticker { view, root });
    }
    let ticker = cx.global_mut::<Ticker>();
    ticker.root = root;
    AnyView::from(ticker.view.clone()).cached(StyleRefinement::default().absolute().size_0()).into_any_element()
}

/// Redesenha só a raiz, fora do desenho: as áreas guardadas são reusadas. `false` sem o relógio montado.
pub fn tick_root(cx: &mut App) -> bool {
    let Some(tick) = cx.try_global::<Ticker>().map(|ticker| ticker.view.entity_id()) else { return false };
    cx.notify(tick);
    true
}

/// Pede o próximo quadro para quem está desenhando: na raiz, pelo relógio dela; numa área ou view própria, só ela.
pub fn request_frame(window: &Window, cx: &App) {
    match cx.try_global::<Ticker>().filter(|ticker| ticker.root == window.current_view()) {
        Some(ticker) => {
            let tick = ticker.view.entity_id();
            window.on_next_frame(move |_, cx| cx.notify(tick));
        }
        None => window.request_animation_frame(),
    }
}

/// Quem anda numa lista que se reordena. Cada desenho da lista abre uma passada (`begin`) com as chaves em ordem e a
/// seção de cada uma; cada linha, ao ganhar lugar (`place`), compara com onde foi pintada na passada anterior. Só uma
/// ordem nova anima: rolar ou redimensionar muda a posição sem mexer na ordem e não desliza nada.
#[derive(Default)]
pub struct FlipBook { pass: u64, epoch: u64, order: Vec<String>, rows: std::collections::HashMap<String, FlipRow> }

struct FlipRow { section: String, y: f32, placed: Option<u64>, epoch: u64, fade: Option<Instant>, slide: Option<Slide> }

impl FlipRow {
    fn new(section: String, epoch: u64) -> Self { Self { section, y: 0., placed: None, epoch, fade: None, slide: None } }
}

#[derive(Clone, Copy)]
struct Slide { start: Instant, from: f32, lift: bool }

/// Onde pintar a linha nesta passada: `dy` somado à posição do layout; `lift` pinta por cima das vizinhas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement { pub dy: f32, pub lift: bool, pub moving: bool }

impl FlipBook {
    /// Abre a passada, antes de desenhar as linhas, com a seção de cada uma que pode mudar de seção: quem mudou acende e
    /// passa por cima (`lifting`), o que o desenho dela precisa saber já.
    pub fn sections(&mut self, entries: &[(String, String)], now: Instant, reduce: bool) {
        self.pass += 1;
        let (pass, epoch) = (self.pass, self.epoch);
        for (key, section) in entries {
            let row = self.rows.entry(key.clone()).or_insert_with(|| FlipRow::new(section.clone(), epoch));
            if reduce { row.fade = None; row.slide = None; }
            // Só quem foi pintado na passada anterior acende: sumida e voltando, entra parada.
            if row.section != *section && row.placed.is_some_and(|p| p + 1 == pass) && !reduce { row.fade = Some(now); }
            row.section.clone_from(section);
        }
    }

    /// Fecha a passada com a ordem de tudo o que a lista vai pintar: ordem nova anima; quem saiu é esquecido.
    pub fn order(&mut self, keys: &[String]) {
        // A transição repete a mesma ordem por vários quadros; só uma lista diferente precisa da tabela de retenção.
        if keys == self.order.as_slice() && self.rows.len() == keys.len() { return; }
        if keys != self.order.as_slice() {
            self.epoch += 1;
            self.order = keys.to_vec();
        }
        let keep: std::collections::HashSet<&str> = keys.iter().map(String::as_str).collect();
        self.rows.retain(|key, _| keep.contains(key.as_str()));
        let epoch = self.epoch;
        for key in keys { self.rows.entry(key.clone()).or_insert_with(|| FlipRow::new(String::new(), epoch)); }
    }

    /// Onde a chave estava na última ordem fechada por `order`.
    pub fn index_of(&self, key: &str) -> Option<usize> { self.order.iter().position(|k| k == key) }

    /// Esquece o que foi pintado: a lista saiu da tela.
    pub fn clear(&mut self) {
        if !self.rows.is_empty() || !self.order.is_empty() { *self = Self::default(); }
    }

    /// A linha está trocando de seção agora.
    pub fn lifting(&self, key: &str, now: Instant) -> bool {
        self.rows.get(key).and_then(|row| row.fade).is_some_and(|start| start + REORDER.total() > now)
    }

    pub fn place(&mut self, key: &str, y: f32, now: Instant, reduce: bool) -> Placement {
        let (pass, epoch) = (self.pass, self.epoch);
        let still = Placement { dy: 0., lift: false, moving: false };
        let Some(row) = self.rows.get_mut(key) else { return still };
        if reduce || !row.placed.is_some_and(|p| p + 1 == pass) { row.slide = None; }
        else if row.epoch != epoch && (row.y - y).abs() > 0.5 {
            // Do lugar em que estava pintada agora: interromper continua do meio, sem pular para a ponta antiga.
            row.slide = Some(Slide { start: now, from: row.y - y, lift: row.fade.is_some() || row.slide.is_some_and(|s| s.lift) });
        }
        row.epoch = epoch;
        row.placed = Some(pass);
        let placement = match row.slide {
            Some(slide) if slide.start + REORDER.total() > now => {
                let t = REORDER.ease(REORDER.raw_at(slide.start, now));
                Placement { dy: slide.from * (1. - t), lift: slide.lift, moving: true }
            }
            _ => { row.slide = None; still }
        };
        row.y = y + placement.dy;
        placement
    }

    /// Quanto a linha que trocou de seção já chegou, de 0 a 1 com curva, contado da troca; `None` parada.
    pub fn arrival(&mut self, key: &str, now: Instant) -> Option<f32> {
        let row = self.rows.get_mut(key)?;
        let start = row.fade?;
        if start + REORDER.total() <= now { row.fade = None; return None; }
        Some(REORDER.ease(REORDER.raw_at(start, now)))
    }
}

/// O elemento da linha: no prepaint lê a posição real do layout e desloca o filho; a que troca de seção é pintada depois
/// das vizinhas, dentro do mesmo recorte da lista, para passar por cima delas.
pub struct Flip {
    key: String,
    child: Option<AnyElement>,
    mask: std::rc::Rc<std::cell::Cell<Option<ContentMask<Pixels>>>>,
    book: std::rc::Rc<std::cell::RefCell<FlipBook>>,
    /// Lista em fileira: a troca de lugar desliza no eixo x.
    horizontal: bool,
}

impl Flip {
    pub fn new(key: String, child: AnyElement, book: std::rc::Rc<std::cell::RefCell<FlipBook>>) -> Self {
        let mask = std::rc::Rc::new(std::cell::Cell::new(None));
        Self { key, child: Some(Masked { child, mask: mask.clone() }.into_any_element()), mask, book, horizontal: false }
    }

    pub fn horizontal(mut self) -> Self { self.horizontal = true; self }
}

impl IntoElement for Flip {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for Flip {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App)
        -> (LayoutId, ()) {
        (self.child.as_mut().expect("filho antes do prepaint").request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (),
        window: &mut Window, cx: &mut App) {
        let still = cx.reduce_motion() || !window.is_window_active();
        let at = if self.horizontal { bounds.origin.x } else { bounds.origin.y };
        let place = self.book.borrow_mut().place(&self.key, f32::from(at), Instant::now(), still);
        if place.moving { request_frame(window, cx); }
        let offset = if self.horizontal { point(px(place.dy), px(0.)) } else { point(px(0.), px(place.dy)) };
        if place.lift {
            let child = self.child.take().expect("prepaint uma vez só");
            let (at, mask) = (window.element_offset() + offset, window.content_mask());
            // O adiado só recorta a pintura: a área clicável também precisa do recorte da lista.
            self.mask.set(Some(mask));
            window.defer_draw(child, at, 1, Some(mask));
        } else if let Some(child) = self.child.as_mut() {
            window.with_element_offset(offset, |window| child.prepaint(window, cx));
        }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (),
        window: &mut Window, cx: &mut App) {
        if let Some(child) = self.child.as_mut() { child.paint(window, cx); }
    }
}

/// Prepaint e pintura do filho dentro do recorte guardado, quando há um.
struct Masked { child: AnyElement, mask: std::rc::Rc<std::cell::Cell<Option<ContentMask<Pixels>>>> }

impl IntoElement for Masked {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for Masked {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App)
        -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (),
        window: &mut Window, cx: &mut App) {
        let child = &mut self.child;
        window.with_content_mask(self.mask.get(), |window| child.prepaint(window, cx));
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (),
        window: &mut Window, cx: &mut App) {
        let child = &mut self.child;
        window.with_content_mask(self.mask.get(), |window| child.paint(window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn catalog_matches_zeron() {
        assert_eq!((FADE_IN.ms, FADE_QUICK.ms, MENU_IN.ms, MENU_OUT.ms, DIALOG_IN.ms, RESIZE.ms, SPLASH_OUT.ms, SPLASH_OUT.delay),
            (500, 150, 140, 100, 180, 200, 500, 150));
        for spec in [FADE_IN, FADE_QUICK, MENU_IN, DIALOG_IN, RESIZE, SPLASH_OUT, WORKING] {
            assert_eq!((spec.ease(0.), spec.ease(1.), spec.ease(2.)), (0., 1., 1.));
            let mut last = 0.;
            for i in 0..=100 {
                let y = spec.ease(i as f32 / 100.);
                assert!((0. ..=1.).contains(&y) && y >= last - 1e-4);
                last = y;
            }
        }
        // Valores de referência do Zeron (bisseção de 80 passos): expo em 0,5 e `ease` em 0,25.
        assert!((FADE_IN.ease(0.5) - 0.971779).abs() < 1e-3 && (FADE_QUICK.ease(0.25) - 0.408511).abs() < 1e-3);
    }

    #[test]
    fn connector_draws_the_previous_trunk_first_and_ends_whole() {
        assert_eq!((connector_parts(0., false), connector_parts(1., false), connector_parts(1., true)), ((0., 0.), (1., 1.), (1., 1.)));
        // Com linha antes, nada desta nasce enquanto o tronco de cima desce.
        assert_eq!(connector_parts(0.4, true), (0., 0.));
        assert_eq!((connector_continuation(Some(0.)), connector_continuation(Some(0.45)), connector_continuation(None)), (0., 1., 1.));
        assert!(shimmer_amount(0.5, 0.5) > 0.99 && shimmer_amount(0.5, 0.25) < 0.01);
    }

    fn entries(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, s)| (k.to_string(), s.to_string())).collect()
    }

    /// A passada inteira de uma lista só de linhas de sessão.
    trait Begin { fn begin(&mut self, entries: &[(String, String)], now: Instant, reduce: bool); }

    impl Begin for FlipBook {
        fn begin(&mut self, entries: &[(String, String)], now: Instant, reduce: bool) {
            self.sections(entries, now, reduce);
            self.order(&entries.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn reorder_slides_from_where_the_row_was_painted() {
        let (mut book, t0) = (FlipBook::default(), Instant::now());
        book.begin(&entries(&[("a", "m"), ("b", "m")]), t0, false);
        assert_eq!(book.place("a", 0., t0, false).dy, 0., "primeira pintura não anima");
        book.place("b", 40., t0, false);
        // "b" passou a trabalhar: sobe para o topo, troca de seção e é pintada por cima.
        book.begin(&entries(&[("b", "w"), ("a", "m")]), t0, false);
        let b = book.place("b", 0., t0, false);
        assert_eq!((b.dy, b.lift, b.moving), (40., true, true));
        assert!(book.lifting("b", t0) && !book.lifting("a", t0));
        assert_eq!(book.place("a", 40., t0, false).dy, -40.);
        assert!(book.arrival("b", t0) == Some(0.) && book.arrival("a", t0).is_none());
        let end = t0 + REORDER.total();
        book.begin(&entries(&[("b", "w"), ("a", "m")]), end, false);
        assert_eq!(book.place("b", 0., end, false), Placement { dy: 0., lift: false, moving: false });
        assert_eq!(book.arrival("b", end), None);
    }

    #[test]
    fn scrolling_without_a_new_order_does_not_slide() {
        let (mut book, t0) = (FlipBook::default(), Instant::now());
        book.begin(&entries(&[("a", "m")]), t0, false);
        book.place("a", 100., t0, false);
        book.begin(&entries(&[("a", "m")]), t0, false);
        assert_eq!(book.place("a", 60., t0, false).dy, 0.);
    }

    #[test]
    fn interrupted_slide_continues_from_the_painted_spot() {
        let (mut book, t0) = (FlipBook::default(), Instant::now());
        book.begin(&entries(&[("a", "m"), ("b", "m")]), t0, false);
        book.place("a", 0., t0, false);
        book.place("b", 40., t0, false);
        book.begin(&entries(&[("b", "w"), ("a", "m")]), t0, false);
        book.place("b", 0., t0, false);
        let mid = t0 + REORDER.total() / 4;
        book.begin(&entries(&[("b", "w"), ("a", "m")]), mid, false);
        let painted = book.place("b", 0., mid, false).dy;
        assert!(painted > 0. && painted < 40.);
        // Parou de trabalhar no meio do caminho: volta de onde está agora, não de 0 nem de 40.
        book.begin(&entries(&[("a", "m"), ("b", "m")]), mid, false);
        assert!((book.place("b", 40., mid, false).dy - (painted - 40.)).abs() < 1e-3);
    }

    #[test]
    fn reduced_motion_and_returning_rows_stay_still() {
        let (mut book, t0) = (FlipBook::default(), Instant::now());
        book.begin(&entries(&[("a", "m"), ("b", "m")]), t0, true);
        book.place("a", 0., t0, true);
        book.place("b", 40., t0, true);
        book.begin(&entries(&[("b", "w"), ("a", "m")]), t0, true);
        assert_eq!(book.place("b", 0., t0, true).dy, 0.);
        assert_eq!(book.arrival("b", t0), None);
        // Sumida numa passada (grupo recolhido) e de volta noutra seção: entra parada.
        book.begin(&entries(&[("a", "m")]), t0, false);
        book.place("a", 0., t0, false);
        book.begin(&entries(&[("a", "m"), ("b", "w")]), t0, false);
        assert_eq!(book.place("b", 40., t0, false).dy, 0.);
        assert_eq!(book.arrival("b", t0), None);
    }

    #[test]
    fn pulse_ends() {
        assert!((pulse_wave(0.) - 0.).abs() < 1e-6 && (pulse_wave(0.5) - 1.).abs() < 1e-6);
    }

    #[test]
    fn stopping_motion_discards_an_inflight_slide_and_highlight() {
        let (mut book, now) = (FlipBook::default(), Instant::now());
        book.begin(&entries(&[("a", "machine"), ("b", "machine")]), now, false);
        book.place("a", 0., now, false);
        book.place("b", 40., now, false);
        let moved = entries(&[("b", "working"), ("a", "machine")]);
        book.begin(&moved, now, false);
        assert!(book.place("b", 0., now, false).moving);
        assert!(book.arrival("b", now).is_some());
        book.begin(&moved, now, true);
        assert_eq!(book.place("b", 0., now, true), Placement { dy: 0., lift: false, moving: false });
        assert_eq!(book.arrival("b", now), None);
        book.begin(&moved, now, false);
        assert!(!book.place("b", 0., now, false).moving, "voltar ao foco não repete a transição");
    }

    #[test]
    fn focus_mask_matches_the_t3_gradient() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        for (u, alpha) in [(0., 0.), (10.8, 0.12), (25.2, 0.55), (36., 1.), (46.8, 0.55), (61.2, 0.12), (72., 0.)] {
            assert!(close(focus_mask(u), alpha), "u={u}: {} != {alpha}", focus_mask(u));
        }
        // Linear entre as paradas, simétrica em volta do centro, zero fora da faixa.
        assert!(close(focus_mask(5.4), 0.06));
        for d in [0., 3., 11.1, 20., 35.] { assert!(close(focus_mask(36. - d), focus_mask(36. + d))); }
        assert_eq!((focus_mask(-0.5), focus_mask(72.5), focus_mask(f32::NAN)), (0., 0., 0.));
    }

    #[test]
    fn focus_phase_steps_thirty_times_per_pass() {
        let step = THINKING_FOCUS / THINKING_FOCUS_STEPS;
        let ms = Duration::from_millis;
        assert_eq!(focus_phase(Duration::ZERO), 0.);
        assert_eq!(focus_phase(step - ms(1)), 0.);
        assert!((focus_phase(step + ms(1)) - 1. / 30.).abs() < 1e-6);
        assert!((focus_phase(THINKING_FOCUS - ms(1)) - 29. / 30.).abs() < 1e-6);
        // A passada recomeça: exatamente 2,2 s é o degrau 0 de novo, e tempo longo não passa de 29/30.
        assert_eq!(focus_phase(THINKING_FOCUS), 0.);
        assert!(focus_phase(Duration::from_secs(3600) + ms(2199)) <= 29. / 30. + 1e-6);
        let mut seen: Vec<u32> = (0..2200).map(|t| (focus_phase(ms(t)) * 30.).round() as u32).collect();
        seen.dedup();
        assert_eq!(seen, (0..30).collect::<Vec<_>>());
    }

    #[test]
    fn focus_band_crosses_from_outside_to_outside() {
        // Linha de 90 px: o trajeto é 90 + 2 × 72 = 234 px.
        assert_eq!(focus_band_left(0., 90.), -72.);
        assert!((focus_band_left(0.5, 90.) - 45.).abs() < 1e-4);
        assert!(focus_strips(0., 90.).is_empty(), "no degrau 0 a faixa ainda está toda à esquerda");
        assert!(focus_strips(29. / 30., 90.).is_empty(), "no último degrau ela já saiu à direita");
        assert!(!focus_strips(0.5, 90.).is_empty());
    }

    #[test]
    fn focus_strips_stay_inside_the_row() {
        for width in [0., 1., 22., 90., 400.] {
            for n in 0..30 {
                for (x, w, alpha) in focus_strips(n as f32 / 30., width) {
                    assert!(x >= 0. && w > 0. && x + w <= width + 1e-4, "width={width} n={n}: ({x}, {w})");
                    assert!(alpha > 0. && alpha <= 1.);
                }
            }
        }
        // No meio da passada sobre uma linha larga, a faixa inteira cabe: 24 recortes de 3 px, o pico no meio.
        let strips = focus_strips(0.5, 400.);
        assert_eq!(strips.len(), 24);
        assert!(strips.iter().all(|&(_, w, _)| (w - 3.).abs() < 1e-4));
        let peak = strips.iter().map(|s| s.2).fold(0., f32::max);
        assert!((peak - focus_mask(34.5).max(focus_mask(37.5))).abs() < 1e-4);
        assert!((0..12).all(|n| strips[n].2 == strips[23 - n].2), "recortes espelhados têm o mesmo alfa");
    }
}
