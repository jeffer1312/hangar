//! Restos: sessões do multiplexador abertas pelo Hangar que não têm mais sessão na lista, e canos sem
//! sessão no registro. Só conta: limpar é do `kill_orphans` da subida, nunca da tela.
use std::collections::HashSet;
use std::time::Duration;

use hangar_api::status::LeftoversBlock;

/// Cano mais novo que isto ainda pode estar sendo ligado à sessão que o abriu.
pub const CANO_GRACE: Duration = Duration::from_secs(30);

/// Uma sessão do multiplexador. `hangar` = a sessão tem `CP_PROVIDER` próprio (o `-e` do `new-session`
/// do lançador); o `#{CP_PROVIDER}` da lista de panes cai no ambiente global quando a sessão não tem o
/// dela, então só vale confirmado na sessão (`show-environment -t =<nome> CP_PROVIDER`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MuxSession {
    pub name: String,
    pub hangar: bool,
    /// `@cp_hidden`: o shell separado da sessão.
    pub hidden: bool,
    /// `@cp_shortcut_key`: terminal de atalho, de nenhuma sessão.
    pub shortcut: bool,
}

/// Nomes das sessões do Hangar no multiplexador sem linha na lista, em ordem. Sessão pessoal (sem a
/// marca) nunca conta.
pub fn tmux_orphans(sessions: &[MuxSession], listed: &HashSet<String>) -> Vec<String> {
    let mut names: Vec<String> = sessions.iter()
        .filter(|s| s.hangar && !s.hidden && !s.shortcut && !listed.contains(&s.name))
        .map(|s| s.name.clone()).collect();
    names.sort();
    names.dedup();
    names
}

/// Um processo de cano e há quanto tempo vive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cano {
    pub pid: u32,
    pub age: Duration,
}

pub fn is_cano(executable: &str) -> bool {
    let lower = executable.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower) == "hangar-cano"
}

/// Canos sem ligação no registro (`bound` = pids dos canos das sessões abertas) e com mais de
/// `CANO_GRACE` de vida.
pub fn cano_orphans(canos: &[Cano], bound: &HashSet<u32>) -> u32 {
    let loose = canos.iter().filter(|c| c.age > CANO_GRACE && !bound.contains(&c.pid)).count();
    u32::try_from(loose).unwrap_or(u32::MAX)
}

/// Os canos do usuário deste processo pelo `sysinfo`, sem as threads; `now` em segundos desde a época.
/// Vale no Linux, no macOS e no Windows (`hangar-cano.exe`). O `System` precisa vir atualizado com o
/// usuário (`ProcessRefreshKind::with_user`): sem ele os donos ficam desconhecidos e o cano de outro
/// usuário da máquina entraria na conta.
pub fn canos_in(sys: &sysinfo::System, now: u64) -> Vec<Cano> {
    let own = sysinfo::get_current_pid().ok().and_then(|pid| sys.process(pid)).and_then(|p| p.user_id().cloned());
    sys.processes().values()
        .filter(|p| p.thread_kind().is_none() && is_cano(&p.name().to_string_lossy()))
        .filter(|p| p.user_id() == own.as_ref())
        .map(|p| Cano { pid: p.pid().as_u32(), age: Duration::from_secs(now.saturating_sub(p.start_time())) })
        .collect()
}

/// O bloco; leitura que não veio fica ausente (multiplexador fora do ar, psmux sem a lista), e sem
/// nenhuma das duas o bloco inteiro some.
pub fn block(tmux_orphans: Option<Vec<String>>, cano_orphans: Option<u32>) -> Option<LeftoversBlock> {
    (tmux_orphans.is_some() || cano_orphans.is_some()).then_some(LeftoversBlock { tmux_orphans, cano_orphans })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str, hangar: bool) -> MuxSession {
        MuxSession { name: name.into(), hangar, hidden: false, shortcut: false }
    }

    #[test]
    fn only_hangar_sessions_without_a_row_count() {
        let sessions = vec![
            session("pessoal", false),
            session("viva", true),
            session("sobra", true),
            MuxSession { hidden: true, ..session("viva-sh", true) },
            MuxSession { shortcut: true, ..session("atalho", true) },
        ];
        let listed = HashSet::from(["viva".to_owned()]);
        assert_eq!(tmux_orphans(&sessions, &listed), vec!["sobra".to_owned()], "a pessoal do usuário nunca conta");
    }

    #[test]
    fn a_young_or_bound_cano_does_not_count() {
        let canos = [
            Cano { pid: 1, age: Duration::from_secs(10) },
            Cano { pid: 2, age: Duration::from_secs(31) },
            Cano { pid: 3, age: Duration::from_secs(3600) },
            Cano { pid: 4, age: CANO_GRACE },
        ];
        let bound = HashSet::from([3]);
        assert_eq!(cano_orphans(&canos, &bound), 1, "o de 10 s e o de 30 s esperam; o ligado é de uma sessão");
    }

    #[test]
    fn the_block_is_absent_without_any_reading() {
        assert_eq!(block(None, None), None);
        assert_eq!(block(None, Some(0)), Some(LeftoversBlock { tmux_orphans: None, cano_orphans: Some(0) }));
        assert!(is_cano("hangar-cano.exe") && is_cano("hangar-cano") && !is_cano("hangar-server"));
    }
}
