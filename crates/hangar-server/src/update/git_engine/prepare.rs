//! Dependências, tela e binários preparados antes de reiniciar o serviço.
use super::{
    Service,
    releases::{self, Held, Release, Source},
    tail,
};
use std::{fs, path::Path, time::Duration};
const SOURCES: [&str; 6] = [
    "frontend",
    "packages",
    "messages",
    "project.inlang",
    "package.json",
    "package-lock.json",
];
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}
impl Service {
    pub fn restore_dist(&self) -> Result<(), String> {
        let previous = self.root.join("frontend/.dist-velho");
        if previous.is_dir() {
            let target = self.root.join("frontend/dist");
            let _ = fs::remove_dir_all(&target);
            fs::rename(previous, target).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn publish_dist(&self, data: &[u8]) -> Result<Option<String>, String> {
        let front = self.root.join("frontend");
        let temp = tempfile::Builder::new()
            .prefix(".dist-baixado.")
            .tempdir_in(&front)
            .map_err(|e| e.to_string())?;
        crate::ui::unpack(data, temp.path()).map_err(|e| e.to_string())?;
        if !temp.path().join("index.html").is_file() {
            return Ok(Some("o dist do CI veio incompleto".into()));
        }
        let target = front.join("dist");
        let previous = front.join(".dist-velho");
        let _ = fs::remove_dir_all(&previous);
        if target.exists() {
            fs::rename(&target, &previous).map_err(|e| e.to_string())?;
        }
        fs::rename(temp.path(), target).map_err(|e| e.to_string())?;
        Ok(None)
    }
    pub fn update_dist(&self, source: &dyn Source) -> Result<Option<String>, String> {
        let mut args = vec!["status", "--porcelain", "--"];
        args.extend(SOURCES);
        let status = self.git(&args, 30)?;
        let branch = self
            .git(&["rev-parse", "--abbrev-ref", "HEAD"], 30)?
            .stdout
            .trim()
            .to_owned();
        let mut build = status.code != 0
            || !status.stdout.trim().is_empty()
            || !matches!(branch.as_str(), "main" | "master");
        let head = self
            .git(&["rev-parse", "HEAD"], 30)?
            .stdout
            .trim()
            .to_owned();
        let top = self
            .git(&["rev-parse", &format!("origin/{}", self.target()?)], 30)?
            .stdout
            .trim()
            .to_owned();
        if !build
            && !head.is_empty()
            && !top.is_empty()
            && head != top
            && self
                .git(&["merge-base", "--is-ancestor", &head, &top], 30)?
                .code
                == 0
        {
            let refs = SOURCES.map(|path| format!("HEAD:{path}"));
            let mut args = vec!["rev-parse"];
            args.extend(refs.iter().map(String::as_str));
            let command = std::iter::once("git".to_owned())
                .chain(["-C".into(), self.root.display().to_string()])
                .chain(args.iter().map(|arg| (*arg).to_owned())).collect::<Vec<_>>();
            let trees = super::super::process::run_capture_with_env(
                &command, &self.root, Duration::from_secs(30), &self.environment,
            ).map_err(|error| error.to_string())?;
            let key = if trees.code == 0 { releases::digest(trees.stdout.as_bytes()) } else { String::new() };
            let failure = if key.is_empty() {
                ("chave_ilegivel", String::new())
            } else {
                let url = format!("{}/frontend-dist-{key}.tar.gz", crate::ui::DIST_COMMITS_URL);
                let fetched = (|| -> Result<Option<String>, (&str, String)> {
                    let download = |url: &str, timeout| source.get(url, timeout).map_err(|error| {
                        (if error.status == Some(404) { "sem_chave" } else { "download_falhou" }, error.message)
                    });
                    let expected = download(&format!("{url}.sha256"), Duration::from_secs(15))?;
                    let data = download(&url, Duration::from_secs(180))?;
                    if String::from_utf8_lossy(&expected).split_whitespace().next()
                        != Some(&releases::digest(&data))
                    {
                        return Err(("sha256_divergente", String::new()));
                    }
                    self.publish_dist(&data).map_err(|error| ("download_falhou", error))
                })();
                match fetched {
                    Ok(None) => return Ok(None),
                    Ok(Some(problem)) => ("tar_incompleto", problem),
                    Err(problem) => problem,
                }
            };
            let (code, detail) = failure;
            if code == "sem_chave" {
                tracing::info!("tela da chave {key} não publicada; compilando aqui");
            } else {
                tracing::warn!("tela da chave {key} não veio do CI ({code}: {detail}); compilando aqui");
                let mut environment = crate::accounts::environment::AccountEnvironment::from_map(self.environment.clone());
                environment.home = self.home.clone();
                let diary = crate::diag::diary_dir_in(&environment);
                crate::diag::record_blocking(diary.as_deref(), &[serde_json::json!({
                    "evento":"atualizacao.tela_da_chave", "nivel":"aviso", "codigo":code, "detalhe":detail,
                })], "atualização da tela por chave");
            }
            build = true;
        }
        if build {
            let Some(npm) = self.binary("npm") else {
                return Ok(Some(
                    "tela não atualizada: ela precisa ser compilada aqui e não achei o npm".into(),
                ));
            };
            let dist = self.root.join("frontend/dist");
            let previous = self.root.join("frontend/.dist-velho");
            let _ = fs::remove_dir_all(&previous);
            if dist.is_dir() {
                if let Err(e) = copy_tree(&dist, &previous) {
                    let _ = fs::remove_dir_all(&previous);
                    return Ok(Some(format!(
                        "tela não atualizada: não consegui guardar a tela anterior ({e})"
                    )));
                }
            }
            let result = self.run(
                &[
                    npm.display().to_string(),
                    "--prefix".into(),
                    "frontend".into(),
                    "run".into(),
                    "build".into(),
                ],
                &self.root,
                600,
            );
            let output = match result {
                Ok(output) => output,
                Err(e) => {
                    let _ = self.restore_dist();
                    return Err(e);
                }
            };
            if output.code != 0 || !dist.join("index.html").is_file() {
                let error = self.restore_dist().err();
                return Ok(Some(format!(
                    "tela não atualizada: o build local do frontend falhou: {}{}",
                    tail(&output, 4),
                    error
                        .map(|e| format!("; e a tela anterior não voltou: {e}"))
                        .unwrap_or_default()
                )));
            }
            return Ok(None);
        }
        let url = "https://github.com/jeffer1312/hangar/releases/download/dist-latest";
        let result: Result<Option<String>, String> = (|| {
            let sha = source
                .get(&format!("{url}/frontend-dist.sha"), Duration::from_secs(15))
                .map_err(|e| e.message)?;
            let data = source
                .get(
                    &format!("{url}/frontend-dist.tar.gz"),
                    Duration::from_secs(180),
                )
                .map_err(|e| e.message)?;
            if let Some(problem) = self.publish_dist(&data)? {
                return Ok(Some(format!("tela não atualizada: {problem}")));
            }
            let remote = String::from_utf8_lossy(&sha).trim().to_owned();
            Ok((remote != head).then(|| {
                format!(
                    "a tela é do commit {}; este checkout está em {}",
                    remote.chars().take(8).collect::<String>(),
                    head.chars().take(8).collect::<String>()
                )
            }))
        })();
        Ok(match result {
            Ok(value) => value,
            Err(e) => Some(format!(
                "tela não atualizada: não consegui baixar o dist do CI ({e})"
            )),
        })
    }
    pub fn prepare(
        &self,
        topology: &str,
        dist: bool,
        release: Option<&Release>,
        source: &(dyn Source + Sync),
        platform: Option<&str>,
    ) -> Result<(), String> {
        let uv = self.binary("uv")
            .ok_or("uv nao encontrado: nao da pra sincronizar as dependencias do backend")?;
        let output = self.run(
            &[uv.display().to_string(), "sync".into()],
            &self.root.join("backend"),
            600,
        )?;
        if output.code != 0 {
            return Err(format!(
                "as dependencias do backend nao sincronizaram: {}",
                tail(&output, 6)
            ));
        }
        let hash = fs::read(self.root.join("package-lock.json"))
            .ok()
            .map(|b| releases::digest(&b))
            .unwrap_or_default();
        let marker = self.state_dir.join("package-lock.sha");
        let applied = match fs::read_to_string(&marker) {
            Ok(value) => crate::transcript::py::strip(&value).to_owned(),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                return Err("update_prepare_marker_invalid_utf8".into());
            },
            Err(_) => hash.clone(),
        };
        if !hash.is_empty() && hash != applied && self.root.join("node_modules").is_dir() {
            let npm = self.binary("npm")
                .ok_or("package-lock.json mudou e nao achei o npm pra instalar as dependencias")?;
            if topology == "windows" {
                self.stop_windows_front();
            }
            let output = self.run(
                &[
                    npm.display().to_string(),
                    "ci".into(),
                    "--workspace=@hangar/core".into(),
                    "--workspace=frontend".into(),
                ],
                &self.root,
                900,
            )?;
            if output.code != 0 {
                if topology == "windows" {
                    let _=self.command(&["powershell","-NoProfile","-Command","Start-ScheduledTask -TaskName hangar-frontend -ErrorAction SilentlyContinue"],30);
                }
                return Err(format!(
                    "as dependencias do front nao instalaram: {}",
                    tail(&output, 6)
                ));
            }
        }
        if !hash.is_empty() {
            fs::create_dir_all(&self.state_dir)
                .and_then(|_| fs::write(&marker, hash))
                .map_err(|e| e.to_string())?;
        }
        if dist {
            let mut held: Vec<Held> = vec![];
            let (ui, binaries) = std::thread::scope(|scope| {
                let ui = scope.spawn(|| self.update_dist(source));
                let tag = releases::tag_for(
                    &self
                        .git(&["rev-parse", "--abbrev-ref", "HEAD"], 30)
                        .map(|v| v.stdout.trim().to_owned())
                        .unwrap_or_default(),
                );
                let binaries = releases::fetch(
                    source,
                    releases::BASE,
                    Some(&tag),
                    platform,
                    &self.home.join(".hangar/bin"),
                    release,
                    Some(&mut held),
                );
                (
                    ui.join()
                        .unwrap_or_else(|_| Err("a preparação da tela foi interrompida".into())),
                    binaries,
                )
            });
            if let Ok(Some(warning)) = &ui {
                self.state().warn(warning);
            }
            for warning in binaries.unwrap_or_default() {
                self.state().warn(&warning);
            }
            ui?;
            for warning in releases::publish(&held) {
                self.state().warn(&warning);
            }
            self.refresh_chromium();
        }
        Ok(())
    }
}
