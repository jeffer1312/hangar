//! `hangar-server install-token`: o token do dono quando a máquina não tem `openssl`. A mesma forma do
//! `openssl rand -hex 24`: 48 dígitos hexadecimais minúsculos e uma quebra de linha.
use std::io::{self, Write};
use std::process::ExitCode;

use ring::rand::{SecureRandom, SystemRandom};

/// 24 bytes do gerador do sistema em hexadecimal minúsculo.
pub fn generate() -> Result<String, &'static str> {
    let mut bytes = [0u8; 24];
    SystemRandom::new().fill(&mut bytes).map_err(|_| "gerador aleatório do sistema indisponível")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    if let Some(arg) = args.first() {
        let _ = writeln!(err, "uso: hangar-server install-token (argumento inesperado '{arg}')");
        return 2;
    }
    match generate() {
        Ok(token) => {
            let _ = writeln!(out, "{token}");
            0
        }
        Err(message) => {
            let _ = writeln!(err, "erro: {message}");
            1
        }
    }
}

pub fn cli(args: &[String]) -> ExitCode {
    let mut out = io::stdout().lock();
    let code = run(args, &mut out, &mut io::stderr().lock());
    let _ = out.flush();
    ExitCode::from(code)
}
