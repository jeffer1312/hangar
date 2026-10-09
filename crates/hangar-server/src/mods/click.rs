//! Clique de mod numa sessão com terminal que o Rust atende (Linux, macOS e Windows com psmux): lê a tela,
//! ativa a aba, alcança o botão e clica pelo mouse; o teclado é a reserva só nos casos medidos (T3 a T6,
//! T9). O `backend/app/plugin_click.py` de hoje fica só para o processo sem Rust. O pane é falado por
//! operações do executor do terminal (`TerminalHandle::pane`), seriais com a entrada e fora do diário da
//! fila.
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::model::*;
use super::screen::{self, COLLAPSED_TEXT, Screen};
use super::state::{FocusSeen, Mods, TerminalView};
use super::tree;
use crate::terminal_input::PaneFormats;

/// O que o clique pede ao pane. Linha e coluna a partir de 0.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneOp {
    Formats,
    Clients,
    Screen,
    Mouse { row: u16, col: u16 },
    Wheel { row: u16, col: u16, down: bool },
    Keys(Vec<String>),
    Resize { columns: u16, rows: u16 },
    /// Reserva o pane ao clique por `millis`: o executor guarda os comandos e a drenagem da fila até o
    /// `Release` ou o fim do prazo (C6).
    Hold { millis: u64 },
    Release,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaneReply {
    Formats(PaneFormats),
    Clients(usize),
    Screen(String),
    Done,
}

pub type PaneFuture = Pin<Box<dyn Future<Output = Result<PaneReply, ModsError>> + Send>>;

/// O pane da sessão: o executor do terminal, ou um de mentira nos testes.
pub trait Pane: Send + Sync {
    /// `start_by`: a operação que chega à vez dela depois disso não age e volta recusada. É o prazo de
    /// quem pediu menos o que a ação ainda precisa (C1): uma tecla que ficou na caixa do executor não
    /// chega ao terminal depois de o app ouvir que o clique falhou. O `Release` é a exceção e solta sempre,
    /// mesmo atrasado: é a limpeza do clique, e recusado deixaria a fila guardada até o fim da reserva.
    fn op(&self, op: PaneOp, start_by: Instant) -> PaneFuture;
}

/// Sem terminal ligado o Hangar é dono do tamanho (T9): 144 colunas colocam o painel aberto sem pedido e o
/// deixam ao lado da conversa; 40 linhas mantêm a faixa desenhada (achado 12).
pub const MIN_COLUMNS: u16 = 144;
pub const MIN_ROWS: u16 = 40;
/// Altura temporária para alcançar um botão fora da área visível; medida no psmux com 120 e 250 linhas.
pub const TALL_ROWS: u16 = 250;
/// Folga depois da confirmação do clique final: a operação no pane e a volta da resposta à rota.
pub const ACTION_MARGIN: Duration = Duration::from_millis(300);
/// Prazo da limpeza (`finish`), que roda depois da resposta e fora do prazo de quem pediu.
pub const UNDO_MAX: Duration = Duration::from_secs(2);
/// Folga da reserva renovada na limpeza além do tempo que ela cobre: a operação ainda na caixa do executor.
pub const HOLD_MARGIN: Duration = Duration::from_millis(500);
/// Distância, em linhas e colunas, até a qual um clique conta como vizinho do anterior (`click_gap_near`).
const NEAR_CELLS: usize = 2;
/// Teto de uma volta ao prompt na limpeza: abaixo dos 10 s de cada reserva no executor, com a folga.
pub const BACK_MAX: Duration = Duration::from_secs(9);
/// O mais longo que a limpeza segura o pane: a primeira volta ao prompt, as novas tentativas até
/// `keep_held` (a última pode começar no fim dele) e as devoluções da altura e do pane.
pub const CLEANUP_MAX: Duration = Duration::from_secs(9 + 10 + 9 + 2 * 2);

/// Tempos medidos (`medicoes-terminal.md`, `medicoes-psmux.md`), cortados para caber nos 7,5 s do pedido
/// (fase 2); `quick` para os testes.
#[derive(Clone, Debug)]
pub struct Limits {
    pub confirm: Duration,
    pub activate_poll: Duration,
    pub activate_max: Duration,
    pub wheel_gap: Duration,
    pub wheel_events: usize,
    pub wheel_max: Duration,
    pub scroll_wait: Duration,
    pub key_gap: Duration,
    pub focus_wait: Duration,
    pub settle_poll: Duration,
    pub settle_max: Duration,
    /// Teto para manter o pane reservado depois de uma volta ao prompt que falhou, tentando de novo: a
    /// fila não entrega mensagem com o teclado ainda num painel. Cada tentativa renova a reserva no executor,
    /// que corta cada uma em 10 s.
    pub keep_held: Duration,
    pub retry_gap: Duration,
    /// Intervalo mínimo entre dois cliques de mouse no pane: mais perto que isso o Claude Code os toma por
    /// duplo clique e engole o segundo (medido no tmux: a 2 células ou mais, até 250 ms some e a partir de
    /// 300 ms funciona).
    pub click_gap: Duration,
    /// O intervalo quando o segundo clique cai a até `NEAR_CELLS` do anterior: aí o Claude Code engole até
    /// 450 ms e aceita a partir de 500 ms. É também o preferido: usado sempre que o prazo comporta.
    pub click_gap_near: Duration,
    /// Quanto um passo do anel custa num botão da faixa que não está desenhado: a tela não muda, e o passo
    /// termina com o `ui.focus` do botão, não com a espera da tela.
    pub ring_hidden_step: Duration,
    /// Quanto o anel do teclado espera a tela mudar depois de uma tecla (ela aparece em 50 a 60 ms no tmux e
    /// no psmux): a leitura seguinte é repetida até ver a mudança ou até este teto.
    pub key_settle: Duration,
    /// Quanto um passo do anel do `ctrl+x tab` pode custar: a tecla, a leitura e a espera da tela. Nas provas
    /// de 06/10/2026 o anel andou a 0,04 s por passo no tmux e a 0,06 a 0,13 s no psmux (com o pane conferido
    /// uma vez por reserva); o teto guarda folga.
    pub ring_step: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        // Roda até 4 s: a medição pedia 10 s, que não cabem no orçamento; o piso de cada evento corta antes.
        Self { confirm: Duration::from_secs(2), activate_poll: Duration::from_millis(50), activate_max: Duration::from_millis(300),
            wheel_gap: Duration::from_millis(150), wheel_events: 80, wheel_max: Duration::from_secs(4), scroll_wait: Duration::from_millis(600),
            key_gap: Duration::from_millis(20), focus_wait: Duration::from_millis(500), settle_poll: Duration::from_millis(100),
            settle_max: Duration::from_secs(1), keep_held: Duration::from_secs(10), retry_gap: Duration::from_millis(500),
            click_gap: Duration::from_millis(350), click_gap_near: Duration::from_millis(550),
            ring_hidden_step: Duration::from_millis(200), key_settle: Duration::from_millis(300), ring_step: Duration::from_millis(200) }
    }
}

impl Limits {
    pub fn quick() -> Self {
        Self { confirm: Duration::from_millis(300), activate_poll: Duration::from_millis(1), activate_max: Duration::from_millis(50),
            wheel_gap: Duration::ZERO, wheel_events: 80, wheel_max: Duration::from_secs(2), scroll_wait: Duration::from_millis(50),
            key_gap: Duration::ZERO, focus_wait: Duration::from_millis(100), settle_poll: Duration::from_millis(1),
            settle_max: Duration::from_millis(10), keep_held: Duration::from_millis(100), retry_gap: Duration::from_millis(20),
            click_gap: Duration::from_millis(20), click_gap_near: Duration::from_millis(20),
            ring_hidden_step: Duration::from_millis(20), key_settle: Duration::from_millis(10), ring_step: Duration::from_millis(50) }
    }
}

/// Como voltar o teclado ao prompt: a leitura da tela precisa dos títulos e da âncora, e o laço, do tamanho
/// do anel do `ctrl+x tab`.
#[derive(Clone)]
struct Back { titles: Vec<String>, anchor: Option<String>, cap: usize }

#[derive(Default)]
struct Pending { hold: bool, height: Option<(u16, u16)>, focus: Option<String>, keyboard: Option<Back> }

/// O que o pedido deixou para desfazer. Cada item entra antes da ação que o pede, para valer também com o
/// pedido cortado no meio; `finish` desfaz.
#[derive(Default)]
pub struct Undo(Mutex<Pending>);

impl Undo {
    pub fn set_hold(&self) { self.0.lock().unwrap().hold = true; }
    /// A altura de antes; a primeira registrada vale.
    pub fn height(&self, columns: u16, rows: u16) { self.0.lock().unwrap().height.get_or_insert((columns, rows)); }
    pub fn focus(&self, attempt: &str) { self.0.lock().unwrap().focus = Some(attempt.to_owned()); }
    pub fn keyboard(&self, titles: Vec<String>, anchor: Option<String>, cap: usize) {
        self.0.lock().unwrap().keyboard = Some(Back { titles, anchor, cap });
    }
    pub fn is_empty(&self) -> bool {
        let pending = self.0.lock().unwrap();
        !pending.hold && pending.height.is_none() && pending.focus.is_none() && pending.keyboard.is_none()
    }
    /// Lê ou muda o pendente sob a trava. Um item só sai depois de desfeito: cortado no meio da volta, ele
    /// continua aqui para a guarda refazer.
    fn with<T>(&self, change: impl FnOnce(&mut Pending) -> T) -> T { change(&mut self.0.lock().unwrap()) }
}

/// Quando e em que célula saiu o último clique de mouse no pane.
pub type LastClick = Option<(Instant, (usize, usize))>;

pub struct Ctx<'a> {
    pub name: &'a str,
    pub pane: &'a dyn Pane,
    pub mods: &'a Mods,
    pub limits: &'a Limits,
    /// Prazo de quem pediu (a rota: 7,5 s desde a entrada). Ação no mod só começa com tempo para a
    /// confirmação dentro dele.
    pub until: Instant,
    pub undo: &'a Undo,
    /// A vida da sessão que pediu (`Mods::new_life`): o que o clique lê e escreve no registro é só dela, e
    /// a sessão que reabriu com o mesmo nome no meio do clique não recebe nada dele.
    pub life: u64,
    /// Quando e onde saiu o último clique de mouse neste pane, deste pedido ou de um anterior
    /// (`Parts::clicked`).
    pub clicked: &'a Mutex<LastClick>,
}

/// O lugar que o app pediu (faixa ou painel), resolvido no espelho que o plugin mandou.
struct Target {
    site: String,
    tree: Value,
    ids: Vec<String>,
    titles: Vec<String>,
    anchor: Option<String>,
    band_buttons: usize,
}

/// O botão do clique, no lugar do `Target`; fechar e trocar de aba não têm.
struct Button {
    plugin: String,
    key: String,
    label: String,
    /// O rótulo aparece mais de uma vez na árvore do painel ou da faixa, visível ou não: o mouse não
    /// distingue um do outro e o clique vai pelo teclado (T5).
    repeated: bool,
    /// Outro mod usa a mesma `key` no lugar: o foco contado sem o mod não diz de qual é.
    shared: bool,
}

fn target_of(view: &TerminalView, site: &str, tree: Value) -> Target {
    Target { site: site.into(), ids: view.ids(), titles: view.titles(), anchor: tree::anchor(&view.above),
        band_buttons: tree::count_buttons(&view.above), tree }
}

fn target(ctx: &Ctx<'_>, site: &str) -> Result<Target, ModsError> {
    // Sem a vista ainda, a faixa é botão que não está na tela, como na sessão sem terminal.
    let view = ctx.mods.terminal_view_in(ctx.name, ctx.life)
        .ok_or_else(|| if site == BAND_SITE { missing() } else { pane_missing() })?;
    let tree = if site == BAND_SITE { view.above.clone() }
        else { view.panes.iter().find(|p| p.id == site).map(|p| p.tree.clone()).ok_or_else(pane_missing)? };
    Ok(target_of(&view, site, tree))
}

/// O botão `key` do mod `plugin` no lugar do `t`. O mesmo mod com a mesma `key` duas vezes no lugar não
/// diz qual é, e nenhum é acionado.
fn button(t: &Target, plugin: &str, key: &str) -> Result<Button, ModsError> {
    if tree::ambiguous(&t.tree, Some(plugin), key, &["Button"]) { return Err(missing()); }
    let label = tree::label(&t.tree, plugin, key).ok_or_else(missing)?;
    let repeated = tree::label_count(&t.tree, &label) > 1;
    let shared = tree::ambiguous(&t.tree, None, key, &["Button"]);
    Ok(Button { plugin: plugin.into(), key: key.into(), label, repeated, shared })
}

enum Found { Cell((usize, usize)), Keyboard, Clicked }

impl<'a> Ctx<'a> {
    fn left(&self) -> Duration { self.until.saturating_duration_since(Instant::now()) }
    /// O mesmo pedido com o prazo da limpeza (`UNDO_MAX`) contado de agora, fora do de quem pediu.
    fn fresh(&self) -> Ctx<'a> { self.within(UNDO_MAX) }
    /// O mesmo pedido com o prazo `budget` contado de agora.
    fn within(&self, budget: Duration) -> Ctx<'a> { Ctx { until: Instant::now() + budget, ..*self } }
    /// A sessão ainda é a que pediu: com o nome reaberto por outro processo (ou a sessão fora), o pane é de
    /// outra vida e nada do clique chega a ele.
    fn alive(&self) -> bool { self.mods.life(self.name) == Some(self.life) }
    /// Até quando uma ação que espera `after` antes do clique final ainda pode começar: precisa sobrar
    /// `after`, a confirmação do clique final e a folga. Sem isso, recusa sem mandar nada (C1).
    fn start_by(&self, after: Duration) -> Result<Instant, ModsError> {
        let need = after + self.limits.confirm + ACTION_MARGIN;
        if self.left() < need { return Err(no_answer()); }
        Ok(self.until - need)
    }
    /// Uma operação no pane, cortada no prazo: a resposta que não vier a tempo é a do mod sem resposta.
    async fn op(&self, op: PaneOp, start_by: Instant) -> Result<PaneReply, ModsError> {
        tokio::time::timeout_at(self.until.into(), self.pane.op(op, start_by)).await.unwrap_or_else(|_| Err(no_answer()))
    }
    /// Ação no mod que espera `after` antes do clique final. Conferida a vida logo antes: a sessão que
    /// reabriu no meio do clique não recebe a ação, e o pedido segue para a limpeza.
    async fn act(&self, op: PaneOp, after: Duration) -> Result<(), ModsError> {
        if !self.alive() { return Err(pane_missing()); }
        let start_by = self.start_by(after)?;
        self.op(op, start_by).await.map(|_| ())
    }
    async fn formats(&self) -> Result<PaneFormats, ModsError> {
        match self.op(PaneOp::Formats, self.until).await? { PaneReply::Formats(f) => Ok(f), _ => Err(pane_failed("formats_shape")) }
    }
    async fn clients(&self) -> Result<usize, ModsError> {
        match self.op(PaneOp::Clients, self.until).await? { PaneReply::Clients(n) => Ok(n), _ => Err(pane_failed("clients_shape")) }
    }
    async fn raw_screen(&self) -> Result<String, ModsError> {
        match self.op(PaneOp::Screen, self.until).await? { PaneReply::Screen(s) => Ok(s), _ => Err(pane_failed("screen_shape")) }
    }
    /// Uma leitura nova: nenhuma coordenada vale de uma operação para outra (T10).
    async fn read_view(&self, titles: &[String], anchor: Option<&str>) -> Result<(Screen, PaneFormats), ModsError> {
        let f = self.formats().await?;
        let ansi = self.raw_screen().await?;
        Ok((screen::read_screen(&ansi, usize::from(f.columns), usize::from(f.rows), titles, anchor), f))
    }
    async fn read(&self, t: &Target) -> Result<(Screen, PaneFormats), ModsError> {
        self.read_view(&t.titles, t.anchor.as_deref()).await
    }
    /// Uma leitura nova com o tamanho já lido no pedido: o anel do teclado e a roda não mudam o tamanho, e
    /// cada leitura de formato é mais um processo do multiplexador (no psmux, dezenas de ms cada). Devolve
    /// também a tela crua, para a leitura seguinte saber se ela mudou.
    async fn read_known(&self, titles: &[String], anchor: Option<&str>, f: PaneFormats) -> Result<(Screen, String), ModsError> {
        let ansi = self.raw_screen().await?;
        Ok((screen::read_screen(&ansi, usize::from(f.columns), usize::from(f.rows), titles, anchor), ansi))
    }
    /// `read_known` depois de uma tecla: repete a leitura até a tela mudar em relação a `before`, até
    /// `key_settle`. Sem a espera, a captura logo depois da tecla ainda pode mostrar a tela de antes. Com
    /// `focus_after`, um `ui.focus` mais novo que ele também encerra a espera: num botão da faixa que não está
    /// desenhado a tela não muda, e o evento é o único sinal de que a tecla chegou.
    async fn read_after(&self, titles: &[String], anchor: Option<&str>, f: PaneFormats, before: &str, focus_after: Option<u64>) -> Result<(Screen, String), ModsError> {
        let deadline = Instant::now() + self.limits.key_settle;
        loop {
            let (s, ansi) = self.read_known(titles, anchor, f).await?;
            let focused = focus_after.is_some_and(|seq| self.mods.focus_seq(self.name, self.life) > seq);
            if ansi != before || focused || Instant::now() >= deadline { return Ok((s, ansi)); }
            tokio::time::sleep(self.limits.activate_poll.min(deadline.saturating_duration_since(Instant::now()))).await;
        }
    }
    /// Clique de mouse, depois do anterior no mesmo pane: `click_gap_near` sempre que o prazo comporta, e
    /// sempre a até `NEAR_CELLS` do anterior; fora disso, ao menos `click_gap`. A espera entra na conta do
    /// prazo: sem tempo para ela, a ação e a confirmação, o clique não sai.
    async fn click(&self, (row, col): (usize, usize), after: Duration) -> Result<(), ModsError> {
        let last = *self.clicked.lock().unwrap();
        let wait = |gap: Duration| last.map_or(Duration::ZERO, |(at, _)| (at + gap).saturating_duration_since(Instant::now()));
        let near = last.is_some_and(|(_, (r, c))| r.abs_diff(row) <= NEAR_CELLS && c.abs_diff(col) <= NEAR_CELLS);
        let mut pause = wait(self.limits.click_gap_near);
        if !pause.is_zero() && self.start_by(pause + after).is_err() && !near { pause = wait(self.limits.click_gap); }
        if !pause.is_zero() {
            self.start_by(pause + after)?;
            tokio::time::sleep(pause).await;
        }
        let result = self.act(PaneOp::Mouse { row: row as u16, col: col as u16 }, after).await;
        *self.clicked.lock().unwrap() = Some((Instant::now(), (row, col)));
        result
    }
    async fn wheel(&self, (row, col): (usize, usize), down: bool) -> Result<(), ModsError> {
        self.act(PaneOp::Wheel { row: row as u16, col: col as u16, down }, self.limits.scroll_wait).await
    }
    async fn keys(&self, keys: &[&str], after: Duration) -> Result<(), ModsError> {
        self.act(PaneOp::Keys(keys.iter().map(|k| (*k).to_owned()).collect()), after).await?;
        tokio::time::sleep(self.limits.key_gap).await;
        Ok(())
    }
    /// Espera a tela parar de mudar depois de um redimensionamento (70 a 200 ms no psmux), sem passar do
    /// piso do clique final.
    async fn settle(&self) {
        let budget = self.limits.settle_max.min(self.left().saturating_sub(self.limits.confirm + ACTION_MARGIN));
        let deadline = Instant::now() + budget;
        let mut last = None;
        while Instant::now() < deadline {
            tokio::time::sleep(self.limits.settle_poll).await;
            let Ok(screen) = self.raw_screen().await else { return };
            if last.as_ref() == Some(&screen) { return; }
            last = Some(screen);
        }
    }
    /// A confirmação de um clique ou tecla final, sem passar do prazo.
    fn confirm(&self) -> Duration { self.limits.confirm.min(self.left()) }
}

/// Um clique cortado pelo fim do servidor deixa a janela com a altura esticada (`TALL_ROWS`): a guarda de
/// limpeza dele não rodou. Sem terminal ligado, quem abre a sessão devolve o tamanho mínimo, e o
/// redimensionar do executor devolve junto o `window-size latest`. Com um terminal ligado, o
/// `window-size latest` já lhe deu o tamanho. `true` quando devolveu; qualquer falha deixa a janela como
/// está. Fala direto com o pane: roda antes de existir o elo e a vida no `Mods`.
pub async fn unstretch(pane: &dyn Pane, until: Instant) -> bool {
    let op = |op: PaneOp| tokio::time::timeout_at(until.into(), pane.op(op, until));
    let Ok(Ok(PaneReply::Formats(f))) = op(PaneOp::Formats).await else { return false };
    if f.rows != TALL_ROWS { return false; }
    let Ok(Ok(PaneReply::Clients(0))) = op(PaneOp::Clients).await else { return false };
    matches!(op(PaneOp::Resize { columns: f.columns.max(MIN_COLUMNS), rows: MIN_ROWS }).await, Ok(Ok(_)))
}

/// Sem terminal de verdade ligado, garante o tamanho mínimo (T9).
pub async fn floor(ctx: &Ctx<'_>) -> Result<(), ModsError> {
    floor_formats(ctx).await.map(|_| ())
}

/// `floor` que devolve os formatos de depois: sem redimensionar, os que acabou de ler.
async fn floor_formats(ctx: &Ctx<'_>) -> Result<PaneFormats, ModsError> {
    let f = ctx.formats().await?;
    if (f.columns < MIN_COLUMNS || f.rows < MIN_ROWS) && ctx.clients().await? == 0 {
        ctx.act(PaneOp::Resize { columns: f.columns.max(MIN_COLUMNS), rows: f.rows.max(MIN_ROWS) }, ctx.limits.settle_max).await?;
        ctx.settle().await;
        return ctx.formats().await;
    }
    Ok(f)
}

async fn prepare(ctx: &Ctx<'_>) -> Result<PaneFormats, ModsError> {
    let f = floor_formats(ctx).await?;
    // Em modo de rolagem o ESC do clique cancela o modo e o resto da sequência vira texto no prompt.
    if f.in_mode { return Err(terminal_in_mode()); }
    Ok(f)
}

async fn wait_screen(ctx: &Ctx<'_>, t: &Target, ok: impl Fn(&Screen) -> bool) -> Result<Option<Screen>, ModsError> {
    let deadline = Instant::now() + ctx.limits.activate_max;
    loop {
        tokio::time::sleep(ctx.limits.activate_poll).await;
        let (s, _) = ctx.read(t).await?;
        if ok(&s) { return Ok(Some(s)); }
        if Instant::now() >= deadline { return Ok(None); }
    }
}

/// Com um diálogo (ou a pesquisa) na tela, só o painel já mostrado e ao lado da conversa: o título de
/// outra aba não troca, e em caixa o painel nem é desenhado (T6; psmux, captura 700).
fn refuse_dialog_in_pane(t: &Target, s: &Screen) -> Result<(), ModsError> {
    let index = t.ids.iter().position(|id| *id == t.site);
    if (s.dialog || s.survey) && (s.placement != Some("dock") || s.active != index) { return Err(dialog_open()); }
    Ok(())
}

/// Traz a aba para a frente clicando no meio do texto do título (o vão entre abas dá o teclado ao painel,
/// (c)). `None` quando o título não está na linha das abas ((x)).
async fn activate(ctx: &Ctx<'_>, t: &Target, s: Screen) -> Result<Option<Screen>, ModsError> {
    let index = t.ids.iter().position(|id| *id == t.site).ok_or_else(pane_missing)?;
    if s.active == Some(index) { return Ok(Some(s)); }
    let (Some(tab), Some(row)) = (s.tabs.iter().find(|tab| tab.index == index), s.tab_row) else { return Ok(None) };
    ctx.click((row, (tab.start + tab.end) / 2), ctx.limits.activate_max).await?;
    wait_screen(ctx, t, |s| s.active == Some(index)).await?.map(Some).ok_or_else(no_answer)
}

async fn in_band(ctx: &Ctx<'_>, t: &Target, label: &str) -> Result<(usize, usize), ModsError> {
    let (mut s, _) = ctx.read(t).await?;
    if s.dialog || s.survey { return Err(dialog_open()); }
    if s.band_state == "collapsed" {
        // Um clique em qualquer ponto da linha expande ((ad)): no meio do texto, longe do fim.
        let row = s.collapsed_row.unwrap_or(0);
        let col = screen::find_label(&s.text, COLLAPSED_TEXT, row..row + 1, 0, None).first().map_or(2, |hit| hit.1);
        ctx.click((row, col), ctx.limits.activate_max).await?;
        s = wait_screen(ctx, t, |s| s.band_state != "collapsed").await?.ok_or_else(no_answer)?;
    }
    // Faixa encolhida (`↓ N more`) ou não desenhada: o mouse não tem onde clicar e o caso não está na reserva.
    let band = match (&s.band, s.band_state) { (Some(band), "full") => band.clone(), _ => return Err(unreachable_pane()) };
    match screen::find_in(&s, label, &band).as_slice() {
        [one] => Ok(*one),
        [] => Err(not_found(label)),
        _ => Err(ambiguous(label)),
    }
}

/// Relê a tela, confere que o rótulo continua na mesma célula e clica no meio dele; confirma pelo press do
/// plugin, sem repetir às cegas. Se o rótulo andou e continua único, confere a célula nova: coordenada
/// velha cai em célula vazia, no `[-]` ou numa opção de diálogo (achados 1 e 9). No painel, confere também
/// que a aba dele continua na frente: o mesmo rótulo na mesma célula de outra aba é outro botão.
async fn click_confirmed(ctx: &Ctx<'_>, t: &Target, b: &Button, mut cell: (usize, usize)) -> Result<(), ModsError> {
    let label = b.label.as_str();
    for _ in 0..2 {
        let (s, _) = ctx.read(t).await?;
        if t.site == BAND_SITE {
            if s.dialog || s.survey { return Err(dialog_open()); }
        } else {
            refuse_dialog_in_pane(t, &s)?;
            if s.active != t.ids.iter().position(|id| *id == t.site) { return Err(no_answer()); }
        }
        let region = if t.site == BAND_SITE { s.band.clone() } else { s.body.clone() };
        let hits = region.map(|r| screen::find_in(&s, label, &r)).unwrap_or_default();
        match hits.as_slice() {
            [one] if *one == cell => {
                let since = Instant::now();
                ctx.click(cell, Duration::ZERO).await?;
                return if ctx.mods.wait_pressed(ctx.name, ctx.life, &t.site, &b.plugin, &b.key, since, ctx.confirm()).await { Ok(()) } else { Err(no_answer()) };
            }
            [one] => cell = *one,
            [] => return Err(not_found(label)),
            _ => return Err(ambiguous(label)),
        }
    }
    Err(not_found(label))
}

/// Devolve já a altura de antes de esticar (sem terminal ligado). Só sai da limpeza quando voltou: a volta
/// que falhar (pane sem resposta, prazo) fica para o `finish`.
async fn give_back_now(ctx: &Ctx<'_>) {
    if let Some((columns, rows)) = ctx.undo.with(|p| p.height)
        && give_back(ctx, columns, rows, ctx.until).await {
        ctx.undo.with(|p| p.height = None);
    }
}

/// Com um terminal ligado no meio, o `window-size latest` já lhe entregou o tamanho: a altura não volta.
/// `true` quando a altura voltou ou não precisa voltar. Não confere a vida pelo nome: a sessão renomeada no
/// meio do clique é o mesmo pane, e numa vida que acabou o executor dela morre junto e recusa (Task 15).
async fn give_back(ctx: &Ctx<'_>, columns: u16, rows: u16, start_by: Instant) -> bool {
    match ctx.clients().await {
        Ok(0) => ctx.op(PaneOp::Resize { columns, rows }, start_by).await.is_ok(),
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Sem terminal ligado o corpo do painel acompanha a altura da janela ((u), (q)): estica, clica e deixa a
/// altura de antes para a limpeza. Sem tempo para esticar, assentar e ainda clicar, nem estica. `None`: não
/// alcançou, segue para a roda com a altura já devolvida.
async fn stretched(ctx: &Ctx<'_>, t: &Target, b: &Button, f: PaneFormats) -> Result<Option<Found>, ModsError> {
    if ctx.start_by(ctx.limits.settle_max).is_err() { return Ok(None); }
    // Antes de mandar: a altura volta mesmo se o pedido for cortado logo depois.
    ctx.undo.height(f.columns, f.rows);
    ctx.act(PaneOp::Resize { columns: f.columns, rows: TALL_ROWS }, ctx.limits.settle_max).await?;
    ctx.settle().await;
    let (s, g) = ctx.read(t).await?;
    let hits = if g.rows == f.rows { Vec::new() } else { s.body.as_ref().map(|body| screen::find_in(&s, &b.label, body)).unwrap_or_default() };
    match hits.as_slice() {
        [one] => click_confirmed(ctx, t, b, *one).await.map(|()| Some(Found::Clicked)),
        found => {
            // Nada a clicar na janela esticada: devolve antes da roda ou do teclado.
            give_back_now(ctx).await;
            Ok(if found.is_empty() { None } else { Some(Found::Keyboard) })
        }
    }
}

/// Quanto do prazo a reserva por teclado precisa para chegar a `t`, pela tela `s`: a leitura que abre o
/// anel, um passo do anel por painel até o dele e mais dois, os botões da faixa que estão no anel (nenhum com
/// a faixa recolhida, e os sem desenho mais baratos, porque o passo termina no `ui.focus`), a espera da tela
/// ao entrar no painel, o `Tab`, a espera do foco, a confirmação do `Enter` e a folga.
fn keyboard_need(ctx: &Ctx<'_>, t: &Target, s: &Screen) -> Duration {
    let index = t.ids.iter().position(|id| *id == t.site).unwrap_or(t.ids.len());
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let band = match s.band_state {
        "collapsed" => Duration::ZERO,
        "full" => ctx.limits.ring_step.saturating_mul(count(t.band_buttons)),
        _ => ctx.limits.ring_hidden_step.saturating_mul(count(t.band_buttons)),
    };
    band + ctx.limits.ring_step.saturating_mul(count(index + 3)) + ctx.limits.key_settle + ctx.limits.focus_wait + ctx.limits.confirm
        + ACTION_MARGIN
}

/// Roda com o ponteiro sobre o corpo até o rótulo aparecer. Cada evento espaçado rola pouco (uma linha no
/// tmux, três no psmux) e só em rajada acelera, sem conta previsível: um botão a dezenas de linhas não cabe
/// no prazo. A roda para a tempo de a reserva por teclado ainda caber, ou no teto dela, e passa ao teclado,
/// cujo `Tab` rola o painel sozinho até o botão. Se o teclado não cabia, a roda usa o prazo inteiro, que é
/// a única chance. Chegar às duas pontas sem o rótulo é recusa.
/// `keyboard_by`: até quando a roda pode ir e ainda sobrar o tempo do teclado (`in_pane`); `None` quando o
/// teclado não cabe e a roda fica com o prazo inteiro.
async fn roll_until(ctx: &Ctx<'_>, t: &Target, label: &str, keyboard_by: Option<Instant>) -> Result<Found, ModsError> {
    let f = ctx.formats().await?;
    let (titles, anchor) = (&t.titles, t.anchor.as_deref());
    let (s, _) = ctx.read_known(titles, anchor, f).await?;
    let body = s.body.clone().ok_or_else(unreachable_pane)?;
    let pointer = ((body.rows.0 + body.rows.1) / 2, (body.lo + body.hi) / 2);
    let (mut seq, mut last) = ctx.mods.last_scroll(ctx.name, ctx.life, &t.site);
    let mut down = true;
    let started = Instant::now();
    // Uma volta (evento, espera da rolagem, leitura e intervalo) só começa se outra do tamanho da maior até
    // aqui ainda termina antes de `keyboard_by`: a que passasse dele comeria o tempo do teclado. Antes da
    // primeira, a volta conta ao menos o intervalo dela.
    let (mut lap, mut lap_start) = (ctx.limits.wheel_gap, started);
    for _ in 0..ctx.limits.wheel_events {
        lap = lap.max(lap_start.elapsed());
        lap_start = Instant::now();
        if let Some(by) = keyboard_by && (started.elapsed() >= ctx.limits.wheel_max || Instant::now() + lap >= by) { return Ok(Found::Keyboard); }
        ctx.wheel(pointer, down).await?;
        match ctx.mods.wait_scroll(ctx.name, ctx.life, &t.site, seq, ctx.limits.scroll_wait.min(ctx.left())).await {
            Some((next, offset)) if Some(offset) != last => { seq = next; last = Some(offset); }
            other => {
                if let Some((next, _)) = other { seq = next; }
                // As duas pontas sem o rótulo (ou um painel que a roda não rola): o `Tab` do teclado rola o
                // painel até o botão, se ainda houver tempo para ele.
                if !down { return if keyboard_by.is_some() { Ok(Found::Keyboard) } else { Err(unreachable_pane()) }; }
                down = false;   // chegou ao fim: tenta para cima
                continue;
            }
        }
        let (s, _) = ctx.read_known(titles, anchor, f).await?;
        let hits = s.body.as_ref().map(|b| screen::find_in(&s, label, b)).unwrap_or_default();
        match hits.len() { 0 => {}, 1 => return Ok(Found::Cell(hits[0])), _ => return Ok(Found::Keyboard) }
        tokio::time::sleep(ctx.limits.wheel_gap).await;
    }
    Ok(Found::Keyboard)
}

async fn in_pane(ctx: &Ctx<'_>, t: &Target, b: &Button) -> Result<Found, ModsError> {
    let (s, f) = ctx.read(t).await?;
    refuse_dialog_in_pane(t, &s)?;
    if s.placement.is_none() { return Err(unreachable_pane()); }
    // Rótulo repetido na árvore, mesmo fora da tela: nenhuma ação de mouse, direto ao teclado.
    if b.repeated { return Ok(Found::Keyboard); }
    // A escolha entre o mouse (ativar a aba, esticar, rolar) e o teclado é feita aqui, antes de ativar: o
    // teclado traz a aba sozinho pelo anel, e precisa do tempo dele reservado. Se ativar a aba já não deixa
    // esse tempo, vai direto ao teclado; se o teclado não cabe nem agora, o mouse fica com o prazo inteiro.
    let need = keyboard_need(ctx, t, &s);
    let index = t.ids.iter().position(|id| *id == t.site);
    let gap = (*ctx.clicked.lock().unwrap()).map_or(Duration::ZERO, |(at, _)| (at + ctx.limits.click_gap_near).saturating_duration_since(Instant::now()));
    let activation = if s.active == index { Duration::ZERO } else { gap + ctx.limits.activate_max + ACTION_MARGIN };
    let fits = ctx.left() >= need;
    if fits && ctx.left() < need + activation { return Ok(Found::Keyboard); }
    let keyboard_by = fits.then(|| ctx.until - need);
    let Some(s) = activate(ctx, t, s).await? else { return Ok(Found::Keyboard) };
    let hits = s.body.as_ref().map(|body| screen::find_in(&s, &b.label, body)).unwrap_or_default();
    match hits.len() { 0 => {}, 1 => return Ok(Found::Cell(hits[0])), _ => return Ok(Found::Keyboard) }   // repetido no painel: teclado
    if ctx.clients().await? == 0
        && let Some(found) = stretched(ctx, t, b, f).await? {
        return Ok(found);
    }
    roll_until(ctx, t, &b.label, keyboard_by).await
}

/// Conferidas antes da primeira tecla (T5): com um diálogo qualquer tecla mexe nele, e com rascunho o
/// `Enter` que perdesse o alvo o enviaria ((y), (aa)).
fn locks(s: &Screen) -> Result<(), ModsError> {
    if s.dialog || s.survey { return Err(dialog_open()); }
    if !s.draft.is_empty() { return Err(draft_in_prompt()); }
    Ok(())
}

/// O anel do teclado lê o tamanho uma vez e guarda a última tela crua: cada passo custa a tecla e uma
/// leitura, e a leitura depois da tecla espera a tela mudar. No psmux cada operação é um processo, e reler
/// o tamanho a cada passo estourava o prazo com uma dúzia de botões na faixa.
struct Ring { f: PaneFormats, last: String, hidden_band: bool }

impl Ring {
    async fn start(ctx: &Ctx<'_>, t: &Target) -> Result<(Self, Screen), ModsError> {
        let f = ctx.formats().await?;
        let (s, last) = ctx.read_known(&t.titles, t.anchor.as_deref(), f).await?;
        Ok((Self { f, last, hidden_band: band_hidden(&s) }, s))
    }
    async fn read(&mut self, ctx: &Ctx<'_>, t: &Target) -> Result<Screen, ModsError> {
        let (s, last) = ctx.read_known(&t.titles, t.anchor.as_deref(), self.f).await?;
        self.last = last;
        Ok(s)
    }
    /// `focus`: o `focus_seq` de antes da tecla. Com a faixa sem desenho, o `ui.focus` do botão dela encerra
    /// a espera da tela.
    async fn after_key(&mut self, ctx: &Ctx<'_>, t: &Target, focus: u64) -> Result<Screen, ModsError> {
        let focus_after = self.hidden_band.then_some(focus);
        let (s, last) = ctx.read_after(&t.titles, t.anchor.as_deref(), self.f, &self.last, focus_after).await?;
        self.last = last;
        Ok(s)
    }
}

/// A faixa está no anel do `ctrl+x tab` sem aparecer na tela: encolhida (`↓ N more`) ou não desenhada. A
/// recolhida sai do anel.
fn band_hidden(s: &Screen) -> bool { !matches!(s.band_state, "full" | "collapsed") }

/// Tamanho do anel do `ctrl+x tab`: os botões das faixas, os painéis e o prompt.
fn cap(t: &Target) -> usize { t.band_buttons + t.ids.len() + 1 }

/// O foco visto é o botão pedido: no lugar, com a `key` e do mod dele, sem recusa. O plugin do Hangar
/// carregado numa sessão viva antes de o foco levar o mod não o conta: com a `key` em mais de um mod no
/// lugar não há como saber de qual é o foco, e o clique é recusado, como antes de o pedido levar o mod.
fn is_target(seen: Option<&FocusSeen>, site: &str, b: &Button) -> Result<bool, ModsError> {
    let Some(seen) = seen.filter(|seen| seen.request_id == site && !seen.denied && seen.element.as_deref() == Some(b.key.as_str()))
        else { return Ok(false) };
    match seen.plugin.as_deref() {
        Some(plugin) => Ok(plugin == b.plugin),
        None if b.shared => Err(missing()),
        None => Ok(true),
    }
}

/// O último foco do alvo depois de `after`: espera até `wait` pelo primeiro e pega também os que chegaram
/// depois dele. Um evento atrasado de uma tecla anterior não passa pelo da tecla mais nova.
async fn last_focus(ctx: &Ctx<'_>, attempt: &str, after: u64, wait: Duration) -> Option<FocusSeen> {
    let mut last = ctx.mods.wait_focus(ctx.name, ctx.life, attempt, after, wait, |_| true).await?;
    while let Some(next) = ctx.mods.wait_focus(ctx.name, ctx.life, attempt, last.seq, Duration::ZERO, |_| true).await {
        last = next;
    }
    Some(last)
}

/// `ctrl+x tab` até o painel pedido estar na frente e com o teclado. O anel passa antes pelos botões das
/// faixas ((t)); se a borda apagar depois de ter passado por painéis, ele não está no ciclo. Cada tecla
/// só sai com tempo para a espera do foco e o `Enter` confirmado. Devolve o `focus_seq` de logo antes do
/// `ctrl+x tab` que deu o teclado ao painel.
/// Recebe o tamanho e a tela crua da leitura anterior (`Ring`): cada passo é a tecla e uma leitura só, sem
/// reler o tamanho.
async fn reach_pane(ctx: &Ctx<'_>, t: &Target, ring: &mut Ring) -> Result<u64, ModsError> {
    let index = t.ids.iter().position(|id| *id == t.site).ok_or_else(pane_missing)?;
    let mut passed = false;
    for _ in 0..cap(t) {
        let seq = ctx.mods.focus_seq(ctx.name, ctx.life);
        ctx.keys(&["C-x", "Tab"], ctx.limits.focus_wait).await?;
        let s = ring.after_key(ctx, t, seq).await?;
        if s.dialog || s.survey { return Err(dialog_open()); }
        match s.focus {
            Some("pane") => { passed = true; if s.active == Some(index) { return Ok(seq); } }
            Some("prompt") if passed => break,
            _ => {}
        }
    }
    Err(unreachable_pane())
}

/// `ctrl+x tab` até o `ui.focus` da faixa trazer a `key` pedida: o hook do plugin reescreve no primeiro
/// evento do mod do alvo, e a reescrita não atravessa de um mod para outro ((t)). Cada botão da faixa dá
/// um `ui.focus`: sem ele no prazo, recusa, porque o evento atrasado seria achado na tecla seguinte, com o
/// foco já adiante. Devolve o `seq` do foco confirmado.
async fn reach_band_key(ctx: &Ctx<'_>, t: &Target, b: &Button, attempt: &str, ring: &mut Ring) -> Result<u64, ModsError> {
    for _ in 0..cap(t) {
        let seq = ctx.mods.focus_seq(ctx.name, ctx.life);
        ctx.keys(&["C-x", "Tab"], ctx.limits.focus_wait).await?;
        let seen = last_focus(ctx, attempt, seq, ctx.limits.focus_wait.min(ctx.left())).await;
        let s = ring.after_key(ctx, t, seq).await?;
        if s.dialog || s.survey { return Err(dialog_open()); }
        match seen {
            Some(seen) if is_target(Some(&seen), BAND_SITE, b)? => return Ok(seen.seq),
            None if s.focus == Some("band") => return Err(no_answer()),
            _ => {}
        }
        if s.focus != Some("band") { break; }   // passou da faixa sem o botão
    }
    Err(unreachable_pane())
}

/// Lê a tela imediatamente antes do `Enter`: um diálogo que chegou tomaria o `Enter` como aprovação, e uma
/// letra no meio teria devolvido o teclado ao prompt ((y), (aa)). O teclado tem de estar na faixa, para
/// botão da faixa, ou no painel pedido e na frente; e nenhum foco mais novo que o confirmado (`seq`) pode
/// ter levado o teclado a outro elemento.
async fn enter_confirmed(ctx: &Ctx<'_>, t: &Target, b: &Button, attempt: &str, seq: u64, ring: &mut Ring) -> Result<(), ModsError> {
    let s = ring.read(ctx, t).await?;
    if s.dialog || s.survey { return Err(dialog_open()); }
    let placed = if t.site == BAND_SITE { s.focus == Some("band") }
        else { s.focus == Some("pane") && s.active.is_some() && s.active == t.ids.iter().position(|id| *id == t.site) };
    if !placed { return Err(no_answer()); }
    let newer = last_focus(ctx, attempt, seq, Duration::ZERO).await;
    if newer.is_some() && !is_target(newer.as_ref(), &t.site, b)? {
        return Err(no_answer());
    }
    let since = Instant::now();
    ctx.keys(&["Enter"], Duration::ZERO).await?;
    if ctx.mods.wait_pressed(ctx.name, ctx.life, &t.site, &b.plugin, &b.key, since, ctx.confirm()).await { Ok(()) } else { Err(no_answer()) }
}

/// Clique pelo teclado (T5), só nos casos medidos e com as travas; uma tecla por operação, com pausa.
/// Desarmar o alvo e voltar ao prompt ficam com a limpeza, registrados antes da primeira tecla.
async fn reserve_press(ctx: &Ctx<'_>, t: &Target, b: &Button) -> Result<Value, ModsError> {
    let (mut ring, s) = Ring::start(ctx, t).await?;
    locks(&s)?;
    let attempt = ctx.mods.arm_focus(ctx.name, ctx.life, &t.site, Some(&b.plugin), &b.key);
    ctx.undo.focus(&attempt);
    ctx.undo.keyboard(t.titles.clone(), t.anchor.clone(), cap(t));
    let seq = if t.site == BAND_SITE {
        reach_band_key(ctx, t, b, &attempt, &mut ring).await?
    } else {
        let entry = reach_pane(ctx, t, &mut ring).await?;
        // O hook pode reescrever já no `ctrl+x tab` que deu o teclado ao painel: com o alvo focado, o `Tab`
        // o tiraria de lá, e o evento atrasado desse `ctrl+x tab` ainda passaria na conferência. A espera é a
        // da tela (`key_settle`), não a do foco: nas provas a reescrita veio no `Tab`, e esperar o foco inteiro
        // custava meio segundo a cada entrada no painel. Um evento que chegue depois do `Tab` não faz sair o
        // `Enter`: vale o último foco visto.
        let mut seen = last_focus(ctx, &attempt, entry, ctx.limits.key_settle.min(ctx.left())).await;
        if !is_target(seen.as_ref(), &t.site, b)? {
            let seq = ctx.mods.focus_seq(ctx.name, ctx.life);
            ctx.keys(&["Tab"], ctx.limits.focus_wait).await?;
            seen = last_focus(ctx, &attempt, seq, ctx.limits.focus_wait.min(ctx.left())).await;
        }
        if !is_target(seen.as_ref(), &t.site, b)? { return Err(no_answer()); }
        seen.ok_or_else(no_answer)?.seq
    };
    enter_confirmed(ctx, t, b, &attempt, seq, &mut ring).await?;
    Ok(json!({}))
}

/// Fechar pelo teclado: o painel com o teclado e `ctrl+x x` ((m)), confirmado pelo `ui.close`. A volta ao
/// prompt fica com a limpeza.
async fn reserve_close(ctx: &Ctx<'_>, t: &Target) -> Result<Value, ModsError> {
    let (mut ring, s) = Ring::start(ctx, t).await?;
    locks(&s)?;
    ctx.undo.keyboard(t.titles.clone(), t.anchor.clone(), cap(t));
    reach_pane(ctx, t, &mut ring).await?;
    ctx.keys(&["C-x", "x"], Duration::ZERO).await?;
    if ctx.mods.wait_pane_gone(ctx.name, ctx.life, &t.site, ctx.confirm()).await { Ok(json!({})) } else { Err(no_answer()) }
}

async fn press_inner(ctx: &Ctx<'_>, site: &str, plugin: &str, key: &str) -> Result<Value, ModsError> {
    let t = target(ctx, site)?;
    let b = button(&t, plugin, key)?;
    let f = prepare(ctx).await?;
    // Sem tela cheia o clique enviado é ignorado (achado 8): vai pelo teclado.
    if !f.mouse { return reserve_press(ctx, &t, &b).await; }
    if site == BAND_SITE {
        // Rótulo repetido na faixa, mesmo fora da tela: nenhuma ação de mouse, direto ao teclado.
        if b.repeated { return reserve_press(ctx, &t, &b).await; }
        let cell = in_band(ctx, &t, &b.label).await?;
        click_confirmed(ctx, &t, &b, cell).await?;
        return Ok(json!({}));
    }
    match in_pane(ctx, &t, &b).await? {
        Found::Cell(cell) => click_confirmed(ctx, &t, &b, cell).await?,
        Found::Keyboard => return reserve_press(ctx, &t, &b).await,
        Found::Clicked => {}
    }
    Ok(json!({}))
}

/// Clique num botão de mod pedido pelo app (T4).
pub async fn press(ctx: &Ctx<'_>, site: &str, plugin: &str, key: &str) -> Result<Value, ModsError> {
    let result = press_inner(ctx, site, plugin, key).await;
    // A aba da frente pode ter mudado sem redesenho (troca para painel já desenhado, (s)).
    ctx.mods.schedule_shown_in(ctx.name, ctx.life);
    result
}

/// Fechar: ativa a aba e clica na célula exata do `✕`, nunca por busca do glifo ((b)). Os outros
/// fechamentos que o mod faz em seguida não são erro ((e)).
pub async fn close(ctx: &Ctx<'_>, site: &str) -> Result<Value, ModsError> {
    let result: Result<Value, ModsError> = async {
        let t = target(ctx, site)?;
        let f = prepare(ctx).await?;
        if !f.mouse { return reserve_close(ctx, &t).await; }
        let (s, _) = ctx.read(&t).await?;
        refuse_dialog_in_pane(&t, &s)?;
        if s.placement.is_none() { return Err(unreachable_pane()); }
        if activate(ctx, &t, s).await?.is_none() { return reserve_close(ctx, &t).await; }
        // Relê logo antes do clique, como o `click_confirmed`: com outra aba na frente, o `✕` da mesma
        // célula fecharia outro painel.
        let (s, _) = ctx.read(&t).await?;
        refuse_dialog_in_pane(&t, &s)?;
        if s.active != t.ids.iter().position(|id| id == site) { return Err(no_answer()); }
        let cell = s.close.ok_or_else(|| not_found("✕"))?;
        ctx.click(cell, Duration::ZERO).await?;
        if ctx.mods.wait_pane_gone(ctx.name, ctx.life, site, ctx.confirm()).await { Ok(json!({})) } else { Err(no_answer()) }
    }.await;
    ctx.mods.schedule_shown_in(ctx.name, ctx.life);
    result
}

/// Troca de aba pedida pelo app (T2): o clique no título, sozinho. A reserva por teclado não deixa o painel
/// pedido na frente (a volta por `ctrl+x tab` passa pelos seguintes, (z)): título fora da linha é recusa.
pub async fn show(ctx: &Ctx<'_>, site: &str) -> Result<Value, ModsError> {
    let t = target(ctx, site)?;
    let f = prepare(ctx).await?;
    if !f.mouse { return Err(mouse_off()); }
    let (s, _) = ctx.read(&t).await?;
    if s.dialog || s.survey { return Err(dialog_open()); }
    activate(ctx, &t, s).await?.ok_or_else(unreachable_pane)?;
    ctx.mods.set_screen_shown(ctx.name, ctx.life, Some(site.to_owned()));
    Ok(json!({"shown_id": site}))
}

/// O pedido do app ao clique com terminal.
pub async fn dispatch(ctx: &Ctx<'_>, call: ModsCall) -> Result<Value, ModsError> {
    match call {
        ModsCall::Press { site, plugin, key } => press(ctx, &site, &plugin, &key).await,
        ModsCall::Close { site } => close(ctx, &site).await,
        ModsCall::Show { site } => show(ctx, &site).await,
        // Com terminal não há por onde digitar no campo do mod (fora do escopo desta entrega).
        ModsCall::Input { .. } => Err(no_typing()),
    }
}

/// O painel que a linha de abas mostra na frente; `None` sem linha de abas na tela.
pub async fn read_shown(ctx: &Ctx<'_>) -> Option<String> {
    let view = ctx.mods.terminal_view_in(ctx.name, ctx.life)?;
    if view.panes.is_empty() { return None; }
    let t = target_of(&view, "", Value::Null);
    let (s, _) = ctx.read(&t).await.ok()?;
    s.active.and_then(|index| view.panes.get(index)).map(|p| p.id.clone())
}

/// Volta por `ctrl+x tab` até a borda apagar e nada ficar em inverso na faixa; nunca por `Escape`. Com um
/// diálogo na tela qualquer tecla mexe nele: para ali. `true` com o teclado no prompt. Como o `give_back`,
/// não confere a vida pelo nome: quem garante que a tecla não chega à vida nova é o executor, que morre com
/// a vida dele.
async fn back_to_prompt(ctx: &Ctx<'_>, back: &Back) -> bool {
    let (titles, anchor) = (&back.titles, back.anchor.as_deref());
    let Ok(f) = ctx.formats().await else { return false };
    let Ok((mut s, mut last)) = ctx.read_known(titles, anchor, f).await else { return false };
    for _ in 0..=back.cap {
        if s.focus == Some("prompt") && !s.dialog && !s.survey { return true; }
        if s.dialog || s.survey { return false; }
        if ctx.op(PaneOp::Keys(vec!["C-x".into(), "Tab".into()]), ctx.until).await.is_err() { return false; }
        tokio::time::sleep(ctx.limits.key_gap).await;
        let Ok(read) = ctx.read_after(titles, anchor, f, &last, None).await else { return false };
        (s, last) = read;
    }
    s.focus == Some("prompt") && !s.dialog && !s.survey
}

/// Prazo de uma volta ao prompt: um passo do anel (`ring_step`) por parada dele e mais um, entre `UNDO_MAX`
/// e `BACK_MAX`. Com doze botões na faixa e dez painéis o anel tem 23 paradas, e no psmux os 2 s de antes
/// cobriam cinco passos: a limpeza desistia com o teclado num painel (prova na VM, 06/10/2026).
fn back_budget(limits: &Limits, back: &Back) -> Duration {
    let steps = u32::try_from(back.cap + 2).unwrap_or(u32::MAX);
    limits.ring_step.saturating_mul(steps).clamp(UNDO_MAX, BACK_MAX)
}

/// Renova a reserva do pane por `cover` mais a folga. O executor troca a reserva anterior por esta.
async fn renew(ctx: &Ctx<'_>, cover: Duration) {
    let millis = u64::try_from((cover + HOLD_MARGIN).as_millis()).unwrap_or(u64::MAX);
    let _ = ctx.op(PaneOp::Hold { millis }, ctx.until).await;
}

/// A volta ao prompt falhou com o pane reservado: soltar agora deixaria a fila entregar uma mensagem com o
/// teclado num painel, e o `Enter` dela apertaria um botão. Tenta de novo até `keep_held`, renovando a
/// reserva antes de cada tentativa; `false` quando o teto passou sem voltar.
async fn keep_trying(ctx: &Ctx<'_>, back: &Back) -> bool {
    let keep_until = Instant::now() + ctx.limits.keep_held;
    let budget = back_budget(ctx.limits, back);
    while Instant::now() < keep_until {
        renew(&ctx.fresh(), ctx.limits.retry_gap + budget).await;
        tokio::time::sleep(ctx.limits.retry_gap).await;
        if back_to_prompt(&ctx.within(budget), back).await { return true; }
    }
    false
}

/// Desfaz o que o pedido deixou, com prazo próprio (`UNDO_MAX`, e a volta ao prompt com o dela), fora do de
/// quem pediu: renova a reserva do
/// pane para cobrir a limpeza, desarma o alvo do foco, volta o teclado ao prompt, devolve a altura (só sem
/// terminal ligado) e solta o pane. Roda depois da resposta, também com o pedido cortado. Cada item sai do
/// `Undo` depois da sua volta: se a tarefa sumir no meio da limpeza, a guarda refaz só o que faltou (todas
/// as voltas podem repetir sem efeito novo). Com a volta ao prompt falhando até o teto (`keep_held`), o
/// pane não é solto: a reserva vence sozinha no executor.
pub async fn finish(ctx: &Ctx<'_>) {
    let undo = ctx.undo;
    let held = undo.with(|p| p.hold);
    let back = undo.with(|p| p.keyboard.clone());
    let budget = back.as_ref().map_or(UNDO_MAX, |back| back_budget(ctx.limits, back));
    // A reserva do pedido conta do começo dele e pode vencer no meio da limpeza.
    if held { renew(&ctx.fresh(), budget).await; }
    if let Some(attempt) = undo.with(|p| p.focus.clone()) {
        ctx.mods.disarm_focus(ctx.name, ctx.life, &attempt);
        undo.with(|p| p.focus = None);
    }
    let mut back_ok = true;
    if let Some(back) = back {
        back_ok = back_to_prompt(&ctx.within(budget), &back).await || (held && keep_trying(ctx, &back).await);
        if !back_ok {
            tracing::warn!(session = ctx.name, code = "mods_keyboard_return", held,
                "o teclado não voltou ao prompt depois da reserva; reservado, o pane fica assim até a reserva vencer");
        }
        undo.with(|p| p.keyboard = None);
    }
    if let Some((columns, rows)) = undo.with(|p| p.height) {
        let clean = ctx.fresh();
        // A reserva renovada no começo cobria a volta ao prompt: a devolução da altura ganha a dela.
        if held { renew(&clean, UNDO_MAX).await; }
        give_back(&clean, columns, rows, clean.until).await;
        undo.with(|p| p.height = None);
    }
    if held {
        if back_ok {
            let clean = ctx.fresh();
            let _ = clean.op(PaneOp::Release, clean.until).await;
        }
        undo.with(|p| p.hold = false);
    }
}

/// O que o clique precisa possuir para rodar numa tarefa própria.
#[derive(Clone)]
pub struct Parts {
    pub name: String,
    pub pane: Arc<dyn Pane>,
    pub mods: Mods,
    pub limits: Limits,
    /// Um pedido por vez no pane, contando a limpeza do anterior: a vez da rota solta antes dela.
    pub busy: Arc<tokio::sync::Mutex<()>>,
    /// A vida da sessão a que o elo pertence (`Ctx::life`).
    pub life: u64,
    /// O último clique de mouse no pane, entre um pedido e o seguinte (`Ctx::clicked`).
    pub clicked: Arc<Mutex<LastClick>>,
}

impl Parts {
    /// O contexto de um pedido neste pane, com o prazo `until` e o pendente `undo`.
    pub fn ctx<'a>(&'a self, until: Instant, undo: &'a Undo) -> Ctx<'a> {
        Ctx { name: &self.name, pane: self.pane.as_ref(), mods: &self.mods, limits: &self.limits, until, undo, life: self.life,
            clicked: &self.clicked }
    }
}

/// Desfaz o que ficou quando a tarefa do clique some sem chegar ao fim (pânico, servidor encerrando,
/// `abort`): o `Drop` passa o pendente e a vez do pane a uma tarefa nova, que fala com o executor.
struct UndoOnDrop { parts: Parts, undo: Arc<Undo>, busy: Option<tokio::sync::OwnedMutexGuard<()>> }

impl Drop for UndoOnDrop {
    fn drop(&mut self) {
        let busy = self.busy.take();
        if self.undo.is_empty() { return; }
        let (parts, undo) = (self.parts.clone(), self.undo.clone());
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _busy = busy;
                finish(&parts.ctx(Instant::now(), &undo)).await;
            });
        }
    }
}

/// Reserva o pane no executor pelo resto do prazo mais a limpeza: a fila não entrega mensagem no meio do
/// clique (o `Enter` dela apertaria um botão com o teclado num painel) nem muda a tela entre a leitura e o
/// clique (C6).
async fn hold(ctx: &Ctx<'_>) -> Result<(), ModsError> {
    ctx.undo.set_hold();
    let millis = u64::try_from((ctx.left() + UNDO_MAX).as_millis()).unwrap_or(u64::MAX);
    ctx.op(PaneOp::Hold { millis }, ctx.until).await.map(|_| ())
}

/// Atende o pedido do app numa tarefa própria. A resposta sai pelo canal assim que se sabe o resultado; a
/// limpeza vem depois, ainda com a vez do pane. Quem pediu pode desistir (a rota corta no fim do orçamento):
/// a tarefa não começa ação nova sem tempo para ela, e a limpeza roda mesmo assim.
pub fn spawn(parts: Parts, call: ModsCall, until: Instant) -> (tokio::task::JoinHandle<()>, oneshot::Receiver<Result<Value, ModsError>>) {
    let (answer, reply) = oneshot::channel();
    let task = tokio::spawn(async move {
        let Ok(busy) = tokio::time::timeout_at(until.into(), parts.busy.clone().lock_owned()).await else {
            let _ = answer.send(Err(no_answer()));
            return;
        };
        let undo = Arc::new(Undo::default());
        let _guard = UndoOnDrop { parts: parts.clone(), undo: undo.clone(), busy: Some(busy) };
        let ctx = parts.ctx(until, &undo);
        let result = match hold(&ctx).await {
            Ok(()) => dispatch(&ctx, call).await,
            Err(error) => Err(error),
        };
        let _ = answer.send(result);
        finish(&ctx).await;
    });
    (task, reply)
}
