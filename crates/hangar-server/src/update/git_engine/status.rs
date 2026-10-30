//! Retrato da atualização, com a publicação remota compartilhada entre os polls da tela.
use super::{
    Service,
    releases::{self, Release, Source},
    switches,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
#[derive(Clone)]
struct Published {
    at: Instant,
    target: String,
    stale: bool,
    api: u64,
}
static PUBLISHED: LazyLock<Mutex<HashMap<String, Published>>> = LazyLock::new(Default::default);
impl Service {
    pub fn capture_boot(&self) {crate::boot_version::capture(&self.root,&self.environment);}
    pub fn boot_version(&self) -> (String,Option<String>) {
        crate::boot_version::get(&self.root).map(|v|(v.describe,v.readable)).unwrap_or_else(||("indisponivel".into(),None))
    }
    pub fn describe(&self) -> String {
        if let Some(manifest)=crate::update::read_manifest(&self.root) {
            return format!("{}-g{}",manifest.version,manifest.commit.chars().take(12).collect::<String>());
        }

        self.git(
            &[
                "describe",
                "--tags",
                "--match",
                "dist-latest",
                "--long",
                "--always",
                "--dirty",
            ],
            5,
        )
        .ok()
        .map(|v| v.stdout.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "indisponivel".into())
    }
    pub fn readable_version(&self, reference: &str) -> Option<String> {
        if reference=="HEAD" {if let Some(manifest)=crate::update::read_manifest(&self.root){return Some(manifest.version);}}

        let count = self
            .git(&["rev-list", "--count", reference], 5)
            .ok()?
            .stdout
            .trim()
            .to_owned();
        if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let mut version = self
            .git(&["show", &format!("{reference}:VERSION")], 5)
            .ok()?
            .stdout
            .trim()
            .to_owned();
        if version.is_empty() && reference == "HEAD" {
            version = std::fs::read_to_string(self.root.join("VERSION"))
                .ok()?
                .trim()
                .to_owned();
        }
        (!version.is_empty()).then(|| format!("{version}.{count}"))
    }
    pub fn pending_steps(&self) -> Result<Value, String> {
        let done = crate::update::steps::applied(&self.state_dir)?;
        let folder = self.root.join("docs/atualizacoes");
        let (steps, _) = crate::update::steps::read_steps(&folder, cfg!(windows));
        let mut text = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&folder) {
            for entry in entries.flatten() {
                if let Ok(raw) = std::fs::read_to_string(entry.path()) {
                    let (fields, body) = crate::update::steps::frontmatter(&raw);
                    let id = fields
                        .get("id")
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .unwrap_or_else(|| {
                            entry
                                .path()
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into()
                        });
                    text.insert(id, body);
                }
            }
        }
        Ok(json!(steps.into_iter().filter(|step|!done.contains(&step.id)).map(|step|json!({"id":step.id,"titulo":step.title,"texto":text.get(&step.id).cloned().unwrap_or_default()})).collect::<Vec<_>>()))
    }
    fn published(
        &self,
        refresh: bool,
        source: &dyn Source,
        platform: Option<&str>,
    ) -> Result<Published, String> {
        let target = self.target()?;
        let key = format!("{}:{}:{target}", self.root.display(), self.home.display());
        if !refresh {
            if let Some(value) = PUBLISHED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&key)
                .filter(|v| v.at.elapsed() < Duration::from_secs(300))
            {
                return Ok(value.clone());
            }
        }
        let (selected, release) = self.pinned_target(&target, source, platform);
        let value = Published {
            at: Instant::now(),
            target: selected.unwrap_or_else(|| format!("origin/{target}")),
            stale: self.stale_release(release.as_ref(), platform),
            api: super::pinning::published_api(release.as_ref(), platform.unwrap_or("")),
        };
        PUBLISHED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, value.clone());
        Ok(value)
    }
    pub fn stale_release(&self, release: Option<&Release>, platform: Option<&str>) -> bool {
        let (Some(release), Some(platform)) = (release, platform) else {
            return false;
        };
        if !releases::has_build(&release.manifest, platform) {
            return false;
        }
        let ext = if platform.starts_with("windows") {
            ".exe"
        } else {
            ""
        };
        releases::NAMES.into_iter().any(|name| {
            release.manifest["files"][format!("{platform}/{name}")]["sha256"]
                .as_str()
                .is_some_and(|sha| {
                    !releases::matches_file(
                        &self.home.join(".hangar/bin").join(format!("{name}{ext}")),
                        &sha.to_lowercase(),
                    )
                })
        })
    }
    pub fn status(
        &self,
        refresh: bool,
        source: &dyn Source,
        platform: Option<&str>,
    ) -> Result<Value, String> {
        if refresh {
            let _ = self.git(&["fetch", "origin"], 120)?;
        }
        let pre = self.preflight()?;
        let published = self.published(refresh, source, platform)?;
        let changes = self.git(
            &[
                "log",
                "--format=%h%x00%s",
                &format!("HEAD..{}", published.target),
            ],
            30,
        )?;
        let unknown = changes.code != 0;
        let changes: Vec<_> = if unknown {
            vec![]
        } else {
            changes
                .stdout
                .lines()
                .filter_map(|line| {
                    let (sha, title) = line.split_once('\0')?;
                    (!title.is_empty()).then(|| json!({"sha":sha,"titulo":title}))
                })
                .collect()
        };
        let switching = switches(
            pre["branch"].as_str().unwrap_or(""),
            pre["alvo"].as_str().unwrap_or("main"),
        ) && pre["branch_de_trabalho"] != true;
        let repo = self.describe();
        let readable = self.readable_version("HEAD");
        let running = self.boot_version();
        let mut body = json!({"versoes":{"repo":repo,"backend":running.0},"versao_legivel":{"repo":readable,"backend":running.1,"remoto":self.readable_version(&format!("origin/{}",self.target()?))},"atras":changes.len(),"atualizacao_disponivel":unknown||!changes.is_empty()||switching||published.stale,"servidor_desatualizado":published.stale,"api_publicado":published.api,"mudancas":changes,"passos":self.pending_steps()?,"pre_voo":pre,"estado":null});
        body["estado"]=self.screen_state().into_value();
        Ok(body)
    }
}
pub fn clear_published() {
    PUBLISHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}
