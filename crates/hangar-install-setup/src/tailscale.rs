//! `hangar-server tailscale-dns-name`: o nome desta máquina no tailnet, lido do `tailscale status
//! --json` na entrada padrão, sem o ponto final. O instalador chama
//! `tailscale status --json 2>/dev/null | "$SRV" tailscale-dns-name 2>/dev/null` e trata saída vazia
//! como "Tailscale sem login". O código de saída do `tailscale` não conta: vale o JSON que veio.
//!
//! Sem nome (JSON ilegível, sem `Self`, `DNSName` nulo, não texto ou vazio) não imprime nada e sai 1;
//! o Python imprimia uma linha vazia para o nome vazio, o que dá o mesmo `nome=""` no shell.
use std::io::{self, Read, Write};
use std::process::ExitCode;

/// O nome sem os pontos do fim; `None` quando não há nome.
pub fn dns_name(status: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(status).ok()?;
    let name = value.get("Self")?.get("DNSName")?.as_str()?.trim_end_matches('.');
    (!name.is_empty()).then(|| name.to_owned())
}

pub fn run(args: &[String], stdin: &[u8], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    if let Some(arg) = args.first() {
        let _ = writeln!(err, "uso: tailscale status --json | hangar-server tailscale-dns-name (argumento inesperado '{arg}')");
        return 2;
    }
    match dns_name(stdin) {
        Some(name) => {
            let _ = writeln!(out, "{name}");
            0
        }
        None => 1,
    }
}

pub fn cli(args: &[String]) -> ExitCode {
    let mut stdin = Vec::new();
    let mut err = io::stderr().lock();
    if let Err(error) = io::stdin().read_to_end(&mut stdin) {
        let _ = writeln!(err, "erro: não consegui ler a entrada padrão: {error}");
        return ExitCode::from(1);
    }
    let mut out = io::stdout().lock();
    let code = run(args, &stdin, &mut out, &mut err);
    let _ = out.flush();
    ExitCode::from(code)
}
