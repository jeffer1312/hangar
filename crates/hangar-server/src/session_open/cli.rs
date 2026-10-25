//! `hangar-server open|resolve`: o lançador do Hangar como comando de terminal. O mesmo
//! `resolve_open()` do servidor, lendo os mesmos arquivos, para o terminal decidir igual com o servidor
//! fora do ar. Subcomando do `hangar-server` porque ele já está instalado em toda máquina.
//!
//!   hangar-server resolve [PEDIDO]  resolve nesta máquina e imprime o spec, sem efeito nenhum
//!   hangar-server open [PEDIDO]     pede ao servidor; fora do ar, abre aqui só a sessão com terminal
//!                                   (o servidor adota quando voltar); sem terminal, só resolve
//!
//! Token, porta e as opções `CP_*` vêm do `~/.hangar/config/.env` ou, sem ele, do `backend/.env` da
//! cópia do Hangar: a de onde o binário mora (checkout) ou a que o servidor anotou ao subir
//! (`~/.hangar/backend-dir`, para o `~/.hangar/bin`).
//! PEDIDO é o JSON do `POST /api/sessions`; omitido, vem da entrada padrão. Recusa sai em stderr no
//! envelope `{"detail": …}` com código 1; uso errado, 2; sem token no `.env`, 4 (nada é enviado).
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::accounts::environment::AccountEnvironment;
use super::claude::PluginBridge;
use super::local::{LocalHost, dotenv};
use super::open::{open_session, opens_here};
use super::{OpenCaller, OpenError, SessionOpenRequest, facts::LocalFacts, resolve_open};

/// `args` sem o nome do programa: `[open|resolve, PEDIDO?]`.
pub fn run(args: &[String]) -> ExitCode {
    let (command, body) = match args {
        [command] => (command.as_str(), None),
        [command, body] => (command.as_str(), Some(body.clone())),
        _ => return usage(),
    };
    if !matches!(command, "resolve" | "open") {
        return usage();
    }
    let body = match body {
        Some(body) => body.into_bytes(),
        None => {
            let mut bytes = Vec::new();
            if let Err(error) = std::io::stdin().read_to_end(&mut bytes) {
                eprintln!("hangar-server: entrada ilegível: {error}");
                return ExitCode::from(2);
            }
            bytes
        }
    };
    let request = match SessionOpenRequest::parse(&body) {
        Ok(request) => request,
        Err(error) => return refused(&error),
    };
    let env_path = match env_file() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("hangar-server: não consegui conferir o .env do servidor: {error}");
            return ExitCode::from(2);
        }
    };
    let server = env_path.as_ref().and_then(|path| std::fs::read_to_string(path).ok()).map(|text| dotenv(&text)).unwrap_or_default();
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("hangar-server: runtime: {error}");
            return ExitCode::from(2);
        }
    };
    if command == "open" {
        let Some(token) = server.get("CP_AUTH_TOKEN").filter(|t| !t.is_empty()) else {
            let place = env_path.map_or_else(|| "nem ~/.hangar/config/.env nem backend/.env acima do comando".to_owned(), |p| p.display().to_string());
            eprintln!("hangar-server: sem CP_AUTH_TOKEN no .env do servidor ({place}); nada foi enviado");
            return ExitCode::from(4);
        };
        let port = server.get("CP_PORT").map_or("8765", String::as_str);
        match runtime.block_on(ask_server(&format!("http://127.0.0.1:{port}"), token, &body)) {
            Ask::Answered(status, text) if (200..300).contains(&status) => {
                println!("{text}");
                return ExitCode::SUCCESS;
            }
            Ask::Answered(_, text) => {
                eprintln!("{text}");
                return ExitCode::from(1);
            }
            // Sem resposta a sessão pode estar nascendo lá: resolver aqui abriria uma segunda.
            Ask::Uncertain(reason) => {
                eprintln!("hangar-server: o servidor não respondeu ({reason}); confira a lista antes de tentar de novo");
                return ExitCode::from(1);
            }
            Ask::Down(reason) => eprintln!("hangar-server: servidor fora do ar ({reason}); segue nesta máquina"),
        }
    }
    let facts = LocalFacts::new(AccountEnvironment::from_map(server_env(std::env::vars(), &server)), runtime.handle().clone()).without_server();
    // Quem roda o comando no terminal da máquina é o dono.
    let resolved = match resolve_open(&request, &OpenCaller::Owner, &facts) {
        Ok(resolved) => resolved,
        Err(error) => return refused(&error),
    };
    // Sem terminal o processo é do servidor: com ele fora, o comando só resolve.
    if command == "open" && opens_here(&resolved.spec) && resolved.spec.surface == super::Surface::Terminal {
        // A mesma abertura do servidor: token e porta do `.env` dele, para a ponte do plugin
        // valer quando ele voltar.
        let bridge = server.get("CP_AUTH_TOKEN").filter(|t| !t.is_empty())
            .map(|token| PluginBridge { port: server.get("CP_PORT").and_then(|p| p.parse().ok()).unwrap_or(8765), secret: token.clone() });
        let host = LocalHost::new(server_env(std::env::vars(), &server), bridge);
        return match runtime.block_on(open_session(&host, resolved, &OpenCaller::Owner)) {
            Ok(opened) => {
                println!("{}", opened.body);
                ExitCode::SUCCESS
            }
            Err(error) => refused(&error),
        };
    }
    println!("{}", serde_json::to_string_pretty(&resolved).unwrap_or_default());
    if command == "open" { ExitCode::from(3) } else { ExitCode::SUCCESS }
}

/// O `.env` do servidor: `~/.hangar/config/.env` ou, sem ele, o da `backend/` da cópia do Hangar.
fn env_file() -> std::io::Result<Option<PathBuf>> { crate::user_data::env_file(backend_dir().as_deref()) }

/// Pasta `backend/` de uma cópia do Hangar. O `.env` pode já estar em `~/.hangar/config/`, então um
/// arquivo que só o Hangar tem também a identifica (`app/main.py` casaria com qualquer projeto).
pub fn is_backend(dir: &std::path::Path) -> bool {
    dir.join(".env").is_file() || dir.join("app").join("rust_server.py").is_file()
}

/// A `backend/` da raiz da instalação (`install_root::root`): o `~/.hangar/current` do pacote, ou a
/// cópia acima do binário (checkout), ou a que o servidor anotou ao subir (binário em `~/.hangar/bin`).
pub fn backend_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)?;
    Some(crate::install_root::root(&home)?.join("backend")).filter(|dir| is_backend(dir))
}

/// O ambiente que o servidor teria: a conta e as opções `CP_*` de quem chama não valem (uma sessão
/// dentro da conta B leria a configuração da B); as `CP_*` vêm do `.env` do servidor.
fn server_env(caller: impl Iterator<Item = (String, String)>, server: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = caller
        .filter(|(key, _)| !matches!(key.as_str(), "CLAUDE_CONFIG_DIR" | "CODEX_HOME") && !key.starts_with("CP_"))
        .collect();
    env.extend(server.iter().filter(|(key, _)| key.starts_with("CP_")).map(|(k, v)| (k.clone(), v.clone())));
    env
}

enum Ask { Answered(u16, String), Down(String), Uncertain(String) }

// Acima dos 180 s que a abertura no servidor pode levar: a resposta dele chega antes da desistência.
const OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(190);

async fn ask_server(base: &str, token: &str, body: &[u8]) -> Ask {
    let client = match crate::http_client().timeout(OPEN_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => return Ask::Uncertain(error.to_string()),
    };
    let sent = client.post(format!("{base}/api/sessions")).bearer_auth(token)
        .header("content-type", "application/json").body(body.to_vec()).send().await;
    match sent {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.text().await {
                Ok(text) => answer(status, text),
                Err(error) => Ask::Uncertain(error.to_string()),
            }
        }
        Err(error) if error.is_connect() => Ask::Down(error.to_string()),
        Err(error) => Ask::Uncertain(error.to_string()),
    }
}

/// O Hangar sem o Rust (modo Python) responde mas não abre: conta como fora, e a sessão nasce aqui.
fn answer(status: u16, text: String) -> Ask {
    let code = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|body| body["detail"]["code"].as_str().map(str::to_owned));
    if status == 421 || code.as_deref() == Some("session_open_needs_rust_server") {
        return Ask::Down(format!("HTTP {status} sem o servidor Rust"));
    }
    Ask::Answered(status, text)
}

fn refused(error: &OpenError) -> ExitCode {
    eprintln!("{}", error.body());
    ExitCode::from(1)
}

fn usage() -> ExitCode {
    eprintln!("uso: hangar-server resolve|open [PEDIDO-JSON]");
    ExitCode::from(2)
}

/// Onde o servidor anota a pasta `backend/` dele, para o comando instalado longe dela achar o `.env`.
pub(crate) fn pointer(home: &std::path::Path) -> PathBuf { home.join(".hangar").join("backend-dir") }

/// O servidor anota a `backend/` de onde roda (a pasta de trabalho dele) ao subir.
pub fn publish_backend_dir(home: &std::path::Path, backend: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(home.join(".hangar"))?;
    crate::runtime::queue::atomic_write(&pointer(home), backend.to_string_lossy().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::{Ask, OPEN_TIMEOUT, answer};

    /// A abertura no servidor pode levar até 180 s (bastão com resumo pelo modelo): desistir antes vira erro
    /// falso, e tentar de novo abre uma segunda sessão `-2`.
    #[test]
    fn the_command_waits_longer_than_the_server_open() {
        assert!(OPEN_TIMEOUT > std::time::Duration::from_secs(180), "{OPEN_TIMEOUT:?}");
    }

    /// O Hangar no modo Python (Rust caído) não abre: o comando abre na máquina, como com ele fora.
    #[test]
    fn a_server_without_rust_counts_as_down() {
        let python = r#"{"detail":{"code":"session_open_needs_rust_server","params":{},"msg":"abrir e reabrir sessão precisam do servidor Rust, que não está rodando"}}"#;
        assert!(matches!(answer(503, python.into()), Ask::Down(_)));
        assert!(matches!(answer(421, String::new()), Ask::Down(_)));
        assert!(matches!(answer(409, r#"{"detail":{"code":"erro_nome_em_uso"}}"#.into()), Ask::Answered(409, _)));
        assert!(matches!(answer(503, r#"{"detail":{"code":"runtime_unhealthy"}}"#.into()), Ask::Answered(503, _)));
    }
}
