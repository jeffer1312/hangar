//! Falha aberta, mas não calada: uma linha em `<raiz>/logs/privado/<arquivo>` (raiz `%LOCALAPPDATA%\hangar`
//! no Windows, `~/.hangar` nos outros). Pasta 0700, arquivo 0600 em append; passou de 256 KiB, o
//! arquivo vai para `.1` e recomeça. Se nem logar der, engole.
use crate::Context;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub const MAX: u64 = 256 * 1024;
/// O motivo é cortado: nunca leva o prompt inteiro ao disco.
pub const REASON_MAX: usize = 300;

pub fn private_dir(context: &Context) -> Option<PathBuf> {
    let root = if cfg!(windows) {
        context.var("LOCALAPPDATA").map(PathBuf::from).or_else(|| Some(context.home.as_ref()?.join("AppData").join("Local")))?.join("hangar")
    } else {
        context.home.as_ref()?.join(".hangar")
    };
    Some(root.join("logs").join("privado"))
}

pub fn record(context: &Context, file: &str, reason: &str) {
    let _ = (|| -> Option<()> {
        let dir = private_dir(context)?;
        if !dir.is_dir() {
            // Só a última pasta é privada; `logs` nasce com o modo comum, como no `makedirs` do Python.
            fs::create_dir_all(dir.parent()?).ok()?;
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => return None,
                _ => {}
            }
        }
        let path = dir.join(file);
        if fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX) {
            let mut rotated = path.clone().into_os_string();
            rotated.push(".1");
            hangar_core::fs::replace(&path, &PathBuf::from(rotated)).ok()?;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let reason: String = reason.chars().take(REASON_MAX).collect();
        let line = format!("{} {reason}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        options.open(&path).ok()?.write_all(line.as_bytes()).ok()
    })();
}
