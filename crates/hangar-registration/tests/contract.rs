//! Os casos capturados do Python da base (`tests/contract/*.json`, recortados da captura R3 em
//! `contract/clis/`) rodados contra os subcomandos: código, stdout, stderr e a árvore de arquivos
//! (tipo, modo, bytes, alvo do link) depois de cada rodada. O que a migração mudou de propósito está
//! em `adjust`, cada caso com a decisão que o mudou.
#![cfg(unix)]
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use base64::Engine;
use hangar_registration::{Invocation, agents_install, claude_md_block, mcp_headers, mcp_register};
use serde_json::{Value, json};

type Runner = fn(&Invocation, &mut dyn Write, &mut dyn Write) -> u8;

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Dir(u32),
    File(u32, Vec<u8>),
    Link(String),
}

struct Step {
    code: u8,
    stdout: String,
    stderr: String,
    created: BTreeMap<String, Value>,
    changed: BTreeMap<String, Value>,
    removed: Vec<String>,
}

struct Case {
    name: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    before: BTreeMap<String, Value>,
    steps: Vec<Step>,
    sources: BTreeMap<String, String>,
    /// Precisa de pasta sem escrita: como root, o teste não prova nada.
    needs_permissions: bool,
}

fn text(value: &Value) -> String {
    value["text"].as_str().map(str::to_owned)
        .unwrap_or_else(|| panic!("saída sem texto: {value}"))
}

fn load(group: &str) -> (Vec<Case>, Option<String>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/contract").join(format!("{group}.json"));
    let data: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let object = |value: &Value| -> BTreeMap<String, Value> {
        value.as_object().map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
    };
    let cases = data["cases"].as_array().unwrap().iter().map(|case| Case {
        name: case["case"].as_str().unwrap().to_owned(),
        args: case["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect(),
        env: case["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect(),
        before: object(&case["files_before"]),
        steps: case["steps"].as_array().unwrap().iter().map(|step| Step {
            code: step["code"].as_u64().unwrap() as u8,
            stdout: text(&step["stdout"]),
            stderr: text(&step["stderr"]),
            created: object(&step["created"]),
            changed: object(&step["changed"]),
            removed: step["removed"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_owned()).collect(),
        }).collect(),
        sources: case.get("inst_sources").map(|s| s.as_object().unwrap().iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect()).unwrap_or_default(),
        needs_permissions: false,
    }).collect();
    (cases, data["block"].as_str().map(str::to_owned))
}

fn crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

fn file(mode: &str, content: &str) -> Value {
    json!({"type": "file", "mode": mode, "content": {"text": content}})
}

const MARKED_TAIL: &str = "tool_timeout_sec = 660\n";

fn marked(token: &str) -> String {
    format!(
        "# >>> hangar: mcp\n[mcp_servers.hangar]\nurl = \"http://127.0.0.1:8765/mcp/\"\n{MARKED_TAIL}\
         http_headers = {{ \"Authorization\" = \"Bearer {token}\" }}\n\
         env_http_headers = {{ \"X-Hangar-Key\" = \"CP_SESSION_KEY\", \"X-Hangar-Pane\" = \"TMUX_PANE\", \
         \"X-Hangar-Session\" = \"CP_SESSION_NAME\", \"X-Hangar-Jev\" = \"HANGAR_JEV\" }}\n# <<< hangar: mcp\n"
    )
}

/// O que a migração mudou de propósito, por caso. Cada linha cita a decisão que a sustenta.
fn adjust(group: &str, case: &mut Case, block: Option<&str>) {
    let step = &mut case.steps[0];
    let missing = |path: &str| format!("hangar-mcp-headers: CP_AUTH_TOKEN ausente em {path}\n");
    match (group, case.name.as_str()) {
        // R3-D07: sem a linha (ou com `export`, que o leitor do servidor não lê) a falha diz a frase.
        ("mcp-headers", "env-file-empty" | "no-token-line" | "token-export-prefix") => {
            step.code = 1;
            step.stderr = missing("$T/home/.hangar/config/.env");
        }
        ("mcp-headers", "no-env-file") => {
            step.code = 1;
            step.stderr = missing("$T/inst/backend/.env");
        }
        // Sem Python: o subcomando é o próprio binário.
        ("mcp-headers", "no-python3") => {
            step.code = 0;
            step.stdout = "{\"Authorization\": \"Bearer tok-local\"}\n".into();
            step.stderr = String::new();
        }
        // Decisão 16 (achado 18) e decisão 14.8: o `.env` passa pelo leitor do servidor: a última linha
        // vale, aspas e `\r` saem, `#` depois de espaço é comentário.
        ("mcp-headers", "token-duplicated") => step.stdout = "{\"Authorization\": \"Bearer dois\"}\n".into(),
        ("mcp-headers", "token-quoted-crlf") => step.stdout = "{\"Authorization\": \"Bearer tok local\"}\n".into(),
        ("mcp-headers", "token-with-equals-and-hash") => step.stdout = "{\"Authorization\": \"Bearer a=b\"}\n".into(),
        // R3-D02: `.env` ausente é frase, não traceback.
        ("registrar-mcp", "no-env-file") => {
            step.code = 1;
            step.stderr = "erro: .env não encontrado: $T/inst/backend/.env\n".into();
        }
        // Achado 5 e decisão 14.8: o token sai sem as aspas e vai escapado no TOML; o Codex é registrado.
        ("registrar-mcp", "env-quoted-token-export-spaces") => {
            step.code = 0;
            step.stderr = String::new();
            step.stdout.push_str("ok: MCP hangar em $T/home/.codex/config.toml\n");
            step.created.insert("home/.codex/config.toml".into(), file("0o600", &marked("tok q")));
        }
        // Achado 6: a pasta do Codex sem escrita vira `erro:` com código 1, sem traceback.
        ("registrar-mcp", "codex-dir-not-writable") => {
            case.needs_permissions = true;
            step.stderr = "erro: $T/home/.codex/config.toml: MCP hangar não registrado: [Errno 13] Permission denied: \
                           '$T/home/.codex/config.toml.hangar-novo'\nerro: alguma conta do Codex ficou sem o MCP hangar (veja acima)\n".into();
        }
        ("registrar-mcp", "claude-account-dir-not-writable") => case.needs_permissions = true,
        // Achado 8: `.codex-*.lock` e conta adicional vazia não recebem config.toml.
        ("registrar-mcp", "codex-homes-glob") => {
            step.stdout = step.stdout.replace("ok: MCP hangar em $T/home/.codex-b/config.toml\n", "")
                .replace("ok: MCP hangar em $T/home/.codex-x.lock/config.toml\n", "");
            step.created.remove("home/.codex-b/config.toml");
            step.created.remove("home/.codex-x.lock/config.toml");
        }
        // Achado 7: link continua link; grava-se no alvo.
        ("registrar-mcp", "claude-json-is-symlink") => {
            let after = step.changed.remove("home/.claude.json").unwrap();
            step.changed.insert("work/real.claude.json".into(), after);
        }
        ("registrar-mcp", "codex-config-symlink") => {
            let after = step.changed.remove("home/.codex/config.toml").unwrap();
            step.changed.insert("work/real.toml".into(), after);
        }
        // Revisão da R3 (decisão 19): o settings.json mantém o modo dele, como o `.claude.json`.
        ("registrar-mcp", "settings-partial-perms-order") => {
            step.changed.get_mut("home/.claude/settings.json").unwrap()["mode"] = json!("0o600");
        }
        // L7 / R3-D10: o fim de linha do arquivo fica.
        ("registrar-mcp", "codex-crlf-old-token") => {
            let after = step.changed.get_mut("home/.codex/config.toml").unwrap();
            let content = crlf(after["content"]["text"].as_str().unwrap());
            after["content"] = json!({"text": content});
        }
        ("claude-md-block", "crlf-with-block") => {
            let content = format!("antes\r\n{}\r\ndepois\r\n", crlf(block.unwrap()));
            step.changed.insert("home/.claude/CLAUDE.md".into(), file("0o644", &content));
        }
        ("claude-md-block", "crlf-without-block") => {
            let content = format!("antes\r\ndepois\r\n\r\n{}\r\n", crlf(block.unwrap()));
            step.changed.insert("home/.claude/CLAUDE.md".into(), file("0o644", &content));
        }
        // R3-D06: início sem fim deixa o arquivo como está e não diz ATUALIZADO.
        ("claude-md-block", "start-without-end" | "end-before-start") => {
            step.code = 1;
            step.stdout = String::new();
            step.stderr = "aviso: $T/home/.claude/CLAUDE.md tem o início do bloco Sessões-irmãs sem o fim; \
                           arquivo deixado como está\n".into();
        }
        // R3-D02: traceback e erro do bash viram frase com o arquivo.
        ("claude-md-block", "file-read-only" | "file-read-only-with-block") => {
            case.needs_permissions = true;
            step.stderr = "erro: não consegui gravar $T/home/.claude/CLAUDE.md: [Errno 13] Permission denied: \
                           '$T/home/.claude/CLAUDE.md'\n".into();
        }
        ("claude-md-block", "non-utf8-with-block") => {
            step.stderr = "erro: $T/home/.claude/CLAUDE.md não é texto UTF-8 — bloco Sessões-irmãs não gravado\n".into();
        }
        // Achado 4: DEL escapado; o TOML do agente é válido.
        ("instalar-agentes", "body-hard-chars") => {
            for entry in step.created.values_mut().chain(step.changed.values_mut()) {
                if let Some(content) = entry["content"]["text"].as_str().map(|t| t.replace('\u{7f}', "\\u007f")) {
                    entry["content"] = json!({"text": content});
                }
            }
        }
        ("instalar-agentes", "existing-dest-dir") => {
            step.stderr = "erro: $T/home/.claude/agents/a.md: agente a não instalado: [Errno 21] Is a directory: \
                           '$T/home/.claude/agents/a.md'\nerro: algum agente das skills ficou sem instalar (veja acima)\n".into();
        }
        _ => {}
    }
}

fn sub(text: &str, t: &str, block: Option<&str>) -> String {
    let text = text.replace("$T", t);
    match block {
        Some(block) => text.replace("$BLOCK", block),
        None => text,
    }
}

fn node(value: &Value, t: &str, block: Option<&str>) -> Node {
    let mode = || u32::from_str_radix(value["mode"].as_str().unwrap().trim_start_matches("0o"), 8).unwrap();
    match value["type"].as_str().unwrap() {
        "dir" => Node::Dir(mode()),
        "symlink" => Node::Link(sub(value["target"].as_str().unwrap(), t, block)),
        "file" => {
            let content = &value["content"];
            let bytes = match content["text"].as_str() {
                Some(text) => sub(text, t, block).into_bytes(),
                None => base64::engine::general_purpose::STANDARD.decode(content["b64"].as_str().unwrap()).unwrap(),
            };
            Node::File(mode(), bytes)
        }
        other => panic!("tipo desconhecido {other}"),
    }
}

fn materialize(root: &Path, tree: &BTreeMap<String, Node>) {
    for (rel, node) in tree {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        match node {
            Node::Dir(_) => std::fs::create_dir_all(&path).unwrap(),
            Node::File(mode, bytes) => {
                std::fs::write(&path, bytes).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode)).unwrap();
            }
            Node::Link(target) => std::os::unix::fs::symlink(target, &path).unwrap(),
        }
    }
    // Pasta sem escrita só depois do que vai dentro dela.
    for (rel, node) in tree.iter().rev() {
        if let Node::Dir(mode) = node {
            std::fs::set_permissions(root.join(rel), std::fs::Permissions::from_mode(*mode)).unwrap();
        }
    }
}

fn snapshot(root: &Path, skip: &[PathBuf]) -> BTreeMap<String, Node> {
    fn walk(root: &Path, dir: &Path, skip: &[PathBuf], out: &mut BTreeMap<String, Node>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if skip.contains(&path) {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let mode = meta.permissions().mode() & 0o7777;
            if meta.file_type().is_symlink() {
                out.insert(rel, Node::Link(std::fs::read_link(&path).unwrap().to_string_lossy().into_owned()));
            } else if meta.is_dir() {
                if rel.contains('/') {
                    out.insert(rel, Node::Dir(mode));
                }
                walk(root, &path, skip, out);
            } else {
                out.insert(rel, Node::File(mode, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, skip, &mut out);
    out
}

/// Devolve a escrita às pastas sem escrita, para o `tempdir` conseguir apagar.
fn unlock(root: &Path) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(entry.path());
            }
        }
    }
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn replay(group: &str, run: Runner) {
    unsafe { libc::umask(0o022) };
    let (mut cases, block) = load(group);
    let mut failures = Vec::new();
    for case in &mut cases {
        adjust(group, case, block.as_deref());
        if case.needs_permissions && is_root() {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().to_string_lossy().into_owned();
        let env: BTreeMap<String, String> = case.env.iter().map(|(k, v)| (k.clone(), sub(v, &t, None))).collect();
        let home = PathBuf::from(&env["HOME"]);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        let mut tree: BTreeMap<String, Node> = case.before.iter().map(|(k, v)| (k.clone(), node(v, &t, block.as_deref()))).collect();
        let sources: Vec<PathBuf> = case.sources.keys().map(|rel| dir.path().join(rel)).collect();
        // As pastas das fontes copiadas pela captura não entravam na árvore dela; aqui elas existem.
        for rel in case.sources.keys() {
            let mut parent = Path::new(rel).parent();
            while let Some(dir_rel) = parent.filter(|p| p.components().count() > 1) {
                tree.entry(dir_rel.to_string_lossy().into_owned()).or_insert(Node::Dir(0o755));
                parent = dir_rel.parent();
            }
        }
        for (rel, content) in &case.sources {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        materialize(dir.path(), &tree);
        let home_arg = case.args.iter().position(|a| a == "--home").map(|at| PathBuf::from(sub(&case.args[at + 1], &t, None)));
        let invocation = Invocation {
            home,
            home_arg,
            root: Some(dir.path().join("inst")),
            vars: env,
            stdin: block.as_ref().map(|b| format!("{b}\n").into_bytes()).unwrap_or_default(),
            windows: false,
        };
        for (index, step) in case.steps.iter().enumerate() {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run(&invocation, &mut out, &mut err);
            for (rel, value) in step.created.iter().chain(&step.changed) {
                tree.insert(rel.clone(), node(value, &t, block.as_deref()));
            }
            for rel in &step.removed {
                tree.remove(rel);
            }
            let label = format!("{group}/{}#{index}", case.name);
            let shown = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).replace(&t, "$T");
            let (out, err) = (shown(out), shown(err));
            // O Python listava as homes do Codex na ordem do disco; aqui elas saem em ordem alfabética.
            let same_out = if group == "instalar-agentes" { sorted(&out) == sorted(&step.stdout) } else { out == step.stdout };
            if code != step.code || !same_out || err != step.stderr {
                failures.push(format!("{label}: código {code} (esperado {}), stdout {out:?} (esperado {:?}), stderr {err:?} (esperado {:?})",
                    step.code, step.stdout, step.stderr));
            }
            let actual = snapshot(dir.path(), &sources);
            if actual != tree {
                let keys: std::collections::BTreeSet<_> = actual.keys().chain(tree.keys()).collect();
                for key in keys {
                    if actual.get(key) != tree.get(key) {
                        failures.push(format!("{label}: {key}: {:?} (esperado {:?})", actual.get(key).map(brief), tree.get(key).map(brief)));
                    }
                }
            }
        }
        unlock(dir.path());
    }
    assert!(failures.is_empty(), "{} diferença(s):\n{}", failures.len(), failures.join("\n"));
}

fn sorted(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort_unstable();
    lines
}

fn brief(node: &Node) -> String {
    match node {
        Node::File(mode, bytes) => format!("arquivo {mode:o} {:?}", String::from_utf8_lossy(bytes)),
        other => format!("{other:?}"),
    }
}

#[test]
fn mcp_headers_cases() {
    replay("mcp-headers", mcp_headers::run);
}

#[test]
fn mcp_register_cases() {
    replay("registrar-mcp", mcp_register::run);
}

#[test]
fn agents_install_cases() {
    replay("instalar-agentes", agents_install::run);
}

#[test]
fn claude_md_block_cases() {
    replay("claude-md-block", claude_md_block::run);
}
