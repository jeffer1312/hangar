//! Versão legível do app para a página Sobre, no formato do web: data do commit + hash, `-dirty` com mudança
//! local. Sem git, fica o número do Cargo.toml. A data do build é a do dia em UTC.
use std::{process::Command, time::{SystemTime, UNIX_EPOCH}};

/// Só leitura: sem `--no-optional-locks` o `git status` regrava o índice do repositório, fora do OUT_DIR.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("--no-optional-locks").args(args).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8(out.stdout).ok()?.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Dias desde 1970 → ano-mês-dia (algoritmo civil de Howard Hinnant), para não depender de crate de data.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

fn main() {
    let version = git(&["log", "-1", "--format=%cd-%h", "--date=format:%Y.%m.%d"]).map(|v| {
        let dirty = git(&["status", "--porcelain", "--", "."]).is_some();
        if dirty { format!("{v}-dirty") } else { v }
    }).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64 / 86_400).unwrap_or(0);
    let (y, m, d) = civil(days);
    println!("cargo:rustc-env=HANGAR_NATIVE_VERSION={version}");
    println!("cargo:rustc-env=HANGAR_NATIVE_BUILD_DATE={y:04}-{m:02}-{d:02}");
}
