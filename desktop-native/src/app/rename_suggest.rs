//! Faixa "Renomear para X?" da sessão de nome genérico: o servidor gera o nome, a lista o traz, um clique renomeia.
use super::*;
use super::sidebar::Target;

/// A sessão numa vida dela: outra sessão aberta depois com o mesmo nome não herda o pedido nem a recusa.
pub(super) type Life = (Target, Option<String>);

#[derive(Default)]
pub(super) struct RenameSuggest {
    /// Quantos pedidos seus a conversa tinha no último pedido desta vida do app: a sugestão fica no servidor e volta pela
    /// linha da lista, e só a fraca é pedida de novo, quando chega pedido seu depois dela.
    asked: HashMap<Life, usize>,
    /// "Agora não" clicado: some na hora, antes de a lista refletir a recusa.
    pub(super) dismissed: HashSet<Life>,
    /// "Outra" esperando o servidor.
    pub(super) again: HashSet<Target>,
}

/// Mesmo teto de nome do servidor (`session_open/resolve.rs`).
const NAME_MAX: usize = 64;

/// Nome que o Hangar inventou pela pasta: `<pasta>`, `<pasta>-N`, `sessao` ou `sessao-N`. Só filtra o pedido; quem decide é
/// o servidor, que responde 409 ao nome escolhido por alguém.
pub(super) fn looks_generic(name: &str, cwd: &str) -> bool {
    let folder = cwd.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or_default();
    let base = Some(sanitize(folder)).filter(|b| !b.is_empty()).unwrap_or_else(|| "sessao".into());
    let fit = |base: &str, suffix: &str| format!("{}{suffix}", base[..base.len().min(NAME_MAX - suffix.len())].trim_end_matches('-'));
    let from = |base: &str| name == fit(base, "") || name.rsplit_once('-').is_some_and(|(_, n)|
        (1..8).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()) && name == fit(base, &format!("-{n}")));
    from(&base) || from("sessao")
}

/// `sanitize_session_name` do servidor.
/// shortcut: sem NFKD a letra acentuada some em vez de virar a base ("ação" → "ao"); pasta acentuada não ganha sugestão
/// até o app ter normalização Unicode.
fn sanitize(name: &str) -> String {
    let clean = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if name.chars().all(clean) && !name.starts_with('-') && !name.ends_with('-') { return name.to_owned(); }
    let ascii: String = name.chars().filter(char::is_ascii).collect();
    ascii.trim().chars().map(|c| if clean(c) { c } else { '-' }).collect::<String>().trim_matches('-').to_owned()
}

impl Hangar {
    pub(super) fn rename_life(&self, target: &Target) -> Life {
        // Só a conversa: o `lifecycle_id` chega depois do plugin e trocaria a chave no meio da vida.
        (target.clone(), self.target_session(target).and_then(|s| s.jsonl.clone()))
    }

    /// Pede a sugestão da aberta uma vez por sessão nesta vida do app, e a fraca de novo a cada pedido seu. 204 e falha
    /// ficam calados: ninguém pediu nada.
    pub(super) fn request_rename_suggestion(&mut self) {
        let Some(target) = self.selected_target() else { return };
        let Some(session) = self.target_session(&target) else { return };
        if session.read_only() || (session.rename_suggestion.is_some() && !session.rename_weak)
            || !looks_generic(&session.name, session.cwd.as_deref().unwrap_or_default()) { return; }
        let asked = self.chat.events.iter().filter(|event| event.kind == "user_msg" && !event.queued()).count();
        if asked == 0 { return; }
        let life = self.rename_life(&target);
        if life.1.is_none() { return; }
        if self.rename_suggest.asked.get(&life).is_some_and(|&last| !session.rename_weak || asked <= last) { return; }
        let Some(api) = self.machine_api(&target.server) else { return };
        self.rename_suggest.asked.insert(life, asked);
        self.runtime.spawn(async move {
            // O executor do ditado gera o nome; o prazo cobre a fila dele.
            let _ = api.server_send(reqwest::Method::POST, &["sessions", target.name.as_str(), "rename-suggestion"], None, 120).await;
        });
    }

    /// Faixa da aberta, enquanto a linha traz a sugestão e ninguém disse "Agora não".
    pub(super) fn render_rename_suggestion(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.selected_target()?;
        if self.rename_suggest.dismissed.contains(&self.rename_life(&target)) { return None; }
        let session = self.target_session(&target)?;
        let suggestion = session.rename_suggestion.clone().filter(|s| *s != session.name && !session.read_only())?;
        let state = Strip { weak: session.rename_weak, renaming: self.is_renaming(&target), again: self.rename_suggest.again.contains(&target) };
        let (dismiss, again, apply) = (target.clone(), target.clone(), target.clone());
        let name = suggestion.clone();
        // Conversa nova: o nome é um botão que abre o renomear com ele, para mudar antes de aceitar.
        if crate::appearance::conversation_beta() {
            let edit_name = suggestion.clone();
            return Some(beta_rename_strip(&suggestion, state,
                |el| el.on_click(cx.listener(move |this, _, window, cx| this.start_session_rename_with(target.clone(), Some(edit_name.clone()), window, cx))),
                |button| button.on_click(cx.listener(move |this, _, _, cx| this.dismiss_rename_suggestion(dismiss.clone(), cx))),
                |button| popup::anchor(div(), crate::tour::anchors::RENAME_APPLY)
                    .child(button.on_click(cx.listener(move |this, _, _, cx| this.rename_session_to(apply.clone(), name.clone(), cx)))), cx)
                .into_any_element());
        }
        Some(rename_strip(&suggestion, state,
            |button| button.on_click(cx.listener(move |this, _, _, cx| this.dismiss_rename_suggestion(dismiss.clone(), cx))),
            |button| button.on_click(cx.listener(move |this, _, _, cx| this.another_rename_suggestion(again.clone(), cx))),
            // Só a faixa de verdade leva a âncora da dica; a miniatura das novidades não.
            |button| popup::anchor(div(), crate::tour::anchors::RENAME_APPLY)
                .child(button.on_click(cx.listener(move |this, _, _, cx| this.rename_session_to(apply.clone(), name.clone(), cx)))))
            .into_any_element())
    }
}

/// O que a faixa mostra além do nome: o aviso de pouco contexto e quem está esperando o servidor.
#[derive(Clone, Copy, Default)]
pub(crate) struct Strip { pub weak: bool, pub renaming: bool, pub again: bool }

/// A faixa por dado simples: a aberta liga o clique nos botões (`dismiss`, `again` e `apply` recebem cada um), a
/// miniatura das novidades (`crate::news`) usa a mesma sem clique.
pub(crate) fn rename_strip<E: IntoElement>(suggestion: &str, state: Strip, dismiss: impl FnOnce(Button) -> Button,
    again: impl FnOnce(Button) -> Button, apply: impl FnOnce(Button) -> E) -> Stateful<Div> {
    let title = tr_shared("rename_suggest_title", &[("name", suggestion)]);
    let busy = state.renaming || state.again;
    let actions = [
        again(Button::new("rename-suggest-again").small().ghost().label(tr_shared("rename_suggest_again", &[]))
            .loading(state.again).disabled(busy)).into_any_element(),
        dismiss(Button::new("rename-suggest-dismiss").small().ghost().label(tr_shared("rename_suggest_dismiss", &[])).disabled(busy)).into_any_element(),
        apply(Button::new("rename-suggest-apply").small().primary().label(tr_shared("rename_suggest_apply", &[]))
            .loading(state.renaming).disabled(busy)).into_any_element(),
    ];
    div().id("rename-suggest").py_2().flex().items_center().gap_2().border_t_1().border_color(theme::border())
        .role(Role::Group).aria_label(title.clone())
        .child(div().flex_1().min_w_0().flex().flex_col().child(div().text_sm().truncate().child(title))
            .when(state.weak, |el| el.child(div().text_xs().text_color(theme::muted()).truncate().child(tr_shared("rename_suggest_weak", &[])))))
        .children(actions)
}

/// A sugestão de nome da conversa nova: cartão com filete de destaque, a frase com o nome num botão tracejado que abre o
/// renomear (`edit`), a linha de baixo e as ações logo depois do texto. Sem "Outra": a fraca se refaz na próxima mensagem.
pub(crate) fn beta_rename_strip<E: IntoElement>(suggestion: &str, state: Strip, edit: impl FnOnce(Stateful<Div>) -> Stateful<Div>,
    dismiss: impl FnOnce(Button) -> Button, apply: impl FnOnce(Button) -> E, cx: &App) -> Stateful<Div> {
    let title = tr_shared("rename_suggest_title", &[("name", suggestion)]);
    let busy = state.renaming || state.again;
    // Botões do mockup: "Agora não" com borda e "Renomear" na cor de destaque; a borda mora num filho porque a variante
    // própria do kit não desenha borda.
    let button = |id: &'static str, label: String, primary: bool| {
        let (fill, ink) = if primary { (theme::accent(), theme::on_accent()) } else { (transparent_black(), theme::text()) };
        Button::new(id).custom(ButtonCustomVariant::new(cx).color(fill).foreground(ink).hover(if primary { fill.opacity(0.88) } else { theme::hover() }).active(theme::hover()))
            .h(px(32.)).p_0().rounded(px(8.))
            .child(div().h_full().px(px(12.)).rounded(px(8.)).flex().items_center().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).text_color(ink)
                .when(!primary, |el| el.border_1().border_color(theme::border_strong())).child(label.clone()))
            .accessibility_label(label)
    };
    let name = edit(div().id("rename-suggest-edit").flex_none().flex().items_center().gap(px(6.)).px(px(6.)).py(px(1.)).ml(px(2.))
        .rounded(px(6.)).border_1().border_dashed().border_color(theme::accent().alpha(0.45)).text_color(theme::accent_text()).cursor_pointer()
        .hover(|el| el.bg(theme::accent_dim()))
        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(tr("beta_rename_edit")).build(window, cx))
        .child(div().font_family(theme::MONO).text_size(px(13.5)).child(suggestion.to_string()))
        .child(chrome::small_icon(IconName::Pencil, 14., theme::accent_text())));
    let sub = if state.weak { format!("{} {}", tr("rename_beta_weak"), tr("beta_rename_click")) } else { tr("beta_rename_click") };
    div().id("rename-suggest").my_2().w_full().relative().overflow_hidden().rounded(px(12.)).border_1().border_color(theme::border_strong())
        .bg(theme::raised()).pl(px(19.)).pr(px(12.)).py(px(11.)).flex().items_center().gap(px(14.))
        .role(Role::Group).aria_label(title)
        .child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(theme::accent()))
        .child(div().flex_initial().min_w_0().mr(px(16.)).flex().flex_col()
            .child(div().flex().items_center().flex_wrap().gap(px(4.)).text_size(px(14.5)).font_weight(FontWeight::MEDIUM)
                .child(div().text_color(theme::muted()).child(tr("rename_beta_label")))
                .child(div().text_color(theme::text()).child(tr("beta_rename_to")))
                .child(name))
            .child(div().text_size(px(13.)).text_color(theme::muted()).truncate().child(sub)))
        .child(div().flex_none().flex().items_center().gap(px(8.))
            .child(dismiss(button("rename-suggest-dismiss", tr_shared("rename_suggest_dismiss", &[]), false).disabled(busy)))
            .child(apply(button("rename-suggest-apply", tr_shared("rename_suggest_apply", &[]), true).loading(state.renaming).disabled(busy))))
}

#[cfg(test)]
mod tests {
    use super::looks_generic;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn generic_names_are_the_ones_the_folder_gave() {
        let cwd = "/home/x/Projetos/hangar";
        assert!(looks_generic("hangar", cwd));
        assert!(looks_generic("hangar-2", cwd));
        assert!(looks_generic("sessao-3", cwd));
        assert!(looks_generic("sessao", "/"));
        assert!(!looks_generic("ABC-123-Coordenadora", cwd));
        assert!(!looks_generic("etiquetas-maquina-conta", cwd));
        assert!(!looks_generic("hangar-x", cwd));
    }
}
