//! Identidade de um repositório entre máquinas.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(pub String);

impl ProjectId {
    /// `git:<host>/<dono>/<repo>`: o mesmo repositório clonado por https ou ssh dá o mesmo id.
    pub fn from_origin(url: &str) -> Option<Self> {
        let mut rest = url.trim().to_ascii_lowercase();
        let mut had_scheme = false;
        if let Some(i) = rest.find("://") {
            rest = rest[i + 3..].to_string();
            had_scheme = true;
        }
        if let Some(at) = rest.find('@') {
            if !rest[..at].contains('/') {
                rest = rest[at + 1..].to_string();
            }
        }
        if let Some(colon) = rest.find(':') {
            if !rest[..colon].contains('/') {
                let host = &rest[..colon];
                let mut path = &rest[colon + 1..];
                // Porta só existe com esquema; sem ele, `host:123org/repo` é dono que começa com dígito.
                if had_scheme {
                    if let Some(slash) = path.find('/') {
                        if slash > 0 && path[..slash].bytes().all(|b| b.is_ascii_digit()) {
                            path = &path[slash + 1..];
                        }
                    }
                }
                rest = format!("{host}/{}", path.trim_start_matches('/'));
            }
        }
        let trimmed = rest.trim_end_matches('/');
        let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed).trim_end_matches('/');
        let (host, path) = trimmed.split_once('/')?;
        if host.is_empty() || path.is_empty() {
            return None;
        }
        Some(Self(format!("git:{trimmed}")))
    }

    pub fn from_dir(root: &Path) -> Self {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Self(format!("dir:{name}"))
    }

    /// Projeto sem git só junta máquinas depois de confirmado na tela.
    pub fn is_dir(&self) -> bool {
        self.0.starts_with("dir:")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_and_ssh_are_the_same_project() {
        let want = Some(ProjectId("git:github.com/jeffer1312/hangar".into()));
        assert_eq!(ProjectId::from_origin("git@github.com:jeffer1312/hangar.git"), want);
        assert_eq!(ProjectId::from_origin("https://github.com/jeffer1312/hangar"), want);
        assert_eq!(ProjectId::from_origin("https://github.com/Jeffer1312/hangar.git/"), want);
        assert_eq!(ProjectId::from_origin("ssh://git@github.com:22/jeffer1312/hangar.git"), want);
    }

    #[test]
    fn scp_owner_starting_with_digit_keeps_it() {
        assert_eq!(
            ProjectId::from_origin("git@gitlab.example:1password/app.git"),
            Some(ProjectId("git:gitlab.example/1password/app".into()))
        );
    }

    #[test]
    fn origin_without_path_is_not_a_project() {
        assert_eq!(ProjectId::from_origin("github.com"), None);
        assert_eq!(ProjectId::from_origin(""), None);
    }
}
