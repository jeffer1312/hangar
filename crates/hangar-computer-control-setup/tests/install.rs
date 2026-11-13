//! R3: instalar e atualizar o hcu chamando as funções do crate, com um servidor HTTP local no papel
//! do GitHub, HOME e diário em pastas temporárias. A troca do exe em uso no Windows não tem prova aqui.
// Fora do Linux x86_64 só a prova da plataforma sem binário usa o cenário.
#![cfg_attr(not(all(target_os = "linux", target_arch = "x86_64")), allow(dead_code, unused_imports))]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use hangar_computer_control_setup::{Github, Paths, Policy, REPO, Swapped, install_if_missing, update_tick};

const LINUX: &str = "hangar-computer-control-linux-x86_64";
const EXE: &str = "windows-agent.exe";

/// O GitHub de mentira: uma release (ou um status de erro) e os assets por nome.
#[derive(Default)]
struct Fake {
    release: Mutex<Option<Value>>,
    assets: Mutex<HashMap<String, Vec<u8>>>,
    /// Caminho, `User-Agent` e `Accept` de cada pedido.
    seen: Mutex<Vec<(String, String, String)>>,
}

struct Scene {
    _root: tempfile::TempDir,
    paths: Paths,
    diary: PathBuf,
    fake: Arc<Fake>,
    github: Github,
    http: reqwest::Client,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Scene {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Scene {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let fake = Arc::new(Fake::default());
        let app = axum::Router::new().fallback(serve).with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            diary: root.path().join("diario"),
            paths: Paths::new(home, Vec::new()),
            _root: root,
            fake,
            github: Github { api: base.clone(), download: base },
            http: hangar_core::net::http_client().build().unwrap(),
            server,
        }
    }

    /// Release `tag` com os dois assets (`linux`: o binário Linux está na lista e é servido).
    fn publish(&self, tag: &str, body: &str, linux: bool) {
        let mut assets = vec![json!({"name": EXE})];
        if linux {
            assets.push(json!({"name": LINUX}));
        }
        *self.fake.release.lock().unwrap() = Some(json!({"tag_name": tag, "body": body, "assets": assets}));
        let mut served = self.fake.assets.lock().unwrap();
        served.clear();
        served.insert(format!("/{REPO}/releases/download/{tag}/{EXE}"), format!("MZ-{tag}").into_bytes());
        if linux {
            served.insert(format!("/{REPO}/releases/download/{tag}/{LINUX}"), format!("ELF-{tag}").into_bytes());
        }
    }

    fn unpublish_asset(&self, tag: &str, name: &str) {
        self.fake.assets.lock().unwrap().remove(&format!("/{REPO}/releases/download/{tag}/{name}"));
    }

    fn requests(&self) -> Vec<String> {
        self.fake.seen.lock().unwrap().iter().map(|(path, ..)| path.clone()).collect()
    }

    async fn install(&self) -> Result<Swapped, hangar_computer_control_setup::Error> {
        install_if_missing(&self.paths, &self.http, &self.github, Some(&self.diary)).await
    }

    async fn update(&self, policy: &Policy) -> Result<Swapped, hangar_computer_control_setup::Error> {
        update_tick(&self.paths, &self.http, &self.github, policy, Some(&self.diary)).await
    }

    fn dir(&self) -> PathBuf {
        self.paths.install_dir()
    }

    /// Pacote `tag` já instalado como o Python deixava.
    fn installed(&self, tag: &str) {
        std::fs::create_dir_all(self.paths.targets_dir()).unwrap();
        std::fs::write(self.paths.binary(), format!("ELF-{tag}")).unwrap();
        std::fs::write(self.paths.exe(), format!("MZ-{tag}")).unwrap();
        std::fs::write(self.paths.install_json(), format!("{{\n  \"tag\": \"{tag}\"\n}}")).unwrap();
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn target(&self, name: &str, text: &str) -> PathBuf {
        let path = self.paths.targets_dir().join(format!("{name}-agent.json"));
        std::fs::write(&path, text).unwrap();
        path
    }

    /// As linhas do diário, já lidas como JSON.
    fn diary_lines(&self) -> Vec<Value> {
        let Ok(listing) = std::fs::read_dir(&self.diary) else { return Vec::new() };
        let mut lines = Vec::new();
        for entry in listing {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            lines.extend(text.lines().map(|line| serde_json::from_str::<Value>(line).unwrap()));
        }
        lines
    }

    fn diary_codes(&self) -> Vec<String> {
        self.diary_lines().iter().filter_map(|l| l["codigo"].as_str().map(str::to_owned)).collect()
    }
}

async fn serve(State(fake): State<Arc<Fake>>, request: Request) -> Response {
    let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    let path = request.uri().path().to_owned();
    fake.seen.lock().unwrap().push((path.clone(), header("user-agent"), header("accept")));
    if path == format!("/repos/{REPO}/releases/latest") {
        return match fake.release.lock().unwrap().clone() {
            Some(release) => ([(axum::http::header::CONTENT_TYPE, "application/json")], release.to_string()).into_response(),
            None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    match fake.assets.lock().unwrap().get(&path) {
        Some(bytes) => bytes.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn latest_policy() -> Policy {
    Policy { automations: true, package_channel: Some("latest".into()) }
}

fn state_of(paths: &Paths) -> Value {
    hangar_computer_control_setup::state(paths, &hangar_computer_control_setup::Facts::default()).unwrap()
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux {
    use super::*;

    /// D3.2, ROT-80..83, ROT-85, ROT-94, HIS-20: sem nada instalado, baixa os dois assets com os
    /// cabeçalhos do Python, binário 0755, `install.json` só com a tag e a pasta `targets/`.
    #[tokio::test]
    async fn his20_install_downloads_both_assets() {
        let s = Scene::new().await;
        s.publish("v0.7.0", "", true);
        assert_eq!(s.install().await.unwrap(), Swapped::Yes { tag: "v0.7.0".into() });
        assert_eq!(s.read(&s.paths.binary()).as_deref(), Some("ELF-v0.7.0"));
        assert_eq!(s.read(&s.paths.exe()).as_deref(), Some("MZ-v0.7.0"));
        assert_eq!(s.read(&s.paths.install_json()).as_deref(), Some("{\n  \"tag\": \"v0.7.0\"\n}"));
        assert!(s.paths.targets_dir().is_dir());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(s.paths.binary()).unwrap().permissions().mode() & 0o777, 0o755);
        let seen = s.fake.seen.lock().unwrap().clone();
        let paths: Vec<&str> = seen.iter().map(|(p, ..)| p.as_str()).collect();
        assert_eq!(paths, [
            format!("/repos/{REPO}/releases/latest"),
            format!("/{REPO}/releases/download/v0.7.0/{EXE}"),
            format!("/{REPO}/releases/download/v0.7.0/{LINUX}"),
        ]);
        for (_, agent, accept) in &seen {
            assert_eq!((agent.as_str(), accept.as_str()), ("hangar", "application/vnd.github+json"));
        }
        // Primeira instalação não é troca: nada de `.anterior` nem novidade.
        assert!(!s.dir().join("hangar-computer-control.anterior").exists());
        assert!(!s.paths.home.join(".hangar/update-news.json").exists());
        assert_eq!(state_of(&s.paths)["unavailable"], Value::Null);
    }

    /// D3.2: com o binário e o exe no lugar, a subida não pergunta nada ao GitHub.
    #[tokio::test]
    async fn d3_2_install_does_nothing_when_both_files_exist() {
        let s = Scene::new().await;
        s.installed("v0.6.0");
        s.publish("v0.7.0", "", true);
        assert_eq!(s.install().await.unwrap(), Swapped::No);
        assert!(s.requests().is_empty());
        assert_eq!(s.read(&s.paths.binary()).as_deref(), Some("ELF-v0.6.0"));
    }

    /// D3.2: só o exe faltando já instala, mesmo fora do modo pacote (sem portão).
    #[tokio::test]
    async fn d3_2_missing_exe_installs_without_gates() {
        let s = Scene::new().await;
        s.installed("v0.6.0");
        std::fs::remove_file(s.paths.exe()).unwrap();
        s.publish("v0.7.0", "", true);
        assert_eq!(s.install().await.unwrap(), Swapped::Yes { tag: "v0.7.0".into() });
        assert_eq!(s.read(&s.paths.exe()).as_deref(), Some("MZ-v0.7.0"));
    }

    /// D3.6, D1.5, ROT-81: falha do GitHub vai ao diário e fica gravada; o GET mostra
    /// `install_failed` com o código. A próxima instalação que dá certo limpa.
    #[tokio::test]
    async fn d3_6_failed_install_shows_in_state_until_one_works() {
        let s = Scene::new().await;
        let error = s.install().await.unwrap_err();
        assert_eq!(error.code, "computer_control_release_failed");
        assert!(error.msg.contains("o GitHub respondeu 500"), "{}", error.msg);
        assert_eq!(s.diary_codes(), ["computer_control_release_failed"]);
        let state = state_of(&s.paths);
        assert_eq!(state["unavailable"], "install_failed");
        assert_eq!(state["install_error"], "computer_control_release_failed");
        assert!(!s.paths.binary().exists());
        s.publish("v0.7.0", "", true);
        s.install().await.unwrap();
        let state = state_of(&s.paths);
        assert_eq!(state["unavailable"], Value::Null);
        assert!(state.get("install_error").is_none());
        assert!(!s.paths.install_error_file().exists());
    }

    /// ROT-80: release sem `tag_name` texto é falha com código, e nada é baixado.
    #[tokio::test]
    async fn rot80_release_without_tag_is_a_failure() {
        let s = Scene::new().await;
        *s.fake.release.lock().unwrap() = Some(json!({"tag_name": null, "assets": []}));
        assert_eq!(s.install().await.unwrap_err().code, "computer_control_release_failed");
        assert_eq!(s.requests().len(), 1);
        assert!(!s.paths.exe().exists());
    }

    /// D3.8, ROT-84: release sem o binário Linux não instala nada e vai ao diário com o código.
    #[tokio::test]
    async fn d3_8_release_without_linux_binary_installs_nothing() {
        let s = Scene::new().await;
        s.publish("v0.7.0", "", false);
        assert_eq!(s.install().await.unwrap_err().code, "computer_control_release_sem_binario");
        assert!(!s.paths.binary().exists() && !s.paths.exe().exists() && !s.paths.install_json().exists());
        assert_eq!(s.requests(), [format!("/repos/{REPO}/releases/latest")]);
        assert_eq!(s.diary_codes(), ["computer_control_release_sem_binario"]);
        assert_eq!(state_of(&s.paths)["install_error"], "computer_control_release_sem_binario");
    }

    /// D3.8 na atualização: a instalação de antes fica inteira.
    #[tokio::test]
    async fn d3_8_update_to_a_release_without_linux_binary_keeps_everything() {
        let s = Scene::new().await;
        s.installed("v0.6.0");
        s.publish("v0.7.0", "", false);
        assert_eq!(s.update(&latest_policy()).await.unwrap_err().code, "computer_control_release_sem_binario");
        assert_eq!(s.read(&s.paths.binary()).as_deref(), Some("ELF-v0.6.0"));
        assert_eq!(s.read(&s.paths.install_json()).as_deref(), Some("{\n  \"tag\": \"v0.6.0\"\n}"));
        assert!(!s.paths.install_error_file().exists(), "falha da atualização não é bloqueio: binário existe");
    }

    /// HIS-32, HIS-37: 404 de qualquer asset deixa a instalação anterior inteira (binário, exe,
    /// `install.json`) e sem `.anterior` novo.
    #[tokio::test]
    async fn his32_download_failure_keeps_previous_install() {
        for missing in [EXE, LINUX] {
            let s = Scene::new().await;
            s.installed("v0.6.0");
            s.publish("v0.7.0", "", true);
            s.unpublish_asset("v0.7.0", missing);
            let error = s.update(&latest_policy()).await.unwrap_err();
            assert_eq!(error.code, "computer_control_release_failed", "{missing}");
            assert!(error.msg.contains("o GitHub respondeu 404"), "{}", error.msg);
            assert_eq!(s.read(&s.paths.binary()).as_deref(), Some("ELF-v0.6.0"));
            assert_eq!(s.read(&s.paths.exe()).as_deref(), Some("MZ-v0.6.0"));
            assert_eq!(s.read(&s.paths.install_json()).as_deref(), Some("{\n  \"tag\": \"v0.6.0\"\n}"));
            let leftovers: Vec<String> = std::fs::read_dir(s.dir()).unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains("anterior") || n.contains("novo"))
                .collect();
            assert!(leftovers.is_empty(), "{leftovers:?}");
            assert_eq!(s.diary_codes(), ["computer_control_release_failed"]);
        }
    }

    /// D3.3: só troca com automações ligadas, modo pacote e canal `latest`, e só com tag diferente.
    #[tokio::test]
    async fn d3_3_update_gates() {
        let s = Scene::new().await;
        s.installed("v0.6.0");
        s.publish("v0.7.0", "", true);
        let gated = [
            Policy { automations: false, package_channel: Some("latest".into()) },
            Policy { automations: true, package_channel: None },
            Policy { automations: true, package_channel: Some("beta".into()) },
        ];
        for policy in &gated {
            assert_eq!(s.update(policy).await.unwrap(), Swapped::No, "{policy:?}");
        }
        assert!(s.requests().is_empty(), "portão fechado não vai ao GitHub");
        s.publish("v0.6.0", "", true);
        assert_eq!(s.update(&latest_policy()).await.unwrap(), Swapped::No);
        assert_eq!(s.requests().len(), 1, "mesma tag: só a consulta da release");
        assert_eq!(s.read(&s.paths.binary()).as_deref(), Some("ELF-v0.6.0"));
        s.publish("v0.7.0", "", true);
        assert_eq!(s.update(&latest_policy()).await.unwrap(), Swapped::Yes { tag: "v0.7.0".into() });
        assert_eq!(s.read(&s.paths.install_json()).as_deref(), Some("{\n  \"tag\": \"v0.7.0\"\n}"));
    }

    /// D3.4, D3.5: troca com um processo vivo rodando o binário velho; o processo não cai, o novo
    /// toma o lugar e o velho fica como `.anterior` (um só, sobrescrito a cada troca).
    #[tokio::test]
    async fn d3_4_swaps_with_a_live_process_and_keeps_one_anterior() {
        let s = Scene::new().await;
        s.installed("v0.6.0");
        // Um executável de verdade no lugar do binário: escrever por cima dele daria ETXTBSY.
        std::fs::copy("/bin/sh", s.paths.binary()).unwrap();
        let old = std::fs::read(s.paths.binary()).unwrap();
        let mut live = std::process::Command::new(s.paths.binary()).args(["-c", "sleep 300; :"]).spawn().unwrap();
        s.publish("v0.7.0", "", true);
        assert_eq!(s.update(&latest_policy()).await.unwrap(), Swapped::Yes { tag: "v0.7.0".into() });
        assert!(live.try_wait().unwrap().is_none(), "o processo do hcu caiu na troca");
        assert_eq!(s.read(&s.paths.binary()).as_deref(), Some("ELF-v0.7.0"));
        assert_eq!(std::fs::read(s.dir().join("hangar-computer-control.anterior")).unwrap(), old);
        assert_eq!(s.read(&s.dir().join("windows-agent.exe.anterior")).as_deref(), Some("MZ-v0.6.0"));
        s.publish("v0.8.0", "", true);
        s.update(&latest_policy()).await.unwrap();
        assert_eq!(s.read(&s.dir().join("hangar-computer-control.anterior")).as_deref(), Some("ELF-v0.7.0"));
        let anteriores = std::fs::read_dir(s.dir()).unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".anterior")).count();
        assert_eq!(anteriores, 2, "um `.anterior` por arquivo");
        assert!(live.try_wait().unwrap().is_none());
        live.kill().unwrap();
        live.wait().unwrap();
    }

    /// D3.11, P3: a troca grava a novidade do hcu sem tocar a versão do Hangar; corpo só com o
    /// link `**Full Changelog**` fica só o título.
    #[tokio::test]
    async fn d3_11_swap_records_the_news() {
        let s = Scene::new().await;
        s.installed("v0.6.0");
        hangar_update_news::unseen(&s.paths.home, "phone").unwrap();
        s.publish("v0.7.0", "Agente mais rápido no Windows.", true);
        s.update(&latest_policy()).await.unwrap();
        s.publish("v0.8.0", "**Full Changelog**: https://github.com/jeffer1312/hangar-computer-control/compare/v0.7.0...v0.8.0", true);
        s.update(&latest_policy()).await.unwrap();
        let news = hangar_update_news::unseen(&s.paths.home, "phone").unwrap().news;
        assert_eq!(news, [
            json!({"id": "computer-control-v0.8.0", "title": "Computer Use atualizado para v0.8.0", "kind": "improvement",
                "text": "", "platforms": "all", "try_it": null, "animation": null}),
            json!({"id": "computer-control-v0.7.0", "title": "Computer Use atualizado para v0.7.0", "kind": "improvement",
                "text": "Agente mais rápido no Windows.", "platforms": "all", "try_it": null, "animation": null}),
        ]);
        assert_eq!(hangar_update_news::known_version(&s.paths.home).unwrap(), None);
    }

    /// ROT-89..93, HIS-17, HIS-29..31, HIS-35, HIS-56, HIS-60, D3.9: cada alvo migrado só se mudou;
    /// o que não migrou vai ao diário com os nomes, sem resposta de rota.
    #[tokio::test]
    async fn rot90_targets_are_migrated_and_skipped_ones_go_to_the_diary() {
        let s = Scene::new().await;
        s.installed("v0.6.0");
        let bin = s.paths.binary().to_string_lossy().into_owned();
        let exe = s.paths.exe().to_string_lossy().into_owned();
        let python = s.target("a-python", r#"{"transport": "local", "command": ["python3", "/hcu/linux_agent.py"], "request_timeout": 15}"#);
        let wrapped = s.target("b-sh", r#"{"transport": "local", "command": ["sh", "-c", "export HYPRLAND_INSTANCE_SIGNATURE=$(ls /run/user/1000/hypr | head -1); exec /usr/bin/python3 /hcu/linux_agent.py"]}"#);
        let ssh = s.target("c-ssh", r#"{"transport": "ssh", "host": "administrator@vm", "agent_path": "C:/velho/windows-agent.exe"}"#);
        let custom_text = r#"{"transport": "local", "command": ["/opt/run.sh", "linux_agent.py"]}"#;
        let custom = s.target("d-custom", custom_text);
        let broken = s.target("e-quebrado", "not-json");
        let same_text = format!(r#"{{"transport":"local","command":["{bin}","agent"]}}"#);
        let same = s.target("f-igual", &same_text);
        s.publish("v0.7.0", "", true);
        s.update(&latest_policy()).await.unwrap();
        let parsed = |path: &Path| serde_json::from_str::<Value>(&s.read(path).unwrap()).unwrap();
        assert_eq!(parsed(&python), json!({"transport": "local", "command": [bin, "agent"], "request_timeout": 15}));
        assert_eq!(parsed(&wrapped)["command"][2],
            format!("export HYPRLAND_INSTANCE_SIGNATURE=$(ls /run/user/1000/hypr | head -1); exec {bin} agent"));
        assert_eq!(parsed(&ssh), json!({"transport": "ssh", "host": "administrator@vm", "agent_path": exe}));
        assert_eq!(s.read(&custom).as_deref(), Some(custom_text), "comando custom não é tocado");
        assert_eq!(s.read(&broken).as_deref(), Some("not-json"), "alvo ilegível não é sobrescrito");
        assert_eq!(s.read(&same).as_deref(), Some(same_text.as_str()), "alvo igual não é regravado");
        // Gravado como o `json.dumps(indent=2)` do Python, sem quebra no fim.
        assert!(s.read(&python).unwrap().starts_with("{\n  \"transport\": \"local\",\n  \"command\": [\n"));
        assert!(!s.read(&python).unwrap().ends_with('\n'));
        let skipped: Vec<Value> = s.diary_lines().into_iter().filter(|l| l["codigo"] == "migration_skipped").collect();
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert_eq!(skipped[0]["alvos"], "d-custom-agent.json, e-quebrado-agent.json");
        assert_eq!(skipped[0]["evento"], "computer_control.migracao");
    }
}

/// D3.7: macOS e Linux fora do x86_64 não baixam nada nem criam arquivo.
#[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
#[tokio::test]
async fn d3_7_unsupported_platform_downloads_nothing() {
    let s = Scene::new().await;
    s.publish("v0.7.0", "", true);
    assert_eq!(s.install().await.unwrap(), Swapped::No);
    assert_eq!(s.update(&latest_policy()).await.unwrap(), Swapped::No);
    assert!(s.requests().is_empty());
    assert!(!s.dir().exists());
    assert_eq!(state_of(&s.paths)["unavailable"], "platform");
}
