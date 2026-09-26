//! Saúde dos harnesses: uma linha por CLI, o que o app instalou nele e o botão do conserto que o servidor já tem.
//! Os textos dos itens são as tabelas de códigos do web (`harness_*`), lidas por `tr_web`.
use super::*;
use super::chrome::Skeleton;
use super::server_config::chip;
use super::settings::{Disclosure, Page, settings_box};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
struct Item {
    id: String,
    ok: Option<bool>,
    codigo: String,
    #[serde(default)]
    params: HashMap<String, String>,
    conserto: Option<String>,
    #[serde(default)]
    info: bool,
}

#[derive(Clone, Deserialize)]
struct Cli { id: String, nome: String, instalado: bool, versao: Option<String>, itens: Vec<Item> }

#[derive(Deserialize)]
struct Repaired { feito: String, harnesses: Vec<Cli> }

/// Conserto em curso ou o desfecho dele, preso ao item (CLI, item) onde o botão estava.
struct Repair { cli: String, item: String, started: Instant, outcome: Option<Result<String, String>> }

#[derive(Default)]
pub(in crate::app) struct Harnesses {
    /// A última lista lida; uma releitura que falha não a apaga (o erro aparece em cima dela).
    list: Option<Vec<Cli>>,
    loading: bool,
    load_seq: u64,
    error: Option<String>,
    repair: Option<Repair>,
    repair_seq: u64,
    why: Vec<(String, String)>,
    _clock: Option<Task<()>>,
}

pub(super) enum HarnessReply {
    Loaded(u64, Result<Value, Failure>),
    Repaired(u64, Result<Value, Failure>),
}

/// Código do servidor → frase do web; código que o app não conhece aparece cru em vez de sumir.
fn item_text(item: &Item) -> String {
    let key = match item.codigo.as_str() {
        "skills_ok" if item.params.contains_key("origem") => "skills_origem".to_owned(),
        "extensoes_outra_fonte" if item.params.contains_key("faltam") => "extensoes_outra_fonte_e_faltam".to_owned(),
        code => code.to_owned(),
    };
    crate::i18n::tr_web(&format!("harness_{key}"), &item.params).unwrap_or_else(|| item.codigo.clone())
}

fn item_label(id: &str) -> String {
    let key = match id {
        "bloco" => "tmux_bloco", "default_terminal" => "tmux_term", "truecolor" => "tmux_truecolor",
        "titulo" => "tmux_titulo", "mouse" => "tmux_mouse", "persistencia" => "tmux_persist", other => other,
    };
    crate::i18n::tr_web(&format!("harness_item_{key}"), &HashMap::new()).unwrap_or_else(|| id.to_owned())
}

/// "por quê?" declarado por CLI: o mesmo id em outro card fala de outra coisa (os hooks do Codex são os do usuário).
fn explained(item: &Item, cli: &str) -> bool {
    match item.id.as_str() {
        "credenciais" => ["codex", "pi", "omp", "kimi"].contains(&cli) && item.conserto.as_deref().is_some_and(|c| c.starts_with("sync:")),
        "extensoes" => ["pi", "omp"].contains(&cli),
        "skills" => ["pi", "kimi"].contains(&cli),
        "hooks" => ["claude", "kimi"].contains(&cli),
        "contas" => cli == "claude",
        _ => false,
    }
}

fn web(key: &str) -> String { crate::i18n::tr_web(key, &HashMap::new()).unwrap_or_else(|| key.to_owned()) }

impl Hangar {
    fn harness_send_later(&self) -> impl Fn(HarnessReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Harness(reply) }).await; })
        }
    }

    /// Conserto em curso sobrevive a sair e voltar: a resposta dele ainda é desta conexão.
    pub(super) fn harness_opened(&mut self, cx: &mut Context<Self>) {
        let h = &mut self.harness;
        (h.error, h.why) = (None, Vec::new());
        if h.repair.as_ref().is_some_and(|r| r.outcome.is_some()) { h.repair = None; }
        self.load_harness(cx);
    }

    fn load_harness(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.harness.load_seq += 1;
        let seq = self.harness.load_seq;
        self.harness.loading = true;
        let done = self.harness_send_later();
        // `--version` de cinco CLIs em série no servidor.
        self.runtime.spawn(async move { done(HarnessReply::Loaded(seq, api.server_read(&["harness"], &[], 30).await)).await });
        cx.notify();
    }

    fn repair_harness(&mut self, id: String, cli: String, item: String, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none()) { return; }
        self.harness.repair_seq += 1;
        let seq = self.harness.repair_seq;
        (self.harness.repair, self.harness.error) = (Some(Repair { cli, item, started: Instant::now(), outcome: None }), None);
        self.harness_clock(cx);
        let done = self.harness_send_later();
        self.runtime.spawn(async move {
            done(HarnessReply::Repaired(seq, api.server_post(&["harness", "conserto", &id], 120).await)).await
        });
        cx.notify();
    }

    fn harness_clock(&mut self, cx: &mut Context<Self>) {
        self.harness._clock = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let running = this.update(cx, |this, cx| {
                cx.notify();
                this.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none())
            });
            if !running.unwrap_or(false) { break; }
        }));
    }

    pub(super) fn receive_harness(&mut self, reply: HarnessReply, cx: &mut Context<Self>) {
        match reply {
            HarnessReply::Loaded(seq, result) => {
                if seq != self.harness.load_seq { return; }
                self.harness.loading = false;
                match result.map_err(|error| Self::fetch_failure(&error))
                    .and_then(|value| serde_json::from_value::<Vec<Cli>>(value).map_err(|_| tr("invalid_response"))) {
                    Ok(list) => (self.harness.list, self.harness.error) = (Some(list), None),
                    Err(error) => self.harness.error = Some(error),
                }
            }
            HarnessReply::Repaired(seq, result) => {
                if seq != self.harness.repair_seq { return; }
                let parsed = result.map_err(|error| Self::fetch_failure(&error))
                    .and_then(|value| serde_json::from_value::<Repaired>(value).map_err(|_| tr("invalid_response")));
                let outcome = match parsed {
                    Ok(done) => {
                        // A lista do conserto é mais nova que qualquer leitura em voo.
                        self.harness.load_seq += 1;
                        (self.harness.list, self.harness.loading) = (Some(done.harnesses), false);
                        Ok(done.feito)
                    }
                    Err(error) => {
                        // O conserto pode ter rodado antes da falha: a lista relida mostra o estado real.
                        self.load_harness(cx);
                        Err(error)
                    }
                };
                if let Some(repair) = &mut self.harness.repair { repair.outcome = Some(outcome); }
            }
        }
        cx.notify();
    }

    fn render_harness_item(&self, cli: &str, item: &Item, cx: &mut Context<Self>) -> Div {
        let running = self.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none());
        let here = self.harness.repair.as_ref().filter(|r| r.cli == cli && r.item == item.id);
        let (glyph, color) = match (item.info, item.ok) {
            (true, _) => ("·", theme::muted()),
            (false, Some(true)) => ("✓", theme::success()),
            (false, Some(false)) => ("✕", theme::danger()),
            (false, None) => ("?", theme::muted()),
        };
        let label = item_label(&item.id);
        let row = div().flex().items_start().gap_2().py(px(3.))
            .child(div().w(px(14.)).flex_shrink_0().text_sm().font_weight(FontWeight::BOLD).text_color(color).child(glyph))
            // Um texto só, rótulo em negrito: quebra por palavra dentro da coluna, como o `<b>` do web.
            .child(div().flex_1().min_w_0().text_sm().whitespace_normal().text_color(theme::muted())
                .child(StyledText::new(format!("{label}  {}", item_text(item))).with_highlights([(0..label.len(),
                    HighlightStyle { color: Some(theme::text()), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() })])))
            .when_some(item.conserto.clone(), |el, id| {
                let text = if id.starts_with("sync:") { "harness_sync" } else if item.ok == Some(false) { "harness_fix" } else { "harness_redo" };
                let (cli, item_id) = (cli.to_owned(), item.id.clone());
                el.child(Button::new(SharedString::from(format!("harness-{cli}-{item_id}-fix"))).outline().small().flex_shrink_0()
                    .label(tr(text)).loading(here.is_some_and(|r| r.outcome.is_none())).disabled(running)
                    .on_click(cx.listener(move |this, _, _, cx| this.repair_harness(id.clone(), cli.clone(), item_id.clone(), cx))))
            });
        let key = (cli.to_owned(), item.id.clone());
        let open = self.harness.why.contains(&key);
        let this = cx.entity().downgrade();
        let why = explained(item, cli).then(|| div().pl(px(22.)).flex().flex_col().gap(px(4.))
            .child(div().flex().items_center().gap(px(6.)).text_size(px(12.5))
                .child(div().text_color(theme::muted()).child(web(&format!("harness_item_{}_vered", item.id))))
                .child(Disclosure::new(format!("harness-{cli}-{}-why", item.id), open, tr("accounts_engine_why"), true)
                    .name(tr("accounts_engine_why_of").replace("{name}", &label))
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| {
                        this.harness.why.retain(|k| *k != key);
                        if open { this.harness.why.push(key.clone()); }
                        cx.notify();
                    }); })))
            .when(open, |el| el.child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
                .child(web(&format!("harness_item_{}_porque", item.id))))));
        div().flex().flex_col().child(row).children(why).children(here.map(|r| repair_line(r, &label)))
    }

    pub(super) fn render_harness(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let running = self.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none());
        let title = div().flex().items_center().gap_2()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Harnesses.title()))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()))
            .child(div().flex_1())
            .when(self.api.is_some(), |el| el.child(Button::new("harness-reload").ghost().small().icon(IconName::RefreshCw)
                .label(tr("reload")).loading(self.harness.loading).disabled(self.harness.loading || running)
                .on_click(cx.listener(|this, _, _, cx| this.load_harness(cx)))));
        let mut page = div().flex().flex_col().gap_4().child(title)
            .child(self.mark(div().rounded(px(6.)).text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr("harness_legend")), "harness_legend"));
        if self.api.is_none() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).into_any_element();
        }
        if let Some(error) = &self.harness.error {
            page = page.child(div().id("harness-load-error").role(Role::Alert).text_sm().text_color(theme::danger())
                .whitespace_normal().child(error.clone()));
        }
        let Some(list) = &self.harness.list else {
            if self.harness.loading {
                return page.child(div().id("harness-loading").role(Role::Status).aria_label(tr("loading")).flex().flex_col().gap_2()
                    .children((0..4usize).map(|i| settings_box().id(("harness-skeleton", i)).p_3().gap_2()
                        .child(div().flex().items_center().gap_2().child(Skeleton::new(("harness-glyph", i)).size(px(22.)))
                            .child(Skeleton::new(("harness-name", i)).w(px(120.)).h(px(12.))))
                        .child(Skeleton::new(("harness-item", i)).w(px(320.)).h(px(10.))))))
                    .into_any_element();
            }
            return page.child(Button::new("harness-retry").outline().small().label(tr("server_retry"))
                .on_click(cx.listener(|this, _, _, cx| this.load_harness(cx)))).into_any_element();
        };
        if list.is_empty() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("harness_empty"))).into_any_element();
        }
        let cards: Vec<Div> = list.iter().map(|h| {
            let bad = h.instalado && h.itens.iter().any(|i| i.ok == Some(false));
            let dot = if !h.instalado { theme::faint() } else if bad { theme::danger() } else { theme::success() };
            let glyph = if h.id == "tmux" {
                div().size(px(22.)).flex_shrink_0().rounded(px(4.)).bg(theme::raised()).flex().items_center().justify_center()
                    .text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(theme::muted()).child("⌗")
            } else { chrome::provider_glyph(&h.id, 22.) };
            let version = if h.instalado { h.versao.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| tr("harness_installed")) }
                else { tr("harness_not_installed") };
            // Desfecho de um item que sumiu depois do conserto (deixou de se aplicar): fica no card.
            let orphan = self.harness.repair.as_ref().filter(|r| r.cli == h.id && r.outcome.is_some() && !h.itens.iter().any(|i| i.id == r.item));
            settings_box().p_3().gap_1().when(!h.instalado, |el| el.opacity(0.7))
                .child(div().flex().items_center().gap_2().pb(px(4.))
                    .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(dot))
                    .child(glyph)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(h.nome.clone()))
                    .child(div().min_w_0().truncate().text_sm().text_color(theme::muted()).font_family(theme::MONO).child(version)))
                .children(h.itens.iter().map(|item| self.render_harness_item(&h.id, item, cx)))
                .children(orphan.map(|r| repair_line(r, &item_label(&r.item))))
        }).collect();
        page.child(div().flex().flex_col().gap_3().children(cards)).into_any_element()
    }
}

/// "Consertando X… Ns" enquanto roda; depois, o que o servidor fez ou o motivo da falha.
fn repair_line(repair: &Repair, label: &str) -> Stateful<Div> {
    let line = div().pl(px(22.)).pb(px(2.)).text_size(px(12.5)).whitespace_normal();
    match &repair.outcome {
        None => line.id("harness-repairing").role(Role::Status).text_color(theme::muted())
            .child(tr("harness_fixing").replace("{item}", label).replace("{s}", &repair.started.elapsed().as_secs().to_string())),
        Some(Ok(done)) => line.id("harness-repaired").role(Role::Status).text_color(theme::success()).child(done.clone()),
        Some(Err(error)) => line.id("harness-repair-error").role(Role::Alert).text_color(theme::danger()).child(error.clone()),
    }
}
