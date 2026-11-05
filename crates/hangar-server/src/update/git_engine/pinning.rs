//! Escolhe o primeiro pai cujo código combina com o binário publicado para esta máquina.
use super::{
    Service,
    releases::{self, Release, Source},
    switches, tail,
};
use serde_json::Value;
const PROTOCOL: &str = "backend/app/rust_server.py";
impl Service {
    pub fn protocol_at(&self, rev: &str) -> Option<u64> {
        let output = self.git(&["show", &format!("{rev}:{PROTOCOL}")], 30).ok()?;
        if output.code != 0 {
            return None;
        }
        output.stdout.lines().find_map(|line| {
            line.strip_prefix("RUST_SERVER_PROTOCOL = ")
                .and_then(|s| s.split_whitespace().next()?.parse().ok())
        })
    }
    pub fn release_binary(&self, release: &Release, platform: &str) -> Option<(String, u64)> {
        if !releases::has_build(&release.manifest, platform) {
            return None;
        }
        let entry = if release.manifest.get("platforms").is_some() {
            &release.manifest["platforms"][platform]
        } else {
            &release.manifest
        };
        let commit = entry["commit"].as_str()?;
        if commit.len() != 40
            || !commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        let protocol = if release.manifest.get("platforms").is_some() {
            entry["protocol"].as_u64()
        } else {
            None
        }
        .or_else(|| self.protocol_at(commit))?;
        Some((commit.into(), protocol))
    }
    pub fn newest_protocol(&self, top: &str, want: u64) -> Option<String> {
        if self.protocol_at(top) == Some(want) {
            return Some(top.into());
        }
        let result = self
            .git(
                &[
                    "log",
                    "-m",
                    "--first-parent",
                    "--no-patch",
                    "--format=%H",
                    "-G",
                    "^RUST_SERVER_PROTOCOL = ",
                    top,
                    "--",
                    PROTOCOL,
                ],
                30,
            )
            .ok()?;
        if result.code != 0 {
            return None;
        }
        for sha in result.stdout.split_whitespace() {
            let parent = self
                .git(
                    &["rev-parse", "--verify", "--quiet", &format!("{sha}^1")],
                    30,
                )
                .ok()?
                .stdout
                .trim()
                .to_owned();
            if !parent.is_empty() && self.protocol_at(&parent) == Some(want) {
                return Some(parent);
            }
        }
        None
    }
    pub fn last_same_build(&self, published: &str, top: &str) -> String {
        let Ok(workflow) = self.git(
            &["show", &format!("{top}:.github/workflows/server.yml")],
            30,
        ) else {
            return published.into();
        };
        if workflow.code != 0 {
            return published.into();
        }
        let mut inside = false;
        let mut paths = vec![];
        for line in workflow.stdout.lines() {
            let text = line.trim();
            if text == "paths:" {
                inside = true;
            } else if inside {
                if let Some(path) = text.strip_prefix("- ") {
                    paths.push(format!(":(glob){}", path.trim().trim_matches(['\'', '"'])));
                } else if !text.is_empty() && !text.starts_with('#') {
                    break;
                }
            }
        }
        if paths.is_empty() {
            return published.into();
        }
        let Ok(log) = self.git(
            &["rev-list", "--first-parent", &format!("{published}..{top}")],
            30,
        ) else {
            return published.into();
        };
        if log.code != 0 {
            return published.into();
        }
        for candidate in log.stdout.split_whitespace() {
            let mut args = vec!["diff", "--quiet", published, candidate, "--"];
            args.extend(paths.iter().map(String::as_str));
            match self.git(&args, 30) {
                Ok(diff) if diff.code == 0 => return candidate.into(),
                Ok(diff) if diff.code == 1 => {}
                _ => return published.into(),
            }
        }
        published.into()
    }
    pub fn pinned_target(
        &self,
        target: &str,
        source: &dyn Source,
        platform: Option<&str>,
    ) -> (Option<String>, Option<Release>) {
        let top = format!("origin/{target}");
        let enabled = self.value("CP_RUST_SERVER").unwrap_or_default();
        if matches!(
            enabled.to_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ) || platform.is_none()
        {
            return (Some(top), None);
        }
        let platform = platform.unwrap();
        let override_url = self
            .environment
            .get("HANGAR_SERVER_RELEASE_URL")
            .filter(|s| !s.is_empty());
        let tag = override_url.is_none().then(|| releases::tag_for(target));
        let url = override_url
            .map(|s| s.trim_end_matches('/').into())
            .unwrap_or_else(|| format!("{}/{}", releases::BASE, tag.as_ref().unwrap()));
        let manifest = match releases::read(source, &url) {
            Ok(value) => value,
            Err(error) if error.status == Some(404) && tag.as_deref() != Some("server-latest") => {
                return (Some(top), None);
            }
            Err(_) => return (None, None),
        };
        let release = Release { url, tag, manifest };
        let Some((published, want)) = self.release_binary(&release, platform) else {
            return (None, Some(release));
        };
        let tip = self
            .git(&["rev-parse", "--verify", "--quiet", &top], 30)
            .ok()
            .map(|v| v.stdout.trim().to_owned())
            .unwrap_or_default();
        let selected = if published == tip {
            Some(top)
        } else if self
            .git(&["merge-base", "--is-ancestor", &published, &top], 30)
            .is_ok_and(|v| v.code == 0)
        {
            Some(self.last_same_build(&published, &top))
        } else {
            self.newest_protocol(&top, want)
        };
        (selected, Some(release))
    }
    pub fn pull(
        &self,
        pre: &Value,
        source: &dyn Source,
        platform: Option<&str>,
    ) -> Result<Option<Release>, String> {
        let target = pre["alvo"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("main");
        let fetch = self.git(&["fetch", "origin"], 300)?;
        if fetch.code != 0 {
            return Err(format!(
                "nao consegui buscar o codigo novo: {}",
                tail(&fetch, 12)
            ));
        }
        let tag = self.git(
            &[
                "fetch",
                "origin",
                "--force",
                "refs/tags/dist-latest:refs/tags/dist-latest",
            ],
            60,
        )?;
        if tag.code != 0 {
            tracing::warn!("tag dist-latest nao atualizada: {}",tail(&tag,12));
        }
        let (reference, release) = self.pinned_target(target, source, platform);
        let reference = if let Some(reference) = reference {
            reference
        } else if let Some(protocol) = self.protocol_at("HEAD") {
            self.state().warn("não consegui conferir o binário do Rust publicado para este sistema; o código não troca de contrato com ele até conferir");
            let Some(reference) = self.newest_protocol(&format!("origin/{target}"), protocol)
            else {
                return Ok(release);
            };
            reference
        } else {
            self.state().warn("não consegui conferir o binário do Rust publicado para este sistema; o código foi ao topo");
            format!("origin/{target}")
        };
        if switches(pre["branch"].as_str().unwrap_or(""), target) {
            self.switch_branch(target, &reference)?;
        }
        if self.git(&["merge", "--ff-only", &reference], 120)?.code == 0 {
            return Ok(release);
        }
        let stash = self.git(
            &[
                "stash",
                "push",
                "--include-untracked",
                "-m",
                "hangar antes do reset",
            ],
            120,
        )?;
        if stash.code != 0 {
            return Err(format!(
                "nao consegui guardar o que estava no disco: {}",
                tail(&stash, 12)
            ));
        }
        let reset = self.git(&["reset", "--hard", &reference], 120)?;
        if reset.code != 0 {
            return Err(format!(
                "nao consegui alinhar com o codigo novo: {}",
                tail(&reset, 12)
            ));
        }
        Ok(release)
    }
}
pub fn published_api(release: Option<&Release>, platform: &str) -> u64 {
    release
        .and_then(|r| r.manifest["platforms"][platform]["api"].as_u64())
        .filter(|v| *v >= 1)
        .unwrap_or(1)
}
