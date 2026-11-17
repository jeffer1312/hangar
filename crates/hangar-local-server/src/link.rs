//! O `~/.hangar/current` e a troca dele: symlink + `rename` no Unix; junction e dois `rename` no
//! Windows. O atualizador do servidor e a ação "apontar para a versão" usam as mesmas funções.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Onde a troca do Windows põe o `current` velho entre os dois `rename`.
pub const ASIDE: &str = "current.old";
/// O link novo antes de entrar no lugar do `current`.
pub const FRESH: &str = "current.new";
/// A troca feita e ainda sem prova (`Unproven`).
pub const UNPROVEN_FILE: &str = "update-swap.json";

pub fn current(hangar: &Path) -> PathBuf { hangar.join("current") }
pub fn aside(hangar: &Path) -> PathBuf { hangar.join(ASIDE) }
pub fn unproven_file(hangar: &Path) -> PathBuf { hangar.join(UNPROVEN_FILE) }

/// O `canonicalize` do Windows devolve `\\?\C:\...`; o `mklink`, o Python e os caminhos gravados
/// pelo Hangar usam `C:\...`.
pub fn without_verbatim(path: PathBuf) -> PathBuf {
    if !cfg!(windows) { return path; }
    path.to_str().and_then(verbatim_stripped).map_or(path, PathBuf::from)
}

fn verbatim_stripped(text: &str) -> Option<String> {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") { return Some(format!(r"\\{rest}")); }
    text.strip_prefix(r"\\?\").filter(|rest| rest.as_bytes().get(1) == Some(&b':')).map(str::to_owned)
}

/// Caminho real, sem o `\\?\` do Windows.
pub fn real(path: &Path) -> std::io::Result<PathBuf> { std::fs::canonicalize(path).map(without_verbatim) }

/// Link ausente já é o resultado pedido.
pub fn absent_ok(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(unix)]
pub fn make_link(target: &Path, link: &Path) -> std::io::Result<()> { std::os::unix::fs::symlink(target, link) }

#[cfg(unix)]
pub fn remove_link(link: &Path) -> std::io::Result<()> { absent_ok(std::fs::remove_file(link)) }

/// `rename` de link por cima de link é atômico: não há instante sem `current`.
#[cfg(unix)]
pub fn replace_link(fresh: &Path, current: &Path) -> std::io::Result<()> { std::fs::rename(fresh, current) }

/// Junction (não precisa de administrador, ao contrário do symlink de pasta), sem janela de console.
#[cfg(windows)]
pub fn make_link(target: &Path, link: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()))
        .raw_arg(format!("/c mklink /J \"{}\" \"{}\"", link.display(), target.display()))
        .creation_flags(crate::host::CREATE_NO_WINDOW).output()?;
    if out.status.success() { return Ok(()); }
    Err(std::io::Error::other(format!("mklink /J: {}{}", String::from_utf8_lossy(&out.stdout).trim(), String::from_utf8_lossy(&out.stderr).trim())))
}

/// `remove_dir` numa junction tira só o link, nunca o conteúdo do alvo.
#[cfg(windows)]
pub fn remove_link(link: &Path) -> std::io::Result<()> { absent_ok(std::fs::remove_dir(link)) }

/// No Windows não há troca atômica de junction: a velha vai para o lado, a nova entra, e a velha
/// volta se a nova não entrar. Hooks toleram o instante sem `current`; queda nesse instante é
/// recuperada por `recover_aside`. Quem apaga o link ao lado é o `swap`, depois de conferir o novo.
#[cfg(windows)]
pub fn replace_link(fresh: &Path, current: &Path) -> std::io::Result<()> {
    let aside = current.with_file_name(ASIDE);
    // Com o `current` presente (o `swap` recuperou antes), o que sobrou ao lado é de troca concluída.
    let _ = remove_link(&aside);
    let had = std::fs::symlink_metadata(current).is_ok();
    if had { std::fs::rename(current, &aside)?; }
    if let Err(e) = std::fs::rename(fresh, current) {
        if had { let _ = std::fs::rename(&aside, current); }
        return Err(e);
    }
    Ok(())
}

/// Aponta o `current` de `hangar` para `target` e devolve para onde ele apontava (caminho real).
/// O erro leva o texto que a tela mostra.
pub fn swap(hangar: &Path, target: &Path) -> std::io::Result<Option<PathBuf>> {
    let target = real(target).map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", target.display())))?;
    recover_aside(hangar)?;
    let current = current(hangar);
    let previous = real(&current).ok();
    let fresh = hangar.join(FRESH);
    let _ = remove_link(&fresh);
    make_link(&target, &fresh)?;
    replace_link(&fresh, &current).inspect_err(|_| { let _ = remove_link(&fresh); })?;
    // O link de antes só sai depois de conferido o novo no lugar.
    if real(&current).ok().as_ref() != Some(&target) {
        return Err(std::io::Error::other(format!("{} não aponta para {} depois da troca", current.display(), target.display())));
    }
    // Sobra ao lado não atrapalha: a troca seguinte a tira antes de usar o nome.
    let _ = remove_link(&aside(hangar));
    Ok(previous)
}

/// A troca do Windows interrompida entre os dois `rename`: o link de antes volta ao lugar.
/// `true` quando havia o que recuperar.
pub fn recover_aside(hangar: &Path) -> std::io::Result<bool> {
    if std::fs::symlink_metadata(current(hangar)).is_ok() || std::fs::symlink_metadata(aside(hangar)).is_err() {
        return Ok(false);
    }
    std::fs::rename(aside(hangar), current(hangar))?;
    Ok(true)
}

/// A troca que a atualização fez e ainda não provou, gravada antes dela: a rodada que morre deixa o
/// que desfazer, e `target` é a versão que não se provou.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unproven {
    pub previous: Option<PathBuf>,
    pub target: PathBuf,
}

/// `Ok(None)` sem registro; ilegível é erro, com o caminho.
pub fn read_unproven(hangar: &Path) -> std::io::Result<Option<Unproven>> {
    let path = unproven_file(hangar);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    serde_json::from_slice(&bytes).map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} ilegível ({e}); confira o current à mão e apague o arquivo", path.display())))
}
