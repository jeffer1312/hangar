//! Escolhas da tela 1 e o estado de cada etapa da lateral (feito, fazendo, precisa de você, pendente), derivados das marcas.
//! Nada aqui toca disco nem rede.
use super::marks::{End, ItemRow, Progress, State, Step};
use super::system::AGENTS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Screen { Welcome, Prepare, Install, Tailscale, Phone, Done }

const ALL: [Screen; 6] = [Screen::Welcome, Screen::Prepare, Screen::Install, Screen::Tailscale, Screen::Phone, Screen::Done];

/// A tela 4 só aparece com "também fora de casa" (spec "As seis telas").
pub(crate) fn screens(outside: bool) -> Vec<Screen> { ALL.into_iter().filter(|s| outside || *s != Screen::Tailscale).collect() }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status { Pending, Doing, NeedsYou, Done }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhoneOutcome { Connected, Skipped }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PasswordMode { Keep, Generate, Choose }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PasswordProblem { Short, Forbidden, Mismatch }

pub(crate) const MIN_PASSWORD: usize = 8;

/// O que vai no `HANGAR_TOKEN`. `None`: com `Keep` o `.env` já tem a senha; com `Generate` o instalador gera uma de 48
/// caracteres quando a variável falta (contrato do plano 1), e o app a lê do `.env` no fim.
pub(crate) fn phone_password(mode: &PasswordMode, typed: &str, confirm: &str) -> Result<Option<String>, PasswordProblem> {
    match mode {
        PasswordMode::Keep | PasswordMode::Generate => Ok(None),
        PasswordMode::Choose if typed.chars().count() < MIN_PASSWORD => Err(PasswordProblem::Short),
        PasswordMode::Choose if forbidden(typed) => Err(PasswordProblem::Forbidden),
        PasswordMode::Choose if typed != confirm => Err(PasswordProblem::Mismatch),
        PasswordMode::Choose => Ok(Some(typed.to_owned())),
    }
}

/// Mesma regra dos scripts: ASCII imprimível, sem os caracteres que quebram o `.env`, sem espaço (o login recusa).
fn forbidden(pw: &str) -> bool {
    !pw.chars().all(|c| (' '..='~').contains(&c))
        || pw.contains(['#', '$', '\'', '"', '\\', ' '])
        || pw == "change-me"
}

/// Claude Code marcado como padrão e os já instalados marcados (spec, tela 1), na ordem do instalador.
pub(crate) fn default_agents(installed: &[&str]) -> Vec<&'static str> {
    AGENTS.iter().map(|(id, _)| *id).filter(|id| *id == "claude" || installed.contains(id)).collect()
}

pub(crate) fn toggle_agent(chosen: &[&'static str], id: &'static str) -> Vec<&'static str> {
    let on = !chosen.contains(&id);
    AGENTS.iter().map(|(a, _)| *a).filter(|a| if *a == id { on } else { chosen.contains(a) }).collect()
}

/// As duas execuções do script: a conferência (`--check`) e a instalação.
#[derive(Default)]
pub(crate) struct Runs { pub check: Option<Progress>, pub install: Option<Progress> }

impl Runs {
    pub(crate) fn step(&self, step: Step) -> Option<State> { self.install.as_ref().and_then(|p| p.step(step)) }

    /// A lista da conferência, atualizada pela instalação pelo mesmo id; o que só a instalação tem entra no fim.
    pub(crate) fn items(&self, step: Step) -> Vec<ItemRow> {
        let mut rows: Vec<ItemRow> = self.check.as_ref().map(|p| p.items(step).cloned().collect()).unwrap_or_default();
        for row in self.install.iter().flat_map(|p| p.items(step)) {
            match rows.iter_mut().find(|r| r.id == row.id) { Some(slot) => *slot = row.clone(), None => rows.push(row.clone()) }
        }
        rows
    }

    pub(crate) fn end(&self) -> Option<End> { self.install.as_ref()?.end }

    /// A execução que importa agora: a instalação, quando já começou.
    pub(crate) fn latest(&self) -> Option<&Progress> { self.install.as_ref().or(self.check.as_ref()) }
}

pub(crate) struct View<'a> {
    pub started: bool,
    pub precheck_blocked: bool,
    pub runs: &'a Runs,
    /// A tela onde a falha mostrada aconteceu.
    pub failed_at: Option<Screen>,
    pub phone: Option<PhoneOutcome>,
}

fn from_state(state: Option<State>) -> Status {
    match state {
        None | Some(State::Queued) => Status::Pending,
        Some(State::Doing) => Status::Doing,
        Some(State::Ok) => Status::Done,
        Some(State::Pending | State::Failed) => Status::NeedsYou,
    }
}

pub(crate) fn status(screen: Screen, v: &View) -> Status {
    if v.failed_at == Some(screen) { return Status::NeedsYou; }
    match screen {
        Screen::Welcome => if v.started { Status::Done } else { Status::Doing },
        // A conferência sozinha não termina a tela 2: quem a fecha é o `preparar ok` da instalação.
        Screen::Prepare => match v.runs.step(Step::Prepare) {
            Some(state) => from_state(Some(state)),
            None if v.precheck_blocked => Status::NeedsYou,
            None if v.started => Status::Doing,
            None => Status::Pending,
        },
        Screen::Install => from_state(v.runs.step(Step::Install)),
        Screen::Tailscale => from_state(v.runs.step(Step::Tailscale)),
        // O backend não registra a entrada do celular (spec, tela 5): só a pessoa fecha este passo.
        Screen::Phone => match (v.phone, v.runs.end()) {
            (Some(_), _) => Status::Done,
            (None, Some(End::Ok | End::Pending)) => Status::Doing,
            _ => Status::Pending,
        },
        Screen::Done => match (v.runs.end(), v.phone) {
            (Some(End::Ok), Some(_)) => Status::Done,
            (Some(End::Pending), Some(_)) => Status::NeedsYou,
            _ => Status::Pending,
        },
    }
}

/// A primeira etapa ainda não feita: a que anda ou a que espera a pessoa.
pub(crate) fn frontier(list: &[Screen], v: &View) -> Screen {
    list.iter().copied().find(|s| status(*s, v) != Status::Done).unwrap_or(Screen::Done)
}

/// A tela acompanha o andamento enquanto a pessoa olha a fronteira anterior; quem voltou para ver outra fica onde está.
pub(crate) fn follow(viewing: Screen, before: Screen, after: Screen) -> Screen { if viewing == before { after } else { viewing } }

fn index(list: &[Screen], screen: Screen) -> usize { list.iter().position(|s| *s == screen).unwrap_or(0) }

/// Até a fronteira dá para ir pela lateral; depois do fim, a qualquer uma.
pub(crate) fn reachable(list: &[Screen], screen: Screen, v: &View) -> bool {
    index(list, screen) <= index(list, frontier(list, v)) || v.runs.end().is_some()
}

pub(crate) fn next(list: &[Screen], screen: Screen) -> Screen { list.get(index(list, screen) + 1).copied().unwrap_or(screen) }
pub(crate) fn prev(list: &[Screen], screen: Screen) -> Screen { list.get(index(list, screen).saturating_sub(1)).copied().unwrap_or(screen) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Primary { Start, Continue, PhoneConnected, OpenHangar }

pub(crate) fn primary(screen: Screen, started: bool) -> Primary {
    match screen {
        Screen::Welcome if !started => Primary::Start,
        Screen::Phone => Primary::PhoneConnected,
        Screen::Done => Primary::OpenHangar,
        _ => Primary::Continue,
    }
}

pub(crate) fn primary_enabled(screen: Screen, v: &View, can_start: bool) -> bool {
    let finished = matches!(v.runs.end(), Some(End::Ok | End::Pending));
    match screen {
        Screen::Welcome => v.started || can_start,
        Screen::Prepare | Screen::Install => status(screen, v) == Status::Done,
        Screen::Tailscale => status(screen, v) == Status::Done || finished,
        Screen::Phone | Screen::Done => finished,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(lines: &[&str]) -> Progress { let mut p = Progress::default(); for l in lines { p.feed(l); } p }

    fn view(runs: &Runs) -> View<'_> { View { started: true, precheck_blocked: false, runs, failed_at: None, phone: None } }

    #[test]
    fn tailscale_screen_only_when_outside() {
        assert_eq!(screens(false), vec![Screen::Welcome, Screen::Prepare, Screen::Install, Screen::Phone, Screen::Done]);
        assert_eq!(screens(true).len(), 6);
    }

    #[test]
    fn phone_password_rules() {
        assert_eq!(phone_password(&PasswordMode::Keep, "", ""), Ok(None));
        // Gerar fica com o instalador: a variável não vai, e ele grava uma de 48 caracteres.
        assert_eq!(phone_password(&PasswordMode::Generate, "", ""), Ok(None));
        assert_eq!(phone_password(&PasswordMode::Choose, "1234567", "1234567"), Err(PasswordProblem::Short));
        assert_eq!(phone_password(&PasswordMode::Choose, "12345678", "12345679"), Err(PasswordProblem::Mismatch));
        assert_eq!(phone_password(&PasswordMode::Choose, "12345678", "12345678"), Ok(Some("12345678".into())));
        assert_eq!(phone_password(&PasswordMode::Choose, "pass+word&1", "pass+word&1"), Ok(Some("pass+word&1".into())));
        // Fora do ASCII imprimível o instalador recusa; o app avisa antes.
        let forbidden = ["çãõéíóúâ", "abcd#efgh", "abcd$efgh", "abcd'efgh", "abcd\"efgh", "abcd\\efgh", " abcdefgh", "abcdefgh ", "pass word 1", "\tabcdefgh", "abcdefgh\t", "change-me"];
        for pw in forbidden {
            assert_eq!(phone_password(&PasswordMode::Choose, pw, pw), Err(PasswordProblem::Forbidden), "{pw:?}");
        }
    }

    #[test]
    fn agents_default_to_claude_plus_installed_and_keep_order() {
        assert_eq!(default_agents(&[]), vec!["claude"]);
        assert_eq!(default_agents(&["kimi", "codex"]), vec!["claude", "codex", "kimi"]);
        assert_eq!(toggle_agent(&["claude", "kimi"], "codex"), vec!["claude", "codex", "kimi"]);
        assert_eq!(toggle_agent(&["claude"], "claude"), Vec::<&str>::new());
    }

    #[test]
    fn prepare_waits_for_the_install_run_not_the_check() {
        let runs = Runs { check: Some(progress(&["##HANGAR-PASSO## preparar ok", "##HANGAR-FIM## ok"])), install: None };
        assert_eq!(status(Screen::Prepare, &view(&runs)), Status::Doing);
        assert_eq!(frontier(&screens(false), &view(&runs)), Screen::Prepare);
        let runs = Runs { install: Some(progress(&["##HANGAR-PASSO## preparar ok", "##HANGAR-PASSO## instalar fazendo"])), ..runs };
        assert_eq!(status(Screen::Prepare, &view(&runs)), Status::Done);
        assert_eq!(frontier(&screens(false), &view(&runs)), Screen::Install);
    }

    #[test]
    fn blocked_precheck_needs_you_before_start() {
        let runs = Runs::default();
        let v = View { started: false, precheck_blocked: true, runs: &runs, failed_at: None, phone: None };
        assert_eq!(status(Screen::Prepare, &v), Status::NeedsYou);
        assert_eq!(status(Screen::Welcome, &v), Status::Doing);
        assert!(!primary_enabled(Screen::Welcome, &v, false));
    }

    #[test]
    fn interrupted_run_marks_the_screen_as_needing_you() {
        let runs = Runs { check: None, install: Some(progress(&["##HANGAR-PASSO## preparar ok", "##HANGAR-PASSO## instalar fazendo"])) };
        let v = View { failed_at: Some(Screen::Install), ..view(&runs) };
        assert_eq!(status(Screen::Install, &v), Status::NeedsYou);
        assert_eq!(frontier(&screens(false), &v), Screen::Install);
    }

    #[test]
    fn resume_frontier_follows_the_logs() {
        let lines = ["##HANGAR-PASSO## preparar ok", "##HANGAR-PASSO## instalar ok", "##HANGAR-PASSO## tailscale fazendo"];
        let runs = Runs { check: None, install: Some(progress(&lines)) };
        assert_eq!(frontier(&screens(true), &view(&runs)), Screen::Tailscale);
        // Quem olhava a fronteira anterior acompanha; quem voltou para ver outra tela fica.
        assert_eq!(follow(Screen::Install, Screen::Install, Screen::Tailscale), Screen::Tailscale);
        assert_eq!(follow(Screen::Welcome, Screen::Install, Screen::Tailscale), Screen::Welcome);
    }

    #[test]
    fn end_moves_to_phone_and_then_done() {
        let lines = ["##HANGAR-PASSO## preparar ok", "##HANGAR-PASSO## instalar ok", "##HANGAR-PASSO## final ok", "##HANGAR-FIM## ok"];
        let runs = Runs { check: None, install: Some(progress(&lines)) };
        let v = view(&runs);
        assert_eq!(frontier(&screens(false), &v), Screen::Phone);
        assert!(primary_enabled(Screen::Phone, &v, false));
        assert_eq!(primary(Screen::Phone, true), Primary::PhoneConnected);
        let v = View { phone: Some(PhoneOutcome::Skipped), ..view(&runs) };
        assert_eq!(status(Screen::Done, &v), Status::Done);
        assert!(primary_enabled(Screen::Done, &v, false));
    }

    #[test]
    fn pending_tailscale_holds_the_frontier_but_continue_works_after_the_end() {
        let lines = ["##HANGAR-PASSO## preparar ok", "##HANGAR-PASSO## instalar ok", "##HANGAR-PASSO## tailscale pendente", "##HANGAR-FIM## pendente"];
        let runs = Runs { check: None, install: Some(progress(&lines)) };
        let v = view(&runs);
        let list = screens(true);
        assert_eq!(frontier(&list, &v), Screen::Tailscale);
        assert!(primary_enabled(Screen::Tailscale, &v, false));
        assert!(reachable(&list, Screen::Done, &v));
        assert_eq!(next(&list, Screen::Tailscale), Screen::Phone);
        let v = View { phone: Some(PhoneOutcome::Connected), ..view(&runs) };
        assert_eq!(status(Screen::Done, &v), Status::NeedsYou);
    }

    #[test]
    fn items_merge_the_check_and_the_install_by_id() {
        let runs = Runs {
            check: Some(progress(&["##HANGAR-PASSO## preparar fazendo", "##HANGAR-ITEM## tmux ok tmux", "##HANGAR-ITEM## claude fila Claude Code"])),
            install: Some(progress(&["##HANGAR-PASSO## preparar fazendo", "##HANGAR-ITEM## claude fazendo Claude Code · baixando"])),
        };
        let items: Vec<_> = runs.items(Step::Prepare).into_iter().map(|i| (i.id, i.state)).collect();
        assert_eq!(items, vec![("tmux".to_owned(), State::Ok), ("claude".to_owned(), State::Doing)]);
    }
}
