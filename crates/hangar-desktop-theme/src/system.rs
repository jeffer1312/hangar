use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crate::reader::{Home, Pointer, read_text};

pub(crate) fn home() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOMEPATH").map(|path| {
                PathBuf::from(std::env::var_os("HOMEDRIVE").unwrap_or_default()).join(path)
            })
        });
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(std::env::home_dir);
    home.ok_or_else(|| io::Error::other("não foi possível determinar a pasta pessoal"))
}

pub(crate) fn pointer(home: Home) -> Pointer {
    #[cfg(windows)]
    {
        let _ = home;
        Arc::new(registry_wallpaper)
    }
    #[cfg(not(windows))]
    {
        Arc::new(move || {
            read_text(&home()?.join(".local/state/quickshell/user/generated/wallpaper/path.txt"))
        })
    }
}

#[cfg(windows)]
fn registry_wallpaper() -> io::Result<String> {
    windows_registry::CURRENT_USER
        .open(r"Control Panel\Desktop")
        .and_then(|key| key.get_string("WallPaper"))
        .map_err(|error| {
            let kind = match error.code().0 as u32 & 0xffff {
                2 | 3 => io::ErrorKind::NotFound,
                5 => io::ErrorKind::PermissionDenied,
                _ => io::ErrorKind::Other,
            };
            io::Error::new(kind, error.to_string())
        })
}

pub(crate) fn expand_user(raw: &str, home: &Home) -> io::Result<PathBuf> {
    let Some(rest) = raw.strip_prefix('~') else {
        return Ok(raw.into());
    };
    let end = rest
        .find(|character| character == '/' || cfg!(windows) && character == '\\')
        .unwrap_or(rest.len());
    let user = &rest[..end];
    let base = if user.is_empty() {
        home()?
    } else {
        #[cfg(unix)]
        {
            hangar_core::account_env::named_user_home(user)
                .ok_or_else(|| io::Error::other("pasta do usuário indisponível"))?
        }
        #[cfg(windows)]
        {
            let base = home()?;
            let current = std::env::var("USERNAME").unwrap_or_default();
            if user == current {
                base
            } else if base
                .file_name()
                .is_some_and(|name| name == current.as_str())
            {
                base.parent()
                    .map(|parent| parent.join(user))
                    .ok_or_else(|| io::Error::other("pasta do usuário indisponível"))?
            } else {
                return Err(io::Error::other("pasta do usuário indisponível"));
            }
        }
    };
    Ok(base
        .join(rest[end..].trim_start_matches(|character| {
            character == '/' || cfg!(windows) && character == '\\'
        })))
}
