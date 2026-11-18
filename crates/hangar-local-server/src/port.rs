//! Quem escuta a porta do servidor: `ss` no Linux, `Get-NetTCPConnection` no Windows, `lsof` no
//! macOS. O pid sai do sistema; nome, dono e nascimento, da lista de processos.
use crate::host::Host;
use crate::service::{powershell, Os};

/// Um processo escutando a porta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortHolder {
    pub pid: u32,
    pub name: String,
    /// Do mesmo usuário deste processo: só esse pode ser encerrado daqui.
    pub same_user: bool,
    /// Segundos desde a época; `free_port` confere que é o mesmo processo.
    pub start_time: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listeners {
    pub holders: Vec<PortHolder>,
    /// Alguém escuta, mas o sistema não diz quem (processo de outro usuário no `ss`).
    pub hidden: bool,
    /// A consulta falhou: a resposta é desconhecida, não "ninguém".
    pub error: Option<String>,
}

impl Listeners {
    pub fn is_empty(&self) -> bool { self.holders.is_empty() && !self.hidden }
}

pub fn listeners(host: &dyn Host, os: Os, port: u16) -> Listeners {
    let read = match os {
        Os::Linux => host.run("ss", &["-ltnpH", &format!("sport = :{port}")]).map(|out| (out.code == Some(0), parse_ss(&out.stdout), out.stderr)),
        // Só o "não achou" do cmdlet é ninguém escutando; outro erro é consulta que falhou.
        Os::Windows => powershell(host, &format!(concat!(
            "$c=@(Get-NetTCPConnection -State Listen -LocalPort {port} -EA SilentlyContinue -EV e); ",
            "$f=@($e | Where-Object {{ $_.FullyQualifiedErrorId -notlike 'CmdletizationQuery_NotFound*' }}); ",
            "if($f.Count){{ [Console]::Error.WriteLine(($f | Out-String)); exit 1 }}; ",
            "$c | ForEach-Object {{ $_.OwningProcess }} | Sort-Object -Unique"), port = port))
            .map(|out| (out.code == Some(0), (parse_pid_lines(&out.stdout), false), out.stderr)),
        // `lsof` sai 1 quando ninguém escuta.
        Os::Macos => host.run("lsof", &["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fp"])
            .map(|out| (matches!(out.code, Some(0 | 1)), (parse_lsof(&out.stdout), false), out.stderr)),
        Os::Other => return Listeners { error: Some("sistema sem consulta de porta".to_owned()), ..Listeners::default() },
    };
    let (pids, hidden) = match read {
        Ok((true, found, _)) => found,
        Ok((false, _, stderr)) => return Listeners { error: Some(format!("consulta da porta falhou: {}", stderr.trim())), ..Listeners::default() },
        Err(e) => return Listeners { error: Some(format!("consulta da porta não rodou: {e}")), ..Listeners::default() },
    };
    let me = host.user();
    let holders = pids.into_iter().map(|pid| match host.process(pid) {
        Some(p) => PortHolder { pid, name: p.name, same_user: me.is_some() && p.user == me, start_time: p.start_time },
        None => PortHolder { pid, name: String::new(), same_user: false, start_time: 0 },
    }).collect();
    Listeners { holders, hidden, error: None }
}

/// `LISTEN 0 1024 0.0.0.0:8765 0.0.0.0:* users:(("hangar-server",pid=1234,fd=9))`. Linha sem
/// `users:` é processo de outro usuário: escuta, mas o `ss` não diz quem.
pub fn parse_ss(text: &str) -> (Vec<u32>, bool) {
    let mut pids = Vec::new();
    let mut hidden = false;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut found = false;
        for part in line.split("pid=").skip(1) {
            if let Ok(pid) = part.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>() {
                found = true;
                if !pids.contains(&pid) { pids.push(pid); }
            }
        }
        hidden |= !found;
    }
    (pids, hidden)
}

pub fn parse_pid_lines(text: &str) -> Vec<u32> {
    let mut pids: Vec<u32> = text.lines().filter_map(|l| l.trim().parse().ok()).collect();
    pids.dedup();
    pids
}

pub fn parse_lsof(text: &str) -> Vec<u32> {
    let mut pids: Vec<u32> = text.lines().filter_map(|l| l.strip_prefix('p')?.trim().parse().ok()).collect();
    pids.dedup();
    pids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ss_lines_give_pids_and_a_hidden_owner() {
        let text = concat!(
            "LISTEN 0      1024         0.0.0.0:8765      0.0.0.0:*    users:((\"python3\",pid=48211,fd=7),(\"python3\",pid=48212,fd=7))\n",
            "LISTEN 0      1024            [::]:8765         [::]:*    users:((\"python3\",pid=48211,fd=8))\n");
        assert_eq!(parse_ss(text), (vec![48211, 48212], false));
        assert_eq!(parse_ss("LISTEN 0 1024 0.0.0.0:8765 0.0.0.0:*\n"), (vec![], true));
        assert_eq!(parse_ss(""), (vec![], false));
    }

    #[test]
    fn windows_and_lsof_pid_lists() {
        assert_eq!(parse_pid_lines("4242\r\n\r\n"), vec![4242]);
        assert_eq!(parse_lsof("p4242\nf9\n"), vec![4242]);
    }
}
