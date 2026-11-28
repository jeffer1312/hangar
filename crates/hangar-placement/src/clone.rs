//! O clone do repositório que uma máquina não tem, na pasta de clone dela: por uma pasta temporária ao lado,
//! renomeada só no fim, para clone pela metade nunca parecer repositório. Quem chama registra o que sobrou.
use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use hangar_core::project::ProjectId;
use hangar_workspace::{git, process, worktrees};

/// A recusa do clone, com o status e o código que a rota devolve.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub code: &'static str,
    pub msg: String,
    pub params: Value,
}

fn refusal(status: u16, code: &'static str, msg: impl Into<String>, params: Value) -> Refusal {
    Refusal { status, code, msg: msg.into(), params }
}

/// O resultado e, quando a pasta temporária ficou no disco, o erro de E/S que a segurou.
#[derive(Debug)]
pub struct Cloned {
    pub result: Result<String, Refusal>,
    pub leftover: Option<ErrorKind>,
}

/// Clona `origin` em `<dir>/<nome>`. A pasta que já existe com o mesmo `origin` é a resposta.
pub fn clone_into(origin: &str, dir: &Path, wait: Duration) -> Cloned {
    let done = |result| Cloned { result, leftover: None };
    let origin = origin.trim();
    let Some(name) = folder_name(origin).filter(|_| !origin.starts_with('-')) else {
        return done(Err(refusal(422, "placement_clone_invalid", "origin sem nome de repositório", json!({}))));
    };
    if !dir.is_absolute() {
        return done(Err(refusal(422, "placement_clone_invalid", "a pasta de clone precisa ser absoluta", json!({}))));
    }
    let target = dir.join(&name);
    if target.symlink_metadata().is_ok() { return done(existing(&target, origin)) }
    if let Err(e) = std::fs::create_dir_all(dir) {
        return done(Err(refusal(500, "placement_clone_failed", format!("não consegui criar a pasta de clone {}: {e}", dir.display()), json!({}))));
    }
    let tmp = dir.join(format!(".{name}.tmp-{}", hangar_core::ids::random_hex(8)));
    // Sem o gerenciador de credenciais abrir janela: credencial ausente é erro, nunca espera.
    let ran = process::run_program(process::git_command(None).arg("-C").arg(dir).args(["clone", "--", origin]).arg(&tmp)
        .env("GCM_INTERACTIVE", "never"), wait);
    let failure = match ran {
        Ok(out) if out.code == 0 => None,
        Ok(out) => Some(refusal(409, "placement_clone_failed", format!("o clone de {name} falhou: {}", useful_line(&out.stderr)), json!({"name": name}))),
        Err(e) if e.status == 504 => Some(refusal(504, "placement_clone_timeout",
            format!("o clone de {name} não terminou em {} min", wait.as_secs() / 60), json!({"name": name}))),
        Err(e) => Some(refusal(500, "placement_clone_failed", format!("o clone de {name} não rodou: {}", e.detail.as_str().unwrap_or("git")),
            json!({"name": name}))),
    };
    match failure {
        Some(failure) => Cloned { result: Err(failure), leftover: discard(&tmp) },
        None => settle(&tmp, &target, origin),
    }
}

/// O clone de `tmp` passa a `target`. Com o nome já ocupado (outro pedido terminou o mesmo clone antes) a
/// resposta é a de `existing`; sem ele, a troca que falhou é falha do clone, com o erro de E/S.
fn settle(tmp: &Path, target: &Path, origin: &str) -> Cloned {
    let Err(error) = std::fs::rename(tmp, target) else {
        return Cloned { result: Ok(target.to_string_lossy().into_owned()), leftover: None };
    };
    let leftover = discard(tmp);
    let result = if target.symlink_metadata().is_ok() { existing(target, origin) } else {
        Err(refusal(500, "placement_clone_failed", format!("não consegui pôr o clone em {}: {error}", target.display()),
            json!({"root": target.to_string_lossy()})))
    };
    Cloned { result, leftover }
}

/// Apaga a pasta temporária do clone; devolve o erro da que ficou no disco (a que nunca nasceu não conta).
fn discard(tmp: &Path) -> Option<ErrorKind> {
    std::fs::remove_dir_all(tmp).err().map(|e| e.kind()).filter(|kind| *kind != ErrorKind::NotFound)
}

/// A pasta que já ocupa o nome: serve se for o mesmo repositório.
fn existing(target: &Path, origin: &str) -> Result<String, Refusal> {
    let root = target.to_string_lossy().into_owned();
    match worktrees::origin_url(&root) {
        Some(found) if same_repo(&found, origin) => Ok(root),
        found => Err(refusal(409, "placement_clone_conflict", format!("{root} já existe e não é um clone deste repositório"),
            json!({"root": root, "other": found.is_some()}))),
    }
}

/// O mesmo repositório por https ou ssh; endereço sem host (pasta local) só por igualdade.
fn same_repo(a: &str, b: &str) -> bool {
    match (ProjectId::from_origin(a), ProjectId::from_origin(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/'),
    }
}

/// O nome da pasta, como o `git clone` daria: o último trecho do endereço, sem `.git`.
fn folder_name(origin: &str) -> Option<String> {
    let path = origin.trim_end_matches(['/', '\\']);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let name = path.rsplit(['/', '\\', ':']).next()?;
    let ok = !name.is_empty() && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    ok.then(|| name.to_owned())
}

/// A primeira linha do git que diz o que houve (sem o "Cloning into"), sem credencial e curta.
fn useful_line(stderr: &str) -> String {
    let line = stderr.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("Cloning into")).unwrap_or("git falhou");
    git::scrub(line).chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(600);

    #[test]
    fn the_folder_is_named_like_git_clone_names_it() {
        assert_eq!(folder_name("git@github.com:jeffer1312/hangar.git").as_deref(), Some("hangar"));
        assert_eq!(folder_name("https://github.com/Jeffer1312/Hangar/").as_deref(), Some("Hangar"));
        assert_eq!(folder_name("/srv/remote.git").as_deref(), Some("remote"));
        assert_eq!(folder_name("C:\\repos\\app.git").as_deref(), Some("app"));
        // Nada que saia da pasta de clone nem vire pasta escondida.
        for bad in ["https://github.com/a/..", "https://github.com/a/.git", "", "https://x/a b"] {
            assert_eq!(folder_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn https_and_ssh_of_the_same_repo_are_the_same_clone() {
        assert!(same_repo("git@github.com:jeffer1312/hangar.git", "https://github.com/jeffer1312/hangar"));
        assert!(!same_repo("git@github.com:jeffer1312/hangar.git", "https://github.com/outro/hangar"));
        assert!(same_repo("/srv/remote.git/", "/srv/remote.git"));
    }

    #[test]
    fn the_git_line_shown_skips_the_preamble_and_hides_the_credential() {
        // URL montada em partes: a trava de commit do repositório público recusa credencial literal.
        let url = format!("https://user{}segredo{}github.com/a/b/", ':', '@');
        let stderr = format!("Cloning into '/x/.a.tmp-1'...\nfatal: unable to access '{url}': error\n");
        let line = useful_line(&stderr);
        assert!(line.starts_with("fatal: unable to access"), "{line}");
        assert!(!line.contains("segredo"), "{line}");
    }

    #[test]
    fn an_invalid_origin_or_a_relative_dir_is_refused_before_anything() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(clone_into("--upload-pack=x", base.path(), WAIT).result.unwrap_err().code, "placement_clone_invalid");
        assert_eq!(clone_into("https://github.com/a/b", Path::new("rel"), WAIT).result.unwrap_err().code, "placement_clone_invalid");
        assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_rename_that_fails_with_the_name_free_is_a_clone_failure_not_a_conflict() {
        let base = tempfile::tempdir().unwrap();
        // A pasta temporária sumiu: a troca falha e o destino continua livre.
        let failed = settle(&base.path().join(".b.tmp-1"), &base.path().join("b"), "https://github.com/a/b");
        let refused = failed.result.unwrap_err();
        assert_eq!((refused.status, refused.code, failed.leftover), (500, "placement_clone_failed", None));
        assert!(refused.msg.contains("não consegui pôr o clone"), "{}", refused.msg);
        // O nome ocupado por outra coisa continua sendo conflito.
        let tmp = base.path().join(".c.tmp-1");
        std::fs::create_dir(&tmp).unwrap();
        std::fs::write(base.path().join("c"), "").unwrap();
        let taken = settle(&tmp, &base.path().join("c").join("x"), "https://github.com/a/c");
        assert_eq!(taken.result.unwrap_err().code, "placement_clone_failed", "pai que é arquivo: o nome não está ocupado");
        std::fs::create_dir(&tmp).ok();
        std::fs::create_dir(base.path().join("d")).unwrap();
        std::fs::write(base.path().join("d").join("f"), "").unwrap();
        let conflict = settle(&tmp, &base.path().join("d"), "https://github.com/a/d");
        assert_eq!(conflict.result.unwrap_err().code, "placement_clone_conflict");
        assert!(!tmp.exists(), "a temporária sai também no conflito");
    }
}
