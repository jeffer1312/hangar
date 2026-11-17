//! O `codex app-server` da sessão: porta de loopback, espera até aceitar conexão, a cauda do stderr
//! dele quando morre, e a linha do diário com a cauda à parte, só para quem tem acesso à máquina.
use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::Instant;

use super::Screen;

/// Quanto esperar o app-server aceitar conexão (a TUI conecta uma vez só e morre se ele não está lá).
pub(crate) const PORT_WAIT: Duration = Duration::from_secs(60);
/// Quanto do stderr dele vai à tela e ao arquivo privado: a última linha costuma dizer o motivo.
const TAIL_MAX: u64 = 2000;

/// Reserva e solta uma porta de loopback. Quem perde a corrida pelo bind vê o servidor sair, e a
/// espera falha em vez de deixar a TUI num servidor alheio.
pub(crate) fn free_endpoint() -> io::Result<String> {
    let socket = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(format!("ws://127.0.0.1:{}", socket.local_addr()?.port()))
}

/// O stderr dos app-servers desta vida num arquivo temporário: um cano sem leitor encheria numa sessão
/// longa, e jogar fora perderia o motivo de uma largada que falhou.
pub(crate) struct Tail { file: Option<std::fs::File> }

impl Tail {
    pub fn new() -> io::Result<Self> { Ok(Self { file: Some(tempfile::tempfile()?) }) }

    pub fn none() -> Self { Self { file: None } }

    fn stdio(&self) -> Stdio {
        match self.file.as_ref().map(std::fs::File::try_clone) {
            Some(Ok(file)) => Stdio::from(file),
            _ => Stdio::null(),
        }
    }

    /// Os últimos bytes; volta ao fim, porque o mesmo arquivo segue sendo o stderr do próximo servidor.
    pub fn read(&mut self) -> io::Result<Vec<u8>> {
        let Some(file) = self.file.as_mut() else { return Ok(Vec::new()) };
        let size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(size.saturating_sub(TAIL_MAX)))?;
        let mut tail = Vec::new();
        file.by_ref().take(TAIL_MAX).read_to_end(&mut tail)?;
        file.seek(SeekFrom::End(0))?;
        Ok(tail)
    }
}

/// Uma falha do app-server, com os campos que o diário guarda; nunca o stderr, o endereço ou o token.
#[derive(Clone, Debug, Default)]
pub(crate) struct Failure {
    code: &'static str,
    stage: &'static str,
    returned: Option<i32>,
    limit_ms: Option<u64>,
    kind: Option<String>,
    errno: Option<i32>,
}

impl Failure {
    pub fn new(code: &'static str, stage: &'static str) -> Self { Self { code, stage, ..Self::default() } }

    pub fn kind(mut self, kind: &str) -> Self {
        self.kind = Some(kind.to_owned());
        self
    }

    pub fn returned(mut self, code: Option<i32>) -> Self {
        self.returned = code;
        self
    }

    pub fn limit(mut self, limit: Duration) -> Self {
        self.limit_ms = Some(limit.as_millis() as u64);
        self
    }

    pub fn error(mut self, error: &io::Error) -> Self {
        self.kind = Some(error_kind(error).to_owned());
        self.errno = error.raw_os_error();
        self
    }

    fn fields(&self, session: &str) -> serde_json::Map<String, Value> {
        let mut fields = serde_json::Map::new();
        fields.insert("sessao".into(), json!(session));
        fields.insert("provider".into(), json!("codex"));
        fields.insert("codigo".into(), json!(self.code));
        fields.insert("etapa".into(), json!(self.stage));
        if let Some(code) = self.returned { fields.insert("retorno".into(), json!(code)); }
        if let Some(limit) = self.limit_ms { fields.insert("limite_ms".into(), json!(limit)); }
        if let Some(kind) = &self.kind { fields.insert("erro_tipo".into(), json!(kind)); }
        if let Some(errno) = self.errno {
            fields.insert(if cfg!(windows) { "winerror" } else { "errno" }.into(), json!(errno));
        }
        fields
    }
}

/// O nome que o diário já conhecia para a falha de disco ou de processo.
pub(crate) fn error_kind(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::TimedOut => "TimeoutError",
        _ => "OSError",
    }
}

/// O diário exportável dos comandos do lançador e a cauda privada da última falha da sessão.
pub(crate) struct Diary { session: String, base: Option<PathBuf> }

impl Diary {
    pub fn new(session: &str, base: Option<PathBuf>) -> Self { Self { session: session.to_owned(), base } }

    /// Grava a falha e a cauda do stderr; devolve a cauda para a tela. Falha ao gravar a cauda vira
    /// outra linha do diário, nunca erro para quem chamou.
    pub fn failure(&self, failure: &Failure, tail: &mut Tail) -> Vec<u8> {
        let mut event = failure.fields(&self.session);
        event.insert("evento".into(), json!("codex.app_server.falhou"));
        event.insert("nivel".into(), json!("erro"));
        self.write(Value::Object(event));
        let bytes = match tail.read() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.tail_failed(failure, &error);
                return Vec::new();
            }
        };
        if let Err(error) = self.keep_tail(&bytes) {
            self.tail_failed(failure, &error);
        }
        bytes
    }

    fn tail_failed(&self, failure: &Failure, error: &io::Error) {
        let mut event = failure.fields(&self.session);
        event.extend(Failure::new("gravar_cauda", failure.stage).error(error).fields(&self.session).into_iter()
            .filter(|(key, _)| matches!(key.as_str(), "codigo" | "erro_tipo" | "errno" | "winerror")));
        event.insert("evento".into(), json!("codex.app_server.log_falhou"));
        event.insert("nivel".into(), json!("aviso"));
        self.write(Value::Object(event));
    }

    /// `<logs>/privado/codex-app-server-<sha256 da sessão>.log`, só a última cauda, pasta 0700 e arquivo 0600.
    fn keep_tail(&self, bytes: &[u8]) -> io::Result<()> {
        let Some(base) = &self.base else { return Ok(()) };
        std::fs::create_dir_all(base)?;
        let dir = base.join("privado");
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let mut file = tempfile::Builder::new().prefix(".codex-app-server-").tempfile_in(&dir)?;
        file.write_all(bytes)?;
        file.persist(dir.join(format!("codex-app-server-{}.log", digest(&self.session)))).map_err(|error| error.error)?;
        Ok(())
    }

    /// Um arquivo do diário só dos lançadores, sob trava própria: no Windows dois processos anexando no
    /// mesmo arquivo se sobrescrevem. Falha não chega a quem chamou.
    fn write(&self, event: Value) {
        let Some(base) = &self.base else { return };
        let diary = base.join("diario");
        let dir = diary.join("codex-tui");
        if std::fs::create_dir_all(&dir).is_err() { return; }
        let Ok(_held) = hangar_core::fs::lock_file(&diary.join("codex-tui.lock")) else { return };
        let _ = hangar_core::diag::write_diary(&dir, &[event], chrono::Local::now());
    }
}

/// Os 16 primeiros hexadecimais do sha256 do nome: o mesmo nome de arquivo da versão anterior.
fn digest(session: &str) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, session.as_bytes());
    hash.as_ref().iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

/// Um app-server desta vida.
pub(crate) struct Server {
    child: tokio::process::Child,
    pub pid: u32,
    status: Option<i32>,
    exited: bool,
    /// Vira verdade quando ele morre ou é derrubado: a escuta de threads desiste calada.
    pub gone: Arc<AtomicBool>,
    #[cfg(windows)]
    tree: hangar_core::terminal_process::CommandTree,
}

impl Server {
    /// Sobe no cwd da sessão, com o ambiente dela, stdout no lixo e o stderr na cauda. No Windows o Job
    /// leva a árvore inteira junto (o `codex` do npm é um atalho que sobe o `node`).
    pub fn spawn(argv: &[String], cwd: &str, env: &BTreeMap<String, String>, tail: &Tail) -> io::Result<Self> {
        let mut command = tokio::process::Command::new(&argv[0]);
        command.args(&argv[1..]).current_dir(cwd).env_clear().envs(env).stdin(Stdio::null()).stdout(Stdio::null()).stderr(tail.stdio());
        #[cfg(windows)]
        let mut tree = hangar_core::terminal_process::CommandTree::configure(&mut command)?;
        let child = command.spawn()?;
        #[cfg(windows)]
        tree.attach(&child)?;
        let pid = child.id().ok_or_else(|| io::Error::other("app-server sem pid"))?;
        Ok(Self { child, pid, status: None, exited: false, gone: Arc::new(AtomicBool::new(false)), #[cfg(windows)] tree })
    }

    /// Já saiu? Recolhe o código na primeira vez.
    pub fn exited(&mut self) -> bool {
        if !self.exited && let Ok(Some(status)) = self.child.try_wait() {
            self.exited = true;
            self.status = hangar_core::terminal_process::exit_code(status);
            self.gone.store(true, Ordering::Release);
        }
        self.exited
    }

    pub fn status(&self) -> Option<i32> { self.status }

    /// `terminate`, 5 s, `kill`. No Windows o Job inteiro.
    pub async fn stop(&mut self) {
        self.gone.store(true, Ordering::Release);
        if self.exited() { return; }
        #[cfg(unix)]
        {
            // SAFETY: `kill` só recebe o pid deste filho, ainda não recolhido.
            unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM); }
        }
        #[cfg(windows)]
        {
            use hangar_core::terminal_process::OwnedTree;
            let _ = self.tree.terminate();
        }
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await.is_err() {
            let _ = self.child.kill().await;
        }
        let _ = self.exited();
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.exited();
    }
}

/// O stderr dele é a única pista de por que não abriu a porta.
pub(crate) fn report_dead(server: &Server, endpoint: &str, stage: &'static str, tail: &mut Tail, diary: &Diary, screen: &dyn Screen) {
    let bytes = diary.failure(&Failure::new("processo_encerrou", stage).returned(server.status()), tail);
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim();
    let code = server.status().map_or_else(|| "None".to_owned(), |code| code.to_string());
    let mut line = format!("o app-server saiu com codigo {code} antes de abrir {endpoint}");
    if !text.is_empty() {
        line.push_str(":\n");
        line.push_str(text);
    }
    screen.say(&line);
}

/// Espera o app-server aceitar conexão: a TUI não tenta de novo, e subir antes do bind deixava o pane
/// morto em 1 s justamente com a máquina carregada.
pub(crate) async fn wait_port(server: &mut Server, endpoint: &str, limit: Duration, tail: &mut Tail, diary: &Diary, screen: &dyn Screen) -> bool {
    let address = endpoint.strip_prefix("ws://").unwrap_or(endpoint).to_owned();
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if server.exited() {
            report_dead(server, endpoint, "abrir_app_server", tail, diary, screen);
            return false;
        }
        if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(500), tokio::net::TcpStream::connect(address.as_str())).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    screen.say(&format!("app-server nao respondeu em {endpoint}"));
    let returned = if server.exited() { server.status() } else { None };
    diary.failure(&Failure::new("timeout", "abrir_app_server").returned(returned).limit(limit), tail);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lines(std::sync::Mutex<Vec<String>>);
    impl Screen for Lines {
        fn say(&self, line: &str) { self.0.lock().unwrap().push(line.to_owned()); }
    }

    fn diary_lines(base: &std::path::Path) -> Vec<Value> {
        let dir = base.join("diario").join("codex-tui");
        std::fs::read_dir(&dir).unwrap().flatten().flat_map(|entry| std::fs::read_to_string(entry.path()).unwrap()
            .lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>()).collect()
    }

    /// O nome da cauda privada é o da versão anterior: sha256 do nome da sessão, 16 hexadecimais.
    #[test]
    fn the_private_tail_name_is_the_session_digest() {
        // `launcher__app-server-dies-at-start`: sessão `codex-proj`.
        assert_eq!(digest("codex-proj"), "dae16489dfddfd3a");
    }

    /// Só a última cauda (2000 bytes) fica, em 0600 dentro de uma pasta 0700; o diário leva o código, a
    /// etapa e o retorno, nunca o stderr.
    #[test]
    fn a_failure_keeps_only_the_last_tail_and_the_diary_has_no_stderr() {
        let base = tempfile::tempdir().unwrap();
        let diary = Diary::new("codex-proj", Some(base.path().to_path_buf()));
        let mut tail = Tail::new().unwrap();
        tail.file.as_mut().unwrap().write_all(&b"linha de erro privado com token sk-SEGREDO\n".repeat(80)).unwrap();
        let bytes = diary.failure(&Failure::new("processo_encerrou", "abrir_app_server").returned(Some(3)), &mut tail);
        assert_eq!(bytes.len(), 2000);
        let kept = base.path().join("privado").join("codex-app-server-dae16489dfddfd3a.log");
        assert_eq!(std::fs::read(&kept).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(kept.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let [line] = diary_lines(base.path()).try_into().unwrap();
        for (field, value) in [("evento", json!("codex.app_server.falhou")), ("nivel", json!("erro")), ("sessao", json!("codex-proj")),
            ("provider", json!("codex")), ("codigo", json!("processo_encerrou")), ("retorno", json!(3)), ("etapa", json!("abrir_app_server")),
            ("origem", json!("servidor"))] {
            assert_eq!(line[field], value, "{field}");
        }
        assert!(!line.to_string().contains("SEGREDO") && line.get("limite_ms").is_none());
        // A segunda falha troca a cauda: só a última fica.
        tail.file.as_mut().unwrap().write_all(b"outra").unwrap();
        let second = diary.failure(&Failure::new("timeout", "abrir_app_server").limit(PORT_WAIT), &mut tail);
        assert!(second.ends_with(b"outra"));
        assert_eq!(std::fs::read(&kept).unwrap(), second);
        assert_eq!(std::fs::read_dir(kept.parent().unwrap()).unwrap().count(), 1, "nenhum temporário sobrou");
        assert_eq!(diary_lines(base.path())[1]["limite_ms"], 60000);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_that_dies_before_the_port_says_why() {
        let base = tempfile::tempdir().unwrap();
        let diary = Diary::new("cx", Some(base.path().to_path_buf()));
        let mut tail = Tail::new().unwrap();
        let argv: Vec<String> = ["/bin/sh", "-c", "echo 'fake app-server crashed' >&2; exit 9"].map(str::to_owned).to_vec();
        let env = BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
        let mut server = Server::spawn(&argv, "/", &env, &tail).unwrap();
        let lines = Lines(Default::default());
        let endpoint = free_endpoint().unwrap();
        assert!(!wait_port(&mut server, &endpoint, Duration::from_secs(10), &mut tail, &diary, &lines).await);
        assert_eq!(lines.0.lock().unwrap().as_slice(), [format!("o app-server saiu com codigo 9 antes de abrir {endpoint}:\nfake app-server crashed")]);
        assert!(server.gone.load(Ordering::Acquire));
        // Prazo: vivo e sem porta.
        let argv: Vec<String> = ["/bin/sh", "-c", "exec sleep 30"].map(str::to_owned).to_vec();
        let mut server = Server::spawn(&argv, "/", &env, &tail).unwrap();
        let lines = Lines(Default::default());
        assert!(!wait_port(&mut server, &endpoint, Duration::from_millis(300), &mut tail, &diary, &lines).await);
        assert_eq!(lines.0.lock().unwrap().as_slice(), [format!("app-server nao respondeu em {endpoint}")]);
        server.stop().await;
        assert!(server.exited());
        let codes: Vec<Value> = diary_lines(base.path()).iter().map(|line| line["codigo"].clone()).collect();
        assert_eq!(codes, [json!("processo_encerrou"), json!("timeout")]);
    }
}
