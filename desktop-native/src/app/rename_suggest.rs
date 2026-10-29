//! Faixa "Renomear para X?" da sessão de nome genérico: o servidor gera o nome, a lista o traz, um clique renomeia.
use super::*;
use super::sidebar::Target;

#[derive(Default)]
pub(super) struct RenameSuggest {
    /// Já pedida nesta vida do app: a sugestão fica no servidor e volta pela linha da lista.
    asked: HashSet<Target>,
    /// "Agora não" clicado: some na hora, antes de a lista refletir a recusa.
    pub(super) dismissed: HashSet<Target>,
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
    /// Pede a sugestão da aberta uma vez por sessão nesta vida do app. 204 e falha ficam calados: ninguém pediu nada.
    pub(super) fn request_rename_suggestion(&mut self) {
        let Some(target) = self.selected_target() else { return };
        if self.rename_suggest.asked.contains(&target) { return; }
        let Some(session) = self.target_session(&target) else { return };
        if session.read_only() || session.rename_suggestion.is_some()
            || !looks_generic(&session.name, session.cwd.as_deref().unwrap_or_default()) { return; }
        if !self.chat.events.iter().any(|event| event.kind == "user_msg" && !event.queued()) { return; }
        let Some(api) = self.machine_api(&target.server) else { return };
        self.rename_suggest.asked.insert(target.clone());
        self.runtime.spawn(async move {
            // O executor do ditado gera o nome; o prazo cobre a fila dele.
            let _ = api.server_send(reqwest::Method::POST, &["sessions", target.name.as_str(), "rename-suggestion"], None, 120).await;
        });
    }

    /// Faixa da aberta, enquanto a linha traz a sugestão e ninguém disse "Agora não".
    pub(super) fn render_rename_suggestion(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.selected_target()?;
        if self.rename_suggest.dismissed.contains(&target) { return None; }
        let session = self.target_session(&target)?;
        let suggestion = session.rename_suggestion.clone().filter(|s| *s != session.name && !session.read_only())?;
        let renaming = self.is_renaming(&target);
        let (dismiss, apply) = (target.clone(), target);
        let name = suggestion.clone();
        Some(rename_strip(&suggestion, renaming,
            |button| button.on_click(cx.listener(move |this, _, _, cx| this.dismiss_rename_suggestion(dismiss.clone(), cx))),
            |button| button.on_click(cx.listener(move |this, _, _, cx| this.rename_session_to(apply.clone(), name.clone(), cx))))
            .into_any_element())
    }
}

/// A faixa por dado simples: a aberta liga o clique nos botões (`dismiss` e `apply` recebem cada um), a miniatura das
/// novidades (`crate::news`) usa a mesma sem clique.
pub(crate) fn rename_strip(suggestion: &str, renaming: bool, dismiss: impl FnOnce(Button) -> Button, apply: impl FnOnce(Button) -> Button)
    -> Stateful<Div> {
    let title = tr_shared("rename_suggest_title", &[("name", suggestion)]);
    div().id("rename-suggest").py_2().flex().items_center().gap_2().border_t_1().border_color(theme::border())
        .role(Role::Group).aria_label(title.clone())
        .child(div().flex_1().min_w_0().text_sm().truncate().child(title))
        .child(dismiss(Button::new("rename-suggest-dismiss").small().ghost().label(tr_shared("rename_suggest_dismiss", &[])).disabled(renaming)))
        .child(apply(Button::new("rename-suggest-apply").small().primary().label(tr_shared("rename_suggest_apply", &[]))
            .loading(renaming).disabled(renaming)))
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
