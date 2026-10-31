//! Diagnóstico utilizável com o servidor parado; as sondas não dependem do Python.
use super::{Service, network};
use serde_json::{Value, json};
use std::{
    io::IsTerminal,
    path::PathBuf,
    time::{Duration, Instant},
};
use unicode_normalization::UnicodeNormalization;
pub type Line = (String, String, String);
fn repair(platform: &str, service: &Service) -> String {
    match platform{"windows"=>r"Start-ScheduledTask hangar-backend  (log: %LOCALAPPDATA%\hangar\logs\privado\backend.log)".into(),"macos"=>format!("{} --restart  (log: ~/.hangar/logs/privado/backend.log)",service.backend.as_ref().and_then(|p|p.parent()).unwrap_or(&service.home).join("scripts/services-launchd.sh").display()),_=>"systemctl --user restart hangar-backend  (log: ~/.hangar/logs/privado/backend.log; journalctl --user -u hangar-backend -n 50)".into()}
}
pub fn diagnose(service: &Service, f: &Value, platform: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut add =
        |level: &str, title: String, repair: String| out.push((level.into(), title, repair));
    let port = service.port;
    if service.token.is_empty() || service.token == "change-me" {
        add(
            "erro",
            "token de acesso não definido".into(),
            "edite backend/.env: CP_AUTH_TOKEN=<sua senha> e reinicie o Hangar".into(),
        )
    } else {
        add("ok", "token de acesso definido".into(), String::new())
    }
    if f["port_open"] != true {
        add(
            "erro",
            format!("Hangar não responde na porta {port}"),
            repair(platform, service),
        )
    } else if let Some(t) = f["latency"].as_f64() {
        if t > 3. {
            add("aviso",format!("Hangar lento: respondeu em {t:.1} s na porta {port}"),"máquina sobrecarregada? veja CPU/memória; no Windows a vigia reinicia acima de 3 s".into())
        } else {
            add(
                "ok",
                format!(
                    "Hangar respondendo em http://127.0.0.1:{port} ({:.0} ms)",
                    t * 1000.
                ),
                String::new(),
            )
        }
    } else {
        add(
            "erro",
            format!("Hangar travado: a porta {port} aceita conexão mas não responde em 10 s"),
            repair(platform, service),
        )
    }
    if platform == "windows" {
        match f["priority"].as_str() {
            None => add(
                "aviso",
                "não deu para ler a prioridade do processo do backend".into(),
                String::new(),
            ),
            Some(p @ ("ociosa" | "abaixo do normal")) => add(
                "aviso",
                format!("backend rodando com prioridade {p} — com a CPU cheia ele fica lento"),
                "rode o Atualizar do app (ou install.ps1 -Update): a tarefa volta a nascer normal"
                    .into(),
            ),
            Some(p) => add("ok", format!("backend com prioridade {p}"), String::new()),
        }
    }
    match f["restarts"].as_array() {
        None => add(
            "aviso",
            "não deu para ler os reinícios automáticos do backend".into(),
            String::new(),
        ),
        Some(a) if a.is_empty() => add(
            "ok",
            "nenhum reinício automático do backend nos últimos 7 dias".into(),
            String::new(),
        ),
        Some(a) => {
            let where_ = if platform == "windows" {
                r"%LOCALAPPDATA%\hangar\logs\privado\hangar-vigia.log"
            } else {
                "journalctl --user -u hangar-backend --since -7d"
            };
            add(
                "aviso",
                format!(
                    "o vigia reiniciou o backend {} vez(es) nos últimos 7 dias (última: {})",
                    a.len(),
                    a.last().and_then(Value::as_str).unwrap_or("")
                ),
                format!("detalhes: {where_}"),
            );
        }
    }
    if f["mux"] == true {
        add(
            "ok",
            "multiplexador de terminal (tmux) no PATH".into(),
            String::new(),
        )
    } else {
        add(
            "erro",
            "tmux não encontrado — sem ele nenhuma sessão abre".into(),
            if platform == "windows" {
                "winget install marlocarlo.psmux"
            } else {
                "instale o tmux pelo gerenciador de pacotes"
            }
            .into(),
        )
    }
    if f["claude"] != true {
        let others = f["others"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| match s {
                "codex" => "Codex",
                "pi" => "Pi",
                "kimi" => "Kimi Code",
                _ => s,
            })
            .collect::<Vec<_>>();
        if others.is_empty() {
            add("erro","nenhum agente de código no PATH (Claude Code, Codex, Pi, omp ou Kimi Code)".into(),"curl -fsSL https://claude.ai/install.sh | bash  (Windows: irm https://claude.ai/install.ps1 | iex)".into())
        } else {
            add(
                "ok",
                format!("agentes de código: {}", others.join(", ")),
                String::new(),
            )
        }
    } else if f["logged_in"] == true {
        add("ok", "Claude Code instalado e logado".into(), String::new())
    } else {
        add(
            "erro",
            "Claude Code instalado mas sem login".into(),
            "abra um terminal, rode `claude` e siga o login; depois rode hangar-doctor de novo"
                .into(),
        )
    }
    if f["tailscale"] == "ausente" {
        add(
            "aviso",
            "Tailscale não instalado — o celular só entra no mesmo Wi-Fi".into(),
            "quer usar fora de casa? rode o instalador de novo e responda Sim ao Tailscale".into(),
        )
    } else if f["tailscale_login"] != "logado" {
        add(
            "erro",
            "Tailscale instalado mas sem login".into(),
            "sudo tailscale up  (Windows: tailscale up) e depois o instalador de novo".into(),
        )
    } else if service.value("CP_PUBLIC_URL").is_empty() {
        add(
            "aviso",
            "Tailscale logado mas o Hangar não está publicado nele".into(),
            "rode o instalador de novo (ele publica e grava CP_PUBLIC_URL em backend/.env)".into(),
        )
    } else {
        add(
            "ok",
            format!("publicado no Tailscale: {}", service.value("CP_PUBLIC_URL")),
            String::new(),
        )
    }
    if f["lan"] == true {
        add(
            "ok",
            "endereço da rede local responde (celular no mesmo Wi-Fi)".into(),
            String::new(),
        )
    } else {
        add("aviso","endereço da rede local não responde".into(),"firewall? Linux: sudo ./scripts/lan-setup.sh 8765 — Windows: regra 'hangar 8765' no perfil Private".into())
    }
    out
}
pub fn find_binary(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .flat_map(|p| {
            let names = if cfg!(windows) {
                vec![
                    name.to_owned(),
                    format!("{name}.exe"),
                    format!("{name}.cmd"),
                ]
            } else {
                vec![name.into()]
            };
            names.into_iter().map(move |n| p.join(n))
        })
        .find(|p| p.is_file())
}
async fn output(program: &str, args: &[&str], seconds: u64) -> Option<std::process::Output> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(seconds), cmd.output())
        .await
        .ok()?
        .ok()
}
async fn restarts(service: &Service) -> Value {
    if cfg!(windows) {
        let path = service
            .environment
            .get("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| service.home.join("AppData/Local"))
            .join("hangar/logs/privado/hangar-vigia.log");
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return json!([]),
            Err(_) => return Value::Null,
        };
        let cutoff = (chrono::Local::now() - chrono::Duration::days(7))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();
        return json!(
            text.lines()
                .filter(|l| l.contains("hangar-backend sem resposta HTTP")
                    && l.get(..19).is_some_and(|v| v >= cutoff.as_str()))
                .filter_map(|l| l.split_whitespace().next())
                .collect::<Vec<_>>()
        );
    }
    let Some(out) = output(
        "journalctl",
        &[
            "--user",
            "-u",
            "hangar-backend",
            "--since",
            "-7d",
            "-o",
            "short-iso",
            "--no-pager",
            "-q",
        ],
        15,
    )
    .await
    .filter(|o| o.status.success()) else {
        return Value::Null;
    };
    json!(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains("Scheduled restart job"))
            .filter_map(|l| l.split_whitespace().next())
            .collect::<Vec<_>>()
    )
}
async fn claude_login() -> bool {
    let Some(exe) = find_binary("claude") else {
        return false;
    };
    let env = crate::accounts::environment::AccountEnvironment::capture();
    let service = crate::accounts::AccountService::new(env);
    let Ok(mut accounts) = service.snapshot(crate::accounts::Provider::Claude) else {
        return false;
    };
    accounts.sort_by_key(|a| !a.is_default);
    for account in accounts {
        if !account.is_default && !account.home.join(".credentials.json").is_file() {
            continue;
        }
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.args(["auth", "status", "--json"])
            .env("CLAUDE_CONFIG_DIR", &account.home)
            .kill_on_drop(true);
        if let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await {
            if serde_json::from_slice::<Value>(&out.stdout).is_ok_and(|v| v["loggedIn"] == true) {
                return true;
            }
        }
    }
    false
}
pub async fn collect(service: &Service) -> Value {
    let port_open = tokio::time::timeout(
        Duration::from_millis(1500),
        tokio::net::TcpStream::connect(("127.0.0.1", service.port)),
    )
    .await
    .is_ok_and(|r| r.is_ok());
    let latency = if port_open {
        let start = Instant::now();
        match crate::http_client()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => c
                .get(format!("http://127.0.0.1:{}/", service.port))
                .send()
                .await
                .ok()
                .map(|_| start.elapsed().as_secs_f64()),
            Err(_) => None,
        }
    } else {
        None
    };
    let ts = network::tailscale().await;
    let has_ts = find_binary("tailscale").is_some();
    let logged_in = claude_login().await;
    let reach = network::reach(service).await;
    let mut facts = json!({"port_open":port_open,"latency":latency,"restarts":restarts(service).await,"mux":find_binary("tmux").is_some(),"claude":find_binary("claude").is_some(),"others":(["codex","pi","omp","kimi"].into_iter().filter(|n|find_binary(n).is_some()).collect::<Vec<_>>()),"logged_in":logged_in,"tailscale":if has_ts{"instalado"}else{"ausente"},"tailscale_login":if ts["Self"]["DNSName"].as_str().is_some_and(|s|!s.is_empty()){"logado"}else{"deslogado"},"lan":reach["enderecos"].as_array().into_iter().flatten().any(|v|v["tipo"]=="rede_local"&&v["estado"]=="ok")});
    if cfg!(windows) {
        let script = format!(
            "$p=Get-NetTCPConnection -State Listen -LocalPort {} -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty OwningProcess; if($p){{(Get-Process -Id $p).PriorityClass.ToString()}}",
            service.port
        );
        let priority = output("powershell", &["-NoProfile", "-Command", &script], 10)
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
        facts["priority"] = match priority.as_deref() {
            Some("Idle") => json!("ociosa"),
            Some("BelowNormal") => json!("abaixo do normal"),
            Some("Normal") => json!("normal"),
            Some("AboveNormal") => json!("acima do normal"),
            Some("High") => json!("alta"),
            Some("RealTime") => json!("tempo real"),
            _ => Value::Null,
        };
    }
    facts
}
pub fn ascii(text: &str) -> String {
    text.replace(['—', '–'], "-")
        .nfkd()
        .filter(char::is_ascii)
        .collect()
}
pub fn pairing_url(service: &Service) -> String {
    let public = service.value("CP_PUBLIC_URL");
    let base = if public.is_empty() {
        let port = service
            .value("CP_FRONT_PORT")
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .unwrap_or(service.port);
        let bind = service.value("CP_LAN_BIND_IP");
        let host = if ["", "127.0.0.1", "localhost", "::1", "0.0.0.0", "::", "auto"]
            .contains(&bind.as_str())
        {
            network::lan_ip()
        } else {
            bind
        };
        format!("http://{host}:{port}")
    } else {
        public.trim_end_matches('/').into()
    };
    format!("{base}/?token={}", service.token)
}
pub fn print_url_qr(url: &str, tty: bool) -> u8 {
    if !tty {
        println!("  Abra no celular: {url}\n");
        return 2;
    }
    let text=match super::pairing::qr_text(url) {Ok(text)=>text,Err(error)=>{eprintln!("{error}");return 1;}};
    println!("{text}");
    println!("  Aponte a câmera do celular para o QR acima; ou abra: {url}\n");
    0
}
pub fn cli(args: &[String]) -> std::process::ExitCode {
    let home = std::env::home_dir().unwrap_or_default();
    let backend = if let Some(index)=args.iter().position(|argument|argument=="--backend") {
        let Some(path)=args.get(index+1) else {eprintln!("doctor: --backend exige uma pasta");return 2.into();};
        Some(std::path::PathBuf::from(path))
    } else {crate::session_open::cli::backend_dir()};
    let snapshot=match super::dotenv::SettingsSnapshot::capture(&home,backend.as_deref(),super::dotenv::SettingsEnvironment::capture()) {
        Ok(snapshot)=>snapshot,Err(error)=>{eprintln!("pairing_config_unavailable: {error}");return 1.into();}
    };
    let settings=match super::pairing::Settings::from_snapshot(&snapshot) {
        Ok(settings)=>settings,Err(error)=>{eprintln!("{error}");return 1.into();}
    };
    if args.iter().any(|argument|argument=="--qr") {
        return print_url_qr(&settings.urls(&super::network::lan_ip()).0,std::io::stdout().is_terminal()).into();
    }
    let port=match settings.port.parse::<u16>() {
        Ok(port)=>port,Err(_)=>{eprintln!("doctor_config_invalid: CP_PORT fora de 0..65535");return 1.into();}
    };
    let server_id=match snapshot.value("CP_SERVER_ID") {
        Ok(value)=>value.unwrap_or_default(),Err(error)=>{eprintln!("pairing_config_unavailable: {error}");return 1.into();}
    };
    let service = match Service::new(
        &home,
        backend.as_deref(),
        settings.token,
        port,
        settings.bind,
        server_id,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}", e.msg);
            return 1.into();
        }
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let facts = runtime.block_on(collect(&service));
    let lines = diagnose(&service, &facts, std::env::consts::OS);
    let print = |s: &str| println!("{}", if cfg!(windows) { ascii(s) } else { s.into() });
    for (level, title, repair) in &lines {
        print(&format!(
            "  {}{title}",
            match level.as_str() {
                "ok" => "ok  ",
                "aviso" => "--  ",
                _ => "X   ",
            }
        ));
        if !repair.is_empty() {
            print(&format!("        conserto: {repair}"));
        }
    }
    let errors = lines.iter().filter(|l| l.0 == "erro").count();
    let warnings = lines.iter().filter(|l| l.0 == "aviso").count();
    print("");
    print(&if errors > 0 {
        format!("  {errors} item(ns) para consertar (marcados com X)")
    } else if warnings > 0 {
        format!("  {warnings} aviso(s) — nada quebrado, mas veja acima")
    } else {
        "  tudo certo".into()
    });
    u8::from(errors > 0).into()
}
