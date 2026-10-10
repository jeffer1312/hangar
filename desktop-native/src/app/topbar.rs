//! Barra do app acima de tudo, igual em qualquer tela: no meio o campo "Buscar conversas" que abre a paleta (Ctrl+K),
//! à direita o botão de Custos e a engrenagem das Configurações. A barra vazia arrasta a janela e o
//! duplo clique maximiza, como a barra de título do Zeron e do Zed: a janela não tem decoração no Linux. No Windows a
//! barra do sistema some e os botões dela são desenhados aqui; no macOS fica a do sistema.
//!
//! Colados, ela é a barra de título do Zeron: sem linha embaixo e o conteúdo um pouco abaixo do meio. Com a barra
//! lateral à esquerda, a lateral sobe até o topo e esta começa na borda dela, com a cor do chat; nas abas e nas páginas
//! vai de ponta a ponta com o material da lateral. Soltos, é a faixa do web: sem fundo, o papel de parede passa por
//! trás, só uma linha fina embaixo, e os painéis flutuam abaixo dela com a margem deles.
use super::*;
use super::costs::web;

/// Colada, a altura e o respiro de cima da barra de título do Zeron.
const TOPBAR_HEIGHT: f32 = 38.;
const TOPBAR_TOP_PAD: f32 = 4.;
/// Solta, a altura da faixa de abas do web.
const TOPBAR_FLOATING_HEIGHT: f32 = 44.;
/// A cota da pílula da conta é relida de tempos em tempos, além de ao conectar.
const ACCOUNT_EVERY: Duration = Duration::from_secs(300);

#[derive(Default)]
pub(super) struct TopBar {
    /// Só o relógio mais novo relê: reconectar não empilha relógios.
    account_tick: u64,
    /// Botão apertado na barra vazia: o próximo movimento com ele apertado passa o arrasto ao compositor.
    should_move: bool,
}

/// Controle dentro da barra: o apertar dele não chega à barra, que senão arrastaria a janela.
fn control(el: impl IntoElement) -> Div {
    div().flex_shrink_0().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(el)
}

/// Controle que cede espaço na janela estreita (busca, conta): encolhe e corta o texto em vez de cobrir o vizinho.
fn shrinking(el: impl IntoElement) -> Div {
    div().min_w_0().flex_shrink(1.).flex().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(el)
}

/// Minimizar, maximizar e fechar do Windows, desenhados na barra do app. A área de cada um é marcada para o sistema, que
/// cuida do clique e do menu de encaixe do Windows 11; por isso não há `on_click` aqui, nem `stop_propagation`: apertar
/// consumido pelo GPUI nunca vira clique de botão do sistema.
fn window_buttons(window: &Window, floating: bool) -> Div {
    let button = |id: &'static str, icon: IconName, area: WindowControlArea, close: bool| div().id(id)
        .w(px(46.)).h_full().flex().items_center().justify_center().text_color(theme::muted())
        .hover(move |el| if close { el.bg(gpui::rgb(0xc42b1c)).text_color(gpui::white()) } else { el.bg(theme::hover()).text_color(theme::text()) })
        // Sem tapar a barra, o teste de área do GPUI acha primeiro o Drag dela (pintada antes) e o sistema recebe barra
        // de título em vez de botão.
        .occlude()
        .window_control_area(area)
        .child(Icon::new(icon).size(px(14.)));
    let max = if window.is_maximized() { IconName::WindowRestore } else { IconName::WindowMaximize };
    // Encostados no canto da janela, como os do sistema: desfazem o respiro da barra à direita e em cima.
    div().flex_shrink_0().self_stretch().flex().ml(px(4.))
        .mr(px(if floating { -8. } else { -6. })).mt(px(if floating { 0. } else { -TOPBAR_TOP_PAD }))
        .child(button("window-min", IconName::WindowMinimize, WindowControlArea::Min, false))
        .child(button("window-max", max, WindowControlArea::Max, false))
        .child(button("window-close", IconName::WindowClose, WindowControlArea::Close, true))
}

/// Altura da barra, para a view guardada que a desenha.
pub(super) fn height() -> f32 { if theme::is_floating() { TOPBAR_FLOATING_HEIGHT } else { TOPBAR_HEIGHT } }

impl Hangar {
    pub(super) fn schedule_account_refresh(&mut self, cx: &mut Context<Self>) {
        self.topbar.account_tick += 1;
        let tick = self.topbar.account_tick;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ACCOUNT_EVERY).await;
            let _ = this.update(cx, |this, cx| if this.topbar.account_tick == tick {
                this.refresh_default_account(cx);
                this.schedule_account_refresh(cx);
            });
        }).detach();
    }

    /// `beside`: ao lado da barra lateral, com a largura do painel direito aberto (0 fechado); a busca fica no meio do chat.
    pub(super) fn render_topbar(&mut self, beside: Option<f32>, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let floating = theme::is_floating();
        let online = self.api.is_some();
        let settings_open = self.settings.is_some() && !self.settings_live();
        let search = Button::new("topbar-search")
            .custom(ButtonCustomVariant::new(cx).color(theme::inset()).foreground(theme::muted()).hover(theme::hover()).active(theme::hover()))
            .w(px(420.)).max_w_full().min_w_0().h(px(26.)).px(px(10.)).rounded(px(8.)).border_1().border_color(theme::border()).disabled(!online)
            .accessibility_label(web("lista_buscar"))
            .child(div().w_full().flex().items_center().gap(px(8.))
                .child(chrome::small_icon(IconName::Search, 14., theme::faint()))
                .child(div().flex_1().min_w_0().truncate().text_left().text_size(px(13.)).text_color(theme::faint()).child(format!("{}…", web("lista_buscar"))))
                .child(chrome::kbd("Ctrl K")))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_search(window, cx)));
        let pill = Button::new("topbar-cost").custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted())
                .hover(theme::hover()).active(theme::hover()))
            .icon(chrome::small_icon(IconName::ChartColumn, 16., theme::muted())).size(px(28.)).rounded(px(6.))
            .selected(self.costs.view.is_some()).disabled(!online)
            .accessibility_label(web("nav_custos")).tooltip_with_action(web("nav_custos"), &OpenCosts, None)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_costs(window, cx)));
        let worktrees = Button::new("topbar-worktrees").custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted())
                .hover(theme::hover()).active(theme::hover()))
            .icon(chrome::small_icon(IconName::GitBranch, 16., theme::muted())).size(px(28.)).rounded(px(6.))
            .selected(self.worktrees.view.is_some()).disabled(!online)
            .accessibility_label(tr_shared("worktrees_titulo", &[])).tooltip_with_action(tr_shared("worktrees_titulo", &[]), &OpenWorktrees, None)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_worktrees(window, cx)));
        // A conta da sessão em foco, como a pílula de cota do web: glifo, anel e "44% 5h · nome"; clique abre o cartão de contas.
        let proxy = self.has_proxy_session();
        let account = self.focused_account().or_else(|| proxy.then(|| ("codex".into(), tr("create_proxy_choose_account"), None)))
            .map(|(kind, name, window)| {
            let quota = self.focused_proxy_quota();
            let quota = if quota.is_empty() { tr("no_data") } else { quota };
            let label = if proxy { format!("{name} · {quota}") } else { match &window {
                Some((label, pct)) => format!("{}% {label} · {name}", pct.round()),
                None => format!("{} · {name}", tr("no_data")),
            } };
            Button::new("topbar-account").ghost().small().selected(self.accounts.card && self.accounts.card_top).disabled(!online)
                .h(px(26.)).px(px(8.)).rounded_full().max_w(px(280.)).min_w_0()
                .child(div().min_w_0().flex().items_center().gap(px(6.)).text_size(px(12.5))
                    .child(chrome::provider_glyph(&kind, 14.))
                    .child(chrome::ring(window.as_ref().map(|w| w.1)))
                    .when(proxy, |el| el.child(div().min_w_0().truncate().text_color(theme::muted()).child(name))
                        .child(div().flex_shrink_0().text_color(theme::muted()).child(quota)))
                    .when(!proxy, |el| el.child(div().min_w_0().truncate().text_color(theme::muted()).child(label.clone()))))
                .accessibility_label(format!("{}: {label}", tr("ring_account")))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_top_usage_card(cx)))
        });
        let gear = Button::new("topbar-settings").custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted())
                .hover(theme::hover()).active(theme::hover()))
            .icon(chrome::small_icon(IconName::Settings, 16., theme::muted())).size(px(28.)).rounded(px(6.)).selected(settings_open)
            .accessibility_label(tr("settings")).tooltip_with_action(tr("settings_open"), &OpenSettings, None)
            .on_click(cx.listener(|this, _, window, cx| {
                if this.settings.is_some() && !this.settings_live() { this.close_settings(window, cx) }
                else { this.open_settings(settings::Page::Appearance, window, cx) }
            }));
        let voice = self.render_voice_pill(cx);
        let presence = self.render_presence_button(cx);
        let updater = cx.try_global::<crate::update::Handle>().map(|handle| handle.0.clone());
        let outdated = updater.as_ref().filter(|u| u.read(cx).server_outdated()).map(|u| {
            let (running, app) = u.read(cx).outdated_versions();
            let tip = tr("app_server_outdated_tip").replace("{server}", &self.server_label(cx)).replace("{running}", &running).replace("{app}", &app);
            Button::new("topbar-server-outdated").ghost().small().h(px(26.)).px(px(10.)).rounded_full().border_1().border_color(theme::warning())
                .child(div().flex().items_center().gap(px(6.)).text_size(px(12.5))
                    .child(Icon::new(IconName::TriangleAlert).size(px(14.)).text_color(theme::warning()))
                    .child(tr("app_server_outdated")))
                .accessibility_label(tip.clone()).tooltip(tip)
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(settings::Page::Servers, window, cx)))
        });
        // Colada, a barra continua a lateral que está embaixo dela: a de conversas tem superfície própria.
        let page_open = settings_open || self.costs.view.is_some() || self.worktrees.view.is_some();
        let wall = if !page_open && appearance::get().navigation == appearance::Navigation::Conversations { theme::conversation_sidebar().0 }
            else { theme::chrome() };
        let bar = div().id("topbar").w_full().flex_shrink_0().flex().items_center().gap(px(8.))
            .map(|el| if floating { el.h(px(TOPBAR_FLOATING_HEIGHT)).px(px(8.)).border_b_1().border_color(theme::border_strong()) }
                else { el.h(px(TOPBAR_HEIGHT)).pt(px(TOPBAR_TOP_PAD)).pl(px(10.)).pr(px(6.)) })
            // Ao lado da lateral, sem fundo próprio: o que está atrás é o do chat.
            .when(!floating && beside.is_none(), |el| el.bg(wall))
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.topbar.should_move = true;
                // No Windows o arrasto da barra é o laço modal do sistema, que engole o soltar: uma seleção de texto
                // começada aqui ficaria presa e o ponteiro solto seguiria selecionando o chat.
                gpui_kit::component::GlobalState::suppress_text_selection(cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.topbar.should_move = false))
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.topbar.should_move = false))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, _| {
                // Só com o botão de fato apertado: um apertar antigo não pode arrastar a janela com o ponteiro solto.
                if this.topbar.should_move && event.pressed_button == Some(MouseButton::Left) {
                    this.topbar.should_move = false;
                    window.start_window_move();
                }
            }))
            // No Windows a barra é legenda do sistema, que já alterna maximizar no duplo clique.
            .on_click(|event, window, _| if event.click_count() == 2 && !cfg!(target_os = "windows") {
                if cfg!(target_os = "macos") { window.titlebar_double_click() } else { window.zoom_window() }
            })
            .map(|el| {
                let controls = div().flex().justify_end().gap(px(6.))
                    .children(account.map(|account| shrinking(popup::anchor(div().min_w_0(), "topbar-account").child(account))))
                    .children(presence.map(control))
                    .child(control(Button::new("topbar-orq-history").ghost().icon(IconName::Clock).size(px(28.)).disabled(!online)
                        .tooltip(tr_shared("orq_history_title", &[])).accessibility_label(tr_shared("orq_history_title", &[]))
                        .on_click(cx.listener(|this, _, window, cx| this.open_orq_history(window, cx)))))
                    .child(control(worktrees))
                    .child(control(pill))
                    .children(outdated.map(control))
                    .children(updater.map(control))
                    .children(voice.map(control))
                    .child(control(gear));
                match beside {
                    // Com o painel direito aberto, a busca centra no chat e os controles ficam sobre o painel; mais largos que
                    // ele, invadem o vazio do chat sem empurrar a busca.
                    Some(side) if side > 0. => el.child(div().flex_1().min_w_0().flex().child(div().flex_1()).child(shrinking(search).min_w(px(140.))).child(div().flex_1()))
                        .child(controls.flex_shrink_0().w(px(side))),
                    _ => el.child(div().flex_1()).child(shrinking(search).min_w(px(140.))).child(controls.flex_1().min_w_0()),
                }
            })
            .when(cfg!(target_os = "windows"), |el| el.child(window_buttons(window, floating)));
        if floating || beside.is_some() { bar.into_any_element() } else { chrome::glass_panel(bar, px(0.)) }
    }
}
