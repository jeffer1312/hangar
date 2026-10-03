# hangar-server, parte 1 — Plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** O `hangar-server` (Rust) assume a porta 8765 e responde sozinho o histórico e o chat ao vivo das conversas do Claude e do Codex, repassando o resto ao Python; o `cano.py` ganha um substituto em Rust (`hangar-cano`). Web, celular, nativo e peers não mudam nada.

**Architecture:** O serviço continua subindo o Python. O `main.py` acha o binário e o liga como filho na porta pública, passando a escutar numa porta interna de loopback. O `hangar-server` lê os transcripts do Claude e do Codex com parsers portados byte a byte e recebe do Python, por uma conexão interna por sessão (`/internal/sessions/{name}/side-events`), o estado, a prévia, as perguntas e a fila, repassando tudo a N aparelhos. Sem o binário, ou se ele cair, o Python atende sozinho como hoje.

**Tech Stack:** Rust 1.98.1 (tokio, axum, hyper, serde, notify) em `crates/`; Python 3.14 + FastAPI no backend; pytest e `cargo test`.

**Spec:** `docs/superpowers/specs/2026-10-02-hangar-server-conversas-design.md`

**Contexto:** `docs/analise-backend-rust-2026-10-02.md`. Spec e plano NÃO são commitados (pedido do dono).

## Global Constraints

- Web, celular, nativo e peers não mudam nada: mesmos nomes de evento, mesmos campos de `ChatEvent`, mesmo `id: <session_key>:<offset>`, mesmas regras de retomada.
- Sem binário, com `CP_RUST_SERVER=0` ou com o binário caindo 3 vezes em 60 s, tudo funciona como hoje.
- O `hangar-server` só atende sozinho requisição com o token do dono e sessão de provider `claude`, `claude-headless` ou `codex`; o resto é repassado ao Python.
- Portas 8766 (convidados) e 8768 (Caddy) continuam direto no Python.
- Ids de evento do Codex (`_event_id`) e a impressão digital do `RewriteFilter` saem byte a byte iguais aos do Python (`json.dumps` com separadores `", "`/`": "` e `ensure_ascii=True`).
- Nunca usar conversa real em fixture; o log do `hangar-server` nunca leva texto de conversa.
- Rust: toolchain 1.98.1, edition 2024, versões fixadas com `=`, as mesmas do `desktop-native` quando a crate já existe lá.
- Identificador novo em inglês; comentário e texto de tela em português; comentário curto, sobre o porquê, sem data, medição ou número de bug.
- Testes: escrever os de cada Task; **rodar só quando o usuário autorizar** (regra do `CLAUDE.md` do projeto). Os passos "Rodar" valem quando houver autorização; sem ela, pular e dizer no relatório.
- Commits: mensagem descritiva em inglês; `git add` só dos caminhos da Task (nunca `-A`/`.`). Em `main`, perguntar antes de commitar. Spec e plano ficam fora do commit.
- Nunca subir um segundo backend nem reiniciar o serviço vivo durante a execução: mata sessões sem terminal.
- Nada de formatter `--write` (`cargo fmt` só nos arquivos novos da Task). Casar indentação.

## Review Focus

- Aparelho reconecta com `Last-Event-ID` enquanto o transcript está no meio de uma linha (gravação parcial no fim do arquivo): não pode perder nem duplicar mensagem (Task 12 testa linha parcial e retomada).
- `/clear` ou troca de provider com três aparelhos no mesmo chat: todos recebem um único `reset`, e a conexão interna recomeça uma vez só (Tasks 10 e 12).
- Sessão fechada e recriada com o MESMO nome: o `hangar-server` não pode servir o transcript da morta pelo cache de `info` (Task 12 testa troca de `jsonl` com o mesmo nome).
- Linha com surrogate solto ou UTF-8 inválido no transcript: a sessão abre e a linha sai igual ao Python (Tasks 6 e 7).
- Convidado com login próprio na porta 8765 (`GuestUserGate`): nunca é atendido pelo Rust, sempre repassado ao Python (Task 11 testa token de convidado → proxy).

---

### Task 1: Workspace `crates/` e crate `hangar-api`

**Files:**
- Create: `crates/Cargo.toml`
- Create: `crates/rust-toolchain.toml`
- Create: `crates/.gitignore`
- Create: `crates/Cargo.lock` (gerado pelo cargo)
- Create: `crates/hangar-api/Cargo.toml`
- Create: `crates/hangar-api/src/lib.rs`
- Create: `crates/hangar-api/src/chat.rs`
- Create: `crates/hangar-api/src/state.rs`
- Create: `crates/hangar-api/src/preview.rs`
- Create: `crates/hangar-api/src/ask.rs`
- Create: `crates/hangar-server/Cargo.toml`
- Create: `crates/hangar-server/src/lib.rs` (esqueleto; a frente 3 e a frente 4 preenchem)
- Create: `crates/hangar-server/src/main.rs` (esqueleto; a frente 4 preenche)
- Create: `crates/hangar-cano/Cargo.toml`
- Create: `crates/hangar-cano/src/main.rs` (esqueleto; a frente 2 preenche)
- Create: `backend/tests/fixtures/contract/gen_api_samples.py`
- Create: `backend/tests/fixtures/contract/api_samples/*.json` (gerados pelo script)
- Test: `crates/hangar-api/tests/contract.rs`
- Test: `backend/tests/test_contract_api_samples.py`

**Interfaces:**
- Consumes:
  - `backend/app/models.py`: `ChatKind` (linha 9), `ChatEvent` (191), `ShellVivo` (247), `StateEvent` (254), `PreviewEvent` (304), `AskOption` (356), `AskQuestionItem` (364), `AskQuestion` (371).
- Produces:
  - Workspace `crates/` (`resolver = "3"`, membros `hangar-api`, `hangar-server`, `hangar-cano`), `[workspace.package] version = "0.1.0"`, `edition = "2024"`.
  - `[workspace.dependencies]`: `serde = { version = "=1.0.229", features = ["derive"] }`, `serde_json = { version = "=1.0.151", features = ["preserve_order"] }`, `hangar-api = { path = "hangar-api" }`.
  - `hangar_api::chat::ChatKind` — `enum { UserMsg, AssistantMsg, ToolUse, ToolResult, Thinking, Notice, Other(String) }`; `fn as_str(&self) -> &str`; `impl From<&str>`; `impl PartialEq<&str>`; `Default` = `Other(String::new())`; serde como texto.
  - `hangar_api::chat::ChatEvent` — `Clone, Debug, Default, PartialEq, Serialize, Deserialize`; campos `kind: ChatKind, id: String, text: Option<String>, tool_name: Option<String>, tool_input: Option<Map<String, Value>>, tool_use_id: Option<String>, result: Option<String>, is_error: Option<bool>, ts: Option<f64>, cache_read: Option<u64>, cache_ttl_s: Option<u64>, desistiu: Option<bool>, hook_error: Option<String>, skill: Option<Map<String, Value>>, orq: Option<Map<String, Value>>, queued_delivered: Option<bool>, queued_ts: Option<f64>, queued_confirmed: Option<bool>, image_count: Option<u32>, #[serde(skip)] offset: Option<u64>`.
  - `hangar_api::state::ShellVivo { pid: i64, cmd: String, desde: Option<f64> }`.
  - `hangar_api::state::StateEvent` — 24 campos na ordem de models.py (ver Step 7), todos com padrão.
  - `hangar_api::preview::PreviewEvent { session: String, text: String, md: bool, full: bool, vivo: bool }`, todos com padrão.
  - `hangar_api::ask::{AskQuestion { questions }, AskQuestionItem { header, question, multi_select (JSON "multiSelect"), options }, AskOption { label, description, preview }}`.
  - `gen_api_samples.OUT: Path`, `gen_api_samples.samples() -> dict[str, str]` (nome do arquivo → conteúdo exato).

- [x] **Step 1: Workspace e esqueletos compiláveis**

`crates/Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["hangar-api", "hangar-server", "hangar-cano"]

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
# Mesmas versões do desktop-native/Cargo.toml: o hangar-api entra nos dois builds.
serde = { version = "=1.0.229", features = ["derive"] }
# preserve_order: o tool_input reescrito sai com as chaves na ordem do dict do Python.
serde_json = { version = "=1.0.151", features = ["preserve_order"] }
hangar-api = { path = "hangar-api" }

[profile.release]
strip = true
```

A versão de `serde`/`serde_json` é a do desktop (conferido 2026-10-02: `desktop-native/Cargo.toml:18-19`), e o
`serde_json` do desktop já é compilado com `indexmap`, ou seja, com `preserve_order` ligado por outra dependência
(conferido 2026-10-02: `desktop-native/Cargo.lock:7161-7162`). Ligar o recurso aqui não muda nada no build do desktop.

`crates/rust-toolchain.toml` (o mesmo canal do desktop, conferido 2026-10-02: `desktop-native/rust-toolchain.toml:2`):

```toml
[toolchain]
channel = "1.98.1"
profile = "minimal"
```

`crates/.gitignore`:

```gitignore
/target/
```

`crates/hangar-api/Cargo.toml`:

```toml
[package]
name = "hangar-api"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
serde.workspace = true
serde_json.workspace = true
```

`crates/hangar-api/src/lib.rs` (vazio por enquanto; o Step 7 preenche):

```rust
//! Formatos da conversa que o backend manda aos aparelhos.
```

`crates/hangar-server/Cargo.toml`:

```toml
[package]
name = "hangar-server"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
```

`crates/hangar-server/src/lib.rs`:

```rust
//! Servidor do Hangar: lê as conversas do Claude e do Codex e repassa o resto ao backend Python.
```

`crates/hangar-server/src/main.rs`:

```rust
fn main() {}
```

`crates/hangar-cano/Cargo.toml`:

```toml
[package]
name = "hangar-cano"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
```

`crates/hangar-cano/src/main.rs`:

```rust
fn main() {}
```

O `src/lib.rs` e o `src/main.rs` do `hangar-server` são descobertos sozinhos pelo cargo: a lib sai como
`hangar_server` e o binário como `hangar-server`, sem seção `[lib]`/`[[bin]]`.

- [x] **Step 2: Gerador das amostras**

```python
# backend/tests/fixtures/contract/gen_api_samples.py
"""Amostras JSON dos formatos da conversa, geradas pelos modelos reais de app/models.py.

O crate crates/hangar-api lê cada arquivo, escreve de volta e compara: campo que mudar aqui e não
lá quebra o teste de ida e volta. Depois de mexer em ChatEvent, StateEvent, PreviewEvent ou
AskQuestion, rodar de backend/:

    uv run python tests/fixtures/contract/gen_api_samples.py
"""
from __future__ import annotations

import sys
from pathlib import Path
from typing import get_args

HERE = Path(__file__).resolve().parent
OUT = HERE / "api_samples"
# Rodado como script, o pacote `app` não está no caminho de import: a raiz é backend/.
sys.path.insert(0, str(HERE.parents[2]))

from app.models import (  # noqa: E402
    AskOption,
    AskQuestion,
    AskQuestionItem,
    ChatEvent,
    ChatKind,
    PreviewEvent,
    ShellVivo,
    StateEvent,
)


def _models() -> dict:
    cases = {
        "chat_minimal": ChatEvent(kind="user_msg", id="u1"),
        "chat_full": ChatEvent(
            kind="tool_use", id="toolu_01", text="acentuação e emoji 🚀", tool_name="Bash",
            tool_input={"command": "ls -la", "nested": {"list": [1, 2.5, None, True, "x"]}},
            tool_use_id="toolu_01", result="ok", is_error=False, ts=1727712000.123,
            cache_read=1234, cache_ttl_s=3600, desistiu=True, hook_error="hook recusou",
            skill={"name": "pdf", "path": "/skills/pdf/SKILL.md", "body": "# PDF"},
            orq={"kind": "woke", "task": 4, "body": "texto", "alarm": False},
            queued_delivered=True, queued_ts=1727712001.5, queued_confirmed=False, image_count=2,
            offset=999,  # exclude=True: não aparece no JSON
        ),
        "state_minimal": StateEvent(session="s1", state="idle"),
        "state_full": StateEvent(
            session="s1", state="awaiting_input", codex_mode="plan",
            codex_question={"provider": "codex", "request_id": 7, "is_async": False, "questions": []},
            codex_buffering=True, claude_permission_mode="acceptEdits",
            claude_previous_non_plan="default",
            claude_plan_pending={"plan": "# Plano", "path": "/tmp/p.md", "tool_use_id": "toolu_9"},
            label="Pensando…", question="Continuar?", options=["Sim", "Não"],
            status_line="opus · 12%", overlay=True, login=True, limited=True, limit_reset="3pm",
            loop_status="rodando", loop_iter=2, loop_max=5, problema="turno_com_erro",
            problema_detalhe="detalhe", headless=True, recarregar_motivo="config",
            shells=[ShellVivo(pid=42, cmd="sleep 999", desde=1727712000.5),
                    ShellVivo(pid=43, cmd="tail -f x")],
        ),
        "preview_minimal": PreviewEvent(session="s1", text=""),
        "preview_full": PreviewEvent(session="s1", text="**negrito** e acentuação", md=True,
                                     full=True, vivo=True),
        "ask_minimal": AskQuestion(questions=[]),
        "ask_full": AskQuestion(questions=[AskQuestionItem(
            header="Escolha", question="Qual caminho?", multiSelect=True,
            options=[AskOption(label="A", description="primeiro", preview="```\nA\n```"),
                     AskOption(label="B")],
        )]),
    }
    # Um por tipo: tipo novo no models.py vira amostra nova, e o teste Rust cobra a variante.
    for kind in get_args(ChatKind):
        cases[f"chat_kind_{kind}"] = ChatEvent(kind=kind, id=f"{kind}-1", text="x")
    return cases


def samples() -> dict[str, str]:
    """Nome do arquivo -> conteúdo exato, como o backend serializa (`model_dump_json`)."""
    return {f"{name}.json": model.model_dump_json() + "\n" for name, model in _models().items()}


def main() -> None:
    OUT.mkdir(exist_ok=True)
    want = samples()
    for old in OUT.glob("*.json"):
        if old.name not in want:
            old.unlink()
    for name, text in want.items():
        # newline="\n": gerado no Windows sai igual ao do Linux.
        (OUT / name).write_text(text, encoding="utf-8", newline="\n")
    print(f"{len(want)} amostras em {OUT}")


if __name__ == "__main__":
    main()
```

- [x] **Step 3: Gerar as amostras**

Run: `cd backend && uv run python tests/fixtures/contract/gen_api_samples.py`
Expected: `14 amostras em …/backend/tests/fixtures/contract/api_samples` (8 casos fixos + 6 tipos de `ChatKind`).

Conferir uma: `cat backend/tests/fixtures/contract/api_samples/chat_minimal.json` deve ser exatamente
(conferido 2026-10-02 rodando `ChatEvent(kind='user_msg', id='a', ts=5).model_dump_json()`: `None` sai como `null`,
`offset` não sai, `ts` inteiro sai como `5.0`):

```json
{"kind":"user_msg","id":"u1","text":null,"tool_name":null,"tool_input":null,"tool_use_id":null,"result":null,"is_error":null,"ts":null,"cache_read":null,"cache_ttl_s":null,"desistiu":null,"hook_error":null,"skill":null,"orq":null,"queued_delivered":null,"queued_ts":null,"queued_confirmed":null,"image_count":null}
```

- [x] **Step 4: Teste Python que segura as amostras em dia**

```python
# backend/tests/test_contract_api_samples.py
"""As amostras que o crate hangar-api testa saem dos modelos atuais: mudou models.py, regenere."""
import importlib.util
from pathlib import Path

GEN = Path(__file__).parent / "fixtures" / "contract" / "gen_api_samples.py"


def _gen():
    spec = importlib.util.spec_from_file_location("gen_api_samples", GEN)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def test_api_samples_match_the_models():
    gen = _gen()
    have = {p.name: p.read_text(encoding="utf-8") for p in gen.OUT.glob("*.json")}
    assert have == gen.samples(), "rode: uv run python tests/fixtures/contract/gen_api_samples.py"
```

- [x] **Step 5: Testes Rust de ida e volta e de tolerância**

```rust
// crates/hangar-api/tests/contract.rs
use std::{fs, path::{Path, PathBuf}};

use hangar_api::{
    ask::AskQuestion,
    chat::{ChatEvent, ChatKind},
    preview::PreviewEvent,
    state::StateEvent,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn samples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/api_samples")
}

fn samples(prefix: &str) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = fs::read_dir(samples_dir())
        .expect("sem amostras: rode backend/tests/fixtures/contract/gen_api_samples.py")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with(prefix))
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            (name, value)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "nenhuma amostra {prefix}*");
    out
}

// Lê, escreve de volta e compara como JSON: campo a mais ou a menos de um dos lados aparece aqui.
fn round_trip<T: Serialize + DeserializeOwned>(prefix: &str) -> Vec<(String, T)> {
    samples(prefix)
        .into_iter()
        .map(|(name, original)| {
            let typed: T = serde_json::from_value(original.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(serde_json::to_value(&typed).unwrap(), original, "{name}");
            (name, typed)
        })
        .collect()
}

#[test]
fn chat_samples_round_trip() {
    for (name, event) in round_trip::<ChatEvent>("chat_") {
        assert!(!matches!(event.kind, ChatKind::Other(_)), "{name}: tipo novo no models.py sem variante no ChatKind");
    }
}

#[test]
fn state_samples_round_trip() {
    round_trip::<StateEvent>("state_");
}

#[test]
fn preview_samples_round_trip() {
    round_trip::<PreviewEvent>("preview_");
}

#[test]
fn ask_samples_round_trip() {
    round_trip::<AskQuestion>("ask_");
}

#[test]
fn unknown_and_missing_fields_do_not_break() {
    let event: ChatEvent = serde_json::from_value(json!({"kind": "assistant_msg", "id": "a1", "campo_novo": {"x": 1}})).unwrap();
    assert_eq!(event.kind, ChatKind::AssistantMsg);
    assert!(event.text.is_none() && event.tool_input.is_none() && event.offset.is_none());

    let future: ChatEvent = serde_json::from_value(json!({"kind": "tipo_futuro", "id": "f1"})).unwrap();
    assert_eq!(future.kind, "tipo_futuro");
    assert_eq!(serde_json::to_value(&future).unwrap()["kind"], "tipo_futuro");
    assert!(serde_json::from_value::<ChatEvent>(json!({"kind": "user_msg"})).is_err(), "id continua obrigatório");

    let state: StateEvent = serde_json::from_value(json!({"state": "working", "extra": true})).unwrap();
    assert_eq!((state.session.as_str(), state.state.as_str(), state.login, state.shells.len()), ("", "working", false, 0));
    let shells: StateEvent = serde_json::from_value(json!({"shells": [{"pid": 1}]})).unwrap();
    assert_eq!((shells.shells[0].pid, shells.shells[0].cmd.as_str(), shells.shells[0].desde), (1, "", None));

    let preview: PreviewEvent = serde_json::from_value(json!({"text": "oi", "novo": 1})).unwrap();
    assert!(preview.session.is_empty() && !preview.md && !preview.vivo);

    let ask: AskQuestion = serde_json::from_value(json!({"questions": [
        {"header": "h", "question": "q", "options": [{"label": "a"}], "novo": 1}]})).unwrap();
    assert!(!ask.questions[0].multi_select);
    assert_eq!((ask.questions[0].options[0].description.as_str(), ask.questions[0].options[0].preview.as_str()), ("", ""));
}

#[test]
fn none_is_written_as_null_and_offset_never() {
    let event = ChatEvent { kind: ChatKind::UserMsg, id: "u".into(), offset: Some(10), ..Default::default() };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["text"], Value::Null);
    assert!(value.as_object().unwrap().contains_key("text"));
    assert!(!value.as_object().unwrap().contains_key("offset"));
    let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(&keys[..3], ["kind", "id", "text"], "mesma ordem do model_dump_json");
}
```

- [x] **Step 6: Gerar o `Cargo.lock` do workspace**

Run: `cd crates && cargo generate-lockfile`
Expected: cria `crates/Cargo.lock` (só resolve, não compila). Conferir que `serde` está em `1.0.229` e `serde_json` em `1.0.151`:
`grep -A1 '^name = "serde"$\|^name = "serde_json"$' crates/Cargo.lock`.

- [x] **Step 7: Rodar e ver falhar** (quando autorizado)

Run: `cd crates && cargo test --locked -p hangar-api`
Expected: FAIL de compilação com `error[E0432]: unresolved imports hangar_api::ask, hangar_api::chat, hangar_api::preview, hangar_api::state`.

- [x] **Step 8: Implementar o `hangar-api`**

`crates/hangar-api/src/lib.rs`:

```rust
//! Formatos da conversa que o backend manda aos aparelhos (`backend/app/models.py`), lidos e escritos
//! como o pydantic: mesmos nomes e ordem de campos, `null` no que falta, e leitura que ignora campo
//! desconhecido.
pub mod ask;
pub mod chat;
pub mod preview;
pub mod state;
```

`crates/hangar-api/src/chat.rs`:

```rust
//! Mensagem da conversa (`ChatEvent`, backend/app/models.py:191).
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// `ChatKind` de models.py:9. `Other` guarda o tipo que este crate ainda não conhece: quem lê não
/// quebra e o valor volta igual quando é reenviado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatKind {
    UserMsg,
    AssistantMsg,
    ToolUse,
    ToolResult,
    Thinking,
    Notice,
    Other(String),
}

impl ChatKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::UserMsg => "user_msg",
            Self::AssistantMsg => "assistant_msg",
            Self::ToolUse => "tool_use",
            Self::ToolResult => "tool_result",
            Self::Thinking => "thinking",
            Self::Notice => "notice",
            Self::Other(kind) => kind,
        }
    }
}

impl From<&str> for ChatKind {
    fn from(kind: &str) -> Self {
        match kind {
            "user_msg" => Self::UserMsg,
            "assistant_msg" => Self::AssistantMsg,
            "tool_use" => Self::ToolUse,
            "tool_result" => Self::ToolResult,
            "thinking" => Self::Thinking,
            "notice" => Self::Notice,
            other => Self::Other(other.to_owned()),
        }
    }
}

// Vazio, como o `String` que o desktop usava antes do crate.
impl Default for ChatKind {
    fn default() -> Self {
        Self::Other(String::new())
    }
}

impl PartialEq<&str> for ChatKind {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl Serialize for ChatKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ChatKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?.as_str()))
    }
}

/// Campos na ordem de models.py: o JSON sai com as chaves na ordem do `model_dump_json`.
/// Contagens sem sinal porque o Python nunca manda negativo, e são os tipos que o desktop já lia.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatEvent {
    pub kind: ChatKind,
    pub id: String,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Map<String, Value>>,
    pub tool_use_id: Option<String>,
    pub result: Option<String>,
    pub is_error: Option<bool>,
    pub ts: Option<f64>,
    pub cache_read: Option<u64>,
    pub cache_ttl_s: Option<u64>,
    pub desistiu: Option<bool>,
    pub hook_error: Option<String>,
    pub skill: Option<Map<String, Value>>,
    pub orq: Option<Map<String, Value>>,
    pub queued_delivered: Option<bool>,
    pub queued_ts: Option<f64>,
    pub queued_confirmed: Option<bool>,
    pub image_count: Option<u32>,
    /// Byte logo após a linha do transcript: vira o `id:` do SSE e nunca vai no JSON (`exclude=True`).
    #[serde(skip)]
    pub offset: Option<u64>,
}
```

`crates/hangar-api/src/state.rs`:

```rust
//! Estado ao vivo da sessão (`ShellVivo` e `StateEvent`, backend/app/models.py:247 e 254).
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Comando de fundo que a sessão deixou rodando.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShellVivo {
    pub pid: i64,
    #[serde(default)]
    pub cmd: String,
    pub desde: Option<f64>,
}

/// `state` e `codex_mode` ficam em texto: o hangar-server só repassa este evento, e valor novo do
/// Python passa adiante igual. Tudo tem padrão porque o desktop lê o estado antes do primeiro evento.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateEvent {
    pub session: String,
    pub state: String,
    pub codex_mode: Option<String>,
    pub codex_question: Option<Map<String, Value>>,
    pub codex_buffering: bool,
    pub claude_permission_mode: Option<String>,
    pub claude_previous_non_plan: Option<String>,
    pub claude_plan_pending: Option<Map<String, Value>>,
    pub label: Option<String>,
    pub question: Option<String>,
    pub options: Option<Vec<String>>,
    pub status_line: Option<String>,
    pub overlay: bool,
    pub login: bool,
    pub limited: bool,
    pub limit_reset: Option<String>,
    pub loop_status: Option<String>,
    pub loop_iter: Option<u32>,
    pub loop_max: Option<u32>,
    pub problema: Option<String>,
    pub problema_detalhe: Option<String>,
    pub headless: bool,
    pub recarregar_motivo: Option<String>,
    pub shells: Vec<ShellVivo>,
}
```

`crates/hangar-api/src/preview.rs`:

```rust
//! Prévia ao vivo do bloco em andamento (`PreviewEvent`, backend/app/models.py:304).
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewEvent {
    pub session: String,
    pub text: String,
    pub md: bool,
    pub full: bool,
    pub vivo: bool,
}
```

`crates/hangar-api/src/ask.rs`:

```rust
//! Pergunta do AskUserQuestion do Claude com terminal (`AskQuestion`, backend/app/models.py:356-372).
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskQuestionItem {
    pub header: String,
    pub question: String,
    #[serde(rename = "multiSelect", default)]
    pub multi_select: bool,
    pub options: Vec<AskOption>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskQuestion {
    pub questions: Vec<AskQuestionItem>,
}
```

Regra de obrigatoriedade: `ChatEvent.kind`/`id` e os campos obrigatórios de `AskQuestion` continuam obrigatórios
como no Python; `Option` ausente vira `None` pelo próprio serde; `StateEvent` e `PreviewEvent` têm padrão em tudo
porque o `SessionState`/`Preview` do desktop já liam assim (`desktop-native/src/api/dto.rs:419-441,499-505`).

- [x] **Step 9: Rodar e ver passar** (quando autorizado)

Run: `cd crates && cargo test --locked --workspace`
Expected: PASS (6 testes em `hangar-api/tests/contract.rs`; `hangar-server` e `hangar-cano` compilam sem teste).

Run: `cd backend && uv run pytest tests/test_contract_api_samples.py -v`
Expected: PASS (1 teste).

- [x] **Step 10: Commit**

```bash
git add crates/Cargo.toml crates/Cargo.lock crates/rust-toolchain.toml crates/.gitignore \
  crates/hangar-api/Cargo.toml crates/hangar-api/src/lib.rs crates/hangar-api/src/chat.rs \
  crates/hangar-api/src/state.rs crates/hangar-api/src/preview.rs crates/hangar-api/src/ask.rs \
  crates/hangar-api/tests/contract.rs \
  crates/hangar-server/Cargo.toml crates/hangar-server/src/lib.rs crates/hangar-server/src/main.rs \
  crates/hangar-cano/Cargo.toml crates/hangar-cano/src/main.rs \
  backend/tests/fixtures/contract/gen_api_samples.py backend/tests/fixtures/contract/api_samples \
  backend/tests/test_contract_api_samples.py
git commit -m "feat(server): crates workspace with hangar-api chat types checked against pydantic samples"
```

---

### Task 2: `desktop-native` passa a usar o `hangar-api`

**Files:**
- Modify: `desktop-native/Cargo.toml:17` (dependência nova logo após `reqwest`)
- Modify: `desktop-native/Cargo.lock` (só a entrada do `hangar-api`)
- Modify: `desktop-native/src/api/dto.rs:1-3,100-124,410-449,465-469,499-505,546-662`
- Modify: `desktop-native/src/api/sse.rs:7`
- Modify: `desktop-native/src/chat.rs:3,106,530,621`
- Modify: `desktop-native/src/conversation.rs:2,236,242,478-479,511-517,560,720-722`
- Modify: `desktop-native/src/editdiff.rs:168`
- Modify: `desktop-native/src/interaction.rs:53,222`
- Modify: `desktop-native/src/app/find.rs:58`
- Modify: `desktop-native/src/app/orq_timeline.rs:152`
- Modify: `desktop-native/src/app/side.rs:692`
- Modify: `desktop-native/src/app.rs:3070,3203,3890-3892,4865,5220,5295`
- Test: `desktop-native/src/api/dto.rs` (módulo `tests`)

**Interfaces:**
- Consumes (Task 1): `hangar_api::chat::{ChatEvent, ChatKind}`, `hangar_api::state::{StateEvent, ShellVivo}`, `hangar_api::preview::PreviewEvent`.
- Produces (em `crate::api::dto`):
  - `pub use hangar_api::chat::ChatEvent;`
  - `pub use hangar_api::preview::PreviewEvent as Preview;`
  - `pub use hangar_api::state::{ShellVivo as ShellAlive, StateEvent as SessionState};`
  - `pub trait ChatEventExt { fn queued(&self) -> bool; fn body(&self) -> String; fn loaded_skill(&self) -> Option<SkillLoaded>; fn orq_entry(&self) -> Option<OrqEntry>; }` com `impl ChatEventExt for ChatEvent`.
  - `pub fn plan_pending(state: &SessionState) -> Option<PlanPending>`.
  - `conversation::summarize_input(name: Option<&str>, input: Option<&Map<String, Value>>) -> String` e `conversation::pretty_input(input: Option<&Map<String, Value>>) -> String` (antes `Option<&Value>`).
- Fica como está: `AskPayload`, `AskItem`, `AskOption` do `dto.rs`. Eles leem o evento `ask_question`, que chega em dois formatos: o
  `AskQuestion.model_dump()` do Claude com terminal (`backend/app/sse.py:181`) e o `codex_question` do Codex e do
  Claude sem terminal (`backend/app/sse.py:1110-1114`, montado em `backend/app/adapters/codex/async_questions.py:47-48`
  com `provider`, `request_id`, `is_async`, e itens com `id`/`isOther`/`isSecret`). O `AskQuestion` do models.py só
  tem `questions`; trocar o `AskPayload` por ele apagaria a resposta ao Codex (conferido 2026-10-02:
  `desktop-native/src/interaction.rs:12-15` usa `provider`, `is_other`).

Adaptações, uma por uma (tudo o resto compila sem mudança):

| Diferença | Onde o desktop usa | Adaptação |
|---|---|---|
| `kind: String` → `ChatKind` | `e.kind == "x"` (60+ lugares), `kind.as_str()`, `kind: "x".into()` | nenhuma: `PartialEq<&str>`, `as_str()` e `From<&str>` do crate |
| `tool_input: Option<Value>` → `Option<Map>` | `.get(key)` em `conversation.rs:163,306-326`, `activity.rs:918`, `rows.rs:270`, `app.rs:2869` | nenhuma: `Map::get(&str)` devolve o mesmo `Option<&Value>` |
| | `summarize_input`/`pretty_input`/`loose_id`/`whole_list` recebem `Option<&Value>` | assinatura passa a `Option<&Map<String, Value>>` |
| | `app.rs:3070` passa o `Value` da ferramenta ao vivo | `tool.input.as_object()` |
| | `editdiff.rs:168` chama `edits(…, Option<&Value>)` | converte só quando a chamada não está no cache |
| | `find.rs:58` percorre o `Value` | percorre os valores do mapa |
| | `chat.rs:106` guarda o `Value` | `map(Value::Object)` |
| | `interaction.rs:53` usa `{input}` (Display de `Value`) | `serde_json::to_string(input)`: mesmo texto |
| | testes que montam `tool_input: Some(json!(…))` | `json!(…).as_object().cloned()` |
| `skill: Option<SkillLoaded>` → `Option<Map>` | `app.rs:3890,4865` | `event.loaded_skill()` |
| `orq: Option<OrqEntry>` → `Option<Map>` | `orq_timeline.rs:152`; teste `dto.rs:661` | `event.orq_entry()` |
| `impl ChatEvent { queued, body }` (tipo de fora não aceita `impl`) | `chat.rs`, `api/sse.rs`, `app.rs`, `app/baton.rs`, `app/controls.rs` | trait `ChatEventExt`; `app.rs` e os submódulos de `app` (`use super::*`) já recebem pelo `dto::*` |
| `login: Option<bool>` → `bool` | `app.rs:5220` | `self.chat.state.login` |
| `limited: Option<bool>` → `bool` | `side.rs:692`, `app.rs:5295` (`.or(lista)`: antes do primeiro `state` vale a lista) | `state.state.is_empty()` marca "ainda sem evento" |
| `claude_plan_pending: Option<PlanPending>` → `Option<Map>` | `app.rs:3203` | `plan_pending(state)`; `is_some()` em `controls.rs:797`, `app.rs:3246` não muda |
| `Preview` ganhou `session` | teste `chat.rs:530` sem `..Default::default()` | acrescenta `..Default::default()` |

- [x] **Step 1: Dependência e `Cargo.lock`**

Em `desktop-native/Cargo.toml`, logo após a linha `reqwest = …` (linha 17):

```toml
# Formatos da conversa compartilhados com o hangar-server: um tipo só dos dois lados.
hangar-api = { path = "../crates/hangar-api" }
```

Não mexer em `rust-toolchain.toml` (continua `1.98.1`) nem em `vendor/`/`[patch.crates-io]`.

Run: `cd desktop-native && cargo metadata --format-version 1 > /dev/null && git diff desktop-native/Cargo.lock`
Expected: o `cargo metadata` só resolve e grava o lock, sem compilar. O diff tem só isto (nenhuma versão de outra
crate muda, porque `serde`/`serde_json` são as mesmas e o `indexmap` já está no lock):

```diff
+[[package]]
+name = "hangar-api"
+version = "0.1.0"
+dependencies = [
+ "serde",
+ "serde_json",
+]
+
 [[package]]
 name = "hangar-native"
 version = "0.1.0"
 dependencies = [
  …
+ "hangar-api",
  …
```

Se o diff trouxer qualquer outra linha, parar e reverter o lock (`git checkout -- desktop-native/Cargo.lock`): é sinal
de que o workspace `crates/` puxou uma versão diferente, e isso volta para a Task 1.

- [x] **Step 2: Escrever o teste novo do `dto.rs`**

No fim do `mod tests` de `desktop-native/src/api/dto.rs` (antes do `}` da linha 708):

```rust
    #[test]
    fn chat_event_from_the_crate_keeps_the_desktop_reading() {
        let event: ChatEvent = serde_json::from_value(json!({"kind": "tipo_futuro", "id": "k1",
            "skill": {"name": "pdf", "path": "/s/pdf/SKILL.md", "body": "# PDF"}, "tool_input": {"command": "ls"}})).unwrap();
        assert_eq!(event.kind, "tipo_futuro");
        assert_eq!(event.loaded_skill().map(|s| (s.name, s.body)), Some(("pdf".to_owned(), "# PDF".to_owned())));
        assert_eq!(event.body(), "{\n  \"command\": \"ls\"\n}");
        let bad_skill: ChatEvent = serde_json::from_value(json!({"kind": "notice", "id": "n", "skill": {"body": "x"}})).unwrap();
        assert!(bad_skill.loaded_skill().is_none(), "skill sem nome não derruba a mensagem");
        let state: SessionState = serde_json::from_value(json!({"session": "s", "state": "idle",
            "claude_plan_pending": {"plan": "# Plano", "path": "/p.md", "tool_use_id": "t"}})).unwrap();
        let plan = plan_pending(&state).unwrap();
        assert_eq!((plan.plan.as_str(), plan.path.as_deref()), ("# Plano", Some("/p.md")));
        assert!(plan_pending(&SessionState::default()).is_none());
    }
```

E a linha 547 passa a importar o que o teste usa:

```rust
    use super::{ChatEvent, ChatEventExt, OrqEntry, OrqLine, OrqPanel, SessionInfo, SessionState, plan_pending};
```

- [x] **Step 3: Rodar e ver falhar** (quando autorizado)

Run: `cd desktop-native && cargo test --locked -- api::dto`
Expected: FAIL de compilação com `unresolved imports super::ChatEventExt, super::plan_pending`.

- [x] **Step 4: Trocar as structs do `dto.rs` pelas do crate**

Linhas 1-3 passam a ser:

```rust
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

// Os formatos da conversa vêm do crate que o hangar-server também usa; os nomes antigos ficam para o
// resto do app não mudar.
pub use hangar_api::chat::ChatEvent;
pub use hangar_api::preview::PreviewEvent as Preview;
pub use hangar_api::state::{ShellVivo as ShellAlive, StateEvent as SessionState};
```

Apagar `pub struct ChatEvent { … }` inteira (linhas 100-124). `SkillLoaded` (126-130) e todas as `Orq*` ficam.

Trocar o `impl ChatEvent { … }` (linhas 410-417) por:

```rust
/// O que o desktop tira de uma mensagem; o formato em si é o do `hangar-api`.
pub trait ChatEventExt {
    fn queued(&self) -> bool;
    fn body(&self) -> String;
    /// Skill de um notice `skill_loaded`; sem `name` em texto fica sem skill, sem derrubar a mensagem.
    fn loaded_skill(&self) -> Option<SkillLoaded>;
    /// Entrada da linha do tempo de uma sessão `orq`; formato que este app não lê fica sem entrada.
    fn orq_entry(&self) -> Option<OrqEntry>;
}

impl ChatEventExt for ChatEvent {
    fn queued(&self) -> bool { self.id.starts_with("queued-") }
    fn body(&self) -> String {
        self.text.clone().or_else(|| self.result.clone()).unwrap_or_else(|| {
            self.tool_input.as_ref().map(|v| serde_json::to_string_pretty(v).unwrap_or_default()).unwrap_or_default()
        })
    }
    fn loaded_skill(&self) -> Option<SkillLoaded> {
        let skill = self.skill.as_ref()?;
        let name = skill.get("name")?.as_str()?.to_owned();
        let body = skill.get("body").and_then(Value::as_str).unwrap_or_default().to_owned();
        Some(SkillLoaded { name, body })
    }
    fn orq_entry(&self) -> Option<OrqEntry> {
        serde_json::from_value(Value::Object(self.orq.clone()?)).ok()
    }
}
```

Apagar `pub struct SessionState { … }` e `pub struct ShellAlive { … }` (linhas 419-449). `Stats` fica.

Logo após `pub struct PlanPending { … }` (linhas 465-469), acrescentar:

```rust
/// Plano do Claude sem terminal esperando aprovação, lido do mapa que o estado traz.
pub fn plan_pending(state: &SessionState) -> Option<PlanPending> {
    let pending = state.claude_plan_pending.as_ref()?;
    let text = |key: &str| pending.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(PlanPending { plan: text("plan").unwrap_or_default(), path: text("path") })
}
```

Apagar `pub struct Preview { … }` (linhas 499-505). `AskOption`, `AskItem`, `AskPayload`, `Delivery`, `CommandInfo`,
`Uploaded`, `UploadFile`, `Steered` ficam.

No teste `chat_event_without_orq_is_unchanged` (linhas 654-662), a última linha passa a ser:

```rust
        assert_eq!(with.orq_entry().unwrap().kind, "notice");
```

Os outros testes do módulo (`orq_entry_*`, `orq_line_*`, `orq_panel_*`, `unreadable_codex_*`, `orq_row_*`,
`only_readable_*`, `pair_session_is_read_only`) não mudam: usam `OrqEntry`, `OrqLine`, `OrqPanel` e `SessionInfo`,
que continuam no `dto.rs`.

- [x] **Step 5: Adaptar os usos**

`desktop-native/src/api/sse.rs:7`:

```rust
use super::{Api, Failure, dto::{ChatEvent, ChatEventExt}};
```

`desktop-native/src/chat.rs:3`:

```rust
use crate::{api::dto::{ChatEvent, ChatEventExt, Preview, SessionState}, interaction::Ask};
```

`desktop-native/src/chat.rs:106`:

```rust
                let durable = LiveTool { name: event.tool_name.clone().unwrap_or_default(), input: event.tool_input.clone().map(Value::Object).unwrap_or(Value::Null) };
```

`desktop-native/src/chat.rs:530`:

```rust
        chat.update_preview(Preview { text: "long".into(), md: true, full: true, vivo: true, ..Default::default() });
```

`desktop-native/src/chat.rs:621`:

```rust
            tool_input: serde_json::json!({"command": "ls"}).as_object().cloned(), tool_use_id: Some("x".into()), ..Default::default() });
```

`desktop-native/src/conversation.rs:2`:

```rust
use serde_json::{Map, Value};
```

`desktop-native/src/conversation.rs:236` e `:242` (só a assinatura; o corpo usa `.get`, que o `Map` também tem):

```rust
fn loose_id(input: Option<&Map<String, Value>>, keys: &[&str]) -> String {
```

```rust
fn whole_list(input: Option<&Map<String, Value>>, list: &str, title: &str) -> Option<Vec<(ActivityTask, bool)>> {
```

`desktop-native/src/conversation.rs:478-479`:

```rust
pub fn summarize_input(name: Option<&str>, input: Option<&Map<String, Value>>) -> String {
    let Some(map) = input else { return String::new(); };
```

`desktop-native/src/conversation.rs:511-517` (o `Value::Null` de antes não existe mais no tipo):

```rust
pub fn pretty_input(input: Option<&Map<String, Value>>) -> String {
    match input {
        Some(map) if !map.is_empty() => serde_json::to_string_pretty(map).unwrap_or_default(),
        _ => String::new(),
    }
}
```

`desktop-native/src/conversation.rs:560` (todos os chamadores passam `json!({…})`):

```rust
    fn with_input(mut event: ChatEvent, input: Value) -> ChatEvent { event.tool_input = input.as_object().cloned(); event }
```

`desktop-native/src/conversation.rs:720-722`:

```rust
        assert_eq!(summarize_input(Some("Bash"), json!({"command": "ls   -la\n/tmp"}).as_object()), "ls -la /tmp");
        assert_eq!(summarize_input(Some("Grep"), json!({"pattern": "foo", "path": "src"}).as_object()), "\"foo\" src");
        assert_eq!(summarize_input(Some("mcp_x"), json!({"other": 3}).as_object()), "3");
```

`desktop-native/src/editdiff.rs:168` (o `edits` e os testes dele continuam com `Value`; a cópia só acontece quando
a chamada não está no cache):

```rust
        let found: Option<Rc<[Edit]>> = edits(call.tool_name.as_deref(), call.tool_input.clone().map(Value::Object).as_ref()).map(Into::into);
```

`desktop-native/src/app/find.rs:58`:

```rust
    if let Some(input) = &event.tool_input { for value in input.values() { values(value, &mut parts); } }
```

`desktop-native/src/interaction.rs:53` (`Display` de `Value` e `serde_json::to_string` dão o mesmo texto compacto):

```rust
    Some(Ask { fingerprint: format!("tool:{id}:{}", serde_json::to_string(input).unwrap_or_default()), payload: AskPayload { provider: None, request_id: None, is_async: false, questions }, tool_use_id: Some(id) })
```

`desktop-native/src/interaction.rs:222`:

```rust
            tool_input: input.as_object().cloned(), ..Default::default() }
```

`desktop-native/src/app/orq_timeline.rs:152`:

```rust
        let Some(orq) = event.orq_entry() else { return div().into_any_element() };
```

`desktop-native/src/app/side.rs:692`:

```rust
        // Antes do primeiro `state` da conversa vale o que a lista de sessões diz.
        let limited = if self.chat.state.state.is_empty() { self.selected.as_ref().and_then(|s| s.limited) == Some(true) } else { self.chat.state.limited };
```

`desktop-native/src/app.rs:3070`:

```rust
        let summary = conversation::summarize_input(Some(&tool.name), tool.input.as_object());
```

`desktop-native/src/app.rs:3203`:

```rust
        let plan = plan_pending(state).filter(|p| !p.plan.trim().is_empty());
```

`desktop-native/src/app.rs:3890-3892`:

```rust
                "notice" => event.loaded_skill()
                    .and_then(|skill| crate::i18n::tr_web("notice_skill_loaded", &HashMap::from([("name".to_owned(), skill.name)])))
                    .unwrap_or_else(|| tr("notice")),
```

`desktop-native/src/app.rs:4865`:

```rust
        "notice" => event.loaded_skill().map(|skill| skill.body).unwrap_or_else(|| tr(&event.body())),
```

`desktop-native/src/app.rs:5220`:

```rust
        let pending = card.is_none() && !answered && !ask_pane && !prethread_open && (self.chat.state.state == "awaiting_input" || self.chat.state.login);
```

`desktop-native/src/app.rs:5295`:

```rust
        let limited_now = if self.chat.state.state.is_empty() { self.selected.as_ref().and_then(|s| s.limited) == Some(true) } else { self.chat.state.limited };
```

Se o compilador disser que `queued`/`body` não existe em `app/baton.rs` ou `app/controls.rs` ("items from traits can
only be used if the trait is in scope"), acrescentar `use crate::api::dto::ChatEventExt;` no topo do arquivo.

- [x] **Step 6: Rodar e ver passar** (quando autorizado)

Run: `cd desktop-native && cargo test --locked -- api::dto chat:: conversation:: interaction:: editdiff:: app::`
Expected: PASS, com os 12 testes de `api::dto` (11 de antes + o novo).

Run: `cd desktop-native && cargo build --locked`
Expected: compila sem aviso novo.

- [ ] **Step 7: Uso real** (quando autorizado)

Abrir o app nativo (`cd desktop-native && cargo run --release`) ligado ao backend vivo e conferir: um chat do Claude
com ferramentas (chip com o resumo do Bash, diff de um Edit, busca na conversa achando texto da entrada da
ferramenta), um chat do Codex com pergunta pendente (o cartão responde), a prévia ao vivo, o selo de limite, o plano
pendente do Claude sem terminal, um notice de skill carregada e a linha do tempo de uma sessão `orq`.

- [x] **Step 8: Commit**

```bash
git add desktop-native/Cargo.toml desktop-native/Cargo.lock desktop-native/src/api/dto.rs desktop-native/src/api/sse.rs \
  desktop-native/src/chat.rs desktop-native/src/conversation.rs desktop-native/src/editdiff.rs \
  desktop-native/src/interaction.rs desktop-native/src/app/find.rs desktop-native/src/app/orq_timeline.rs \
  desktop-native/src/app/side.rs desktop-native/src/app.rs
git commit -m "feat(native): read chat, state and preview events through the shared hangar-api crate"
```

(Mais `desktop-native/src/app/baton.rs`/`controls.rs` no `git add` só se o Step 5 precisou do `use`.)

---

### Task 3: CI dos crates (`server.yml`) e o `native.yml` atento ao `hangar-api`

**Files:**
- Create: `.github/workflows/server.yml`
- Modify: `.github/workflows/native.yml:9-14` (paths) e `:143` (filtro do "O que mudou")

**Interfaces:**
- Consumes: `crates/Cargo.lock` e `crates/rust-toolchain.toml` (Task 1); `backend/tests/fixtures/contract/api_samples/` (Task 1, lidos pelo `cargo test`); `backend/tests/test_claude_headless_cano.py` parametrizado por `CP_RUST_CANO_BIN` (Task 5).
- Produces (frente 5 baixa):
  - Release fixa `server-latest`, criada com `--latest=false` para não tirar o selo "Latest" do `native-latest`.
  - Assets `hangar-server-<plataforma>[.exe]` e `hangar-cano-<plataforma>[.exe]`, plataformas `linux-x86_64`, `windows-x86_64`, `macos-aarch64`.
  - `server-latest.json`: `{"commit":"<sha>","files":{"<plataforma>/<bin>":{"name":"<asset>","sha256":"<hex>"}}}`. A chave nunca leva `.exe` (`windows-x86_64/hangar-server`); o `name` leva (`hangar-server-windows-x86_64.exe`).

- [x] **Step 1: Criar `.github/workflows/server.yml`**

Espelha o `native.yml`: matriz com Windows e macOS em `continue-on-error` (conferido 2026-10-02:
`.github/workflows/native.yml:41`), `cargo build --locked --release` com LTO por variável (`native.yml:69-72`),
artefatos juntados com `merge-multiple` (`native.yml:116`), manifesto subindo por último e a tag andando com
`gh api -X PATCH` (`native.yml:188-189`).

```yaml
# hangar-server e hangar-cano (crates/, Rust): testa e compila Linux, Windows e macOS e publica numa release FIXA
# `server-latest`, reescrita a cada push na main, como o `native-latest` do native.yml. O instalador e a atualização
# leem o `server-latest.json` dessa release e conferem o sha256 de cada binário antes de usar.
name: Server

on:
  push:
    branches: [main]
    paths:
      - "crates/**"
      - "backend/tests/fixtures/**"
      # A suíte de contrato do cano roda aqui contra o binário: mudar o cano.py ou ela também testa o Rust.
      - "backend/app/adapters/claude_headless/cano.py"
      - "backend/tests/test_claude_headless_cano.py"
      - ".github/workflows/server.yml"
  workflow_dispatch:

# Dois pushes seguidos não publicam ao mesmo tempo: o segundo espera, e a release termina no commit mais novo.
concurrency:
  group: server-latest
  cancel-in-progress: false

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            platform: linux-x86_64
            exe: ""
            experimental: false
          # Como no native.yml: falha de Windows ou macOS não segura a release do Linux.
          - os: windows-latest
            platform: windows-x86_64
            exe: ".exe"
            experimental: true
          - os: macos-latest
            platform: macos-aarch64
            exe: ""
            experimental: true
    runs-on: ${{ matrix.os }}
    continue-on-error: ${{ matrix.experimental }}
    permissions:
      contents: read
    defaults:
      run:
        working-directory: crates
        shell: bash
    steps:
      - uses: actions/checkout@v4
      # O rustup do runner lê o crates/rust-toolchain.toml (1.98.1) no primeiro cargo.
      - uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            crates/target
          key: server-${{ runner.os }}-${{ hashFiles('crates/Cargo.lock', 'crates/rust-toolchain.toml') }}
          restore-keys: server-${{ runner.os }}-
      # Os testes do hangar-api leem as amostras de backend/tests/fixtures/, que vêm no mesmo checkout.
      - run: cargo test --locked --workspace
      - run: cargo build --locked --release --workspace
        env:
          CARGO_PROFILE_RELEASE_LTO: fat
          CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "1"
      # A suíte de contrato do cano contra o binário que vai ser publicado. Só no Linux: é o único
      # lugar em que ela roda contra o hangar-cano (o ci.yml não compila Rust). Setup igual ao do ci.yml.
      - if: matrix.platform == 'linux-x86_64'
        uses: astral-sh/setup-uv@v5
        with:
          python-version: "3.14"
      - if: matrix.platform == 'linux-x86_64'
        working-directory: backend
        run: uv sync
      - name: Contrato do cano contra o hangar-cano
        if: matrix.platform == 'linux-x86_64'
        working-directory: backend
        run: |
          # Sem o binário a suíte pula os casos Rust e sairia verde: aqui a falta é erro.
          test -x "$GITHUB_WORKSPACE/crates/target/release/hangar-cano"
          CP_RUST_CANO_BIN=$GITHUB_WORKSPACE/crates/target/release/hangar-cano uv run pytest tests/test_claude_headless_cano.py
      - name: Empacotar
        run: |
          mkdir -p out
          for bin in hangar-server hangar-cano; do
            cp "target/release/$bin${{ matrix.exe }}" "out/$bin-${{ matrix.platform }}${{ matrix.exe }}"
          done
          ls -l out
      - uses: actions/upload-artifact@v4
        with:
          name: server-${{ matrix.platform }}
          path: crates/out/*
          retention-days: 1

  # Só este job escreve.
  publish:
    needs: build
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v4
        with:
          pattern: server-*
          path: out
          merge-multiple: true
      # Chave `<plataforma>/<bin>` sem `.exe`; o `name` é o asset exato a baixar.
      - name: sha256 e manifesto
        working-directory: out
        run: |
          for need in hangar-server-linux-x86_64 hangar-cano-linux-x86_64; do
            test -f "$need" || { echo "sem $need, nada a publicar"; exit 1; }
          done
          files='{}'
          for f in hangar-server-* hangar-cano-*; do
            base=${f%.exe}
            case "$base" in hangar-server-*) bin=hangar-server ;; *) bin=hangar-cano ;; esac
            platform=${base#"$bin"-}
            sha=$(sha256sum "$f" | cut -d' ' -f1)
            files=$(jq --arg k "$platform/$bin" --arg n "$f" --arg s "$sha" '.[$k] = {name: $n, sha256: $s}' <<<"$files")
          done
          jq -n --arg commit "$GITHUB_SHA" --argjson files "$files" '{commit: $commit, files: $files}' > server-latest.json
          cat server-latest.json
      # O manifesto sobe por ÚLTIMO: até ele trocar, quem baixa ainda vê o sha antigo e recusa o binário novo em vez
      # de usar um par trocado. Sistema que não compilou desta vez fica com o arquivo anterior e fora do manifesto.
      - working-directory: out
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          gh release view server-latest --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1 \
            || gh release create server-latest --repo "$GITHUB_REPOSITORY" --target "$GITHUB_SHA" --latest=false \
                 --title 'hangar-server e hangar-cano (topo da main)' \
                 --notes 'Reescrita a cada push na main. O instalador e a atualização do Hangar baixam daqui e conferem o sha256 pelo server-latest.json.'
          gh release upload server-latest hangar-server-* hangar-cano-* --repo "$GITHUB_REPOSITORY" --clobber
          gh release upload server-latest server-latest.json --repo "$GITHUB_REPOSITORY" --clobber
          gh api -X PATCH "repos/$GITHUB_REPOSITORY/git/refs/tags/server-latest" -f sha="$GITHUB_SHA" -F force=true >/dev/null
```

- [x] **Step 2: `native.yml` dispara com o `hangar-api`**

Em `.github/workflows/native.yml`, a lista de `paths` (linhas 10-14) passa a ser:

```yaml
    paths:
      - "desktop-native/**"
      - "crates/hangar-api/**"
      # As versões do hangar-api vêm do [workspace.dependencies] daqui.
      - "crates/Cargo.toml"
      - "messages/**"
      - "VERSION"
      - ".github/workflows/native.yml"
```

E a linha 143 inclui o crate no "O que mudou" das notas da release:

```bash
            changes=$(git log --no-merges -n 40 --format='- %s' "$previous..$GITHUB_SHA" -- desktop-native crates/hangar-api VERSION .github/workflows/native.yml)
```

- [x] **Step 3: Conferir os dois YAML e o manifesto localmente**

Run: `cd backend && uv run --with pyyaml python -c "import yaml; [yaml.safe_load(open(f)) for f in ('../.github/workflows/server.yml', '../.github/workflows/native.yml')]; print('ok')"`
Expected: `ok`.

Run (o mesmo laço do passo "sha256 e manifesto", sobre arquivos falsos; Windows de propósito com `.exe` e macOS
ausente, como numa publicação em que ele falhou):

```bash
tmp=$(mktemp -d) && (cd "$tmp" && printf a > hangar-server-linux-x86_64 && printf b > hangar-cano-linux-x86_64 \
  && printf c > hangar-server-windows-x86_64.exe && printf d > hangar-cano-windows-x86_64.exe && GITHUB_SHA=abc123 bash -c '
files="{}"
for f in hangar-server-* hangar-cano-*; do
  base=${f%.exe}
  case "$base" in hangar-server-*) bin=hangar-server ;; *) bin=hangar-cano ;; esac
  platform=${base#"$bin"-}
  sha=$(sha256sum "$f" | cut -d" " -f1)
  files=$(jq --arg k "$platform/$bin" --arg n "$f" --arg s "$sha" ".[\$k] = {name: \$n, sha256: \$s}" <<<"$files")
done
jq -n --arg commit "$GITHUB_SHA" --argjson files "$files" "{commit: \$commit, files: \$files}"'); rm -rf "$tmp"
```

Expected: `commit` = `abc123` e quatro chaves em `files`: `linux-x86_64/hangar-server`, `linux-x86_64/hangar-cano`,
`windows-x86_64/hangar-server` (com `"name": "hangar-server-windows-x86_64.exe"`) e `windows-x86_64/hangar-cano`; o
`sha256` de `linux-x86_64/hangar-server` é `ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb` (sha256 de `a`).

- [x] **Step 4: Commit**

```bash
git add .github/workflows/server.yml .github/workflows/native.yml
git commit -m "ci(server): test and publish hangar-server and hangar-cano to the server-latest release"
```

- [ ] **Step 5: Conferir a primeira publicação** (depois do push autorizado pelo usuário)

Run: `gh run list --workflow server.yml --limit 1` e, terminado, `gh release download server-latest -p server-latest.json -O - | jq .`
Expected: run verde no Linux (Windows/macOS podem ficar amarelos); o manifesto com o `commit` do push e as chaves
`linux-x86_64/hangar-server` e `linux-x86_64/hangar-cano`; `gh release view native-latest --json isLatest` continua `true`.

---

#### Notas (dependências e riscos)

**Desvios do contrato**

1. **`AskPayload` do desktop não foi trocado.** O evento `ask_question` chega em dois formatos (Claude com terminal:
   `AskQuestion` do models.py; Codex e Claude sem terminal: o `codex_question`, com `provider`, `request_id`,
   `is_async`, `id`, `isOther`, `isSecret`). O `hangar_api::ask::AskQuestion` espelha só o models.py, como pede o
   contrato; o desktop continua com `AskPayload`, que cobre os dois. Trocar apagaria a resposta ao Codex.
2. **`StateEvent.state` e `codex_mode` são `String`**, não enum: o hangar-server só repassa este evento, e o desktop
   compara texto. `ChatKind` é enum, com `Other(String)` e serde escrito à mão (sem `#[serde(other)]`) para guardar o
   tipo desconhecido e devolvê-lo igual.
3. **Campos obrigatórios no Python com padrão no Rust:** `StateEvent.session`/`state` e `PreviewEvent.session`/`text`,
   porque o desktop já lia esses objetos sem eles. `ChatEvent.kind`/`id` e os de `AskQuestion` seguem obrigatórios.
4. **Inteiros:** `cache_read`/`cache_ttl_s` `u64`, `image_count`/`loop_iter`/`loop_max` `u32`, `pid` `i64`, os mesmos
   tipos que o desktop lia. A frente 3 monta `cache_read` a partir do `usage` do transcript com esses tipos.
5. **`multiSelect`** é `multi_select` no Rust com `#[serde(rename = "multiSelect")]`; o JSON é o mesmo.
6. **`preserve_order` ligado no `serde_json` do workspace.** A frente 3 precisa dele para o `pyjson::dumps` e para o
   `tool_input` sair na ordem do Python; o desktop já o tinha ligado (o lock dele não muda).
7. **`native.yml` também observa `crates/Cargo.toml`**, além de `crates/hangar-api/**`: as versões do crate vêm de lá.
8. **Manifesto:** chave sem `.exe`, `name` com `.exe`. A release `server-latest` nasce com `--latest=false`. Não há
   `.sha256` avulso por arquivo (o `native-latest` tem, para quem baixa à mão; aqui ninguém baixa à mão).
9. **Extra:** `backend/tests/test_contract_api_samples.py` (roda no `ci.yml`) falha quando o models.py muda sem
   regenerar as amostras. A spec cita "estatísticas" no `hangar-api`; o contrato lista só os quatro módulos, e o
   `stats` ficou de fora.

**Dependências entre Tasks e frentes**

- Task 2 e Task 3 dependem da Task 1 (crate e `crates/Cargo.lock`).
- As Tasks 4 e 6-12 nunca recriam o workspace: acrescentam as próprias linhas em `[workspace.dependencies]` (`tokio`,
  `chrono`, `libc`, `windows-sys` na 4; `regex`, `md-5`, `sha1_smol`, `tempfile` nas 7-9; `axum` e cia. na 11;
  `notify` e cia. na 12, todas na versão do desktop quando ele as tem) e nos `[dependencies]` de cada crate. A Task 4
  troca o `Cargo.toml`/`main.rs` do `hangar-cano`, a Task 6 o `lib.rs` do `hangar-server` e a Task 11 o `main.rs` dele.
- O `hangar-server` passa a depender do crate (`hangar-api.workspace = true`) na Task 7.
- O job Linux do `server.yml` roda `test_claude_headless_cano.py` contra o `hangar-cano` recém-compilado (Task 5).
- **A Task 3 publica o que estiver em `crates/`.** Se ela for para a main antes das frentes 2 e 4, o `server-latest`
  ganha binários que só saem. A frente 5 não pode baixar nada antes disso: subir a Task 3 junto com (ou depois de) um
  `hangar-server` que responde `/__hangar_server/health`, ou a frente 5 só passar a baixar depois.
- Artefato do GitHub perde o bit de execução: a Task 14 grava com `chmod 0o755` o que baixa para `~/.hangar/bin/`.

**Riscos**

- **Herança do workspace por dependência de caminho:** o desktop carrega `../crates/hangar-api`, cujo `Cargo.toml` usa
  `version.workspace = true`/`serde.workspace = true`. O cargo resolve isso subindo até `crates/Cargo.toml`. Se o
  `cargo metadata` do Task 2 Step 1 recusar, trocar no `hangar-api/Cargo.toml` as heranças pelas versões escritas
  (`=1.0.229` com `derive`, `=1.0.151` com `preserve_order`).
- **CRLF no Windows:** o checkout do runner Windows converte as fixtures para CRLF (o `.gitattributes` só fixa `*.sh` e
  `*.hook`). As amostras JSON desta frente não se importam, mas fixture de transcript com offset em byte (frente 3) muda
  no Windows. Sugestão para a frente 3: `backend/tests/fixtures/** -text` no `.gitattributes`.
- **Byte a byte com o Python:** a ordem das chaves sai igual (campos na ordem de models.py, `preserve_order`), mas
  float extremo não: o Python escreve `1e-05`/`1e+16`, o `serde_json` escreve `1e-5`/`1e16`. Epoch e contagens comuns
  saem iguais; comparação byte a byte da frente 3/4 precisa evitar esses valores ou comparar como JSON.
- **Leitura mais tolerante no desktop:** `skill` e `orq` malformados não derrubam mais a mensagem inteira (antes o SSE
  respondia `invalid_response`); `orq_entry()` desserializa a cada desenho, como o `clone()` de antes fazia.
  O contrário também existe: `login`/`limited` em `null` deixam de ser aceitos, mas o Python declara `bool` e nunca
  manda `null`.
- **`tool_input` que não é objeto:** o pydantic recusa (`Optional[dict]`); a frente 3 precisa decidir a mesma coisa
  (não montar o evento ou mandar `None`) olhando o que o parser Python faz nesse caso.
- Os testes do desktop compilam o GPUI inteiro: o Task 2 Step 6 é demorado.

### Task 4: `hangar-cano` em Rust — port fiel do `cano.py`

**Files:**
- Modify: `crates/Cargo.toml` (criado na Task 1; só acrescenta linhas em `[workspace.dependencies]`)
- Modify: `crates/hangar-cano/Cargo.toml` (esqueleto da Task 1; ganha as dependências)
- Modify: `crates/hangar-cano/src/main.rs` (esqueleto da Task 1; argumentos, log, socket, filho, clientes, ciclo de vida)
- Create: `crates/hangar-cano/src/protocol.rs` (rastreamento do snapshot e linhas `cano_*`)
- Create: `crates/hangar-cano/src/stderr_text.rs` (stderr na codepage local)
- Modify: `crates/Cargo.lock` (atualizado pelo `cargo build`)
- Test: módulos `#[cfg(test)]` dos três arquivos de `src/` (`cargo test -p hangar-cano`)

**Interfaces:**
- Consumes: nada de outra Task. Dependências fixadas com `=` nas versões do `desktop-native/Cargo.lock`
  (conferido 2026-10-02: tokio 1.53.1, serde 1.0.229, serde_json 1.0.151, chrono 0.4.45, libc 0.2.189,
  windows-sys 0.61.2).
- Produces (o contrato de `cano.py`, sem mudança para o backend):
  - Linha de comando: `hangar-cano --escuta unix:<caminho>|tcp:<host>:<porta> [--token T] [--log F] [--cwd D] -- <argv...>`;
    também aceita `--opcao=valor`; o comando começa no `--` ou no primeiro posicional (`cano.py:372-379`).
  - Saída 2 sem `--escuta` ou sem comando (`cano.py:380-382`); saída 1 se a escuta ou o filho
    falham (`cano.py:390-400`); saída 0 no fim normal e no SIGTERM (`cano.py:416,420`).
  - Protocolo em linhas UTF-8 terminadas em `\n`:
    - com `--token`, a primeira linha do cliente é o token, prazo de 10 s; errado = fecha sem
      snapshot (`cano.py:235-246`);
    - `{"type": "cano_snapshot", "versao": 1, "pid": int, "init": str|null, "aberto": bool,
      "pendentes": [str], "ultimo_result": str|null, "rate_limit": str|null, "stderr_tail": [str],
      "saiu": int|null}` — `init`/`pendentes`/`ultimo_result`/`rate_limit` são a LINHA CRUA do
      filho (`cano.py:213-221`);
    - `{"type": "cano_stderr", "linha": str}` (`cano.py:101`);
    - `{"type": "cano_saiu", "rc": int, "stderr_tail": [str]}` com prazo de 5 s (`cano.py:176-190`);
    - as demais linhas são as do filho, repassadas sem mudança.
  - `protocol::VERSION: u32 = 1` — o mesmo `cano_mod.VERSAO` que o adapter compara
    (conferido 2026-10-02: `adapter.py:848`).

Regras herdadas que esta Task cumpre, com a prova no código de hoje:
- Um cliente por vez, o novo derruba o antigo, a fila do antigo não vai para o novo (conferido
  2026-10-02: `cano.py:247-257`; `docs/decisoes/harnesses.md:58` "Um cliente por cano").
- Bind ANTES de subir o filho, socket unix `0600`, backlog 2 (conferido 2026-10-02: `cano.py:294-312`).
- Atendimento de cada cliente em tarefa própria; em série, o backend mata o cano como mudo
  (conferido 2026-10-02: `cano.py:223-231`; `docs/decisoes/harnesses.md:1757-1771`).
- O filho fica no mesmo grupo de processos do cano: nada de `setsid`/`process_group` no `Command`
  (conferido 2026-10-02: `cano.py:68`; `adapter.py:2163` põe o cano em sessão nova e `_matar_grupo`
  em `adapter.py:2069` mata o grupo).

- [x] **Step 1: Workspace `crates/`**

O workspace, o `rust-toolchain.toml`, o `.gitignore` e o membro `hangar-cano` já existem (Task 1).
Em `crates/Cargo.toml`, acrescentar ao fim de `[workspace.dependencies]` (o `serde` e o `serde_json`
da Task 1 ficam como estão):

```toml
tokio = { version = "=1.53.1" }
chrono = { version = "=0.4.45", default-features = false, features = ["clock"] }
libc = "=0.2.189"
windows-sys = "=0.61.2"
```

`crates/hangar-cano/Cargo.toml` (troca o esqueleto da Task 1 inteiro):

```toml
[package]
name = "hangar-cano"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
tokio = { workspace = true, features = ["rt", "net", "process", "signal", "io-util", "time", "sync", "macros"] }
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }

[target.'cfg(unix)'.dependencies]
libc = { workspace = true }

[target.'cfg(windows)'.dependencies]
windows-sys = { workspace = true, features = ["Win32_Globalization"] }
```

- [x] **Step 2: Escrever os testes**

`crates/hangar-cano/src/main.rs`, provisório, só para os módulos compilarem nos testes (o Step 5
troca o arquivo inteiro):

```rust
mod protocol;
mod stderr_text;

fn main() {}
```

Fim de `crates/hangar-cano/src/protocol.rs` — rastreamento igual a `cano.py:103-158`, testado pelo
snapshot depois de uma sequência de linhas:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn snap(t: &Tracker) -> Value {
        serde_json::from_str(&t.snapshot(42, &VecDeque::from(["aviso".to_owned()]), None)).unwrap()
    }

    fn pending_ids(t: &Tracker) -> Vec<Value> {
        snap(t)["pendentes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|raw| {
                let ev: Value = serde_json::from_str(raw.as_str().unwrap()).unwrap();
                ev.get("request_id").or_else(|| ev.get("id")).cloned().unwrap()
            })
            .collect()
    }

    #[test]
    fn snapshot_has_the_fields_and_types_of_cano_py() {
        let mut t = Tracker::default();
        let init = r#"{"type": "system", "subtype": "init", "session_id": "sid-1"}"#;
        t.observe_child(init);
        let s = snap(&t);
        let keys: Vec<&str> = s.as_object().unwrap().keys().map(String::as_str).collect();
        let mut expected = vec![
            "type", "versao", "pid", "init", "aberto", "pendentes", "ultimo_result", "rate_limit",
            "stderr_tail", "saiu",
        ];
        let mut got = keys.clone();
        got.sort();
        expected.sort();
        assert_eq!(got, expected);
        assert_eq!(s["type"], "cano_snapshot");
        assert_eq!(s["versao"], 1);
        assert_eq!(s["pid"], 42);
        assert_eq!(s["init"], init); // a linha crua, como string
        assert_eq!(s["aberto"], false);
        assert_eq!(s["pendentes"], serde_json::json!([]));
        assert!(s["ultimo_result"].is_null() && s["rate_limit"].is_null() && s["saiu"].is_null());
        assert_eq!(s["stderr_tail"], serde_json::json!(["aviso"]));
    }

    #[test]
    fn claude_turn_with_pending_permission_then_answered() {
        let mut t = Tracker::default();
        t.observe_client(r#"{"type": "user", "message": {"content": []}}"#);
        t.observe_child(r#"{"type": "control_request", "request_id": "perm-1", "request": {"subtype": "can_use_tool"}}"#);
        assert_eq!(snap(&t)["aberto"], true);
        assert_eq!(pending_ids(&t), vec![Value::from("perm-1")]);
        t.observe_client(r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "perm-1"}}"#);
        assert!(pending_ids(&t).is_empty());
        let result = r#"{"type": "result", "subtype": "success"}"#;
        t.observe_child(result);
        let s = snap(&t);
        assert_eq!(s["aberto"], false);
        assert_eq!(s["ultimo_result"], result);
    }

    #[test]
    fn result_clears_pending_and_lifecycle_opens_turn() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"type": "command_lifecycle", "state": "started"}"#);
        t.observe_child(r#"{"type": "sdk_control_request", "request_id": 7}"#);
        t.observe_child(r#"{"type": "command_lifecycle", "state": "finished"}"#);
        assert_eq!(snap(&t)["aberto"], true);
        assert_eq!(pending_ids(&t), vec![Value::from(7)]);
        t.observe_child(r#"{"type": "result"}"#);
        assert_eq!(snap(&t)["aberto"], false);
        assert!(pending_ids(&t).is_empty());
    }

    #[test]
    fn cancel_and_rate_limit() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"type": "control_request", "request_id": "a"}"#);
        t.observe_child(r#"{"type": "control_request", "request_id": "b"}"#);
        t.observe_child(r#"{"type": "control_cancel_request", "request_id": "a"}"#);
        assert_eq!(pending_ids(&t), vec![Value::from("b")]);
        let rl = r#"{"type": "rate_limit_event", "rate_limit_info": {}}"#;
        t.observe_child(rl);
        assert_eq!(snap(&t)["rate_limit"], rl);
    }

    #[test]
    fn repeated_id_keeps_its_position() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"type": "control_request", "request_id": "a", "v": 1}"#);
        t.observe_child(r#"{"type": "control_request", "request_id": "b"}"#);
        t.observe_child(r#"{"type": "control_request", "request_id": "a", "v": 2}"#);
        let s = snap(&t);
        let first: Value = serde_json::from_str(s["pendentes"][0].as_str().unwrap()).unwrap();
        assert_eq!((first["request_id"].clone(), first["v"].clone()), (Value::from("a"), Value::from(2)));
        assert_eq!(pending_ids(&t), vec![Value::from("a"), Value::from("b")]);
    }

    #[test]
    fn jsonrpc_requests_resolutions_and_turn_completed() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 0, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th"}}"#);
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 1, "method": "item/fileChange/requestApproval", "params": {"threadId": "th"}}"#);
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 2, "method": "item/fileChange/requestApproval", "params": {"threadId": "outra"}}"#);
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 3, "method": "x/requestApproval", "params": {"threadId": "th"}}"#);
        assert_eq!(pending_ids(&t).len(), 4);
        // resposta do cliente: sem type e sem method
        t.observe_client(r#"{"jsonrpc": "2.0", "id": 0, "result": {"decision": "accept"}}"#);
        // outro cliente respondeu: o servidor avisa
        t.observe_child(r#"{"jsonrpc": "2.0", "method": "serverRequest/resolved", "params": {"threadId": "th", "requestId": 3}}"#);
        assert_eq!(pending_ids(&t), vec![Value::from(1), Value::from(2)]);
        t.observe_child(r#"{"jsonrpc": "2.0", "method": "turn/completed", "params": {"threadId": "th"}}"#);
        assert_eq!(pending_ids(&t), vec![Value::from(2)]);
    }

    #[test]
    fn integer_and_string_ids_are_the_same_key_like_python_str() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 1, "method": "m"}"#);
        t.observe_client(r#"{"id": "1"}"#);
        assert!(pending_ids(&t).is_empty());
    }

    #[test]
    fn client_line_with_method_is_not_an_answer() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 5, "method": "m"}"#);
        t.observe_client(r#"{"jsonrpc": "2.0", "id": 5, "method": "turn/start"}"#);
        assert_eq!(pending_ids(&t), vec![Value::from(5)]);
    }

    #[test]
    fn failed_guard_falls_through_to_jsonrpc_like_elif() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"type": "system", "subtype": "status", "method": "x", "id": 9}"#);
        assert!(snap(&t)["init"].is_null());
        assert_eq!(pending_ids(&t), vec![Value::from(9)]);
    }

    #[test]
    fn lone_surrogate_line_is_still_observed_and_kept_raw() {
        let mut t = Tracker::default();
        let line = "{\"type\": \"control_request\", \"request_id\": \"p\", \"input\": \"corte \\ud83d\"}";
        t.observe_child(line);
        assert_eq!(snap(&t)["pendentes"][0], line);
        let result = "{\"type\": \"result\", \"result\": \"fim \\udc00 e \\\\ud800\"}";
        t.observe_child(result);
        assert_eq!(snap(&t)["aberto"], false);
        assert_eq!(snap(&t)["ultimo_result"], result);
    }

    #[test]
    fn scrub_keeps_valid_pairs_and_escaped_backslashes() {
        let pair = "\"\\ud83d\\ude00\"";
        assert_eq!(scrub_lone_surrogates(pair), None);
        assert_eq!(scrub_lone_surrogates("\"\\\\ud800\""), None); // barra escapada: texto, não escape
        let lone_then_pair = "\"a\\ud800\\ud83d\\ude00b\"";
        let fixed = "\"a\\ufffd\\ud83d\\ude00b\"";
        assert_eq!(scrub_lone_surrogates(lone_then_pair).as_deref(), Some(fixed));
    }

    #[test]
    fn invalid_or_non_object_lines_are_ignored() {
        let mut t = Tracker::default();
        t.observe_child("não é json");
        t.observe_child("[1, 2]");
        t.observe_client(r#""user""#);
        let s = snap(&t);
        assert_eq!(s["aberto"], false);
        assert!(s["init"].is_null());
    }

    #[test]
    fn stderr_and_exit_lines() {
        let s: Value = serde_json::from_str(&stderr_line("tchau")).unwrap();
        assert_eq!(s, serde_json::json!({"type": "cano_stderr", "linha": "tchau"}));
        let tail = VecDeque::from(["tchau".to_owned()]);
        let e: Value = serde_json::from_str(&exit_line(Some(3), &tail)).unwrap();
        assert_eq!(e, serde_json::json!({"type": "cano_saiu", "rc": 3, "stderr_tail": ["tchau"]}));
    }
}
```

Fim de `crates/hangar-cano/src/stderr_text.rs` — o primeiro teste é o equivalente de
`test_stderr_na_codepage_do_windows_nao_vira_caractere_quebrado`
(`backend/tests/test_claude_headless_cano.py:295`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Equivalente a `test_stderr_na_codepage_do_windows_nao_vira_caractere_quebrado`
    /// (backend/tests/test_claude_headless_cano.py:295).
    #[test]
    fn stderr_in_windows_codepage_is_not_a_broken_character() {
        assert_eq!(stderr_text("não existe".as_bytes()), "não existe");
        let cp = stderr_text(b"n\xe3o existe"); // "não existe" em cp1252
        assert!(!cp.contains('\u{FFFD}') && cp.starts_with('n') && cp.ends_with("o existe"));
    }

    #[test]
    fn cp1252_matches_the_python_codec() {
        assert_eq!(cp1252(b"n\xe3o \x80 \x81 \x9f"), "não € \u{FFFD} Ÿ");
    }
}
```

- [x] **Step 3: Rodar e ver falhar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-cano`
Expected: FAIL de compilação: `cannot find type Tracker`, `cannot find function stderr_text`,
`cannot find function cp1252`.

- [x] **Step 4: Implementar `protocol.rs` e `stderr_text.rs`**

Notas do port, regra a regra:
- `observe_child` é o `_observar_claude` (`cano.py:103-142`). O `match` com guarda reproduz o
  `elif`: `type == "system"` com `subtype` diferente de `init` segue testando os ramos de baixo,
  inclusive o de JSON-RPC (`"method" in ev`).
- As chaves de `pendentes` são `str()` do Python (`cano.py:116,118,129,131,156,158`): `py_str`
  dá `"None"` para ausente/null e o texto para string; assim o id inteiro `1` do servidor e a
  resposta `"1"` do cliente casam como lá.
- `pendentes` preserva a ordem de inserção e chave repetida troca o valor sem mudar de lugar (dict).
- `ev.get("params") or {}` (`cano.py:131,135,141`): o que não for objeto conta como vazio.
- `turn/completed` remove os pendentes cujo `params.threadId` é igual ao da notificação, com
  ausente igual a null (`cano.py:132-142`); linha guardada que não lê fica.
- `observe_client` é o `_observar_cliente` (`cano.py:144-158`): `type` ausente ou null, sem
  `method` e com `id` não-null = resposta JSON-RPC.
- O `json.loads` aceita escape de surrogate sem par; o `serde_json` recusa
  (`LoneLeadingSurrogateInHexEscape`). Sem tratamento, um `result` com emoji cortado não fecharia o
  turno. `parse_object` tenta de novo trocando só esses escapes pelo de U+FFFD; o que se guarda é
  sempre a linha crua.
- `stderr_text` é o `_texto_do_stderr` (`cano.py:326-341`): UTF-8 válido passa; senão, codepage
  local. No Windows, `GetACP()`; 65001 (UTF-8) e 1252 caem na tabela cp1252 igual ao codec do
  Python (os cinco bytes sem caractere viram U+FFFD); outra codepage vai por `MultiByteToWideChar`.
  Fora do Windows o locale é UTF-8, então cai sempre no cp1252, como o Python faz lá.

`crates/hangar-cano/src/protocol.rs`, acima do `#[cfg(test)] mod tests` do Step 2:

```rust
//! O que o cano observa nos dois sentidos para montar o snapshot, e as linhas que ele mesmo
//! escreve. Port de `cano.py:103-158` e `cano.py:183,213-221`.

use std::collections::VecDeque;

use serde::Serialize;
use serde_json::{Map, Value};

/// `cano.py:32`. O adapter reabre a sessão ociosa quando o snapshot traz outra versão.
pub const VERSION: u32 = 1;

#[derive(Default)]
pub struct Tracker {
    pub init: Option<String>,
    pub turn_open: bool,
    // Ordem de inserção, como o dict do Python: o snapshot lista os pendentes nessa ordem.
    pending: Vec<(String, String)>,
    pub last_result: Option<String>,
    pub rate_limit: Option<String>,
}

impl Tracker {
    /// Linha que o filho escreveu (`cano.py:103`).
    pub fn observe_child(&mut self, line: &str) {
        let Some(ev) = parse_object(line) else { return };
        match ev.get("type").and_then(Value::as_str) {
            Some("system") if str_field(&ev, "subtype") == Some("init") => {
                self.init = Some(line.to_owned());
            }
            Some("command_lifecycle") if str_field(&ev, "state") == Some("started") => {
                self.turn_open = true;
            }
            Some("control_request" | "sdk_control_request") => {
                self.put(py_str(ev.get("request_id")), line);
            }
            Some("control_cancel_request") => self.remove(&py_str(ev.get("request_id"))),
            Some("result") => {
                self.turn_open = false;
                self.last_result = Some(line.to_owned());
                self.pending.clear();
            }
            Some("rate_limit_event") => self.rate_limit = Some(line.to_owned()),
            // Guarda que falha cai aqui, como o `elif` do Python.
            _ if ev.contains_key("method") => self.observe_rpc(&ev, line),
            _ => {}
        }
    }

    /// JSON-RPC do app-server do Codex (`cano.py:125-142`).
    fn observe_rpc(&mut self, ev: &Map<String, Value>, line: &str) {
        if let Some(id) = ev.get("id").filter(|v| !v.is_null()) {
            self.put(py_str(Some(id)), line);
            return;
        }
        match ev.get("method").and_then(Value::as_str) {
            Some("serverRequest/resolved") => {
                self.remove(&py_str(params(ev).and_then(|p| p.get("requestId"))));
            }
            Some("turn/completed") => {
                // Turno fechado leva os pedidos da thread junto, senão o snapshot repovoa um
                // cartão que o servidor já esqueceu.
                let thread = params(ev).and_then(|p| p.get("threadId")).cloned().unwrap_or(Value::Null);
                self.pending.retain(|(_, raw)| match parse_object(raw) {
                    None => true,
                    Some(req) => params(&req).and_then(|p| p.get("threadId")).unwrap_or(&Value::Null) != &thread,
                });
            }
            _ => {}
        }
    }

    /// Linha que o cliente mandou ao filho (`cano.py:144`).
    pub fn observe_client(&mut self, line: &str) {
        let Some(ev) = parse_object(line) else { return };
        match ev.get("type") {
            Some(Value::String(t)) if t == "user" => self.turn_open = true,
            Some(Value::String(t)) if t == "control_response" => {
                let rid = ev.get("response").and_then(Value::as_object).and_then(|r| r.get("request_id"));
                self.remove(&py_str(rid));
            }
            // Resposta JSON-RPC a um pedido do servidor.
            None | Some(Value::Null) if !ev.contains_key("method") => {
                if let Some(id) = ev.get("id").filter(|v| !v.is_null()) {
                    self.remove(&py_str(Some(id)));
                }
            }
            _ => {}
        }
    }

    pub fn snapshot(&self, pid: u32, stderr_tail: &VecDeque<String>, exited: Option<i64>) -> String {
        to_json(&Snapshot {
            kind: "cano_snapshot",
            versao: VERSION,
            pid,
            init: self.init.as_deref(),
            aberto: self.turn_open,
            pendentes: self.pending.iter().map(|(_, raw)| raw.as_str()).collect(),
            ultimo_result: self.last_result.as_deref(),
            rate_limit: self.rate_limit.as_deref(),
            stderr_tail,
            saiu: exited,
        })
    }

    fn put(&mut self, key: String, line: &str) {
        // Chave repetida troca o valor e mantém a posição, como no dict.
        match self.pending.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = line.to_owned(),
            None => self.pending.push((key, line.to_owned())),
        }
    }

    fn remove(&mut self, key: &str) {
        self.pending.retain(|(k, _)| k != key);
    }
}

/// Campos e ordem de `cano.py:214-221`.
#[derive(Serialize)]
struct Snapshot<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    versao: u32,
    pid: u32,
    init: Option<&'a str>,
    aberto: bool,
    pendentes: Vec<&'a str>,
    ultimo_result: Option<&'a str>,
    rate_limit: Option<&'a str>,
    stderr_tail: &'a VecDeque<String>,
    saiu: Option<i64>,
}

#[derive(Serialize)]
struct StderrLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    linha: &'a str,
}

#[derive(Serialize)]
struct ExitLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    rc: Option<i64>,
    stderr_tail: &'a VecDeque<String>,
}

/// `cano.py:101`.
pub fn stderr_line(line: &str) -> String {
    to_json(&StderrLine { kind: "cano_stderr", linha: line })
}

/// `cano.py:183`.
pub fn exit_line(rc: Option<i64>, stderr_tail: &VecDeque<String>) -> String {
    to_json(&ExitLine { kind: "cano_saiu", rc, stderr_tail })
}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("struct sem mapa sempre vira JSON")
}

fn str_field<'a>(ev: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    ev.get(key).and_then(Value::as_str)
}

/// `ev.get("params") or {}`: o que não é objeto conta como vazio.
fn params(ev: &Map<String, Value>) -> Option<&Map<String, Value>> {
    ev.get("params").and_then(Value::as_object)
}

/// `str()` do Python sobre o valor: a chave nunca sai do processo, só precisa casar entre os
/// dois sentidos (`str(1)` e `str("1")` são a mesma chave lá e aqui).
fn py_str(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".to_owned(),
        Some(Value::Bool(true)) => "True".to_owned(),
        Some(Value::Bool(false)) => "False".to_owned(),
        Some(Value::String(s)) => s.clone(),
        // ponytail: número sai como o serde_json escreve; lista ou objeto como id não acontece.
        Some(other) => other.to_string(),
    }
}

fn parse_object(line: &str) -> Option<Map<String, Value>> {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        // O `json.loads` aceita `\ud800` sem par e o serde_json não; a linha guardada é a crua,
        // então trocar o escape só para ler os campos não muda nada do que sai.
        Err(_) => serde_json::from_str(&scrub_lone_surrogates(line)?).ok()?,
    };
    match value {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// Troca escape de surrogate (D800-DFFF) sem par pelo escape de U+FFFD. None quando não havia nada para trocar.
fn scrub_lone_surrogates(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut copied, mut changed) = (0, 0, false);
    while i < b.len() {
        if b[i] != b'\\' {
            i += 1;
            continue;
        }
        if b.get(i + 1) != Some(&b'u') {
            i += 2; // outro escape, inclusive `\\`
            continue;
        }
        let Some(cp) = hex4(b, i + 2) else {
            i += 2;
            continue;
        };
        if (0xD800..0xDC00).contains(&cp)
            && b.get(i + 6) == Some(&b'\\')
            && b.get(i + 7) == Some(&b'u')
            && hex4(b, i + 8).is_some_and(|lo| (0xDC00..0xE000).contains(&lo))
        {
            i += 12;
            continue;
        }
        if (0xD800..0xE000).contains(&cp) {
            out.push_str(&s[copied..i]);
            out.push_str("\\ufffd");
            copied = i + 6;
            changed = true;
        }
        i += 6;
    }
    if !changed {
        return None;
    }
    out.push_str(&s[copied..]);
    Some(out)
}

fn hex4(b: &[u8], at: usize) -> Option<u32> {
    let digits = std::str::from_utf8(b.get(at..at + 4)?).ok()?;
    u32::from_str_radix(digits, 16).ok()
}
```

`crates/hangar-cano/src/stderr_text.rs`, acima do `#[cfg(test)] mod tests` do Step 2:

```rust
//! Texto de uma linha do stderr do filho (`cano.py:326-341`). O claude escreve UTF-8; scripts do
//! Windows no meio (hangar-engine.CMD, cmd) escrevem na codepage local, e decodificar como UTF-8
//! punha U+FFFD no aviso de problema.

pub fn stderr_text(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_owned(),
        Err(_) => local_codepage(raw),
    }
}

/// Locale UTF-8 cai no cp1252: repetir o UTF-8 só trocaria os acentos por U+FFFD.
// ponytail: fora do Windows o locale é UTF-8 na prática; locale Latin-1 no Linux decodificaria
// igual ao cp1252 em tudo menos 0x80-0x9F.
#[cfg(not(windows))]
fn local_codepage(raw: &[u8]) -> String {
    cp1252(raw)
}

#[cfg(windows)]
fn local_codepage(raw: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{GetACP, MultiByteToWideChar};
    // SAFETY: sem argumentos.
    let acp = unsafe { GetACP() };
    if acp == 65001 || acp == 1252 {
        return cp1252(raw);
    }
    let Ok(len) = i32::try_from(raw.len()) else { return cp1252(raw) };
    // SAFETY: ponteiro e tamanho vêm do mesmo slice; a primeira chamada só mede.
    let need = unsafe { MultiByteToWideChar(acp, 0, raw.as_ptr(), len, std::ptr::null_mut(), 0) };
    if need <= 0 {
        return cp1252(raw);
    }
    let mut wide = vec![0u16; need as usize];
    // SAFETY: `wide` tem exatamente `need` posições.
    let got = unsafe { MultiByteToWideChar(acp, 0, raw.as_ptr(), len, wide.as_mut_ptr(), need) };
    if got <= 0 {
        return cp1252(raw);
    }
    String::from_utf16_lossy(&wide[..got as usize])
}

/// cp1252 como o codec do Python com "replace": os cinco bytes sem caractere viram U+FFFD.
pub fn cp1252(raw: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
        '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
        '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
        '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    raw.iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}
```

- [x] **Step 5: Implementar `main.rs`**

Notas do port:
- Runtime `tokio` `current_thread`; estado num `Mutex` comum, nunca segurado através de `await`
  (equivale à `self.trava` de `cano.py:42`).
- Cada cliente tem um canal próprio e um contador; a fila de 5.000 descarta e avisa uma vez por
  cliente (`cano.py:162-174`). Trocar de cliente descarta o canal do antigo inteiro, que é o
  `while not self.saida.empty()` de `cano.py:251-256`.
- `Client._alive` é um `watch::Sender`: quando o `Client` sai do estado (troca, erro de escrita,
  EOF), leitor e escritor dele acordam e fecham o socket — o papel do `_derrubar` (`cano.py:344-359`).
- `cano_saiu` entra no mesmo canal, depois do que já estava na fila, e tem 5 s para ser escrito;
  passou disso, o cliente cai como no `settimeout(5)` de `cano.py:185-189`. Só a escrita
  confirmada acorda o fim do processo (`saiu_entregue`, `cano.py:190`).
- O rc espera até 1 s o stderr do filho terminar, para a `stderr_tail` do `cano_saiu` vir completa.
- `exit_code` devolve o mesmo número do `Popen.returncode`: sinal no Unix vira negativo, e o código
  do Windows é o DWORD sem sinal.
- SIGTERM: manda SIGTERM ao filho se ele ainda não foi colhido, apaga o socket e sai 0
  (`cano.py:403-418`). O filho é colhido pela tarefa que publica o rc, sem `await` entre colher e
  publicar, então "rc vazio" garante que o pid ainda é dele.
- Fim normal: espera o filho, depois até 60 s alguém receber o `cano_saiu`, apaga o socket e sai 0
  (`cano.py:314-323`). `std::process::exit` no fim: tarefa presa em leitura não segura o processo.
- Log `HH:MM:SS msg` em append, ou no stderr sem `--log` (`cano.py:58-63,383`); pânico de tarefa vai
  para o log (`cano.py:386-389`).

`crates/hangar-cano/src/main.rs` (substitui o provisório do Step 2):

```rust
//! Cano: dono do processo de uma sessão sem terminal (claude stream-json ou app-server do Codex),
//! separado do backend. Port de `backend/app/adapters/claude_headless/cano.py`, mesmo contrato:
//! sobe o filho, segura stdin/stdout dele e escuta num socket local; o backend conecta,
//! desconecta e reconecta, e quem chega recebe o snapshot do que está em aberto.
//!
//! Uso: hangar-cano --escuta unix:/x.sock|tcp:127.0.0.1:PORT [--token T] [--log F] [--cwd D] -- argv...

mod protocol;
mod stderr_text;

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Notify, mpsc, watch};

use protocol::Tracker;

const QUEUE_LIMIT: usize = 5000; // cano.py:43
const STDERR_TAIL: usize = 20; // cano.py:52
const TOKEN_DEADLINE: Duration = Duration::from_secs(10); // cano.py:239
const EXIT_DEADLINE: Duration = Duration::from_secs(5); // cano.py:185
const LINGER: Duration = Duration::from_secs(60); // cano.py:33
// O rc sai com a cauda do stderr completa; filho que deixou um neto segurando o stderr não trava o rc.
const STDERR_GRACE: Duration = Duration::from_secs(1);

// ── log (cano.py:58) ───────────────────────────────────────────────────────────────────────

static LOG: OnceLock<Mutex<Box<dyn Write + Send>>> = OnceLock::new();

fn init_log(path: Option<&OsStr>) -> io::Result<()> {
    let sink: Box<dyn Write + Send> = match path {
        Some(p) => Box::new(std::fs::OpenOptions::new().create(true).append(true).open(p)?),
        None => Box::new(io::stderr()),
    };
    let _ = LOG.set(Mutex::new(sink));
    Ok(())
}

fn log(msg: &str) {
    let Some(sink) = LOG.get() else { return };
    let mut w = sink.lock().unwrap_or_else(|e| e.into_inner());
    let _ = writeln!(w, "{} {msg}", chrono::Local::now().format("%H:%M:%S"));
    let _ = w.flush();
}

// ── argumentos (cano.py:372-382) ───────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
struct Args {
    listen: String,
    token: Option<String>,
    log: Option<OsString>,
    cwd: Option<OsString>,
    argv: Vec<OsString>,
}

/// Como o argparse com `REMAINDER`: o comando começa no `--` ou no primeiro posicional.
fn parse_args(raw: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let (mut listen, mut token, mut log, mut cwd) = (None, None, None, None);
    let mut argv = Vec::new();
    let mut it = raw.into_iter();
    while let Some(arg) = it.next() {
        let Some(s) = arg.to_str() else {
            argv.push(arg);
            argv.extend(it);
            break;
        };
        if s == "--" {
            argv.extend(it);
            break;
        }
        if !s.starts_with("--") {
            argv.push(arg);
            argv.extend(it);
            break;
        }
        let (name, inline) = match s.split_once('=') {
            Some((n, v)) => (n.to_owned(), Some(OsString::from(v))),
            None => (s.to_owned(), None),
        };
        let slot = match name.as_str() {
            "--escuta" => &mut listen,
            "--token" => &mut token,
            "--log" => &mut log,
            "--cwd" => &mut cwd,
            _ => return Err(format!("opção desconhecida: {name}")),
        };
        let value = match inline {
            Some(v) => v,
            None => it.next().ok_or_else(|| format!("{name} pede um valor"))?,
        };
        *slot = Some(value);
    }
    let text = |v: Option<OsString>, name: &str| -> Result<Option<String>, String> {
        v.map(|v| v.into_string().map_err(|_| format!("{name} não é UTF-8"))).transpose()
    };
    let listen = text(listen, "--escuta")?.ok_or("faltou --escuta")?;
    let token = text(token, "--token")?;
    if argv.is_empty() {
        return Err("faltou o comando do claude depois de --".to_owned());
    }
    Ok(Args { listen, token, log, cwd, argv })
}

// ── estado compartilhado (cano.py:37-54) ───────────────────────────────────────────────────

enum Out {
    Line(String),
    Exit(String),
}

struct Client {
    id: u64,
    tx: mpsc::UnboundedSender<Out>,
    queued: Arc<AtomicUsize>,
    full_warned: bool,
    // Cair do estado (troca ou saída) derruba leitor e escritor dele: os dois esperam este canal fechar.
    _alive: watch::Sender<()>,
}

#[derive(Default)]
struct State {
    tracker: Tracker,
    stderr_tail: VecDeque<String>,
    exited: Option<i64>,
    exit_delivered: bool,
    client: Option<Client>,
    next_client: u64,
}

impl State {
    /// `cano.py:162`. Só enfileira; quem escreve no socket é a tarefa do cliente. Sem cliente, a
    /// linha é descartada: o snapshot carrega o que importa.
    fn send(&mut self, line: String) {
        let Some(c) = self.client.as_mut() else { return };
        if c.queued.load(Ordering::Relaxed) >= QUEUE_LIMIT {
            if !c.full_warned {
                c.full_warned = true;
                log("fila de saída cheia: cliente não lê; descartando");
            }
            return;
        }
        c.queued.fetch_add(1, Ordering::Relaxed);
        let _ = c.tx.send(Out::Line(line));
    }
}

struct Inner {
    state: Mutex<State>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    delivered: Notify,
    token: Option<String>,
    pid: u32,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn drop_client_if(&self, id: u64) {
        let mut st = self.state();
        if st.client.as_ref().is_some_and(|c| c.id == id) {
            st.client = None;
        }
    }
}

/// `cano.py:176`. A saída vai atrás do que já estava na fila do cliente e tem 5 s para chegar;
/// sem cliente, fica para o snapshot de quem chegar.
fn send_exit(st: &mut State, inner: &Arc<Inner>) {
    let Some(c) = st.client.as_ref() else { return };
    if c.tx.send(Out::Exit(protocol::exit_line(st.exited, &st.stderr_tail))).is_err() {
        return;
    }
    let (id, inner) = (c.id, inner.clone());
    tokio::spawn(async move {
        tokio::time::sleep(EXIT_DEADLINE).await;
        let mut st = inner.state();
        if !st.exit_delivered && st.client.as_ref().is_some_and(|c| c.id == id) {
            st.client = None;
        }
    });
}

// ── filho (cano.py:67-101) ─────────────────────────────────────────────────────────────────

async fn pump_stdout(
    inner: Arc<Inner>,
    out: ChildStdout,
    stderr_task: tokio::task::JoinHandle<()>,
    mut rc: watch::Receiver<Option<i64>>,
) {
    let mut r = BufReader::new(out);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                log(&format!("leitura do stdout do claude falhou: {e}"));
                break;
            }
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let mut st = inner.state();
        st.tracker.observe_child(line);
        st.send(line.to_owned());
    }
    let code = rc.wait_for(Option::is_some).await.ok().and_then(|v| *v).unwrap_or(-1);
    let _ = tokio::time::timeout(STDERR_GRACE, stderr_task).await;
    let mut st = inner.state();
    st.exited = Some(code);
    st.tracker.turn_open = false;
    log(&format!("claude saiu rc={code}"));
    send_exit(&mut st, &inner);
}

async fn pump_stderr(inner: Arc<Inner>, err: ChildStderr) {
    let mut r = BufReader::new(err);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = stderr_text::stderr_text(&buf);
        let line = text.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let mut st = inner.state();
        if st.stderr_tail.len() == STDERR_TAIL {
            st.stderr_tail.pop_front();
        }
        st.stderr_tail.push_back(line.to_owned());
        st.send(protocol::stderr_line(line));
    }
}

/// Código de saída como o `Popen.returncode`: sinal vira negativo, e no Windows o DWORD fica sem sinal.
fn exit_code(status: std::process::ExitStatus) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return -i64::from(sig);
        }
    }
    #[cfg(windows)]
    if let Some(c) = status.code() {
        return i64::from(c as u32);
    }
    status.code().map(i64::from).unwrap_or(-1)
}

// ── cliente (cano.py:192-292) ──────────────────────────────────────────────────────────────

async fn serve_client<S>(inner: Arc<Inner>, stream: S)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (r, w) = tokio::io::split(stream);
    let mut r = BufReader::new(r);
    let mut buf = Vec::new();
    if let Some(token) = &inner.token {
        // TCP em loopback: qualquer processo local alcança a porta; o token faz o cano ser só do backend.
        let read = tokio::time::timeout(TOKEN_DEADLINE, r.read_until(b'\n', &mut buf)).await;
        if !matches!(read, Ok(Ok(_))) || String::from_utf8_lossy(&buf).trim() != token {
            return;
        }
    }
    let (tx, rx) = mpsc::unbounded_channel();
    let (alive_tx, alive_rx) = watch::channel(());
    let queued = Arc::new(AtomicUsize::new(0));
    let (id, snapshot) = {
        let mut st = inner.state();
        st.next_client += 1;
        let id = st.next_client;
        // Um cliente por vez: o antigo cai aqui, e o que sobrou na fila dele já está no snapshot.
        st.client = Some(Client { id, tx, queued: queued.clone(), full_warned: false, _alive: alive_tx });
        let snapshot = st.tracker.snapshot(inner.pid, &st.stderr_tail, st.exited);
        if st.exited.is_some() {
            // Já saiu: quem chegou leva o rc como se acontecesse agora, e aí o cano pode morrer.
            send_exit(&mut st, &inner);
        }
        (id, snapshot)
    };
    log("cliente conectado");
    tokio::spawn(write_client(inner.clone(), id, w, snapshot, rx, queued, alive_rx.clone()));
    read_client(&inner, r, buf, alive_rx).await;
    inner.drop_client_if(id);
    log("cliente saiu");
}

async fn write_client<W: AsyncWrite + Unpin>(
    inner: Arc<Inner>,
    id: u64,
    mut w: W,
    snapshot: String,
    mut rx: mpsc::UnboundedReceiver<Out>,
    queued: Arc<AtomicUsize>,
    mut alive: watch::Receiver<()>,
) {
    let work = async {
        write_line(&mut w, &snapshot).await?;
        while let Some(out) = rx.recv().await {
            match out {
                Out::Line(line) => {
                    queued.fetch_sub(1, Ordering::Relaxed);
                    write_line(&mut w, &line).await?;
                }
                Out::Exit(line) => {
                    write_line(&mut w, &line).await?;
                    inner.state().exit_delivered = true;
                    inner.delivered.notify_one();
                }
            }
        }
        Ok::<(), io::Error>(())
    };
    let failed = tokio::select! {
        r = work => r.is_err(),
        _ = alive.changed() => false,
    };
    if failed {
        inner.drop_client_if(id);
    }
    let _ = w.shutdown().await;
}

async fn write_line<W: AsyncWrite + Unpin>(w: &mut W, line: &str) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    w.write_all(&bytes).await
}

async fn read_client<R: AsyncRead + Unpin>(
    inner: &Inner,
    mut r: BufReader<R>,
    mut buf: Vec<u8>,
    mut alive: watch::Receiver<()>,
) {
    loop {
        buf.clear();
        let read = tokio::select! {
            r = r.read_until(b'\n', &mut buf) => r,
            _ = alive.changed() => return,
        };
        if !matches!(read, Ok(n) if n > 0) {
            return;
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let exited = {
            let mut st = inner.state();
            st.tracker.observe_client(line);
            st.exited.is_some()
        };
        if exited {
            continue;
        }
        let mut stdin = inner.stdin.lock().await;
        let Some(pipe) = stdin.as_mut() else { continue };
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        let wrote = match pipe.write_all(&bytes).await {
            Ok(()) => pipe.flush().await,
            Err(e) => Err(e),
        };
        if let Err(e) = wrote {
            log(&format!("stdin do claude falhou: {e}"));
        }
    }
}

// ── escuta (cano.py:294-312) ───────────────────────────────────────────────────────────────

enum Listener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    Tcp(tokio::net::TcpListener),
}

fn listen(spec: &str) -> io::Result<Listener> {
    if let Some(path) = spec.strip_prefix("unix:") {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            let sock = tokio::net::UnixSocket::new_stream()?;
            sock.bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            return Ok(Listener::Unix(sock.listen(2)?));
        }
        #[cfg(not(unix))]
        return Err(io::Error::new(io::ErrorKind::Unsupported, format!("socket unix indisponível: {path}")));
    }
    // Como o cano.py: o que não é unix: é tcp:host:porta.
    let rest = spec.get(4..).unwrap_or("");
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "endereço sem porta"))?;
    let port: u16 = port.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "porta inválida"))?;
    let addr = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "host sem IPv4"))?;
    let sock = tokio::net::TcpSocket::new_v4()?;
    sock.set_reuseaddr(true)?;
    sock.bind(addr)?;
    Ok(Listener::Tcp(sock.listen(2)?))
}

/// Uma tarefa por cliente: em série, quem chega só recebe o snapshot quando o ligado sai, e o
/// backend que não recebe snapshot a tempo mata o cano como mudo.
async fn accept_loop(listener: Listener, inner: Arc<Inner>) {
    loop {
        let accepted = match &listener {
            #[cfg(unix)]
            Listener::Unix(l) => l.accept().await.map(|(s, _)| {
                tokio::spawn(serve_client(inner.clone(), s));
            }),
            Listener::Tcp(l) => l.accept().await.map(|(s, _)| {
                tokio::spawn(serve_client(inner.clone(), s));
            }),
        };
        if let Err(e) = accepted {
            log(&format!("accept falhou, parei de escutar: {e}"));
            return;
        }
    }
}

fn remove_socket(spec: &str) {
    if let Some(path) = spec.strip_prefix("unix:") {
        let _ = std::fs::remove_file(path);
    }
}

// ── ciclo de vida (cano.py:314-420) ────────────────────────────────────────────────────────

/// SIGTERM do backend (encerrar sessão): derruba o filho e não deixa socket velho.
#[cfg(unix)]
fn watch_sigterm(inner: Arc<Inner>, rc: watch::Receiver<Option<i64>>, spec: String) {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sig = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            log(&format!("sem tratador de SIGTERM: {e}"));
            return;
        }
    };
    tokio::spawn(async move {
        sig.recv().await;
        log(&format!("sinal {}: encerrando", libc::SIGTERM));
        // rc ainda vazio = filho não foi colhido, então o pid ainda é dele.
        if rc.borrow().is_none() {
            // SAFETY: kill(2) com pid e sinal válidos.
            unsafe { libc::kill(inner.pid as libc::pid_t, libc::SIGTERM) };
        }
        remove_socket(&spec);
        std::process::exit(0);
    });
}

async fn run(args: Args) -> i32 {
    // ANTES de subir o filho: escuta que falha (caminho unix > 107 bytes, porta ocupada) derruba o
    // cano com log e código de saída, nunca deixa um filho órfão sem porta.
    let listener = match listen(&args.listen) {
        Ok(l) => l,
        Err(e) => {
            log(&format!("não consegui escutar em {}: {e}", args.listen));
            return 1;
        }
    };
    // Mesmo grupo de processos que o cano: matar o grupo do cano mata os dois.
    let mut cmd = Command::new(&args.argv[0]);
    cmd.args(&args.argv[1..]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(cwd) = &args.cwd {
        cmd.current_dir(cwd);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log(&format!("claude não subiu: {e}"));
            drop(listener);
            remove_socket(&args.listen);
            return 1;
        }
    };
    let pid = child.id().unwrap_or(0);
    let stdout = child.stdout.take().expect("stdout em pipe");
    let stderr = child.stderr.take().expect("stderr em pipe");
    let inner = Arc::new(Inner {
        state: Mutex::new(State::default()),
        stdin: tokio::sync::Mutex::new(child.stdin.take()),
        delivered: Notify::new(),
        token: args.token.clone(),
        pid,
    });
    let (rc_tx, rc_rx) = watch::channel(None);
    tokio::spawn(async move {
        let code = match child.wait().await {
            Ok(status) => exit_code(status),
            Err(e) => {
                log(&format!("espera do claude falhou: {e}"));
                -1
            }
        };
        let _ = rc_tx.send(Some(code));
    });
    let stderr_task = tokio::spawn(pump_stderr(inner.clone(), stderr));
    tokio::spawn(pump_stdout(inner.clone(), stdout, stderr_task, rc_rx.clone()));
    log(&format!("claude pid={pid}"));
    tokio::spawn(accept_loop(listener, inner.clone()));
    #[cfg(unix)]
    watch_sigterm(inner.clone(), rc_rx.clone(), args.listen.clone());

    // Vive enquanto o filho viver; depois espera um cliente levar o rc (ou desiste).
    let mut rc = rc_rx;
    let _ = rc.wait_for(Option::is_some).await;
    let _ = tokio::time::timeout(LINGER, inner.delivered.notified()).await;
    remove_socket(&args.listen);
    0
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("cano: {msg}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = init_log(args.log.as_deref()) {
        eprintln!("cano: não abri o log: {e}");
        return ExitCode::from(1);
    }
    // O stderr do cano é DEVNULL: sem isto, pânico numa tarefa sumia sem rastro.
    std::panic::set_hook(Box::new(|info| log(&format!("tarefa estourou: {info}"))));
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            log(&format!("runtime não subiu: {e}"));
            return ExitCode::from(1);
        }
    };
    let code = rt.block_on(run(args));
    // Sai na hora: tarefa presa em leitura de socket não segura o processo.
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Result<Args, String> {
        parse_args(v.iter().map(OsString::from))
    }

    #[test]
    fn parses_the_backend_command_line() {
        let a = args(&["--escuta", "unix:/x.sock", "--log", "/l", "--cwd", "/c", "--token", "t", "--", "claude", "--", "x"])
            .unwrap();
        assert_eq!(a.listen, "unix:/x.sock");
        assert_eq!(a.token.as_deref(), Some("t"));
        assert_eq!(a.log, Some(OsString::from("/l")));
        assert_eq!(a.cwd, Some(OsString::from("/c")));
        assert_eq!(a.argv, ["claude", "--", "x"].map(OsString::from));
    }

    #[test]
    fn remainder_starts_at_first_positional_and_accepts_equals() {
        let a = args(&["--escuta=tcp:127.0.0.1:1", "claude", "--log", "z"]).unwrap();
        assert_eq!(a.listen, "tcp:127.0.0.1:1");
        assert_eq!(a.log, None);
        assert_eq!(a.argv, ["claude", "--log", "z"].map(OsString::from));
    }

    #[test]
    fn missing_command_or_listen_is_an_error() {
        assert!(args(&["--escuta", "unix:/x"]).is_err());
        assert!(args(&["--escuta", "unix:/x", "--"]).is_err());
        assert!(args(&["--", "claude"]).is_err());
        assert!(args(&["--escuta", "unix:/x", "--outra", "v", "--", "claude"]).is_err());
    }
}
```

- [x] **Step 6: Rodar e ver passar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-cano && cargo build --release -p hangar-cano`
Expected: PASS (18 testes) e `crates/target/release/hangar-cano` gerado.

Se o alvo estiver instalado (`rustup target add x86_64-pc-windows-gnu`), conferir também o caminho do Windows:
Run: `cd crates && cargo check -p hangar-cano --all-targets --target x86_64-pc-windows-gnu`
Expected: sem erro nem aviso.

- [x] **Step 7: Conferir o ciclo de vida à mão** (verificação manual)

Run:
```bash
./crates/target/release/hangar-cano --escuta unix:/tmp/cano-prova.sock --log /tmp/cano-prova.log -- sleep 3 &
sleep 0.5; ps -o pid,rss,nlwp,cmd -C hangar-cano; ls -l /tmp/cano-prova.sock
wait; cat /tmp/cano-prova.log; ls /tmp/cano-prova.sock
```
Expected: socket `srw-------`; uma thread; o processo vive os 3 s do `sleep` mais 60 s sem cliente
e sai; log com `claude pid=`, `claude saiu rc=0`; o `ls` final diz que o socket não existe.
Anotar o RSS medido para a comparação com os 22 MB do `cano.py` (spec, "Linha de base").

- [x] **Step 8: Commit**

```bash
git add crates/Cargo.toml crates/Cargo.lock crates/hangar-cano/Cargo.toml crates/hangar-cano/src/main.rs crates/hangar-cano/src/protocol.rs crates/hangar-cano/src/stderr_text.rs
git commit -m "feat(cano): port the headless session pipe to Rust as hangar-cano"
```

---

### Task 5: Backend escolhe o binário e a suíte do cano roda nas duas implementações

**Files:**
- Create: `backend/app/rust_bins.py`
- Test: `backend/tests/test_rust_bins.py`
- Modify: `backend/app/adapters/claude_headless/adapter.py:38` (import) e `:2154-2155` (`subir_cano_processo`)
- Modify: `backend/tests/test_claude_headless.py:4-10` (imports), `:1396-1440` e `:1443-1473`
- Modify: `backend/tests/test_claude_headless_cano.py` (arquivo inteiro)

**Interfaces:**
- Consumes: o binário `hangar-cano` da Task 4 (só para os casos Rust da suíte; sem ele, pulam).
- Produces:
  - `rust_bins.find_bin(name: str, env_var: str) -> Path | None` — ordem: `env_var`;
    `<repo>/crates/target/release/<name>[.exe]`; `~/.hangar/bin/<name>[.exe]`; só devolve arquivo
    que existe e é executável. Variável preenchida com caminho que não executa devolve `None` com
    aviso no log, sem cair para os outros lugares (ver Notas). A frente que sobe o `hangar-server`
    usa a mesma função com `CP_RUST_SERVER_BIN`.
  - `subir_cano_processo` com a mesma assinatura e o mesmo retorno de hoje (`adapter.py:2144-2176`);
    só o lançador muda: `[hangar-cano]` quando achado, senão `[sys.executable, cano.py]`.

O que fica igual, com a prova no código de hoje:
- `systemd-run --scope` na frente do comando (conferido 2026-10-02: `adapter.py:2164-2167`).
- Grupo de processos próprio: `start_new_session` no Linux/mac e `creationflags` no Windows
  (conferido 2026-10-02: `adapter.py:2159-2163`).
- `HANGAR_CANO_KEY` no ambiente: posto pelos chamadores no `env` que chega intacto ao
  `create_subprocess_exec` (conferido 2026-10-02: `adapter.py:901`, `codex/sem_terminal.py:108`,
  `adapter.py:2168-2169`).
- A chave da sessão no cmdline, de que `registry.cwd_atual` depende (`registry.py:176-177` procura
  `chave[:16]` em `/proc/<pid>/cmdline`): ela entra pelo `--log`, que os dois chamadores nomeiam
  `cano-<chave[:16]>.log` (conferido 2026-10-02: `adapter.py:912` e `codex/sem_terminal.py:118`,
  passados em `adapter.py:2155` como `"--log", str(log)`); com socket unix, também pelo caminho da
  escuta (`adapter.py:2185`). O `--log` continua no comando nas duas implementações, e o teste do
  Step 5 afirma a chave no argv.
- O Codex sem terminal passa pela mesma função (conferido 2026-10-02: `codex/sem_terminal.py:121`),
  então ganha o binário sem mudança própria.
- A suíte nunca conecta nos canos reais: `conftest.py:72-91` troca `sessions._dir` e desliga a
  varredura de órfãos para a sessão de testes inteira; os testes do cano usam `tmp_path`.

- [x] **Step 1: Escrever os testes de `rust_bins`**

```python
# backend/tests/test_rust_bins.py
"""Busca dos binários Rust: a variável vence e não cai para outro lugar; sem ela, o build do
checkout vem antes do baixado; arquivo que não executa não conta."""
import os
import stat
from pathlib import Path

import pytest

from app import rust_bins

EXE = "hangar-cano" + (".exe" if os.name == "nt" else "")


def _executavel(p: Path) -> Path:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("#!/bin/sh\n", encoding="utf-8")
    p.chmod(p.stat().st_mode | stat.S_IXUSR)
    return p


@pytest.fixture
def locais(tmp_path, monkeypatch):
    repo, home = tmp_path / "repo", tmp_path / "home"
    monkeypatch.setattr(rust_bins, "_REPO", repo)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("USERPROFILE", str(home))
    monkeypatch.delenv("CP_RUST_CANO_BIN", raising=False)
    return repo / "crates" / "target" / "release" / EXE, home / ".hangar" / "bin" / EXE


def test_sem_nada_devolve_none(locais):
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") is None


def test_build_do_checkout_vem_antes_do_baixado(locais):
    build, baixado = locais
    _executavel(baixado)
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == baixado
    _executavel(build)
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == build


def test_variavel_vence(locais, tmp_path, monkeypatch):
    build, _ = locais
    _executavel(build)
    escolhido = _executavel(tmp_path / "outro" / EXE)
    monkeypatch.setenv("CP_RUST_CANO_BIN", str(escolhido))
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == escolhido


def test_variavel_com_caminho_errado_nao_cai_para_outro_binario(locais, tmp_path, monkeypatch, caplog):
    build, _ = locais
    _executavel(build)
    monkeypatch.setenv("CP_RUST_CANO_BIN", str(tmp_path / "nao-existe"))
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") is None
    assert "CP_RUST_CANO_BIN" in caplog.text


@pytest.mark.skipif(os.name == "nt", reason="no Windows todo arquivo conta como executável")
def test_arquivo_sem_permissao_de_execucao_nao_conta(locais):
    build, _ = locais
    build.parent.mkdir(parents=True)
    build.write_text("x", encoding="utf-8")
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") is None
```

- [x] **Step 2: Rodar e ver falhar** (quando autorizado)

Run: `cd backend && uv run pytest tests/test_rust_bins.py -v`
Expected: FAIL com `ImportError: cannot import name 'rust_bins' from 'app'`

- [x] **Step 3: Implementar `rust_bins.py`**

```python
# backend/app/rust_bins.py
"""Onde estão os binários Rust do Hangar (`hangar-server`, `hangar-cano`).

Ordem: a variável de ambiente (escolha explícita, vence e não cai para as outras), o build
do checkout (`crates/target/release`, desenvolvimento) e o baixado em `~/.hangar/bin`.
"""
from __future__ import annotations

import logging
import os
from pathlib import Path

_log = logging.getLogger(__name__)
_REPO = Path(__file__).resolve().parents[2]


def _executavel(p: Path) -> bool:
    return p.is_file() and os.access(p, os.X_OK)


def find_bin(name: str, env_var: str) -> Path | None:
    exe = name + (".exe" if os.name == "nt" else "")
    escolhido = os.environ.get(env_var, "").strip()
    if escolhido:
        p = Path(escolhido).expanduser()
        if _executavel(p):
            return p
        # Caminho errado não pode virar outro binário calado: a escolha some e aparece no log.
        _log.warning("rust_bins: %s=%s não é executável; seguindo sem %s", env_var, escolhido, name)
        return None
    for p in (_REPO / "crates" / "target" / "release" / exe, Path.home() / ".hangar" / "bin" / exe):
        if _executavel(p):
            return p
    return None
```

- [x] **Step 4: Rodar e ver passar** (quando autorizado)

Run: `cd backend && uv run pytest tests/test_rust_bins.py -v`
Expected: PASS (5 testes; 1 pulado no Windows)

- [x] **Step 5: Testes do adapter aceitam os dois lançadores**

Em `backend/tests/test_claude_headless.py`, junto dos imports do topo (linhas 4-10):

```python
import sys
from pathlib import Path
```

Antes de `test_processo_herda_chave_e_nao_o_pane_do_operador` (linha 1396), a fixture:

```python
_CANO_RUST_FALSO = Path("/opt/hangar/bin/hangar-cano")


@pytest.fixture(params=["cano.py", "hangar-cano"])
def lancador_cano(request, monkeypatch) -> list[str]:
    """O backend sobe o hangar-cano quando acha o binário e o cano.py quando não acha; o resto
    do comando é o mesmo nos dois."""
    if request.param == "cano.py":
        monkeypatch.setattr(A.rust_bins, "find_bin", lambda name, env_var: None)
        return [sys.executable, str(A._CANO_PY)]
    monkeypatch.setattr(A.rust_bins, "find_bin", lambda name, env_var: _CANO_RUST_FALSO)
    return [str(_CANO_RUST_FALSO)]
```

Em `test_processo_herda_chave_e_nao_o_pane_do_operador`, a assinatura ganha a fixture e o bloco
das linhas 1434-1438 vira:

```python
def test_processo_herda_chave_e_nao_o_pane_do_operador(sidecar, monkeypatch, lancador_cano):
```

```python
    # O processo que nasce é o cano, com o comando do claude depois do `--`; o sidecar guarda
    # onde ele escuta, pra o próximo backend religar.
    argv = list(visto["argv"])
    ultimo = len(argv) - 1 - argv[::-1].index("--")   # o escopo do systemd também tem um `--`
    assert argv[ultimo + 1] == "/usr/bin/claude"       # caminho resolvido
    inicio = argv.index(lancador_cano[-1]) - (len(lancador_cano) - 1)
    assert argv[inicio:inicio + len(lancador_cano)] == lancador_cano
    # A chave no cmdline é o que o registry.cwd_atual confere em /proc/<pid>/cmdline.
    assert S.load("s1")["key"][:16] in " ".join(argv)
```

Em `test_sessao_com_motor_chama_o_hangar_engine_pelo_caminho_resolvido` (linha 1443), a assinatura
e a linha 1471:

```python
def test_sessao_com_motor_chama_o_hangar_engine_pelo_caminho_resolvido(sidecar, monkeypatch, lancador_cano):
```

```python
    depois_do_cano = argv[argv.index("--", argv.index(lancador_cano[-1])) + 1:]
```

- [x] **Step 6: Rodar e ver falhar** (quando autorizado)

Run: `cd backend && uv run pytest tests/test_claude_headless.py -k "herda_chave or motor_chama" -v`
Expected: FAIL nos 4 casos com `AttributeError: module 'app.adapters.claude_headless.adapter' has no attribute 'rust_bins'`

- [x] **Step 7: Escolher o binário em `subir_cano_processo`**

`backend/app/adapters/claude_headless/adapter.py:38`:

```python
from app import atomico, cotas, log_paths, model_args, pensamento, runtime_config, rust_bins
```

`backend/app/adapters/claude_headless/adapter.py:2154-2155`, de:

```python
    escuta, token = _escuta_nova(key, log.parent)
    cmd = [sys.executable, str(_CANO_PY), "--escuta", escuta, "--log", str(log), "--cwd", cwd]
```

para:

```python
    escuta, token = _escuta_nova(key, log.parent)
    # Mesmo contrato do cano.py num processo nativo; sem o binário, o cano.py segue valendo.
    cano_bin = rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN")
    lancador = [str(cano_bin)] if cano_bin else [sys.executable, str(_CANO_PY)]
    # A chave da sessão chega ao cmdline pelo `--log` (cano-<chave>.log): é por ela que
    # registry.cwd_atual reconhece o processo.
    cmd = [*lancador, "--escuta", escuta, "--log", str(log), "--cwd", cwd]
```

O resto da função fica como está (token, `creationflags`/`start_new_session`, `_scope_prefix`,
`create_subprocess_exec`, ceifador).

- [x] **Step 8: Parametrizar `test_claude_headless_cano.py`**

Arquivo inteiro. O que muda: a fixture `cano_cmd` (os testes que sobem cano rodam contra `cano.py`
e contra `hangar-cano`; o caso Rust pula com o motivo quando o binário não está compilado);
`cano`, `test_pedido_jsonrpc_...`, `test_token_errado_...` e `test_cliente_novo_...` usam o
lançador dela; três testes novos cobrem o que o port tem de manter e a suíte não cobria — campos
e `versao` do snapshot iguais ao `cano_mod.VERSAO`, códigos de saída 1 e 2, e SIGTERM. O teste de
codepage continua importando `_texto_do_stderr` do `cano.py` (o equivalente Rust é o da Task 4) e o
de sidecars reais não sobe cano; os dois não são parametrizados.

```python
# backend/tests/test_claude_headless_cano.py
"""Cano da sessão sem terminal: o processo sobrevive ao cliente e o snapshot diz o que está em
aberto. O `claude` é um script falso que fala stream-json: responde initialize, e a cada prompt
pede uma permissão e só fecha o turno quando ela é respondida.

Cada teste que sobe um cano roda duas vezes: contra o `cano.py` e contra o `hangar-cano` (Rust).
"""
import json
import os
import signal
import socket
import subprocess
import sys
import time
import uuid
from pathlib import Path

import pytest

from app import rust_bins
from app.adapters.claude_headless import cano as cano_mod

CANO = Path(__file__).resolve().parents[1] / "app" / "adapters" / "claude_headless" / "cano.py"

_CLAUDE_FALSO = r'''
import json, sys
def out(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for linha in sys.stdin:
    ev = json.loads(linha)
    t = ev.get("type")
    if t == "control_request":
        sub = ev["request"]["subtype"]
        if sub == "initialize":
            out({"type": "system", "subtype": "init", "session_id": "sid-1", "model": "haiku", "permissionMode": "default"})
        elif sub == "emit_peer":
            out({"type": "command_lifecycle", "command_uuid": "peer-1", "state": "started"})
        elif sub == "finish_peer":
            out({"type": "result", "subtype": "success", "usage": {"input_tokens": 1}})
        out({"type": "control_response", "response": {"subtype": "success", "request_id": ev["request_id"], "response": {}}})
    elif t == "user":
        if ev["message"]["content"][0]["text"] == "sair":
            sys.stderr.write("tchau\n"); sys.stderr.flush(); sys.exit(3)
        out({"type": "control_request", "request_id": "perm-1",
             "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}}})
    elif t == "control_response":
        out({"type": "assistant", "message": {"content": [{"type": "text", "text": "feito"}]}})
        out({"type": "result", "subtype": "success", "usage": {"input_tokens": 1}})
sys.stderr.write("tchau\n")
'''


@pytest.fixture(params=["cano.py", "hangar-cano"])
def cano_cmd(request) -> list[str]:
    """Lançador do cano. Os mesmos testes provam as duas implementações do mesmo contrato."""
    if request.param == "cano.py":
        return [sys.executable, str(CANO)]
    exe = rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN")
    if exe is None:
        pytest.skip("hangar-cano não compilado: rode `cargo build --release -p hangar-cano` em crates/ "
                    "ou aponte CP_RUST_CANO_BIN para o binário")
    return [str(exe)]


@pytest.fixture
def cano(tmp_path, cano_cmd):
    if os.name == "nt":
        pytest.skip("socket unix")
    falso = tmp_path / "claude_falso.py"
    falso.write_text(_CLAUDE_FALSO, encoding="utf-8")
    sock = tmp_path / "c.sock"
    log = tmp_path / "cano.log"
    p = subprocess.Popen([*cano_cmd, "--escuta", f"unix:{sock}", "--log", str(log),
                          "--cwd", str(tmp_path), "--", sys.executable, str(falso)],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(100):
        if sock.exists():
            break
        time.sleep(0.05)
    assert sock.exists(), log.read_text() if log.exists() else "sem log"
    yield sock, p, log
    if p.poll() is None:
        p.kill()
    p.wait()


class _Cliente:
    def __init__(self, sock: Path):
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.connect(str(sock))
        self.s.settimeout(5)
        self.arq = self.s.makefile("rb")

    def manda(self, obj) -> None:
        self.s.sendall((json.dumps(obj) + "\n").encode())

    def le(self) -> dict:
        return json.loads(self.arq.readline())

    def le_ate(self, tipo: str) -> dict:
        while True:
            ev = self.le()
            if ev.get("type") == tipo:
                return ev

    def fecha(self) -> None:
        self.arq.close()   # o makefile segura o socket: só o close dele entrega o EOF ao cano
        self.s.close()


def test_snapshot_tem_os_campos_e_a_versao_do_cano_py(cano):
    # O adapter compara `versao` com `cano_mod.VERSAO` para decidir se reabre: as duas
    # implementações precisam falar a mesma.
    sock, proc, log = cano
    a = _Cliente(sock)
    snap = a.le()
    assert set(snap) == {"type", "versao", "pid", "init", "aberto", "pendentes", "ultimo_result",
                         "rate_limit", "stderr_tail", "saiu"}
    assert snap["versao"] == cano_mod.VERSAO and isinstance(snap["pid"], int)
    assert snap["pendentes"] == [] and snap["stderr_tail"] == [] and snap["saiu"] is None
    a.fecha()


def test_snapshot_reconstroi_turno_aberto_e_permissao_pendente(cano):
    sock, proc, log = cano
    a = _Cliente(sock)
    snap = a.le()
    assert snap["type"] == "cano_snapshot" and snap["init"] is None and not snap["aberto"]
    a.manda({"type": "control_request", "request_id": "r1", "request": {"subtype": "initialize"}})
    assert a.le_ate("system")["subtype"] == "init"
    a.le_ate("control_response")
    a.manda({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "oi"}]}})
    pedido = a.le_ate("control_request")
    assert pedido["request_id"] == "perm-1"
    # O backend "cai" com a permissão pendente. O claude (falso) continua vivo no cano.
    a.fecha()
    time.sleep(0.2)
    assert proc.poll() is None
    b = _Cliente(sock)
    snap = b.le()
    assert json.loads(snap["init"])["subtype"] == "init"
    assert snap["aberto"] is True
    assert [json.loads(p)["request_id"] for p in snap["pendentes"]] == ["perm-1"]
    assert snap["saiu"] is None
    # O backend novo responde a permissão pendente e o turno fecha normalmente.
    b.manda({"type": "control_response", "response": {"subtype": "success", "request_id": "perm-1",
                                                       "response": {"behavior": "allow"}}})
    assert b.le_ate("result")["subtype"] == "success"
    b.fecha()
    c = _Cliente(sock)
    snap = c.le()
    assert snap["aberto"] is False and snap["pendentes"] == []
    assert json.loads(snap["ultimo_result"])["type"] == "result"
    c.fecha()


def test_snapshot_preserva_turno_iniciado_por_mensagem_de_outra_sessao(cano):
    sock, proc, log = cano
    a = _Cliente(sock)
    a.le()
    a.manda({"type": "control_request", "request_id": "peer", "request": {"subtype": "emit_peer"}})
    assert a.le_ate("command_lifecycle")["state"] == "started"
    a.le_ate("control_response")
    a.fecha()
    time.sleep(0.2)

    b = _Cliente(sock)
    assert b.le()["aberto"] is True
    b.manda({"type": "control_request", "request_id": "fim", "request": {"subtype": "finish_peer"}})
    assert b.le_ate("result")["subtype"] == "success"
    b.le_ate("control_response")
    b.fecha()
    time.sleep(0.2)

    c = _Cliente(sock)
    assert c.le()["aberto"] is False
    c.fecha()


def test_saida_do_claude_chega_ao_cliente_ligado_com_stderr(cano):
    sock, proc, log = cano
    a = _Cliente(sock)
    a.le()
    a.manda({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "sair"}]}})
    assert a.le_ate("cano_stderr")["linha"] == "tchau"
    saiu = a.le_ate("cano_saiu")
    assert saiu["rc"] == 3 and saiu["stderr_tail"] == ["tchau"]
    a.fecha()
    proc.wait(timeout=5)     # entregou o rc: sai sozinho, sem esperar o teto, e limpa o socket
    assert not sock.exists()


def test_saida_do_claude_sem_cliente_fica_no_snapshot(cano):
    sock, proc, log = cano
    a = _Cliente(sock)
    a.le()
    a.manda({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "sair"}]}})
    a.fecha()                # o backend caiu antes de ver a saída
    time.sleep(0.5)
    assert proc.poll() is None   # o cano espera alguém buscar o rc
    b = _Cliente(sock)
    snap = b.le()
    assert snap["saiu"] == 3 and snap["stderr_tail"] == ["tchau"]
    assert b.le()["type"] == "cano_saiu"   # e ainda manda o evento pra quem chegou depois
    b.fecha()
    proc.wait(timeout=5)


def test_sigterm_encerra_o_filho_e_limpa_o_socket(cano):
    if not Path("/proc").exists():
        pytest.skip("confere o filho pelo /proc")
    sock, proc, log = cano
    a = _Cliente(sock)
    filho = a.le()["pid"]
    a.fecha()
    proc.send_signal(signal.SIGTERM)
    assert proc.wait(timeout=5) == 0
    assert not sock.exists()
    for _ in range(100):
        if not _vivo(filho):
            break
        time.sleep(0.05)
    assert not _vivo(filho)


def test_codigos_de_saida(cano_cmd, tmp_path):
    if os.name == "nt":
        pytest.skip("socket unix")
    sem_comando = subprocess.run([*cano_cmd, "--escuta", f"unix:{tmp_path / 'a.sock'}"],
                                 capture_output=True, timeout=10)
    assert sem_comando.returncode == 2
    log = tmp_path / "cano.log"
    longo = f"unix:{tmp_path / ('x' * 120 + '.sock')}"     # passa do limite do kernel para socket unix
    r = subprocess.run([*cano_cmd, "--escuta", longo, "--log", str(log), "--", sys.executable, "-c", "pass"],
                       capture_output=True, timeout=10)
    assert r.returncode == 1 and "não consegui escutar" in log.read_text(encoding="utf-8")
    r = subprocess.run([*cano_cmd, "--escuta", f"unix:{tmp_path / 'b.sock'}", "--log", str(log),
                        "--", str(tmp_path / "nao-existe")], capture_output=True, timeout=10)
    assert r.returncode == 1 and "claude não subiu" in log.read_text(encoding="utf-8")


_APP_SERVER_FALSO = r'''
import json, sys
def out(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for linha in sys.stdin:
    ev = json.loads(linha)
    if ev.get("method") == "turn/start":
        out({"jsonrpc": "2.0", "id": ev["id"], "result": {"turn": {"id": "t1"}}})
        out({"jsonrpc": "2.0", "id": 0, "method": "item/commandExecution/requestApproval",
             "params": {"threadId": "th", "itemId": "i1", "command": "touch x"}})
        out({"jsonrpc": "2.0", "id": 1, "method": "item/fileChange/requestApproval",
             "params": {"threadId": "th", "itemId": "i2"}})
    elif ev.get("method") == "turn/interrupt":
        out({"jsonrpc": "2.0", "id": ev["id"], "result": {}})
        out({"jsonrpc": "2.0", "method": "turn/completed", "params": {"threadId": "th"}})
    elif "method" not in ev and ev.get("id") is not None:
        out({"jsonrpc": "2.0", "method": "serverRequest/resolved", "params": {"threadId": "th", "requestId": ev["id"]}})
        if ev["id"] == 1:
            out({"jsonrpc": "2.0", "method": "turn/completed", "params": {"threadId": "th"}})
'''


def test_pedido_jsonrpc_do_servidor_fica_pendente_no_snapshot(tmp_path, cano_cmd):
    if os.name == "nt":
        pytest.skip("socket unix")
    falso = tmp_path / "app_server_falso.py"
    falso.write_text(_APP_SERVER_FALSO, encoding="utf-8")
    sock = tmp_path / "c.sock"
    p = subprocess.Popen([*cano_cmd, "--escuta", f"unix:{sock}", "--log", str(tmp_path / "cano.log"),
                          "--cwd", str(tmp_path), "--", sys.executable, str(falso)],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            if sock.exists():
                break
            time.sleep(0.05)
        a = _Cliente(sock)
        a.le()
        a.manda({"jsonrpc": "2.0", "id": 7, "method": "turn/start", "params": {}})
        assert a.le()["id"] == 7
        assert a.le()["method"] == "item/commandExecution/requestApproval"
        assert a.le()["method"] == "item/fileChange/requestApproval"
        a.manda({"jsonrpc": "2.0", "id": 0, "result": {"decision": "accept"}})    # respondeu um só
        assert a.le()["method"] == "serverRequest/resolved"
        a.fecha()
        time.sleep(0.2)
        b = _Cliente(sock)
        snap = b.le()
        assert [json.loads(x)["id"] for x in snap["pendentes"]] == [1]
        # Turno fechado sem resolver o pedido (interrupção) leva o pendente junto.
        b.manda({"jsonrpc": "2.0", "id": 8, "method": "turn/interrupt", "params": {"threadId": "th"}})
        assert b.le()["id"] == 8
        assert b.le()["method"] == "turn/completed"
        b.fecha()
        time.sleep(0.2)
        c = _Cliente(sock)
        assert c.le()["pendentes"] == []
        c.fecha()
    finally:
        if p.poll() is None:
            p.kill()
        p.wait()


def test_token_errado_e_recusado(tmp_path, cano_cmd):
    if os.name == "nt":
        pytest.skip("socket unix")
    falso = tmp_path / "claude_falso.py"
    falso.write_text(_CLAUDE_FALSO, encoding="utf-8")
    porta = _porta_livre()
    token = uuid.uuid4().hex
    p = subprocess.Popen([*cano_cmd, "--escuta", f"tcp:127.0.0.1:{porta}", "--token", token,
                          "--cwd", str(tmp_path), "--", sys.executable, str(falso)],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        s = _conectar_tcp(porta)
        s.sendall(b"errado\n")
        assert s.makefile("rb").readline() == b""     # fechado sem snapshot
        s = _conectar_tcp(porta)
        s.sendall((token + "\n").encode())
        assert json.loads(s.makefile("rb").readline())["type"] == "cano_snapshot"
        s.close()
    finally:
        p.kill()
        p.wait()


def test_cliente_novo_substitui_o_ligado_em_tcp(tmp_path, cano_cmd):
    # Roda também no Windows (TCP + token). Com o accept em série, o segundo cliente não recebia
    # snapshot enquanto o primeiro seguia ligado — e quem conecta sem snapshot mata o cano.
    falso = tmp_path / "claude_falso.py"
    falso.write_text(_CLAUDE_FALSO, encoding="utf-8")
    porta, token = _porta_livre(), uuid.uuid4().hex
    p = subprocess.Popen([*cano_cmd, "--escuta", f"tcp:127.0.0.1:{porta}", "--token", token,
                          "--cwd", str(tmp_path), "--", sys.executable, str(falso)],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        a = _conectar_tcp(porta)
        a.sendall((token + "\n").encode())
        arq_a = a.makefile("rb")
        assert json.loads(arq_a.readline())["type"] == "cano_snapshot"
        b = _conectar_tcp(porta)
        b.settimeout(3)
        b.sendall((token + "\n").encode())
        arq_b = b.makefile("rb")
        assert json.loads(arq_b.readline())["type"] == "cano_snapshot"
        try:
            assert arq_a.readline() == b""        # o antigo foi desligado
        except OSError:
            pass
        b.sendall((json.dumps({"type": "control_request", "request_id": "r1",
                               "request": {"subtype": "initialize"}}) + "\n").encode())
        assert json.loads(arq_b.readline())["type"] == "system"   # o novo fala com o claude
        assert p.poll() is None
    finally:
        p.kill()
        p.wait()


def test_stderr_na_codepage_do_windows_nao_vira_caractere_quebrado():
    import importlib.util
    spec = importlib.util.spec_from_file_location("cano_mod", CANO)
    cano_mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cano_mod)
    assert cano_mod._texto_do_stderr("não existe".encode("utf-8")) == "não existe"
    cp = cano_mod._texto_do_stderr("não existe".encode("cp1252"))
    assert "�" not in cp and cp.startswith("n") and cp.endswith("o existe")


def test_suite_nunca_le_os_sidecars_reais():
    from app.adapters.claude_headless import sessions
    real = Path.home() / ".hangar" / "claude-headless"
    assert sessions._dir() != real and real not in sessions._dir().parents


def _vivo(pid: int) -> bool:
    # Zumbi conta como morto: quem colhe o filho do cano é o init, no tempo dele.
    try:
        with open(f"/proc/{pid}/stat", encoding="ascii") as f:
            return f.read().rsplit(") ", 1)[1][0] != "Z"
    except FileNotFoundError:
        return False


def _porta_livre() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _conectar_tcp(porta: int) -> socket.socket:
    for _ in range(100):
        try:
            s = socket.create_connection(("127.0.0.1", porta), timeout=2)
            s.settimeout(5)
            return s
        except OSError:
            time.sleep(0.05)
    raise AssertionError("cano não escutou")
```

- [x] **Step 9: Rodar e ver passar** (quando autorizado)

Run:
```bash
(cd crates && cargo build --release -p hangar-cano)
cd backend && uv run pytest tests/test_rust_bins.py tests/test_claude_headless_cano.py tests/test_claude_headless.py -v
```
Expected: PASS. Em `test_claude_headless_cano.py`, 10 testes `[cano.py]`, 10 `[hangar-cano]` e os
2 sem parâmetro. Sem o binário compilado, os 10 `[hangar-cano]` aparecem como SKIPPED com
"hangar-cano não compilado".

- [ ] **Step 10: Sessão sem terminal real no `hangar-cano`** (verificação manual)

Com o dono, depois do push e do Atualizar no app (o backend do app roda do checkout instalado, não
deste), e com `hangar-cano` em `~/.hangar/bin/` ou no `crates/target/release/` daquele checkout:
1. Criar uma sessão Claude sem terminal e uma Codex sem terminal pelo app.
2. `tr '\0' ' ' < /proc/<pid do cano no sidecar>/cmdline` mostra `hangar-cano --escuta ... --log .../cano-<chave>.log`.
3. Mandar um prompt que peça permissão; o cartão aparece; `systemctl --user restart hangar-backend.service`;
   a sessão volta com o mesmo cartão (snapshot), e `Permitir` fecha o turno.
4. Renomear a pasta da sessão com ela viva: a lista segue mostrando a pasta nova (`registry.cwd_atual`).
5. Encerrar a sessão: cano e filho somem (`pgrep -f cano-<chave>` vazio).
6. Tirar o `hangar-cano` do lugar (renomear o arquivo) e criar outra sessão sem terminal: ela sobe
   no `cano.py`, e a sessão aberta no passo 1 continua viva no binário que já estava rodando.

- [x] **Step 11: Commit**

```bash
git add backend/app/rust_bins.py backend/tests/test_rust_bins.py backend/app/adapters/claude_headless/adapter.py backend/tests/test_claude_headless.py backend/tests/test_claude_headless_cano.py
git commit -m "feat(cano): start hangar-cano when the binary exists and run the cano suite on both"
```

---

#### Notas (dependências e riscos)

Desvios do contrato e do `cano.py`, todos deliberados:
- **`find_bin` com a variável preenchida e errada devolve `None`** em vez de seguir a ordem. O
  contrato diz "ordem: env var; …" sem dizer o que acontece quando ela aponta para o nada; cair
  calado para outro binário trocaria a escolha explícita. Efeito útil: `CP_RUST_CANO_BIN=off`
  força o `cano.py`, já que o cano não tem um `CP_RUST_CANO=0`. A frente 5 usa a mesma função para
  o `hangar-server`: `CP_RUST_SERVER_BIN` errado = Python sozinho, com aviso no log.
- **`find_bin` lê só o ambiente do processo (`os.environ`), como o contrato diz.** Atenção: o
  `backend/.env` NÃO chega ao `os.environ`; ele alimenta só o `Settings` do pydantic
  (conferido 2026-10-02: `config.py:159`, sem `load_dotenv` no backend). Quem puser
  `CP_RUST_CANO_BIN` ou `CP_RUST_SERVER_BIN` no `.env`, como faz com os outros `CP_*`, não terá
  efeito. Se o integrador quiser o `.env` valendo, o caminho é declarar os campos no `Settings` e
  `find_bin` ler `os.environ` e depois o campo; decidir junto com o `CP_RUST_SERVER=0` da frente 5,
  que tem a mesma questão.
- **`cano_saiu` vai atrás das linhas que já estavam na fila do cliente.** No `cano.py` ele vai
  direto no socket e pode passar na frente delas (e duas threads escrevem no mesmo socket). O prazo
  de 5 s cobre fila + linha; estourou, o cliente cai como lá.
- **O rc espera até 1 s o stderr do filho terminar**, para a `stderr_tail` do `cano_saiu` vir
  completa. No `cano.py` é corrida entre duas threads.
- **Linha com `params` ou `response` que não é objeto**: o `cano.py` levanta `AttributeError`, e a
  thread morre (do stdout: o cano para de repassar e nunca publica o rc; do cliente: a conexão cai).
  O Rust trata como vazio.
- **Filho que não sobe**: o Rust apaga o socket antes de sair 1; o `cano.py` deixava o arquivo (o
  backend apaga `cano-<chave>*` de todo jeito).
- **JSON que o próprio cano escreve** (snapshot, `cano_stderr`, `cano_saiu`) sai compacto e em
  UTF-8, não com `", "` e `ensure_ascii` como o `json.dumps`. Os campos e a ordem são os mesmos;
  quem lê faz `json.loads` (conferido 2026-10-02: `adapter.py:2134`, `codex/sem_terminal.py:141`).
  As linhas do filho passam sem mudança.
- **Prazo do token é de 10 s no total**; no `cano.py` são 10 s por `recv`.
- **`accept` que falha vai para o log** antes de parar de escutar; no `cano.py` parava calado.
- **"cliente conectado" no log** sai ao instalar o cliente, um instante antes do snapshot ir ao
  socket; no `cano.py`, depois.
- Limites conhecidos, sem caso real hoje: linha com mais de 128 níveis de aninhamento ou `NaN`
  não é observada (o `serde_json` recusa; o Python aceita); fora do Windows o stderr não-UTF-8
  é sempre lido como cp1252, mesmo com locale Latin-1.

Dependências entre Tasks:
- A Task 4 precisa do workspace `crates/` da Task 1. O Step 1 só acrescenta `tokio`, `chrono`,
  `libc` e `windows-sys`; o `tokio` daqui é o que as Tasks 11 e 12 usam.
- A Task 5 não depende da Task 4 para passar: sem o binário, os casos `[hangar-cano]` pulam com o
  motivo. Para os casos Rust rodarem, compilar antes (`cargo build --release -p hangar-cano`).
- O job Linux do `server.yml` (Task 3) roda esta suíte contra o `hangar-cano` recém-compilado, e
  falha se o binário não existir (sem ele os casos Rust pulariam). O
  `test_cliente_novo_substitui_o_ligado_em_tcp` roda também no Windows.

Riscos:
- **Windows com `.CMD`.** O primeiro item do comando pode ser `hangar-engine.CMD` (sessão com motor,
  `adapter.py:2151-2153`) ou o `codex.cmd` do npm, os dois resolvidos por `shutil.which` em `adapter.py:2148-2153`. O `Command` do Rust
  roda `.bat`/`.cmd` pelo `cmd.exe` com escape próprio e recusa argumento que não consegue escapar;
  o `Popen` do Python usa `list2cmdline`. Os argumentos de hoje são simples (sem quebra de linha),
  mas isso só se prova no Windows: abrir uma sessão Claude com motor e uma Codex sem terminal lá.
- **Versão desencontrada.** Binário velho em `~/.hangar/bin` com backend novo: o snapshot carrega
  `versao`, e o adapter só reabre sessão ociosa (`adapter.py:848`). Se o `VERSAO` do `cano.py` subir,
  `protocol::VERSION` sobe junto; o teste `test_snapshot_tem_os_campos_e_a_versao_do_cano_py` falha
  no caso `[hangar-cano]` enquanto isso não acontecer.
- **Sessões já abertas** seguem no `cano.py` até reabrir (spec, item 5). Nada força a troca.

O que foi conferido ao escrever estas Tasks, numa cópia fora do repo (2026-10-02, Linux x86_64,
toolchain 1.98.1): o código Rust da Task 4 compila sem aviso, os 18 testes passam e o
`cargo check --target x86_64-pc-windows-gnu --all-targets` passa; a suíte do Step 8 passou nas
duas implementações (21 testes; os 10 `[hangar-cano]` repetidos 5 vezes sem falha); o
`test_rust_bins.py` passou (5). Medido com `ps -o rss,nlwp` no build release, filho `sleep 3`,
socket unix, sem cliente: 3,0 MB de RSS e 1 thread. As mudanças em `test_claude_headless.py` e
`adapter.py` NÃO foram rodadas (exigiriam editar o repo).

### Task 6: Fixtures douradas da conversa e base do `transcript` (`pyjson`, `decode_line`)

**Files:**
- Create: `backend/tests/fixtures/contract/gen_golden.py`
- Create (gerados pelo script, commitados): `backend/tests/fixtures/contract/transcripts/claude.jsonl`, `backend/tests/fixtures/contract/transcripts/claude_rewrite_surrogate.jsonl`, `backend/tests/fixtures/contract/transcripts/codex.jsonl`, `backend/tests/fixtures/contract/queue/claude-fixture.jsonl`, `backend/tests/fixtures/contract/queue/codex-fixture.jsonl`, `backend/tests/fixtures/contract/golden/claude.tail.json`, `backend/tests/fixtures/contract/golden/claude_rewrite_surrogate.tail.json`, `backend/tests/fixtures/contract/golden/codex.tail.json`, `backend/tests/fixtures/contract/golden/claude.history.json`, `backend/tests/fixtures/contract/golden/codex.history.json`, `backend/tests/fixtures/contract/golden/pyjson.json`, `backend/tests/fixtures/contract/golden/isotime.json`
- Modify: `.gitattributes` (acrescentar ao fim)
- Modify: `crates/hangar-server/Cargo.toml` (`[dependencies]`)
- Modify: `crates/hangar-server/src/lib.rs` (declarar `pub mod transcript;`)
- Create: `crates/hangar-server/src/transcript/mod.rs`
- Create: `crates/hangar-server/src/transcript/py.rs`
- Create: `crates/hangar-server/src/transcript/pyjson.rs`
- Test: `crates/hangar-server/tests/common/mod.rs`
- Test: `crates/hangar-server/tests/contract_pyjson.rs`

**Interfaces:**
- Consumes:
  - Task 1: workspace `crates/` com `serde_json = { version = "=1.0.151", features = ["preserve_order"] }` em `[workspace.dependencies]` e o esqueleto `crates/hangar-server/src/lib.rs`.
  - Python real, chamado pelo gerador: `TranscriptTailer._read_from` (`backend/app/transcript.py:779`), `RewriteFilter` (`transcript.py:315`), `parse_obj` (`transcript.py:397`), `_ts` (`transcript.py:616`), `parse_rollout_line` (`backend/app/adapters/codex/rollout.py:336`), `merged_history` (`backend/app/pqueue.py:977`), `_queue_dir` (`pqueue.py:27`), `_TAIL_WINDOW` (`pqueue.py:958`), `scrub_surrogates` (`backend/app/models.py:22`), `registry.name_of_pid` (`backend/app/registry.py:546`).
- Produces:
  - `hangar_server::transcript::Provider { Claude, ClaudeHeadless, Codex }` com `fn parse(s: &str) -> Option<Provider>` e `fn as_str(self) -> &'static str`.
  - `hangar_server::transcript::decode_line(raw: &[u8]) -> Option<serde_json::Value>` (surrogate solto já trocado por U+FFFD).
  - `hangar_server::transcript::ts_of_iso(raw: &str) -> Option<f64>` (o `_ts` do Python).
  - `hangar_server::transcript::pyjson::{dumps(v: &Value, sort_keys: bool) -> String, dumps_unicode(v: &Value, sort_keys: bool) -> String, loads_lossless(s: &str) -> Option<Value>, float_repr(x: f64) -> String}`.
  - Uso interno das Tasks 7-9: `py::{is_marker, mark, surrogate_of, py_code, scrub_str, scrub_map, scrub_value, is_space, strip, iso_timestamp}`, `pyjson::number_repr`.
  - Formato do golden: `golden/<fixture>.tail.json` = `[{"offset": int, "event": ChatEvent, "python_error"?: str}]`; `golden/<fixture>.history.json` = `{"<variante>" | "<variante>+queue": [ChatEvent]}` com as variantes `full`, `limit2`, `limit200`, `limit2_w512`, `limit9_w512`, `limit200_w512`; `golden/pyjson.json` = `[{"raw", "sorted", "unsorted", "unicode", "scrubbed"}]`; `golden/isotime.json` = `[[iso, segundos | null]]`.

O surrogate solto é o ponto delicado desta Task. O Python guarda `"\ud83d"` cru no `str` até a borda do `ChatEvent` (o validador troca por U+FFFD, conferido 2026-10-02: `backend/app/models.py:241-244`), e os ids dependem do texto cru: o md5 com `encode("utf-8", "replace")` vira `?` no lugar dele (`transcript.py:423,432,457,472`), e o sha1 do Codex leva `\ud83d` escrito no `json.dumps` (`rollout.py:43`). Como uma `String` do Rust não guarda surrogate, a leitura tolerante troca cada um por um caractere da área privada (U+10F800 + deslocamento), e a troca por U+FFFD acontece só no fim, como no Python.

- [x] **Step 1: Escrever o gerador das fixtures e do golden**

```python
# backend/tests/fixtures/contract/gen_golden.py
"""Transcripts sintéticos do contrato da conversa e o resultado que os parsers Python dão para eles.

Os testes do hangar-server comparam campo a campo com o que este script grava em golden/. Tudo aqui
é inventado: nunca copie conversa real para cá.

Uso, de backend/:  uv run python tests/fixtures/contract/gen_golden.py
"""
import json
import os
import sys
import time
from datetime import datetime
from pathlib import Path

# Relógio sem fuso: o RewriteFilter e o merged_history do Python leem como hora LOCAL, o Rust como
# UTC. Com o processo em UTC os dois concordam no golden.
os.environ["TZ"] = "UTC"
time.tzset()

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))

from app import pqueue, registry  # noqa: E402
from app.adapters.codex.rollout import parse_rollout_line  # noqa: E402
from app.models import scrub_surrogates  # noqa: E402
from app.transcript import RewriteFilter, TranscriptTailer, _ts, parse_obj  # noqa: E402

TRANSCRIPTS = HERE / "transcripts"
QUEUE = HERE / "queue"
GOLDEN = HERE / "golden"

# O recado nativo resolveria o pid pelo tmux desta máquina; o golden não pode depender disso.
registry.name_of_pid = lambda pid: None
pqueue._queue_dir = lambda: QUEUE


def j(obj, ascii=False) -> bytes:
    # Separadores compactos, como o Claude e o Codex gravam; ascii=True escreve o surrogate solto
    # como "\ud83d", que é como ele aparece no arquivo.
    return json.dumps(obj, ensure_ascii=ascii, separators=(",", ":")).encode("utf-8")


def T(sec: int, ms: int = 0) -> str:
    return f"2026-09-01T10:{sec // 60:02d}:{sec % 60:02d}.{ms:03d}Z"


def C(sec: int) -> str:
    return f"2026-09-02T14:{sec // 60:02d}:{sec % 60:02d}.000Z"


def epoch(iso: str) -> float:
    return datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp()


BLOCKED = ('UserPromptSubmit operation blocked by hook:\n["/h/bloqueia.sh"]: falhou\n\n'
           "Original prompt: [de: outra] recado preso")

CLAUDE = [
    j({"type": "permission-mode", "permissionMode": "bypassPermissions", "sessionId": "s-claude"}),
    j({"parentUuid": None, "isSidechain": False, "type": "user", "uuid": "u-001", "timestamp": T(0),
       "message": {"role": "user", "content": "Olá, mundo — teste ✓"}}),
    j({"type": "assistant", "uuid": "a-001", "timestamp": T(1, 250), "message": {
        "role": "assistant",
        "content": [
            {"type": "thinking", "thinking": "Pensando no pedido…", "signature": "x"},
            {"type": "text", "text": "Vou listar os arquivos."},
            {"type": "tool_use", "id": "toolu_01", "name": "Bash",
             "input": {"command": "ls -la", "timeout": 120000, "ratio": 0.1, "tiny": 1e-05,
                       "huge": 1e16, "exact": 9007199254740993, "neg": -0.0, "um": 1.0}},
            {"type": "thinking", "thinking": "", "signature": "y"},
        ],
        "usage": {"input_tokens": 10, "cache_read_input_tokens": 1200,
                  "cache_creation": {"ephemeral_1h_input_tokens": 50, "ephemeral_5m_input_tokens": 0}}}}),
    j({"type": "user", "uuid": "u-002", "timestamp": T(2), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_01",
         "content": [{"type": "text", "text": "a.txt"}, {"type": "text", "text": "b.txt"}], "is_error": False},
        {"type": "tool_result", "tool_use_id": "toolu_02", "content": "falhou", "is_error": True},
        {"type": "tool_result", "tool_use_id": "toolu_03", "content": {"exit": 0, "msg": "it's"}}]}}),
    j({"type": "user", "uuid": "u-003", "timestamp": T(3), "message": {"role": "user", "content": [
        {"type": "text", "text": "[Image #1] olha isto"},
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}}]}}),
    j({"type": "user", "uuid": "u-004", "timestamp": T(3, 10),
       "message": {"role": "user", "content": "[Image: source: /tmp/x.png]"}}),
    j({"type": "user", "uuid": "u-005", "timestamp": T(4), "isMeta": True,
       "message": {"role": "user", "content": "Continue from where you left off."}}),
    j({"type": "user", "uuid": "u-006", "timestamp": T(4, 500),
       "message": {"role": "user", "content": "<command-name>/model</command-name>"}}),
    j({"type": "user", "uuid": "u-007", "timestamp": T(5),
       "message": {"role": "user", "content": "[Request interrupted by user]"}}),
    j({"type": "user", "uuid": "u-008", "timestamp": T(6), "message": {"role": "user", "content":
        '<system-reminder>lembrete</system-reminder>\n<pasted_content id="p1">\ntexto colado\n</pasted_content id="p1">'}}),
    # Surrogate solto numa linha sem relógio: o RewriteFilter deixa passar e o parse_obj recebe o
    # texto cru, que só vira U+FFFD na borda do ChatEvent.
    j({"type": "user", "uuid": "u-009", "message": {"role": "user", "content": "meio emoji \ud83d aqui"}},
      ascii=True),
    j({"type": "queue-operation", "operation": "enqueue", "timestamp": T(7),
       "content": "msg no meio \ud83d do turno"}, ascii=True),
    # O id leva o md5 do texto cru: o surrogate entra como "?" (encode com "replace").
    j({"type": "queue-operation", "operation": "remove", "timestamp": T(8),
       "content": "msg no meio \ud83d do turno"}, ascii=True),
    j({"type": "queue-operation", "operation": "enqueue", "timestamp": T(9),
       "content": "<task-notification><task-id> tsk-1 </task-id><status>completed</status></task-notification>"}),
    j({"type": "system", "subtype": "informational", "timestamp": T(10), "content": BLOCKED}),
    j({"type": "system", "subtype": "informational", "timestamp": T(11), "content": BLOCKED}),
    j({"type": "system", "timestamp": T(11, 500), "content": "Held peer message [de: x] sem âncora"}),
    j({"type": "attachment", "uuid": "att-1", "timestamp": T(12), "attachment": {
        "type": "queued_command", "prompt": [{"type": "text", "text": "orientação no meio"}]}}),
    # Anexo que nunca vira bolha: o /history lê só o relógio dele, sem json.loads.
    j({"parentUuid": "a-001", "isSidechain": False, "attachment": {"type": "hook_success", "content": "ok"},
       "type": "attachment", "uuid": "att-2", "timestamp": T(13)}),
    j({"type": "attachment", "uuid": "att-3", "timestamp": T(14), "attachment": {
        "type": "hook_additional_context", "hookEvent": "Stop", "content": ["Rode os testes.", 7]}}),
    j({"type": "user", "uuid": "u-010", "timestamp": T(15), "isCompactSummary": True,
       "message": {"role": "user", "content": "resumo longo"}}),
    j({"type": "user", "uuid": "u-011", "timestamp": T(16), "message": {"role": "user", "content":
        'Another Claude session sent a message:\n<teammate-message teammate_id="ana">\noi líder\n'
        '</teammate-message>\n<teammate-message teammate_id="bia">\n{"type": "idle"}\n</teammate-message>'}}),
    j({"type": "user", "uuid": "u-012", "timestamp": T(17), "message": {"role": "user", "content": [
        {"type": "text", "text": '<agent-message from="ag-1">[Subagent hand-back] pronto</agent-message>'}]}}),
    j({"type": "user", "uuid": "u-013", "timestamp": T(18), "isMeta": True,
       "origin": {"kind": "peer", "body": "olá par", "name": "Título da outra", "verifiedPeerPid": 999999},
       "message": {"role": "user", "content": '<cross-session-message from="x">…</cross-session-message>'}}),
    j({"type": "user", "uuid": "u-014", "timestamp": T(19), "message": {"role": "user", "content": "ok"}}),
    j({"type": "assistant", "uuid": "a-002", "timestamp": T(20), "message": {
        "role": "assistant", "content": [{"type": "text", "text": "Feito."}],
        "usage": {"cache_read_input_tokens": 3.9,
                  "cache_creation": {"ephemeral_1h_input_tokens": 0, "ephemeral_5m_input_tokens": 12}}}}),
    # `claude --resume` regravou: mesmo relógio e mesmo conteúdo, uuid novo.
    j({"parentUuid": None, "isSidechain": False, "type": "user", "uuid": "u-001-bis", "timestamp": T(0),
       "message": {"role": "user", "content": "Olá, mundo — teste ✓"}}),
    # Relógio uma hora atrás do maior já visto: reescrita.
    j({"type": "assistant", "uuid": "a-velho", "timestamp": "2026-09-01T09:00:00.000Z",
       "message": {"role": "assistant", "content": [{"type": "text", "text": "velho"}]}}),
    b'{"type":"user","message":',
    b"",
    b"   \t",
    j({"type": "user", "uuid": "u-015", "timestamp": T(21),
       "message": {"role": "user", "content": "byte @@FF@@ inválido"}}).replace(b"@@FF@@", b"\xff"),
    j({"type": "user", "uuid": "u-016", "timestamp": "2026-09-01T10:00:22",
       "message": {"role": "user", "content": "relógio sem fuso"}}),
    j({"type": "user", "uuid": "u-017", "timestamp": T(23), "message": {"role": "user", "content": "legenda da foto"}}),
    j({"type": "assistant", "uuid": "a-003", "timestamp": T(24),
       "message": {"role": "assistant", "content": [{"type": "text", "text": "Última resposta."}]}}),
]

# Surrogate solto numa mensagem COM relógio: o RewriteFilter do Python estoura no md5
# (UnicodeEncodeError) e derruba a leitura. O Rust segue; o golden marca a linha.
CLAUDE_REWRITE = [
    j({"type": "user", "uuid": "r-001", "timestamp": T(0), "message": {"role": "user", "content": "antes"}}),
    j({"type": "assistant", "uuid": "r-002", "timestamp": T(1), "message": {
        "role": "assistant", "content": [{"type": "text", "text": "meio \ud83d emoji"}]}}, ascii=True),
    j({"type": "user", "uuid": "r-003", "timestamp": T(2), "message": {"role": "user", "content": "depois"}}),
]


def item(payload, sec, ascii=False):
    return j({"timestamp": C(sec), "type": "response_item", "payload": payload}, ascii=ascii)


def user_text(text, sec):
    return item({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}, sec)


def tool_call(name, code, call, sec):
    return item({"type": "custom_tool_call", "name": name, "call_id": call, "input": code}, sec)


def tool_output(call, output, sec, kind="custom_tool_call_output"):
    return item({"type": kind, "call_id": call, "output": output}, sec)


SCRIPT_HEADER = "Script completed\nWall time 0.1 seconds\nOutput:\n"

CODEX = [
    j({"timestamp": C(0), "type": "session_meta", "payload": {
        "id": "019f0000-0000-7000-8000-000000000001", "cwd": "/tmp/proj", "cli_version": "0.151.0"}}),
    item({"type": "message", "role": "developer",
          "content": [{"type": "input_text", "text": "<permissions instructions>x</permissions instructions>"}]}, 1),
    user_text("<environment_context>\n<cwd>/tmp/proj</cwd>\n</environment_context>", 1),
    user_text("# AGENTS.md instructions for /tmp/proj\n\n<INSTRUCTIONS>\nseja breve\n</INSTRUCTIONS>", 1),
    # Surrogate solto e números que o `_event_id` serializa: o sha1 tem que sair igual.
    item({"type": "message", "role": "user",
          "content": [{"type": "input_text", "text": "Oi Codex — meio emoji \ud83d aqui"}],
          "meta": {"exato": 9007199254740993, "f": 0.1, "e": 1e16, "neg": -2, "tiny": 1e-05, "um": 1.0,
                   "s": "ç\u007f", "lista": [True, None]}}, 2, ascii=True),
    user_text("Rode os testes", 3),
    item({"type": "message", "role": "assistant", "content": [
        {"type": "output_text", "text": "Rodando "}, {"type": "output_text", "text": "agora."},
        {"type": "reasoning_text", "text": "ignorar"}]}, 4),
    item({"type": "function_call", "name": "shell", "call_id": "call_1",
          "arguments": '{"cmd": ["ls"], "um": 1.0, "tiny": 1e-05}'}, 5),
    item({"type": "function_call", "name": "shell", "call_id": "call_1b", "arguments": "não é json"}, 5),
    tool_output("call_1", "a.txt\nb.txt", 6, kind="function_call_output"),
    tool_call("exec", 'const r = await tools.exec_command({cmd:"echo \\"oi\\"","workdir":"/tmp"}); text(r.output);',
              "call_2", 7),
    tool_output("call_2", [{"type": "input_text", "text": SCRIPT_HEADER},
                           {"type": "input_text", "text": json.dumps({"output": "oi\n", "chunk_id": "c1",
                                                                      "wall_time_seconds": 0.1, "exit_code": 0,
                                                                      "session_id": 7})}], 8),
    tool_call("exec", 'await tools.apply_patch("*** Begin Patch\\n*** Update File: src/a.py\\n@@\\n-x = 1\\n+x = 2\\n'
                      '*** Add File: src/b.py\\n+novo\\n*** End Patch\\n");', "call_3", 9),
    tool_output("call_3", "Script failed\nWall time 2 seconds\nOutput:\n"
                + json.dumps({"output": "erro", "chunk_id": "c2", "wall_time_seconds": 2, "exit_code": 2}), 10),
    tool_call("exec", 'await tools.update_plan({explanation:"x",plan:[{step:"lidar com {config}",status:"completed"},'
                      '{step:"testar \\"tudo\\"",status:"in_progress"},{step:"sem status"}]});', "call_4", 11),
    tool_call("exec", 'await tools.exec_command({cmd:"git status"}); await tools.exec_command({cmd:"git diff"});',
              "call_5", 12),
    tool_call("exec", 'await tools.write_stdin({session_id:1,chars:"y"}); await tools.view_image({path:"/a.png"}); '
                      'await tools.write_stdin({session_id:1,chars:"n"});', "call_6", 13),
    tool_call("exec", 'await tools.write_stdin({session_id:1,chars:"q"});', "call_7", 14),
    tool_call("apply_patch", "*** Begin Patch\n*** Delete File: velho.txt\n*** End Patch", "call_8", 15),
    tool_output("call_8", [{"type": "input_text", "text": SCRIPT_HEADER + json.dumps([
        {"status": "fulfilled", "value": {"output": "a", "chunk_id": "c", "wall_time_seconds": 0}},
        {"status": "rejected", "reason": {"msg": "não", "code": 1}}])}], 16),
    tool_output("call_9", {"exit": 0, "msg": "it's"}, 17, kind="function_call_output"),
    tool_output("call_10", SCRIPT_HEADER + "texto solto\n", 18),
    item({"type": "function_call_output", "call_id": "call_11"}, 18),
    user_text("<turn_aborted>\nThe user interrupted.\n</turn_aborted>", 19),
    user_text('<hook_prompt hook_run_id="h1">\n  Rode o lint antes.\n</hook_prompt>', 20),
    user_text("<skill>\n<name> handoff </name>\n<path>/home/x/.codex/skills/handoff/SKILL.md</path>\n# Handoff\n"
              "corpo\n</skill>", 21),
    user_text("<skill><name>sem-caminho</name>corpo curto</skill>", 22),
    item({"type": "reasoning", "summary": [], "encrypted_content": "gAAAA"}, 23),
    j({"timestamp": C(23), "type": "event_msg", "payload": {"type": "token_count"}}),
    b'{"timestamp":"x","type":"response_item","payload":',
    b"",
    user_text("Rode os testes", 24),
]

CLAUDE_QUEUE = [
    {"id": "q-absorvida", "text": "ok", "ts": epoch(T(18, 500)), "delivered": True},
    {"id": "q-segundo-ok", "text": "ok", "ts": epoch(T(19, 500)), "delivered": True},
    {"id": "q-legenda", "text": "legenda da foto — 📎 imagem: /tmp/foto.png", "ts": epoch(T(22, 900)),
     "delivered": True},
    {"id": "q-confirmada", "text": "já confirmada", "ts": epoch(T(5)), "delivered": True, "confirmed": True},
    {"id": "q-local", "text": "Saída do /cost", "ts": epoch(T(6, 500)), "delivered": True, "confirmed": True,
     "papel": "assistant"},
    {"id": "q-velha", "text": "de outra vida", "ts": epoch(T(0)) - 3600, "delivered": True},
    {"id": "q-bastao", "text": "kick-off do bastão", "ts": epoch(T(0)) - 600, "delivered": False,
     "pre_transcript": True},
    {"id": "q-desistiu", "text": "não chegou", "ts": epoch(T(9)), "delivered": True, "desistiu": True,
     "desistiu_ts": epoch(T(20))},
    {"id": "q-sem-ts", "text": "sem relógio", "delivered": False},
    {"id": "q-vazia", "text": "   ", "ts": epoch(T(10))},
    "não é json {",
    # O splitlines do Python quebra no U+2028 cru: a entrada some, e no Rust tem que sumir também.
    {"id": "q-quebrada", "text": "antes depois", "ts": epoch(T(12)), "delivered": False},
    {"id": 42, "text": "id numérico", "ts": epoch(T(12)), "delivered": "sim"},
    {"id": "q-pendente", "text": "ainda na fila", "ts": epoch(T(24, 100)), "delivered": False},
]

CODEX_QUEUE = [
    {"id": "c-absorvida", "text": "Rode os testes", "ts": epoch(C(3)) - 0.5, "delivered": True},
    {"id": "c-pendente", "text": "e depois o lint", "ts": epoch(C(30)), "delivered": True},
    {"id": "c-confirmada", "text": "x", "ts": epoch(C(4)), "delivered": True, "confirmed": True},
]

PYJSON = [
    '{"b": 1, "a": [1, 1.0, 1e-05, 1e16, 1e15, 0.1, -0.0, 1.5e-07, 123.456, 9007199254740993, '
    '-9223372036854775808, 18446744073709551615, 5e-324, 1e300, 2.5e-05, 100.0, 0.0001]}',
    '{"txt": "é ç ã 😀 \\u007f \\u001f \\" \\\\ \\n \\t \\b \\f /", "sur": "meio \\ud83d emoji", '
    '"par": "\\ud83d\\ude00", "baixo": "\\ude00 só", "alto_e_A": "\\ud83d\\u0041"}',
    '{"z": {"y": [], "x": {}}, "a": null, "m": true, "n": false, "": "vazio"}',
    '{"a": 1, "\\ud83d": 2, "\\ue000": 3, "😀": 4, "\\u00e9": 5}',
    '["\\u2028\\u2029", {"k": "\\ud800"}]',
    '"só texto"',
    "12345678901234567890",
]

ISO = [
    "2026-10-02T10:00:00Z", "2026-10-02T10:00:00.1Z", "2026-10-02T10:00:00.1234567Z",
    "2026-10-02T10:00:00.123456789+00:00", "2026-10-02", "2026-10-02 10:00", "20261002T100000Z",
    "2026-10-02T10:00:00+0300", "2026-10-02T10:00:00-03:00", "2026-10-02T10Z", "2026-10-02T10:00:00,5Z",
    "2026-10-02x10:00:00Z", "1969-12-31T23:59:59.5Z", "2026-02-30T10:00:00Z", "2026-13-01T00:00:00Z", "",
    "não é data", "2026-10-02T25:00:00Z", "2026-10-02T10:00:00.Z", "2026-10-02T1000Z",
    "2026-10-02T24:00:00Z", "2026-12-31T24:00:00Z", "2026-10-02T24:00:01Z", "2026-10-02T10:00:00+03",
    "2026-10-02T10:00:00+05:30:15", "2026-10-02T10:00:00+05:30:15.5", "2026-10-02T10:00:00.5",
    "2026-10-02T10:00:60Z", "2026-10-02T10:00:00 ", " 2026-10-02T10:00:00", "2026-10-02T10:00:00+24:00",
    "2026-1-02", "2026-10-02T10:0", "0001-01-01T00:00:00Z", "2026-10-02T10:00:00z", "2026-10-02T",
    "2026-10-0210:00", "20261002", "2026-10-02T10:00:00.12Z", "2026-10-02T100000.5Z",
    "2026-10-02T10:00:00-0000", "2024-02-29T12:00:00Z", "2026-09-01T10:00:01.250Z",
]

# Janela pequena exercita a leitura de trás para frente sem arquivo de 256 KB no repositório; com 9
# a janela para no meio do arquivo e o `--resume` regravado escapa do filtro, como no Python.
VARIANTS = [("full", None, 256 * 1024), ("limit2", 2, 256 * 1024), ("limit200", 200, 256 * 1024),
            ("limit2_w512", 2, 512), ("limit9_w512", 9, 512), ("limit200_w512", 200, 512)]

# Linhas que não são JSON, de propósito. Erro de digitação aqui viraria linha pulada calada.
INVALID_LINES = {"claude.jsonl": 1, "claude_rewrite_surrogate.jsonl": 0, "codex.jsonl": 1}


def write_jsonl(path: Path, lines: list[bytes]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(line + b"\n" for line in lines))
    bad = 0
    for line in lines:
        text = line.decode("utf-8", "replace").strip()
        if not text:
            continue
        try:
            json.loads(text)
        except ValueError:
            bad += 1
    assert bad == INVALID_LINES[path.name], f"{path.name}: {bad} linhas inválidas"


def write_queue(path: Path, entries: list) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    # Mesmo formato do PromptQueue._write_atomic (json.dumps com ensure_ascii=False).
    text = "".join((e if isinstance(e, str) else json.dumps(e, ensure_ascii=False)) + "\n" for e in entries)
    path.write_text(text, encoding="utf-8")


def write_golden(name: str, data) -> None:
    GOLDEN.mkdir(parents=True, exist_ok=True)
    (GOLDEN / name).write_text(json.dumps(data, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")


def event_dict(ev) -> dict:
    return ev.model_dump(mode="json")


def tail(path: Path, codex: bool) -> list[dict]:
    # O que o TranscriptTailer emite lendo do início (o backfill com offset 0).
    tailer = TranscriptTailer(path, parse_line=parse_rollout_line) if codex else TranscriptTailer(path)
    evs, _ = tailer._read_from(0)
    return [{"offset": ev.offset, "event": event_dict(ev)} for ev in evs]


def tail_tolerant(path: Path) -> list[dict]:
    # O laço do _read_from, com a linha que derruba o RewriteFilter lida pelo parse_obj puro.
    rewrite = RewriteFilter()
    out = []
    with path.open("rb") as fh:
        while True:
            start = fh.tell()
            raw = fh.readline()
            if not raw:
                break
            text = raw.decode("utf-8", "replace")
            error = None
            try:
                evs = rewrite.parse_line(text)
            except UnicodeEncodeError as e:
                error = type(e).__name__
                evs = parse_obj(json.loads(text))
            for ev in evs:
                rec = {"offset": start, "event": event_dict(ev)}
                if error:
                    rec["python_error"] = error
                out.append(rec)
    return out


def history(path: Path, provider: str, queue_name: str) -> dict:
    out = {}
    for name, limit, window in VARIANTS:
        for suffix, session in (("", "sem-fila"), ("+queue", queue_name)):
            pqueue._TAIL_WINDOW = window
            evs = pqueue.merged_history(session, str(path), provider, limit)
            if limit is not None and limit > 0:
                evs = evs[-limit:]          # o corte que a rota /history faz (api.py:2918)
            out[name + suffix] = [event_dict(ev) for ev in evs]
    return out


def main() -> None:
    claude = TRANSCRIPTS / "claude.jsonl"
    rewrite = TRANSCRIPTS / "claude_rewrite_surrogate.jsonl"
    codex = TRANSCRIPTS / "codex.jsonl"
    write_jsonl(claude, CLAUDE)
    write_jsonl(rewrite, CLAUDE_REWRITE)
    write_jsonl(codex, CODEX)
    write_queue(QUEUE / "claude-fixture.jsonl", CLAUDE_QUEUE)
    write_queue(QUEUE / "codex-fixture.jsonl", CODEX_QUEUE)

    write_golden("claude.tail.json", tail(claude, codex=False))
    write_golden("claude_rewrite_surrogate.tail.json", tail_tolerant(rewrite))
    write_golden("codex.tail.json", tail(codex, codex=True))
    write_golden("claude.history.json", history(claude, "claude", "claude-fixture"))
    write_golden("codex.history.json", history(codex, "codex", "codex-fixture"))
    write_golden("pyjson.json", [{
        "raw": raw,
        "sorted": json.dumps(json.loads(raw), sort_keys=True),
        "unsorted": json.dumps(json.loads(raw)),
        "unicode": json.dumps(json.loads(raw), ensure_ascii=False),
        "scrubbed": json.dumps(scrub_surrogates(json.loads(raw)), sort_keys=True),
    } for raw in PYJSON])
    write_golden("isotime.json", [[s, _ts({"timestamp": s})] for s in ISO])


if __name__ == "__main__":
    main()
```

- [x] **Step 2: Deixar as fixtures fora da conversão de fim de linha**

O `.gitattributes` de hoje só cuida de `*.sh` e `*.hook` (conferido 2026-10-02: `.gitattributes:13-14`). No checkout do Windows com `core.autocrlf=true` os transcripts ganhariam `\r`, os offsets do golden deixariam de bater e o byte `0xFF` proposital passaria por conversão. Acrescentar ao fim de `.gitattributes`:

```gitattributes
# Fixtures do contrato da conversa: os testes comparam offsets em bytes e há um 0xFF de propósito.
backend/tests/fixtures/contract/transcripts/** -text
backend/tests/fixtures/contract/queue/** -text
```

- [x] **Step 3: Gerar as fixtures e o golden**

Rodar o gerador (não é teste: ele grava os arquivos que os testes do Rust leem):

Run: `cd backend && uv run python tests/fixtures/contract/gen_golden.py`
Expected: sai com código 0. No stderr aparecem avisos `entrada system parece recado e NAO casou a ancora do harness` — são da linha `Held peer message …` do `claude.jsonl`, posta ali para exercitar esse ramo. Os 12 arquivos gerados listados em **Files** passam a existir.

Conferir à mão três pontos que o resto do plano assume:

Run: `cd backend/tests/fixtures/contract && grep -c '"python_error"' golden/claude_rewrite_surrogate.tail.json && grep -o '"queued:2026-09-01T10:00:08.000Z:[0-9a-f]*"' golden/claude.tail.json && python3 -c "import json; print([e['id'] for e in json.load(open('golden/claude.history.json'))['limit9_w512']][0])"`
Expected: `1` (a linha que derruba o `RewriteFilter` do Python), `"queued:2026-09-01T10:00:08.000Z:f3bf7c28"` (o md5 com `?` no lugar do surrogate) e `u-001-bis` (a janela que para no meio do arquivo deixa passar a linha regravada pelo `--resume`, como no Python).

- [x] **Step 4: Dependência do `hangar-server`**

`crates/hangar-server/Cargo.toml`, em `[dependencies]`:

```toml
# float_roundtrip: o float lido sai com o mesmo valor que o float() do Python, e o repr bate.
serde_json = { workspace = true, features = ["float_roundtrip"] }
```

`crates/hangar-server/src/lib.rs` (esqueleto da Task 1) passa a declarar o módulo:

```rust
//! Servidor do Hangar: lê as conversas do Claude e do Codex e repassa o resto ao backend Python.
pub mod transcript;
```

- [x] **Step 5: Escrever os testes**

```rust
// crates/hangar-server/tests/common/mod.rs
//! Caminhos e comparação dos testes de contrato com o golden do Python.
#![allow(dead_code)]

use std::path::PathBuf;

use hangar_server::transcript::pyjson;
use serde_json::Value;

pub fn contract() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract")
}

pub fn golden(name: &str) -> Value {
    let path = contract().join("golden").join(name);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    pyjson::loads_lossless(&raw).expect("golden é JSON")
}

/// Forma canônica para comparar: a mesma serialização dos dois lados, chaves ordenadas.
pub fn canon(v: &Value) -> String {
    pyjson::dumps(v, true)
}
```

```rust
// crates/hangar-server/tests/contract_pyjson.rs
mod common;

use common::golden;
use hangar_server::transcript::{decode_line, pyjson, ts_of_iso};

#[test]
fn dumps_matches_python() {
    for case in golden("pyjson.json").as_array().expect("lista") {
        let raw = case["raw"].as_str().unwrap();
        let v = pyjson::loads_lossless(raw).unwrap_or_else(|| panic!("não leu {raw}"));
        assert_eq!(pyjson::dumps(&v, true), case["sorted"].as_str().unwrap(), "sort_keys: {raw}");
        assert_eq!(pyjson::dumps(&v, false), case["unsorted"].as_str().unwrap(), "ordem original: {raw}");
        assert_eq!(pyjson::dumps_unicode(&v, false), case["unicode"].as_str().unwrap(), "ensure_ascii=False: {raw}");
    }
}

#[test]
fn decode_line_scrubs_like_python() {
    for case in golden("pyjson.json").as_array().unwrap() {
        let raw = case["raw"].as_str().unwrap();
        let v = decode_line(format!("  {raw}\n").as_bytes()).unwrap_or_else(|| panic!("não leu {raw}"));
        assert_eq!(pyjson::dumps(&v, true), case["scrubbed"].as_str().unwrap(), "{raw}");
    }
    assert!(decode_line(b"   \n").is_none());
    assert!(decode_line(br#"{"type": "user", "message":"#).is_none());
}

#[test]
fn iso_clock_matches_python() {
    for case in golden("isotime.json").as_array().unwrap() {
        let s = case[0].as_str().unwrap();
        assert_eq!(ts_of_iso(s), case[1].as_f64(), "{s:?}");
    }
}
```

- [x] **Step 6: Rodar e ver falhar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-server --test contract_pyjson`
Expected: FAIL de compilação, `file not found for module transcript` (o `lib.rs` já declara o módulo e o arquivo ainda não existe).

- [x] **Step 7: Implementar `py.rs`**

`str.isspace()` do Python inclui `\x1c`-`\x1f`, que o `char::is_whitespace` do Rust não inclui; por isso `strip` é próprio. O relógio é porte do `_pydatetime.fromisoformat` do Python 3.14 (`/usr/lib/python3.14/_pydatetime.py:301-490`), seguindo o C onde os dois divergem (fração vazia em `10:00:00.` é aceita); o `isotime.json` confere cada caso contra o Python real.

```rust
// crates/hangar-server/src/transcript/py.rs
//! Semântica do Python que o porte repete: `str.strip`, surrogate solto, `str()`/`repr()` e o relógio
//! do `datetime.fromisoformat`.

use serde_json::{Map, Value};

/// Surrogate solto não cabe numa `String`: ele vira um caractere da área privada até a borda do
/// `ChatEvent`, onde vira U+FFFD como no `scrub_surrogates` (models.py:22). Até lá, os ids que o
/// Python calcula sobre o texto cru saem iguais.
// ponytail: um U+10F800..U+10FFFF de verdade no transcript seria lido como surrogate; trocar por um
// tipo de texto próprio se aparecer.
const MARK_BASE: u32 = 0x10F800;

pub(crate) fn is_marker(c: char) -> bool {
    c as u32 >= MARK_BASE
}

pub(crate) fn mark(surrogate: u32) -> char {
    char::from_u32(MARK_BASE + (surrogate - 0xD800)).expect("surrogate entre D800 e DFFF")
}

pub(crate) fn surrogate_of(c: char) -> u32 {
    c as u32 - MARK_BASE + 0xD800
}

/// Ponto de código como o Python o vê (o marcador volta para U+D800..U+DFFF): ordem do `sort_keys`.
pub(crate) fn py_code(c: char) -> u32 {
    if is_marker(c) { surrogate_of(c) } else { c as u32 }
}

fn has_marker(s: &str) -> bool {
    // Todo caractere acima de U+100000 começa com o byte F4: a busca rápida descarta quase tudo.
    s.as_bytes().contains(&0xF4) && s.chars().any(is_marker)
}

pub(crate) fn scrub_str(s: &mut String) {
    if has_marker(s) {
        *s = s.chars().map(|c| if is_marker(c) { '\u{FFFD}' } else { c }).collect();
    }
}

pub(crate) fn scrub_map(m: &mut Map<String, Value>) {
    if m.keys().any(|k| has_marker(k)) {
        // Chaves que colidem depois da troca: fica a última, na posição da primeira, como no dict.
        for (mut k, mut v) in std::mem::take(m) {
            scrub_str(&mut k);
            scrub_value(&mut v);
            m.insert(k, v);
        }
    } else {
        m.values_mut().for_each(scrub_value);
    }
}

pub(crate) fn scrub_value(v: &mut Value) {
    match v {
        Value::String(s) => scrub_str(s),
        Value::Array(items) => items.iter_mut().for_each(scrub_value),
        Value::Object(m) => scrub_map(m),
        _ => {}
    }
}

/// `str.isspace()`: o White_Space do Unicode mais os separadores \x1c-\x1f.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `transcript._ts` (transcript.py:616): relógio ISO da entrada em segundos; sem fuso vale UTC.
pub fn ts_of_iso(raw: &str) -> Option<f64> {
    if raw.is_empty() {
        return None;
    }
    iso_timestamp(&raw.replace('Z', "+00:00"))
}

/// `datetime.fromisoformat(s).timestamp()`. Sem fuso lê como UTC: o Python lê como hora local no
/// RewriteFilter e no merged_history, mas os transcripts trazem sempre o fuso.
pub(crate) fn iso_timestamp(s: &str) -> Option<f64> {
    let (naive, offset) = fromisoformat(s)?;
    // Mesma conta do Python: microssegundos inteiros divididos por 10**6.
    Some((naive - offset.unwrap_or(0)) as f64 / 1e6)
}

/// (microssegundos desde a época lendo a hora como UTC, deslocamento do fuso em microssegundos).
/// Porte do `_pydatetime.fromisoformat` (o C aceita fração vazia, e este segue o C). Datas por
/// semana ISO (`2026-W40-5`) ficam de fora.
fn fromisoformat(s: &str) -> Option<(i64, Option<i64>)> {
    let cs: Vec<char> = s.chars().collect();
    // Data sozinha de 7 caracteres só existe na forma por semana.
    if cs.len() < 8 {
        return None;
    }
    let sep = if cs[4] == '-' {
        if cs[5] == 'W' {
            return None;
        }
        10
    } else {
        if cs[4] == 'W' {
            return None;
        }
        8
    };
    let (mut y, mut mo, mut d) = parse_date(cs.get(..sep)?)?;
    let (mut h, mi, sec, us, offset) = if cs.len() > sep {
        let t = &cs[sep + 1..];
        if t.is_empty() {
            return None;
        }
        parse_time(t)?
    } else {
        (0, 0, 0, 0, None)
    };
    if !valid_date(y, mo, d) {
        return None;
    }
    if h == 24 {
        if mi != 0 || sec != 0 || us != 0 {
            return None;
        }
        h = 0;
        d += 1;
        if d > days_in_month(y, mo) {
            d = 1;
            mo += 1;
            if mo > 12 {
                mo = 1;
                y += 1;
            }
        }
        if !valid_date(y, mo, d) {
            return None;
        }
    }
    if h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    Some(((((days * 24 + h) * 60 + mi) * 60 + sec) * 1_000_000 + us, offset))
}

fn digits(cs: &[char]) -> Option<i64> {
    if cs.is_empty() {
        return None;
    }
    cs.iter().try_fold(0i64, |acc, c| c.to_digit(10).map(|x| acc * 10 + i64::from(x)))
}

fn parse_date(d: &[char]) -> Option<(i64, i64, i64)> {
    match d.len() {
        10 if d[4] == '-' && d[7] == '-' => Some((digits(&d[0..4])?, digits(&d[5..7])?, digits(&d[8..10])?)),
        8 if d[4] != '-' => Some((digits(&d[0..4])?, digits(&d[4..6])?, digits(&d[6..8])?)),
        _ => None,
    }
}

type Time = (i64, i64, i64, i64, Option<i64>);

fn parse_time(t: &[char]) -> Option<Time> {
    if t.len() < 2 {
        return None;
    }
    // Primeiro '-', senão '+', senão 'Z', como o `_parse_isoformat_time`.
    let tz = ['-', '+', 'Z'].iter().find_map(|m| t.iter().position(|c| c == m));
    let [h, mi, sec, us] = hh_mm_ss_ff(&t[..tz.unwrap_or(t.len())])?;
    let offset = match tz {
        None => None,
        Some(p) if p + 1 == t.len() && t[p] == 'Z' => Some(0),
        Some(p) => {
            let zone = &t[p + 1..];
            if matches!(zone.len(), 0 | 1 | 3) || t[p] == 'Z' {
                return None;
            }
            let [zh, zm, zs, zus] = hh_mm_ss_ff(zone)?;
            let micros = ((zh * 60 + zm) * 60 + zs) * 1_000_000 + zus;
            if micros >= 86_400_000_000 {
                return None;
            }
            Some(if t[p] == '-' { -micros } else { micros })
        }
    };
    Some((h, mi, sec, us, offset))
}

fn hh_mm_ss_ff(t: &[char]) -> Option<[i64; 4]> {
    let mut comps = [0i64; 4];
    let mut pos = 0;
    let mut has_sep = false;
    for comp in 0..3 {
        if t.len() - pos < 2 {
            return None;
        }
        comps[comp] = digits(&t[pos..pos + 2])?;
        pos += 2;
        let next = t.get(pos).copied();
        if comp == 0 {
            has_sep = next == Some(':');
        }
        if next.is_none() || comp >= 2 {
            break;
        }
        if has_sep && next != Some(':') {
            return None;
        }
        pos += usize::from(has_sep);
    }
    if pos < t.len() {
        if !matches!(t[pos], '.' | ',') {
            return None;
        }
        let frac = &t[pos + 1..];
        if !frac.iter().all(char::is_ascii_digit) {
            return None;
        }
        let n = frac.len().min(6);
        let mut us = if n == 0 { 0 } else { digits(&frac[..n])? };
        for _ in n..6 {
            us *= 10;
        }
        comps[3] = us;
    }
    Some(comps)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

fn valid_date(y: i64, m: i64, d: i64) -> bool {
    (1..=9999).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

```

- [x] **Step 8: Implementar `pyjson.rs`**

`float_repr` reproduz o `float.__repr__`: dígitos mais curtos (o `{:e}` do Rust dá os mesmos dígitos) e notação científica quando o expoente decimal fica fora de `[-4, 16)`, com dois dígitos no expoente (`1e-05`, `1e+16`). A ordem do `sort_keys` é por ponto de código, com o surrogate marcado de volta ao lugar dele.

```rust
// crates/hangar-server/src/transcript/pyjson.rs
//! `json.dumps` e `json.loads` do Python na medida que os ids exigem: separadores ", " e ": ",
//! `repr` de float, `ensure_ascii` e surrogate solto aceito na leitura.

use std::fmt::Write;

use serde_json::{Number, Value};

use super::py;

/// `json.dumps(v, sort_keys=sort_keys)` (ensure_ascii=True).
pub fn dumps(v: &Value, sort_keys: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, v, sort_keys, true);
    out
}

/// `json.dumps(v, ensure_ascii=False, sort_keys=sort_keys)`. Surrogate solto sai cru, como no
/// Python, e só vira U+FFFD na borda do `ChatEvent`.
pub fn dumps_unicode(v: &Value, sort_keys: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, v, sort_keys, false);
    out
}

/// `json.loads` que aceita surrogate solto como o Python. O valor guarda o surrogate como marcador;
/// para o valor final, sem marcador, use `transcript::decode_line`.
// ponytail: inteiro além de u64 vira f64 e NaN/Infinity não são aceitos (o Python aceita os dois);
// `arbitrary_precision` do serde_json se aparecer num transcript.
pub fn loads_lossless(s: &str) -> Option<Value> {
    match serde_json::from_str(s) {
        Ok(v) => Some(v),
        Err(_) if s.contains("\\u") => serde_json::from_str(&mark_lone_surrogates(s)).ok(),
        Err(_) => None,
    }
}

/// Troca cada `\uD8xx`..`\uDFxx` sem par, dentro de string JSON, pelo marcador cru.
fn mark_lone_surrogates(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut copied, mut in_str) = (0, 0, false);
    while i < b.len() {
        match b[i] {
            b'"' => {
                in_str = !in_str;
                i += 1;
            }
            b'\\' if in_str => match (b.get(i + 1) == Some(&b'u')).then(|| hex4(b, i + 2)).flatten() {
                // Par válido: o serde_json junta sozinho.
                Some(0xD800..=0xDBFF) if low_follows(b, i + 6) => i += 12,
                Some(code @ 0xD800..=0xDFFF) => {
                    out.push_str(&s[copied..i]);
                    out.push(py::mark(code));
                    i += 6;
                    copied = i;
                }
                _ => i += 2,
            },
            _ => i += 1,
        }
    }
    out.push_str(&s[copied..]);
    out
}

fn low_follows(b: &[u8], at: usize) -> bool {
    b.get(at) == Some(&b'\\') && b.get(at + 1) == Some(&b'u') && hex4(b, at + 2).is_some_and(|lo| (0xDC00..=0xDFFF).contains(&lo))
}

fn hex4(b: &[u8], at: usize) -> Option<u32> {
    let digits = b.get(at..at + 4)?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

fn write_value(out: &mut String, v: &Value, sort_keys: bool, ascii: bool) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number_repr(n)),
        Value::String(s) => write_str(out, s, ascii),
        Value::Array(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, x, sort_keys, ascii);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut pairs: Vec<_> = m.iter().collect();
            if sort_keys {
                // Ordem de ponto de código do Python, com o surrogate marcado no lugar dele.
                pairs.sort_by(|a, b| a.0.chars().map(py::py_code).cmp(b.0.chars().map(py::py_code)));
            }
            out.push('{');
            for (i, (k, x)) in pairs.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_str(out, k, ascii);
                out.push_str(": ");
                write_value(out, x, sort_keys, ascii);
            }
            out.push('}');
        }
    }
}

fn write_str(out: &mut String, s: &str, ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c < ' ' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            // O Python escreve o surrogate solto como \udXXX; com ensure_ascii=False ele sai cru.
            c if ascii && py::is_marker(c) => {
                let _ = write!(out, "\\u{:04x}", py::surrogate_of(c));
            }
            c if ascii && c as u32 > 0x7e => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{u:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `repr` de um número do `json.loads`: inteiro em dígitos, float como o Python.
pub(crate) fn number_repr(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        float_repr(n.as_f64().unwrap_or(f64::NAN))
    }
}

/// `float.__repr__`: dígitos mais curtos que voltam ao mesmo valor; notação científica quando o
/// expoente decimal fica abaixo de -4 ou passa de 15 (1e-05, 1e+16), senão fixa com ".0".
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').expect("formato {:e}");
    let exp: i32 = exp.parse().expect("expoente do {:e}");
    let (neg, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1;
    let body = if decpt <= -4 || decpt > 16 {
        let mut m = digits[..1].to_string();
        if digits.len() > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    } else if decpt <= 0 {
        format!("0.{}{digits}", "0".repeat(decpt.unsigned_abs() as usize))
    } else if decpt as usize >= digits.len() {
        format!("{digits}{}.0", "0".repeat(decpt as usize - digits.len()))
    } else {
        let (int, frac) = digits.split_at(decpt as usize);
        format!("{int}.{frac}")
    };
    if neg { format!("-{body}") } else { body }
}
```

- [x] **Step 9: Implementar `mod.rs`**

```rust
// crates/hangar-server/src/transcript/mod.rs
//! Leitura das conversas do Claude e do Codex, portada de backend/app/transcript.py,
//! adapters/codex/rollout.py e pqueue.py. A saída tem que sair igual à do Python: os aparelhos
//! deduplicam por id, e um id diferente na troca vira mensagem repetida na tela.

mod py;
pub mod pyjson;

use serde_json::Value;

pub use py::ts_of_iso;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    ClaudeHeadless,
    Codex,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Provider> {
        match s {
            "claude" => Some(Self::Claude),
            "claude-headless" => Some(Self::ClaudeHeadless),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ClaudeHeadless => "claude-headless",
            Self::Codex => "codex",
        }
    }
}

/// Linha crua do transcript como JSON, com surrogate solto trocado por U+FFFD (`scrub_surrogates`,
/// models.py:22). None em linha em branco ou que não é JSON.
pub fn decode_line(raw: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(raw);
    let mut v = pyjson::loads_lossless(py::strip(&text))?;
    py::scrub_value(&mut v);
    Some(v)
}
```

- [x] **Step 10: Rodar e ver passar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-server --test contract_pyjson`
Expected: PASS, 3 testes (`dumps_matches_python`, `decode_line_scrubs_like_python`, `iso_clock_matches_python`), sem warning de compilação.

- [x] **Step 11: Commit**

```bash
git add .gitattributes backend/tests/fixtures/contract/gen_golden.py \
  backend/tests/fixtures/contract/transcripts backend/tests/fixtures/contract/queue \
  backend/tests/fixtures/contract/golden \
  crates/hangar-server/Cargo.toml crates/Cargo.lock crates/hangar-server/src/lib.rs \
  crates/hangar-server/src/transcript/mod.rs crates/hangar-server/src/transcript/py.rs \
  crates/hangar-server/src/transcript/pyjson.rs \
  crates/hangar-server/tests/common/mod.rs crates/hangar-server/tests/contract_pyjson.rs
git commit -m "feat(server): conversation contract fixtures and Python-compatible JSON for transcript ids"
```

---

### Task 7: Parser do Claude em Rust

**Files:**
- Create: `crates/hangar-server/src/transcript/claude.rs`
- Modify: `crates/hangar-server/src/transcript/py.rs` (imports do topo e helpers no fim)
- Modify: `crates/hangar-server/src/transcript/mod.rs` (inteiro, versão abaixo)
- Modify: `crates/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `crates/hangar-server/Cargo.toml` (`[dependencies]`)
- Test: `crates/hangar-server/tests/contract_tail.rs`

**Interfaces:**
- Consumes:
  - Task 1: `hangar_api::chat::ChatEvent` (com `Default`, campos de `models.py:191`, `cache_read`/`cache_ttl_s: Option<u64>`, `image_count: Option<u32>`, `offset: Option<u64>` com `#[serde(skip)]`) e `hangar_api::chat::ChatKind::{UserMsg, AssistantMsg, ToolUse, ToolResult, Thinking, Notice}`.
  - Task 6: `py::*`, `pyjson::*`, `Provider`, o golden `claude.tail.json` e `claude_rewrite_surrogate.tail.json`.
  - Python portado: `parse_line` (`transcript.py:304`), `RewriteFilter` (`transcript.py:315-361`), `parse_obj` (`transcript.py:397-613`) e tudo que ele chama (`_teammate_textos` 106, `_agent_msg` 138, `_peer_nome` 170, `_peer_msg` 194, `_peer_msg_embrulhado` 218, `_blocked_prompt` 267, `_strip_meta_blocks` 300, `_sub_id` 364, `_ts` 616, `_cache_info` 633, `_tok` 658).
- Produces:
  - `hangar_server::transcript::LineParser` com `fn new(provider: Provider) -> Self` e `fn feed(&mut self, line: &[u8], offset: u64) -> Vec<ChatEvent>` para `Claude` e `ClaudeHeadless` (o braço do Codex entra na Task 8; até lá uma linha do Codex não gera evento).
  - `hangar_server::transcript::SKIPPED_LINES: AtomicU64` (linhas com texto que não viraram objeto JSON).
  - Internos: `claude::RewriteFilter` (`Default`, `fn keep(&mut self, obj: &Map<String, Value>) -> bool`), `claude::parse_obj(obj: &Map<String, Value>) -> Vec<ChatEvent>`, `event(kind: ChatKind, id: String) -> ChatEvent`, `finish(ev: &mut ChatEvent)`, `py::{lstrip, py_re, truthy, int_of, int_trunc, py_str, py_repr, utf8_replace, md5_hex}`.

Regras do porte que valem para todo o arquivo:
- `\s` do Python casa `\x1c`-`\x1f`; `py_re` troca `\s` por `[\s\x1c-\x1f]` em cada padrão. `match` vira `\A…`, `fullmatch` vira `\A…\z`.
- O `regex` não tem retrorreferência, e `_PASTED_RE` (`transcript.py:83`) usa `\1`: o fechamento com o mesmo id é procurado à mão em `strip_pasted`.
- Campo com tipo errado (um `name` numérico, um `text` que é lista) faz o pydantic levantar `ValidationError` no Python e derrubar a leitura da sessão; aqui o campo conta como ausente.
- `_peer_nome` resolve `verifiedPeerPid` pelo tmux (`registry.name_of_pid`); o Rust não tem o tmux e fica com o nome que veio no recado. O golden fixa o Python no mesmo comportamento (o gerador troca `name_of_pid` por `lambda pid: None`).
- O `RewriteFilter` do Python tira o md5 de `json.dumps(…, ensure_ascii=False)` e estoura com surrogate solto (`UnicodeEncodeError` em `transcript.py:342`). Aqui o md5 é do `pyjson::dumps` com `ensure_ascii=True`: duas linhas são iguais nos dois exatamente quando são iguais no outro, e a linha com surrogate segue. A fixture `claude_rewrite_surrogate.jsonl` cobre esse caso.

- [x] **Step 1: Escrever o teste**

```rust
// crates/hangar-server/tests/contract_tail.rs
mod common;

use common::{canon, contract, golden};
use hangar_server::transcript::{LineParser, Provider};

/// (offset, evento) de cada linha, como o leitor ao vivo vai alimentar o parser.
fn tail(fixture: &str, provider: Provider) -> Vec<(u64, String)> {
    let bytes = std::fs::read(contract().join("transcripts").join(fixture)).expect("fixture");
    let mut parser = LineParser::new(provider);
    let mut out = Vec::new();
    let mut start = 0u64;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        for ev in parser.feed(line, start) {
            assert_eq!(ev.offset, Some(start));
            out.push((start, canon(&serde_json::to_value(&ev).unwrap())));
        }
        start += line.len() as u64;
    }
    out
}

fn want(name: &str) -> Vec<(u64, String)> {
    golden(name)
        .as_array()
        .expect("lista")
        .iter()
        .map(|r| (r["offset"].as_u64().expect("offset"), canon(&r["event"])))
        .collect()
}

fn assert_same(got: Vec<(u64, String)>, want: Vec<(u64, String)>, ctx: &str) {
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "{ctx}: evento {i}");
    }
    assert_eq!(got.len(), want.len(), "{ctx}: quantidade de eventos");
}

#[test]
fn claude_with_and_without_terminal_matches_python() {
    for provider in [Provider::Claude, Provider::ClaudeHeadless] {
        assert_same(tail("claude.jsonl", provider), want("claude.tail.json"), provider.as_str());
    }
}

#[test]
fn lone_surrogate_with_timestamp_keeps_reading() {
    // No Python esta linha estoura o md5 do RewriteFilter; o golden traz o que o parse_obj daria.
    assert_same(
        tail("claude_rewrite_surrogate.jsonl", Provider::Claude),
        want("claude_rewrite_surrogate.tail.json"),
        "reescrita com surrogate",
    );
}

#[test]
fn pasted_content_needs_the_same_id_to_close() {
    let text_of = |content: &str| {
        let line = serde_json::json!({"type": "user", "uuid": "u", "message": {"role": "user", "content": content}});
        LineParser::new(Provider::Claude).feed(line.to_string().as_bytes(), 0).pop().and_then(|ev| ev.text)
    };
    let pasted = "antes <pasted_content id=\"a\">\noi\n</pasted_content id=\"a\"> depois";
    assert_eq!(text_of(pasted).as_deref(), Some("antes oi depois"));
    let quoted = "<pasted_content id=\"a\">oi</pasted_content id=\"b\">";
    assert_eq!(text_of(quoted).as_deref(), Some(quoted));
}

#[test]
fn provider_from_python_name() {
    assert_eq!(Provider::parse("claude"), Some(Provider::Claude));
    assert_eq!(Provider::parse("claude-headless"), Some(Provider::ClaudeHeadless));
    assert_eq!(Provider::parse("codex"), Some(Provider::Codex));
    assert_eq!(Provider::parse("pi"), None);
}
```

- [x] **Step 2: Rodar e ver falhar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-server --test contract_tail`
Expected: FAIL de compilação, `unresolved import hangar_server::transcript::LineParser`.

- [x] **Step 3: Dependências**

`crates/Cargo.toml`, em `[workspace.dependencies]` (mesmas versões do `desktop-native/Cargo.lock`, conferido 2026-10-02: `regex` 1.13.1 e `md-5` 0.10.6):

```toml
regex = "=1.13.1"
md-5 = "=0.10.6"
```

`crates/hangar-server/Cargo.toml`, em `[dependencies]`:

```toml
hangar-api.workspace = true
regex.workspace = true
md-5.workspace = true
```

- [x] **Step 4: Helpers do Python em `py.rs`**

Trocar a linha `use serde_json::{Map, Value};` do topo de `py.rs` por:

```rust
use std::borrow::Cow;
use std::fmt::Write;

use md5::{Digest, Md5};
use regex::Regex;
use serde_json::{Map, Value};

use super::pyjson;
```

Acrescentar ao fim de `py.rs`:

```rust
/// `s.encode("utf-8", "replace")`: o surrogate solto vira "?".
pub(crate) fn utf8_replace(s: &str) -> Cow<'_, [u8]> {
    if !has_marker(s) {
        return Cow::Borrowed(s.as_bytes());
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        out.push(if is_marker(c) { '?' } else { c });
    }
    Cow::Owned(out.into_bytes())
}

pub(crate) fn md5_hex(s: &str) -> String {
    format!("{:x}", Md5::digest(&*utf8_replace(s)))
}

pub(crate) fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// Regex com o `\s` do Python, que também casa \x1c-\x1f.
pub(crate) fn py_re(pattern: &str) -> Regex {
    Regex::new(&pattern.replace(r"\s", r"[\s\x1c-\x1f]")).expect("regex do porte")
}

/// Verdade do Python (`bool(x)`); chave ausente é falso.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(m)) => !m.is_empty(),
    }
}

/// `isinstance(v, int)`, com `bool` dentro.
pub(crate) fn int_of(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(i128::from(*b)),
        Value::Number(n) => n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)),
        _ => None,
    }
}

/// `int(v)` de um int ou float: o float é truncado em direção a zero.
pub(crate) fn int_trunc(v: &Value) -> Option<i128> {
    int_of(v).or_else(|| match v {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i128),
        _ => None,
    })
}

/// `str(v)` de um valor vindo do `json.loads`.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

pub(crate) fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => pyjson::number_repr(n),
        Value::String(s) => repr_str(s),
        Value::Array(items) => format!("[{}]", items.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter().map(|(k, x)| format!("{}: {}", repr_str(k), py_repr(x))).collect::<Vec<_>>().join(", ")
        ),
    }
}

// ponytail: `str.isprintable` aproximado (controle, espaço que não é ' ' e os invisíveis comuns);
// tabela de categorias do Unicode se um caso raro aparecer.
fn printable(c: char) -> bool {
    !(c.is_control()
        || (c.is_whitespace() && c != ' ')
        || matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}'))
}

fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_marker(c) => {
                let _ = write!(out, "\\u{:04x}", surrogate_of(c));
            }
            c if !printable(c) => {
                let n = c as u32;
                let _ = if n < 0x100 {
                    write!(out, "\\x{n:02x}")
                } else if n < 0x10000 {
                    write!(out, "\\u{n:04x}")
                } else {
                    write!(out, "\\U{n:08x}")
                };
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
```

- [x] **Step 5: Implementar `claude.rs`**

```rust
// crates/hangar-server/src/transcript/claude.rs
//! Porte de `parse_obj` e `RewriteFilter` (backend/app/transcript.py).
//! Campo com tipo errado, que no Python levanta exceção e derruba a leitura, aqui conta como ausente.

use std::collections::HashMap;
use std::sync::LazyLock;

use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Map, Value};

use super::py::{self, lstrip, py_re, strip};
use super::{event, finish, pyjson};

// transcript.py:40-45
static IMAGE_SOURCE: LazyLock<Regex> = LazyLock::new(|| py_re(r"\A\[Image(?:\]|: [^\]]*\])\z"));
static IMAGE_MARKER: LazyLock<Regex> = LazyLock::new(|| py_re(r"\[Image #\d+\]\s*"));
static INTERRUPTED: LazyLock<Regex> = LazyLock::new(|| py_re(r"\A\[Request interrupted by user[^\]]*\]\z"));
// transcript.py:76, 89, 95, 102-103
static META_BLOCK: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)<system-reminder>.*?</system-reminder>"));
static TASK_NOTIF: LazyLock<Regex> = LazyLock::new(|| py_re(r"<task-id>([^<]+)</task-id>"));
static AGENT_MSG: LazyLock<Regex> =
    LazyLock::new(|| py_re(r#"(?s)\A<agent-message from="([^"]+)"[^>]*>(.*)</agent-message>\z"#));
static TEAMMATE_START: LazyLock<Regex> =
    LazyLock::new(|| py_re(r"\A(?:Another Claude session sent a message:\s*)?<teammate-message\b"));
static TEAMMATE_BLOCK: LazyLock<Regex> =
    LazyLock::new(|| py_re(r"(?s)<teammate-message\b([^>]*)>\n?(.*?)\n?</teammate-message>"));
// transcript.py:163-166
static PEER_WRAP: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r"(?s)\A<cross-session-message\b([^>]*)>\n?(.*?)\n?</cross-session-message>\z")
});
static PEER_ATTR: LazyLock<Regex> = LazyLock::new(|| py_re(r#"([\w-]+)="([^"]*)""#));
static PEER_PREFIX: LazyLock<Regex> = LazyLock::new(|| py_re(r"\A\[(de|grupo|painel):\s*[^\]]+\]"));
// transcript.py:263
static ORIGINAL_PROMPT: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r"(?s)UserPromptSubmit operation blocked by hook:\s*(.*?)\s*Original prompt: (.+)$")
});
// transcript.py:57-72
const COMMAND_META_PREFIXES: [&str; 12] = [
    "<command-name>", "<command-message>", "<command-args>", "<local-command-caveat>",
    "<local-command-stdout>", "<local-command-stderr>", "<bash-input>", "<bash-stdout>", "<bash-stderr>",
    "Base directory for this skill:", "<task-notification>", "<system-reminder>",
];

const WINDOW_S: f64 = 60.0;

/// `RewriteFilter` (transcript.py:315): descarta o que o `claude --resume` regravou.
#[derive(Default)]
pub(crate) struct RewriteFilter {
    max: f64,
    recent: HashMap<(String, String), f64>,
}

impl RewriteFilter {
    pub(crate) fn keep(&mut self, obj: &Map<String, Value>) -> bool {
        if !matches!(obj.get("type").and_then(Value::as_str), Some("user" | "assistant")) {
            return true;
        }
        let Some(t) = obj.get("timestamp").and_then(Value::as_str) else { return true };
        let Some(ts) = py::iso_timestamp(&t.replace('Z', "+00:00")) else { return true };
        if ts < self.max - WINDOW_S {
            return false;
        }
        let content = obj.get("message").and_then(Value::as_object).and_then(|m| m.get("content"));
        // ensure_ascii=True no lugar do False do Python: a igualdade entre duas linhas é a mesma, e o
        // surrogate solto que derruba o md5 do Python aqui passa.
        let fp = (t.to_string(), py::md5_hex(&pyjson::dumps(content.unwrap_or(&Value::Null), true)));
        if self.recent.contains_key(&fp) {
            return false;
        }
        self.recent.insert(fp, ts);
        if ts > self.max {
            self.max = ts;
            if self.recent.len() > 256 {
                self.recent.retain(|_, v| *v >= ts - WINDOW_S);
            }
        }
        true
    }
}

/// `parse_obj` (transcript.py:397), já com o `scrub_surrogates` do `ChatEvent`.
pub(crate) fn parse_obj(obj: &Map<String, Value>) -> Vec<ChatEvent> {
    let mut out = parse_raw(obj);
    out.iter_mut().for_each(finish);
    out
}

fn parse_raw(obj: &Map<String, Value>) -> Vec<ChatEvent> {
    let etype = obj.get("type").and_then(Value::as_str);
    let uid = obj.get("uuid").and_then(Value::as_str).unwrap_or("");
    match etype {
        Some("system") => return system(obj),
        Some("queue-operation") => return queue_operation(obj),
        Some("attachment") => return attachment(obj, uid),
        _ => {}
    }
    let Some(msg) = obj.get("message").and_then(Value::as_object) else { return Vec::new() };
    let content = msg.get("content");
    match (etype, content) {
        (Some("user"), _) => user(obj, uid, content),
        (Some("assistant"), Some(Value::Array(items))) => assistant(obj, msg, uid, items),
        _ => Vec::new(),
    }
}

fn ts(obj: &Map<String, Value>) -> Option<f64> {
    py::ts_of_iso(obj.get("timestamp")?.as_str()?)
}

fn md5_8(s: &str) -> String {
    let mut h = py::md5_hex(s);
    h.truncate(8);
    h
}

fn sub_id(uid: &str, k: usize) -> String {
    if k == 0 { uid.to_string() } else { format!("{uid}:{k}") }
}

fn text_event(kind: ChatKind, id: String, text: String) -> ChatEvent {
    ChatEvent { text: Some(text), ..event(kind, id) }
}

fn task_result(id: String, task: &str) -> ChatEvent {
    ChatEvent {
        tool_use_id: Some(format!("task:{task}")),
        result: Some("task-notification".into()),
        ..event(ChatKind::ToolResult, id)
    }
}

fn first<'a>(items: &'a [Value], type_name: &str) -> Option<&'a Map<String, Value>> {
    items
        .iter()
        .filter_map(Value::as_object)
        .find(|it| it.get("type").and_then(Value::as_str) == Some(type_name))
}

fn is_command_meta(text: &str) -> bool {
    let t = lstrip(text);
    COMMAND_META_PREFIXES.iter().any(|p| t.starts_with(p))
}

/// `_strip_meta_blocks` (transcript.py:300).
fn strip_meta_blocks(text: &str) -> String {
    strip(&strip_pasted(&META_BLOCK.replace_all(text, ""))).to_string()
}

/// `_PASTED_RE.sub(r"\2", …)` (transcript.py:83). O `regex` não tem retrorreferência, então o
/// fechamento com o mesmo id é procurado à mão: o primeiro depois da abertura, sem um "\n" de cada
/// lado do conteúdo.
fn strip_pasted(text: &str) -> String {
    const OPEN: &str = "<pasted_content id=\"";
    let mut out = String::with_capacity(text.len());
    let (mut copied, mut search) = (0, 0);
    while let Some(rel) = text[search..].find(OPEN) {
        let at = search + rel;
        let id_start = at + OPEN.len();
        let Some(id_len) = text[id_start..].find('"') else { break };
        let after = id_start + id_len + 1;
        if !text[after..].starts_with('>') {
            search = at + 1;
            continue;
        }
        let id = &text[id_start..id_start + id_len];
        let mut body = after + 1;
        if text[body..].starts_with('\n') {
            body += 1;
        }
        let close = format!("</pasted_content id=\"{id}\">");
        let Some(rel_close) = text[body..].find(&close) else {
            search = at + 1;
            continue;
        };
        let end = body + rel_close + close.len();
        let mut body_end = body + rel_close;
        if body_end > body && text[..body_end].ends_with('\n') {
            body_end -= 1;
        }
        out.push_str(&text[copied..at]);
        out.push_str(&text[body..body_end]);
        copied = end;
        search = end;
    }
    out.push_str(&text[copied..]);
    out
}

fn attrs(raw: &str) -> HashMap<&str, &str> {
    PEER_ATTR.captures_iter(raw).map(|c| (c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str())).collect()
}

/// `_teammate_textos` (transcript.py:106).
fn teammate_texts(text: Option<&Value>) -> Option<Vec<String>> {
    let t = lstrip(text?.as_str()?);
    if !TEAMMATE_START.is_match(t) {
        return None;
    }
    let blocks: Vec<_> = TEAMMATE_BLOCK.captures_iter(t).collect();
    if blocks.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for b in blocks {
        let body = strip(&b[2]);
        if body.starts_with('{') && matches!(pyjson::loads_lossless(body), Some(Value::Object(_))) {
            continue;
        }
        let name = attrs(&b[1]).get("teammate_id").copied().filter(|n| !n.is_empty()).unwrap_or("colega");
        if !body.is_empty() {
            out.push(format!("[de: {name}] {body}"));
        }
    }
    Some(out)
}

fn teammate_events(text: Option<&Value>, id: &str) -> Option<Vec<ChatEvent>> {
    let texts = teammate_texts(text)?;
    Some(texts.into_iter().enumerate().map(|(k, t)| text_event(ChatKind::UserMsg, sub_id(id, k), t)).collect())
}

/// `_agent_msg` (transcript.py:138).
fn agent_msg(text: Option<&Value>, id: &str) -> Option<Vec<ChatEvent>> {
    let m = AGENT_MSG.captures(strip(text?.as_str()?))?;
    let body = &m[2];
    if body.contains("<agent-message") || body.contains("</agent-message>") {
        return None;
    }
    if !body.contains("[Subagent hand-back]") {
        return Some(Vec::new());
    }
    Some(vec![task_result(id.to_string(), &m[1])])
}

/// `_peer_nome` (transcript.py:170) sem a busca do pid no tmux: fica o nome que veio no recado.
// ponytail: o Python resolve `verifiedPeerPid` pelo tmux (registry.name_of_pid); se o recado
// nativo sem prefixo "[de: …]" ficar comum, pedir o nome ao Python pela conexão interna.
fn peer_name(fallback: Option<&str>) -> String {
    match fallback.map(strip) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "sessão".to_string(),
    }
}

/// `_peer_msg` (transcript.py:194).
fn peer_msg(obj: &Map<String, Value>) -> Option<String> {
    let origin = obj.get("origin")?.as_object()?;
    if origin.get("kind").and_then(Value::as_str) != Some("peer") {
        return None;
    }
    let body = origin.get("body")?.as_str()?;
    if strip(body).is_empty() {
        return None;
    }
    if PEER_PREFIX.is_match(lstrip(body)) {
        return Some(strip(body).to_string());
    }
    Some(format!("[de: {}] {}", peer_name(origin.get("name").and_then(Value::as_str)), strip(body)))
}

/// `_peer_msg_embrulhado` (transcript.py:218).
fn wrapped_peer_msg(text: &str) -> Option<String> {
    let t = strip(text);
    if !t.starts_with("<cross-session-message") || !t.ends_with("</cross-session-message>") {
        return None;
    }
    let m = PEER_WRAP.captures(t)?;
    let body = strip(&m[2]);
    if body.is_empty() || body.contains("<cross-session-message") || body.contains("</cross-session-message>") {
        return None;
    }
    if PEER_PREFIX.is_match(body) {
        return Some(body.to_string());
    }
    Some(format!("[de: {}] {body}", peer_name(attrs(&m[1]).get("from-name").copied())))
}

/// `_blocked_prompt` (transcript.py:267). O aviso de "parece recado" do Python fica de fora: ele
/// levaria texto da conversa ao log.
fn blocked_prompt(content: Option<&Value>) -> Option<(String, String)> {
    let m = ORIGINAL_PROMPT.captures(content?.as_str()?)?;
    let text = strip(&m[2]);
    if text.is_empty() {
        return None;
    }
    Some((wrapped_peer_msg(text).unwrap_or_else(|| text.to_string()), m[1].to_string()))
}

fn system(obj: &Map<String, Value>) -> Vec<ChatEvent> {
    let Some((text, error)) = blocked_prompt(obj.get("content")) else { return Vec::new() };
    vec![ChatEvent {
        ts: ts(obj),
        desistiu: Some(true),
        hook_error: (!error.is_empty()).then_some(error),
        ..text_event(ChatKind::UserMsg, format!("held:{}", md5_8(&text)), text)
    }]
}

fn queue_operation(obj: &Map<String, Value>) -> Vec<ChatEvent> {
    let Some(q) = obj.get("content").and_then(Value::as_str) else { return Vec::new() };
    let queued = obj.get("content");
    let h = md5_8(q);
    if let Some(a) = agent_msg(queued, &format!("queued-agent:{h}")) {
        return a;
    }
    if let Some(c) = teammate_events(queued, &format!("queued-teammate:{h}")) {
        return c;
    }
    if lstrip(q).starts_with("<task-notification>") {
        return match TASK_NOTIF.captures(q) {
            Some(m) => {
                let tid = strip(&m[1]);
                vec![task_result(format!("queued-task:{tid}"), tid)]
            }
            None => Vec::new(),
        };
    }
    if obj.get("operation").and_then(Value::as_str) != Some("remove") {
        return Vec::new();
    }
    let id = format!("queued:{}:{h}", obj.get("timestamp").map_or_else(String::new, py::py_str));
    if let Some(peer) = wrapped_peer_msg(q) {
        return vec![text_event(ChatKind::UserMsg, id, peer)];
    }
    if is_command_meta(q) {
        return Vec::new();
    }
    let cleaned = strip_meta_blocks(q);
    if cleaned.is_empty() || IMAGE_SOURCE.is_match(&cleaned) {
        return Vec::new();
    }
    let cleaned = strip(&IMAGE_MARKER.replace_all(&cleaned, "")).to_string();
    if cleaned.is_empty() {
        return Vec::new();
    }
    vec![text_event(ChatKind::UserMsg, id, cleaned)]
}

fn attachment(obj: &Map<String, Value>, uid: &str) -> Vec<ChatEvent> {
    let Some(att) = obj.get("attachment").and_then(Value::as_object) else { return Vec::new() };
    let atype = att.get("type").and_then(Value::as_str);
    if atype == Some("queued_command") {
        let parts: Vec<&str> = match att.get("prompt") {
            Some(Value::Array(blocks)) => blocks
                .iter()
                .filter_map(Value::as_object)
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
                .collect(),
            _ => Vec::new(),
        };
        let joined = parts.join("\n");
        let text = strip(&joined);
        if text.is_empty() || is_command_meta(text) {
            return Vec::new();
        }
        let text = strip_meta_blocks(text);
        if !text.is_empty() {
            return vec![ChatEvent { ts: ts(obj), ..text_event(ChatKind::UserMsg, uid.into(), text) }];
        }
    }
    if atype == Some("hook_additional_context") && att.get("hookEvent").and_then(Value::as_str) == Some("Stop") {
        let content = att.get("content");
        let text = match content {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
            Some(v) if py::truthy(content) => py::py_str(v),
            _ => String::new(),
        };
        let t = strip(&text);
        if !t.is_empty() {
            return vec![ChatEvent {
                hook_error: Some(t.to_string()),
                ts: ts(obj),
                ..text_event(ChatKind::Notice, uid.into(), "hook_prompt".into())
            }];
        }
    }
    Vec::new()
}

fn user(obj: &Map<String, Value>, uid: &str, content: Option<&Value>) -> Vec<ChatEvent> {
    if let Some(origin) = obj.get("origin").and_then(Value::as_object) {
        if let Some(c) = teammate_events(origin.get("body"), uid) {
            return c;
        }
    }
    if let Some(peer) = peer_msg(obj) {
        return vec![text_event(ChatKind::UserMsg, uid.into(), peer)];
    }
    if obj.get("isCompactSummary") == Some(&Value::Bool(true)) {
        return vec![text_event(ChatKind::Notice, uid.into(), "compacted".into())];
    }
    if obj.get("isMeta") == Some(&Value::Bool(true)) {
        return Vec::new();
    }
    let first_text = match content {
        Some(Value::String(_)) => content,
        Some(Value::Array(items)) => first(items, "text").and_then(|b| b.get("text")),
        _ => None,
    };
    if let Some(a) = agent_msg(first_text, uid) {
        return a;
    }
    if let Some(c) = teammate_events(first_text, uid) {
        return c;
    }
    match content {
        Some(Value::String(s)) => user_text(uid, s),
        Some(Value::Array(items)) => user_blocks(obj, uid, items),
        _ => Vec::new(),
    }
}

fn user_text(uid: &str, s: &str) -> Vec<ChatEvent> {
    if lstrip(s).starts_with("<task-notification>") {
        return match TASK_NOTIF.captures(s) {
            Some(m) => vec![task_result(uid.into(), strip(&m[1]))],
            None => Vec::new(),
        };
    }
    if is_command_meta(s) {
        return Vec::new();
    }
    if INTERRUPTED.is_match(strip(s)) {
        return vec![text_event(ChatKind::Notice, uid.into(), "interrupted".into())];
    }
    let cleaned = strip_meta_blocks(s);
    if cleaned.is_empty() || IMAGE_SOURCE.is_match(&cleaned) {
        return Vec::new();
    }
    vec![text_event(ChatKind::UserMsg, uid.into(), cleaned)]
}

fn user_blocks(obj: &Map<String, Value>, uid: &str, items: &[Value]) -> Vec<ChatEvent> {
    let is_type = |it: &&Map<String, Value>, t: &str| it.get("type").and_then(Value::as_str) == Some(t);
    let trs: Vec<_> = items.iter().filter_map(Value::as_object).filter(|it| is_type(it, "tool_result")).collect();
    if !trs.is_empty() {
        let ts = ts(obj);
        return trs
            .into_iter()
            .enumerate()
            .map(|(k, tr)| {
                let result = match tr.get("content") {
                    Some(Value::Array(blocks)) => Some(
                        blocks
                            .iter()
                            .filter_map(Value::as_object)
                            .map(|b| b.get("text").map_or_else(String::new, py::py_str))
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    None | Some(Value::Null) => None,
                    Some(other) => Some(py::py_str(other)),
                };
                ChatEvent {
                    tool_use_id: tr.get("tool_use_id").and_then(Value::as_str).map(str::to_string),
                    result,
                    is_error: Some(py::truthy(tr.get("is_error"))),
                    ts,
                    ..event(ChatKind::ToolResult, sub_id(uid, k))
                }
            })
            .collect();
    }
    let img_count = items.iter().filter_map(Value::as_object).filter(|it| is_type(it, "image")).count();
    let t = first(items, "text").and_then(|b| b.get("text")).and_then(Value::as_str).unwrap_or("");
    if is_command_meta(t) {
        return Vec::new();
    }
    if INTERRUPTED.is_match(strip(t)) {
        return vec![text_event(ChatKind::Notice, uid.into(), "interrupted".into())];
    }
    let cleaned = strip_meta_blocks(t);
    if IMAGE_SOURCE.is_match(&cleaned) {
        return Vec::new();
    }
    let cleaned = strip(&IMAGE_MARKER.replace_all(&cleaned, "")).to_string();
    if cleaned.is_empty() && img_count == 0 {
        return Vec::new();
    }
    vec![ChatEvent {
        image_count: (img_count > 0).then(|| img_count.try_into().ok()).flatten(),
        ..text_event(ChatKind::UserMsg, uid.into(), cleaned)
    }]
}

/// `_cache_info` (transcript.py:633).
fn cache_info(msg: &Map<String, Value>) -> (Option<u64>, Option<u64>) {
    let Some(usage) = msg.get("usage").and_then(Value::as_object) else { return (None, None) };
    let read = usage.get("cache_read_input_tokens").and_then(py::int_trunc).and_then(|n| u64::try_from(n).ok());
    let creation = usage.get("cache_creation").and_then(Value::as_object);
    // `_tok`: bool não conta como número aqui.
    let tok = |k: &str| creation.and_then(|c| c.get(k)).filter(|v| !v.is_boolean()).and_then(py::int_trunc).unwrap_or(0);
    let ttl = if tok("ephemeral_1h_input_tokens") > 0 {
        Some(3600)
    } else if tok("ephemeral_5m_input_tokens") > 0 {
        Some(300)
    } else {
        None
    };
    (read, ttl)
}

fn assistant(obj: &Map<String, Value>, msg: &Map<String, Value>, uid: &str, items: &[Value]) -> Vec<ChatEvent> {
    let (cache_read, cache_ttl_s) = cache_info(msg);
    let ts = ts(obj);
    let mut out = Vec::new();
    for it in items.iter().filter_map(Value::as_object) {
        let id = sub_id(uid, out.len());
        match it.get("type").and_then(Value::as_str) {
            Some("tool_use") => out.push(ChatEvent {
                tool_name: it.get("name").and_then(Value::as_str).map(str::to_string),
                tool_use_id: it.get("id").and_then(Value::as_str).map(str::to_string),
                tool_input: Some(match it.get("input") {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Map::new(),
                }),
                ts,
                ..event(ChatKind::ToolUse, id)
            }),
            Some("text") => out.push(ChatEvent {
                // `it.get("text", "")`: ausente vira "", null fica null.
                text: match it.get("text") {
                    None => Some(String::new()),
                    Some(v) => v.as_str().map(str::to_string),
                },
                ts,
                cache_read,
                cache_ttl_s,
                ..event(ChatKind::AssistantMsg, id)
            }),
            Some("thinking") => {
                if let Some(p) = it.get("thinking").and_then(Value::as_str).filter(|p| !strip(p).is_empty()) {
                    out.push(ChatEvent { ts, ..text_event(ChatKind::Thinking, id, p.to_string()) });
                }
            }
            _ => {}
        }
    }
    out
}
```

- [x] **Step 6: `LineParser`, `event` e `finish` em `mod.rs`**

`finish` é o `scrub_surrogates` que o validador do `ChatEvent` aplica em todo campo (conferido 2026-10-02: `backend/app/models.py:241-244`); por isso os ids calculados antes dele usam o texto ainda marcado. `mod.rs` inteiro passa a ser:

```rust
// crates/hangar-server/src/transcript/mod.rs
//! Leitura das conversas do Claude e do Codex, portada de backend/app/transcript.py,
//! adapters/codex/rollout.py e pqueue.py. A saída tem que sair igual à do Python: os aparelhos
//! deduplicam por id, e um id diferente na troca vira mensagem repetida na tela.

mod claude;
mod py;
pub mod pyjson;

use std::sync::atomic::{AtomicU64, Ordering};

use hangar_api::chat::{ChatEvent, ChatKind};
use serde_json::Value;

pub use py::ts_of_iso;

/// Linhas com texto que não viraram objeto JSON. O Python pula calado; aqui o log mostra a conta.
pub static SKIPPED_LINES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    ClaudeHeadless,
    Codex,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Provider> {
        match s {
            "claude" => Some(Self::Claude),
            "claude-headless" => Some(Self::ClaudeHeadless),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ClaudeHeadless => "claude-headless",
            Self::Codex => "codex",
        }
    }
}

/// Linha crua do transcript como JSON, com surrogate solto trocado por U+FFFD (`scrub_surrogates`,
/// models.py:22). None em linha em branco ou que não é JSON.
pub fn decode_line(raw: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(raw);
    let mut v = pyjson::loads_lossless(py::strip(&text))?;
    py::scrub_value(&mut v);
    Some(v)
}

/// Um leitor por arquivo: guarda o estado do `RewriteFilter` entre as linhas, como o
/// `TranscriptTailer` (transcript.py:777).
pub struct LineParser {
    provider: Provider,
    rewrite: claude::RewriteFilter,
}

impl LineParser {
    pub fn new(provider: Provider) -> Self {
        Self { provider, rewrite: claude::RewriteFilter::default() }
    }

    /// Eventos de uma linha completa; `offset` é o byte onde ela começa (vira o `id:` do SSE).
    pub fn feed(&mut self, line: &[u8], offset: u64) -> Vec<ChatEvent> {
        let Some(value) = line_value(line) else { return Vec::new() };
        let mut evs = match (self.provider, &value) {
            (Provider::Claude | Provider::ClaudeHeadless, Value::Object(obj)) if self.rewrite.keep(obj) => {
                claude::parse_obj(obj)
            }
            _ => Vec::new(),
        };
        for ev in &mut evs {
            ev.offset = Some(offset);
        }
        evs
    }
}

/// `parse_line` (transcript.py:304): strip e json; linha em branco não conta como pulada.
fn line_value(raw: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(raw);
    let t = py::strip(&text);
    if t.is_empty() {
        return None;
    }
    match pyjson::loads_lossless(t) {
        Some(v @ Value::Object(_)) => Some(v),
        _ => {
            SKIPPED_LINES.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

pub(crate) fn event(kind: ChatKind, id: String) -> ChatEvent {
    ChatEvent { kind, id, ..ChatEvent::default() }
}

/// O `scrub_surrogates` que o validador do `ChatEvent` aplica em todo campo (models.py:241).
pub(crate) fn finish(ev: &mut ChatEvent) {
    py::scrub_str(&mut ev.id);
    for s in [&mut ev.text, &mut ev.tool_name, &mut ev.tool_use_id, &mut ev.result, &mut ev.hook_error]
        .into_iter()
        .flatten()
    {
        py::scrub_str(s);
    }
    for m in [&mut ev.tool_input, &mut ev.skill, &mut ev.orq].into_iter().flatten() {
        py::scrub_map(m);
    }
}
```

- [x] **Step 7: Rodar e ver passar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-server --test contract_tail --test contract_pyjson`
Expected: PASS (4 testes em `contract_tail`, 3 em `contract_pyjson`), sem warning de compilação.

- [x] **Step 8: Commit**

```bash
git add crates/Cargo.toml crates/Cargo.lock crates/hangar-server/Cargo.toml \
  crates/hangar-server/src/transcript/claude.rs crates/hangar-server/src/transcript/py.rs \
  crates/hangar-server/src/transcript/mod.rs crates/hangar-server/tests/contract_tail.rs
git commit -m "feat(server): port the Claude transcript parser and rewrite filter to Rust"
```

---

### Task 8: Parser do Codex em Rust

**Files:**
- Create: `crates/hangar-server/src/transcript/codex.rs`
- Modify: `crates/hangar-server/src/transcript/mod.rs` (declaração do módulo e um braço no `LineParser::feed`)
- Modify: `crates/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `crates/hangar-server/Cargo.toml` (`[dependencies]`)
- Test: `crates/hangar-server/tests/contract_tail.rs` (um teste novo)

**Interfaces:**
- Consumes:
  - Task 6: `pyjson::{dumps, dumps_unicode, loads_lossless}`, `py::{strip, scrub_*}`, golden `codex.tail.json`.
  - Task 7: `event`, `finish`, `py::{py_re, truthy, int_of, py_str}`, `LineParser`.
  - Python portado: `_event_id` (`rollout.py:40`), `_blocks_text` (46), `_command_from_code` (82), `_unescape_js` (99), `_js_string` (111), `_plan_from_code` (117), `_files_from_patch` (137), `_command_output` (151), `_output_result` (177), `_output_text` (208), `parse_rollout_obj` (222), `parse_rollout_line` (336).
- Produces:
  - `LineParser::new(Provider::Codex)` passa a ler rollout do Codex.
  - Internos: `codex::parse_rollout_obj(line: &Value) -> Vec<ChatEvent>`, `codex::event_id(line: &Value) -> String`.

`_event_id` é o sha1 de `json.dumps(obj, sort_keys=True, default=str)` da linha inteira (`rollout.py:43`), e o `default=str` nunca dispara para o que sai do `json.loads`. O Rust usa `pyjson::dumps(line, true)` sobre a linha com o surrogate ainda marcado, e por isso o id da linha `Oi Codex — meio emoji \ud83d aqui` da fixture sai igual.

- [x] **Step 1: Escrever o teste**

Acrescentar em `crates/hangar-server/tests/contract_tail.rs`, logo antes de `fn claude_with_and_without_terminal_matches_python`:

```rust
#[test]
fn codex_matches_python() {
    assert_same(tail("codex.jsonl", Provider::Codex), want("codex.tail.json"), "codex");
}
```

- [x] **Step 2: Rodar e ver falhar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-server --test contract_tail codex_matches_python`
Expected: FAIL em `codex: quantidade de eventos` (o `LineParser` ainda devolve vazio para o Codex: 0 contra 24).

- [x] **Step 3: Dependência**

`crates/Cargo.toml`, em `[workspace.dependencies]` (a única crate de sha1 no `desktop-native/Cargo.lock`, conferido 2026-10-02: `sha1_smol` 1.0.1):

```toml
sha1_smol = "=1.0.1"
```

`crates/hangar-server/Cargo.toml`, em `[dependencies]`:

```toml
sha1_smol.workspace = true
```

- [x] **Step 4: Implementar `codex.rs`**

```rust
// crates/hangar-server/src/transcript/codex.rs
//! Porte de `parse_rollout_obj` e `_event_id` (backend/app/adapters/codex/rollout.py).
//! Campo com tipo errado, que no Python levanta exceção, aqui conta como ausente.

use std::sync::LazyLock;

use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Map, Value};

use super::py::{self, py_re, strip};
use super::{event, finish, pyjson};

// rollout.py:20-33
static CONTEXT_WRAPPER: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r"\A(<(?:environment_context|recommended_plugins|[a-z][a-z_ ]*instructions)>|# AGENTS\.md instructions(?: for |[ \t]*\r?\n\s*<INSTRUCTIONS>))")
});
static TURN_ABORTED: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)\A<turn_aborted>.*</turn_aborted>\z"));
static HOOK_PROMPT: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)\A<hook_prompt\b[^>]*>(.*)</hook_prompt>\z"));
static SKILL: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r"(?s)\A<skill>\s*<name>([^<]*)</name>\s*(?:<path>([^<]*)</path>)?\s*(.*?)\s*</skill>\z")
});
// rollout.py:67-96
static TOOL_IN_CODE: LazyLock<Regex> = LazyLock::new(|| py_re(r"\btools\.(\w+)\s*\("));
static FIRST_STRING: LazyLock<Regex> = LazyLock::new(|| py_re(r#""((?:[^"\\]|\\.)*)""#));
static STEP: LazyLock<Regex> = LazyLock::new(|| py_re(r#"\bstep\s*:\s*"((?:[^"\\]|\\.)*)""#));
static STATUS: LazyLock<Regex> = LazyLock::new(|| py_re(r#"\bstatus\s*:\s*"(\w+)""#));
static CMD: LazyLock<Regex> = LazyLock::new(|| py_re(r#"\bcmd"?\s*:\s*"((?:[^"\\]|\\.)*)""#));
static PATCH_FILE: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?m)^\*{3} (?:Add|Update|Delete) File: (.+)$"));
// rollout.py:147
static SCRIPT_HEADER: LazyLock<Regex> =
    LazyLock::new(|| py_re(r"\AScript (completed|failed)\nWall time [\d.]+ seconds\nOutput:\n"));

/// `_event_id` (rollout.py:40): sha1 do `json.dumps(obj, sort_keys=True)` da linha inteira.
pub(crate) fn event_id(line: &Value) -> String {
    sha1_smol::Sha1::from(pyjson::dumps(line, true)).digest().to_string()
}

/// `parse_rollout_obj` (rollout.py:222), já com o `scrub_surrogates` do `ChatEvent`.
pub(crate) fn parse_rollout_obj(line: &Value) -> Vec<ChatEvent> {
    let mut out = parse_raw(line);
    out.iter_mut().for_each(finish);
    out
}

fn parse_raw(line: &Value) -> Vec<ChatEvent> {
    let Some(obj) = line.as_object() else { return Vec::new() };
    if obj.get("type").and_then(Value::as_str) != Some("response_item") {
        return Vec::new();
    }
    let Some(payload) = obj.get("payload").and_then(Value::as_object) else { return Vec::new() };
    let id = || event_id(line);
    match payload.get("type").and_then(Value::as_str) {
        Some("message") => message(payload, id),
        Some("function_call") => {
            let args = payload.get("arguments");
            // `json.loads(arguments or "{}")`, e o que não for objeto vira {}.
            let tool_input = match args {
                Some(Value::String(s)) if py::truthy(args) => match pyjson::loads_lossless(s) {
                    Some(Value::Object(m)) => m,
                    _ => Map::new(),
                },
                _ => Map::new(),
            };
            vec![ChatEvent {
                tool_name: str_field(payload, "name"),
                tool_use_id: str_field(payload, "call_id"),
                tool_input: Some(tool_input),
                ..event(ChatKind::ToolUse, id())
            }]
        }
        Some("custom_tool_call") => vec![custom_tool_call(payload, id())],
        Some("custom_tool_call_output" | "function_call_output") => {
            let (result, failed) = output_result(payload.get("output"));
            vec![ChatEvent {
                tool_use_id: str_field(payload, "call_id"),
                result,
                is_error: Some(failed),
                ..event(ChatKind::ToolResult, id())
            }]
        }
        _ => Vec::new(),
    }
}

fn str_field(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k).and_then(Value::as_str).map(str::to_string)
}

fn text_event(kind: ChatKind, id: String, text: String) -> ChatEvent {
    ChatEvent { text: Some(text), ..event(kind, id) }
}

/// `_blocks_text` (rollout.py:46).
fn blocks_text(content: Option<&Value>, block_type: &str) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(Value::as_object)
            .filter(|b| b.get("type").and_then(Value::as_str) == Some(block_type))
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect(),
        _ => String::new(),
    }
}

fn message(payload: &Map<String, Value>, id: impl Fn() -> String) -> Vec<ChatEvent> {
    match payload.get("role").and_then(Value::as_str) {
        Some("user") => {
            let text = blocks_text(payload.get("content"), "input_text");
            let t = strip(&text);
            if CONTEXT_WRAPPER.is_match(t) {
                return Vec::new();
            }
            if TURN_ABORTED.is_match(t) {
                return vec![text_event(ChatKind::Notice, id(), "turn_aborted".into())];
            }
            if let Some(h) = HOOK_PROMPT.captures(t) {
                return vec![ChatEvent {
                    hook_error: Some(strip(&h[1]).to_string()),
                    ..text_event(ChatKind::Notice, id(), "hook_prompt".into())
                }];
            }
            if let Some(s) = SKILL.captures(t) {
                let path = s.get(2).map_or("", |m| strip(m.as_str()));
                let mut skill = Map::new();
                skill.insert("name".into(), strip(&s[1]).into());
                skill.insert("path".into(), if path.is_empty() { Value::Null } else { path.into() });
                skill.insert("body".into(), s[3].into());
                return vec![ChatEvent { skill: Some(skill), ..text_event(ChatKind::Notice, id(), "skill_loaded".into()) }];
            }
            vec![text_event(ChatKind::UserMsg, id(), text)]
        }
        Some("assistant") => {
            vec![text_event(ChatKind::AssistantMsg, id(), blocks_text(payload.get("content"), "output_text"))]
        }
        _ => Vec::new(),
    }
}

/// `_unescape_js` (rollout.py:99).
fn unescape_js(raw: &str) -> String {
    match pyjson::loads_lossless(&format!("\"{raw}\"")) {
        Some(Value::String(s)) => s,
        _ => raw.to_string(),
    }
}

fn js_string(text: &str) -> String {
    FIRST_STRING.captures(text).map(|m| unescape_js(&m[1])).unwrap_or_default()
}

fn command_from_code(code: &str) -> String {
    CMD.captures(code).map(|m| unescape_js(&m[1])).unwrap_or_default()
}

/// `_plan_from_code` (rollout.py:117): o status de cada passo é o primeiro antes do passo seguinte.
fn plan_from_code(code: &str) -> Vec<Value> {
    let steps: Vec<_> = STEP.captures_iter(code).collect();
    steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let end = steps.get(i + 1).map_or(code.len(), |n| n.get(0).unwrap().start());
            let status = STATUS
                .captures_at(&code[..end], step.get(0).unwrap().end())
                .map_or_else(|| "pending".to_string(), |m| m[1].to_string());
            let mut item = Map::new();
            item.insert("step".into(), unescape_js(&step[1]).into());
            item.insert("status".into(), status.into());
            Value::Object(item)
        })
        .collect()
}

/// Ramo `custom_tool_call` (rollout.py:273-322).
fn custom_tool_call(payload: &Map<String, Value>, id: String) -> ChatEvent {
    let code = payload.get("input").and_then(Value::as_str).unwrap_or("");
    let mut name = str_field(payload, "name");
    let mut tool_input = Map::new();
    tool_input.insert("code".into(), code.into());
    let calls: Vec<_> =
        if name.as_deref() == Some("exec") { TOOL_IN_CODE.captures_iter(code).collect() } else { Vec::new() };
    let inner = if calls.len() == 1 { calls.first() } else { None };
    if let Some(c) = inner {
        name = Some(c[1].to_string());
    }
    let patch = match (name.as_deref(), inner) {
        (Some("apply_patch"), Some(c)) => js_string(&code[c.get(0).unwrap().end()..]),
        (Some("apply_patch"), None) => code.to_string(),
        _ => String::new(),
    };
    if name.as_deref() == Some("update_plan") {
        let plan = plan_from_code(code);
        if !plan.is_empty() {
            tool_input.insert("plan".into(), Value::Array(plan));
        }
    } else if !patch.is_empty() {
        let files: Vec<Value> = PATCH_FILE.captures_iter(&patch).map(|m| m[1].into()).collect();
        tool_input.insert("patch".into(), patch.into());
        if !files.is_empty() {
            tool_input.insert("file_path".into(), Value::Array(files));
        }
    } else if calls.len() > 1 {
        let cmds = CMD.captures_iter(code).map(|m| unescape_js(&m[1])).collect::<Vec<_>>().join("\n");
        let command = if cmds.is_empty() {
            let mut names: Vec<&str> = Vec::new();
            for c in &calls {
                let n = c.get(1).unwrap().as_str();
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            names.join(", ")
        } else {
            cmds
        };
        tool_input.insert("command".into(), command.into());
    } else {
        let command = command_from_code(code);
        if !command.is_empty() {
            tool_input.insert("command".into(), command.into());
        }
    }
    ChatEvent {
        tool_name: name,
        tool_use_id: str_field(payload, "call_id"),
        tool_input: Some(tool_input),
        ..event(ChatKind::ToolUse, id)
    }
}

/// `_command_output` (rollout.py:151).
fn command_output(value: &Value) -> Option<(String, bool)> {
    match value {
        Value::Array(items) => {
            let parts = items.iter().map(command_output).collect::<Option<Vec<_>>>()?;
            if parts.is_empty() {
                return None;
            }
            let failed = parts.iter().any(|p| p.1);
            Some((parts.into_iter().map(|p| p.0).collect::<Vec<_>>().join("\n\n"), failed))
        }
        Value::Object(m) => {
            let status = m.get("status").and_then(Value::as_str);
            let only = |a: &str, b: &str| m.len() == 2 && m.contains_key(a) && m.contains_key(b);
            if status == Some("fulfilled") && only("status", "value") {
                return command_output(&m["value"]);
            }
            if status == Some("rejected") && only("status", "reason") {
                let reason = &m["reason"];
                return Some((reason.as_str().map_or_else(|| pyjson::dumps_unicode(reason, false), str::to_string), true));
            }
            let Some(Value::String(output)) = m.get("output") else { return None };
            if !(m.contains_key("chunk_id") && m.contains_key("wall_time_seconds")) {
                return None;
            }
            let failed = m.get("exit_code").and_then(py::int_of).is_some_and(|c| c != 0);
            let text = if failed {
                format!("exit_code: {}\n{output}", py::py_str(&m["exit_code"]))
            } else if let Some(sid) = m.get("session_id").filter(|v| !v.is_null()) {
                format!("session_id: {}\n{output}", py::py_str(sid))
            } else {
                output.clone()
            };
            Some((text, failed))
        }
        _ => None,
    }
}

/// `_output_text` (rollout.py:208).
fn output_text(output: Option<&Value>) -> Option<String> {
    match output? {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        v @ Value::Array(_) => Some(blocks_text(Some(v), "input_text")),
        other => Some(py::py_str(other)),
    }
}

/// `_output_result` (rollout.py:177).
fn output_result(output: Option<&Value>) -> (Option<String>, bool) {
    let Some(raw) = output_text(output) else { return (None, false) };
    let Some(header) = SCRIPT_HEADER.captures(&raw) else { return (Some(raw), false) };
    let mut failed = &header[1] == "failed";
    let mut blocks: Vec<String> = match output {
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(Value::as_object)
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("input_text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_string))
            .collect(),
        _ => vec![raw.clone()],
    };
    // O Python corta pelo índice de caractere do cabeçalho no texto juntado.
    let skip = raw[..header.get(0).unwrap().end()].chars().count();
    if let Some(first) = blocks.first_mut() {
        *first = first.chars().skip(skip).collect();
    }
    let mut parts = Vec::new();
    for block in blocks.into_iter().filter(|b| !b.is_empty()) {
        match pyjson::loads_lossless(&block).as_ref().and_then(command_output) {
            Some((text, f)) => {
                parts.push(text);
                failed = failed || f;
            }
            None => parts.push(block),
        }
    }
    (Some(parts.join("\n\n")), failed)
}
```

- [x] **Step 5: Ligar o Codex no `LineParser`**

Em `mod.rs`, trocar `mod claude;` por:

```rust
mod claude;
mod codex;
```

E, em `LineParser::feed`, acrescentar o braço do Codex como primeiro do `match`:

```rust
        let mut evs = match (self.provider, &value) {
            (Provider::Codex, _) => codex::parse_rollout_obj(&value),
            (Provider::Claude | Provider::ClaudeHeadless, Value::Object(obj)) if self.rewrite.keep(obj) => {
                claude::parse_obj(obj)
            }
            _ => Vec::new(),
        };
```

- [x] **Step 6: Rodar e ver passar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-server --test contract_tail --test contract_pyjson`
Expected: PASS (5 testes em `contract_tail`, 3 em `contract_pyjson`), sem warning.

- [x] **Step 7: Commit**

```bash
git add crates/Cargo.toml crates/Cargo.lock crates/hangar-server/Cargo.toml \
  crates/hangar-server/src/transcript/codex.rs crates/hangar-server/src/transcript/mod.rs \
  crates/hangar-server/tests/contract_tail.rs
git commit -m "feat(server): port the Codex rollout parser and event ids to Rust"
```

---

### Task 9: Histórico em Rust e a rota `info` no Python

**Files:**
- Create: `crates/hangar-server/src/transcript/history.rs`
- Modify: `crates/hangar-server/src/transcript/py.rs` (dois helpers no fim)
- Modify: `crates/hangar-server/src/transcript/claude.rs` (statics do anexo e `silent_attachment_timestamp`)
- Modify: `crates/hangar-server/src/transcript/mod.rs` (declaração e reexport)
- Modify: `crates/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `crates/hangar-server/Cargo.toml` (`[dependencies]` e `[dev-dependencies]`)
- Test: `crates/hangar-server/tests/contract_history.rs`
- Create: `backend/app/internal_api.py`
- Modify: `backend/app/api.py:29` (import) e `backend/app/api.py:637` (montar o router)
- Test: `backend/tests/test_internal_api.py`

**Interfaces:**
- Consumes:
  - Tasks 6-8: `py::*`, `pyjson::*`, `claude::{RewriteFilter, parse_obj}`, `codex::parse_rollout_obj`, `event`, `finish`, `SKIPPED_LINES`, golden `claude.history.json` e `codex.history.json`.
  - Python portado: `merged_history` (`pqueue.py:977-1140`), `_tail_offset` (961), `_ts_of_obj` (227), `_transcript_start_ts` (258), `_chaves_de_commit` (301), `_strip_attach` (288), `_da_sessao_atual` (171), `_saida_local` (203), `_entry_event` (210), `PromptQueue.load` (875), `silent_attachment_timestamp` (`transcript.py:381`), o corte `evs[-limit:]` da rota (`api.py:2918-2919`).
  - Python, para a rota: `_cached_info` (`api.py:1011`), `chave_de` (`backend/app/adapters/__init__.py:23`), `PromptQueue(name).path` (`pqueue.py:554`), `session_key` (`models.py:53`).
- Produces:
  - `hangar_server::transcript::InternalInfo { provider: String, jsonl: Option<PathBuf>, session_key: String, history: Value }` (`Deserialize`, `Clone`, `Debug`) com `fn history_request(&self, limit: Option<usize>) -> Option<HistoryRequest>`.
  - `hangar_server::transcript::HistoryRequest { provider: Provider, jsonl: PathBuf, queue: Option<PathBuf>, limit: Option<usize>, tail_window: u64 }` (`Clone`, `Debug`).
  - `hangar_server::transcript::merged_history(req: &HistoryRequest) -> std::io::Result<Vec<ChatEvent>>` — já com o corte da rota; transcript ausente devolve `Ok(vec![])`.
  - `hangar_server::transcript::history_etag(req: &HistoryRequest) -> Option<String>` — formato próprio `"rs-<tamanho>.<mtime_ns>-<fila>-<provider>-<limit>-<marca do binário>"`, entre aspas.
  - `hangar_server::transcript::TAIL_WINDOW: u64` (256 KB).
  - Python `app.internal_api`: `set_secret(value: str | None) -> None` (o segredo mora na memória do módulo, nunca no `os.environ`), `require_internal(request: Request) -> None`, `info_payload(name: str, provider: str, jsonl: str | None) -> dict` (aplica `chave_de`), `router = APIRouter(prefix="/internal", include_in_schema=False)`, `GET /internal/sessions/{name}/info` → `info_payload(...)` = `{"provider": str, "jsonl": str | None, "session_key": str, "history": {"queue": str}}`. A frente 4 acrescenta `side-events` no mesmo arquivo e reusa `info_payload` no evento `info`.
  - `LineParser: Send` e `HistoryRequest: Send + 'static` (atravessam `spawn_blocking` na Task 12), presos por teste de compilação.

O que o Rust precisa do Python além do `jsonl` é só o caminho do sidecar da fila: o `merged_history` recebe `name` apenas para montar `PromptQueue(name)` (conferido 2026-10-02: `backend/app/pqueue.py:1108`), e o início da sessão sai do próprio transcript. O `provider` do `info` é a chave do adapter (`chave_de`), que distingue o Claude sem terminal; para o histórico os dois Claude são lidos igual, como a rota faz hoje passando `info.provider` (conferido 2026-10-02: `backend/app/api.py:2912`).

O segredo da conexão interna fica numa variável do módulo (`set_secret`), e não no `os.environ`: tudo que o backend sobe (sessões, agentes, hooks) herda o ambiente dele, e o segredo vazaria para cada sessão. Quem o gera e chama `set_secret` é a Task 13, antes de cada subida do `hangar-server`.

A rota fica fora de `/api/`, então o `GuestUserGate` não a toca (conferido 2026-10-02: `backend/app/guest_user_gate.py:92`), e na porta do convidado o `ShareGate` recusa quem não tem convite (conferido 2026-10-02: `backend/app/share_gate.py:228-240`). O uvicorn confia no `X-Forwarded-For` vindo de `127.0.0.1` (conferido 2026-10-02: `backend/app/main.py:231-232`): quem chega de fora pelo repasse do `hangar-server` aparece com o IP real e cai no teste de loopback antes do segredo.

- [x] **Step 1: Escrever o teste do Rust**

```rust
// crates/hangar-server/tests/contract_history.rs
mod common;

use std::path::Path;

use common::{canon, contract, golden};
use hangar_server::transcript::{
    history_etag, merged_history, HistoryRequest, InternalInfo, LineParser, Provider, TAIL_WINDOW,
};

const VARIANTS: [(&str, Option<usize>, u64); 6] = [
    ("full", None, TAIL_WINDOW),
    ("limit2", Some(2), TAIL_WINDOW),
    ("limit200", Some(200), TAIL_WINDOW),
    ("limit2_w512", Some(2), 512),
    ("limit9_w512", Some(9), 512),
    ("limit200_w512", Some(200), 512),
];

fn check_history(fixture: &str, provider: Provider, queue: &str) {
    let g = golden(&format!("{}.history.json", fixture.trim_end_matches(".jsonl")));
    for (name, limit, window) in VARIANTS {
        for (suffix, session) in [("", "sem-queue"), ("+queue", queue)] {
            let req = HistoryRequest {
                provider,
                jsonl: contract().join("transcripts").join(fixture),
                queue: Some(contract().join("queue").join(format!("{session}.jsonl"))),
                limit,
                tail_window: window,
            };
            let key = format!("{name}{suffix}");
            let got: Vec<String> =
                merged_history(&req).unwrap().iter().map(|ev| canon(&serde_json::to_value(ev).unwrap())).collect();
            let want: Vec<String> =
                g[&key].as_array().unwrap_or_else(|| panic!("golden sem {key}")).iter().map(canon).collect();
            for (i, (a, b)) in got.iter().zip(&want).enumerate() {
                assert_eq!(a, b, "{fixture} {key}: evento {i}");
            }
            assert_eq!(got.len(), want.len(), "{fixture} {key}: quantidade");
        }
    }
}

#[test]
fn claude_matches_python() {
    check_history("claude.jsonl", Provider::Claude, "claude-fixture");
}

#[test]
fn codex_matches_python() {
    check_history("codex.jsonl", Provider::Codex, "codex-fixture");
}

fn request(dir: &Path, limit: Option<usize>) -> HistoryRequest {
    HistoryRequest {
        provider: Provider::Claude,
        jsonl: dir.join("s.jsonl"),
        queue: Some(dir.join("queue.jsonl")),
        limit,
        tail_window: TAIL_WINDOW,
    }
}

#[test]
fn no_transcript_means_empty_history_and_no_etag() {
    let dir = tempfile::tempdir().unwrap();
    assert!(merged_history(&request(dir.path(), None)).unwrap().is_empty());
    assert_eq!(history_etag(&request(dir.path(), None)), None);
}

#[test]
fn etag_changes_with_queue_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("s.jsonl"), "{\"type\": \"user\"}\n").unwrap();
    let before = history_etag(&request(dir.path(), None)).unwrap();
    assert!(before.starts_with('"') && before.ends_with('"'));
    assert_eq!(history_etag(&request(dir.path(), None)).unwrap(), before);
    std::fs::write(dir.path().join("queue.jsonl"), "{\"id\": \"a\", \"text\": \"oi\"}\n").unwrap();
    let with_queue = history_etag(&request(dir.path(), None)).unwrap();
    assert_ne!(with_queue, before);
    assert_ne!(history_etag(&request(dir.path(), Some(30))).unwrap(), with_queue);
}

#[test]
fn internal_info_becomes_history_request() {
    let info: InternalInfo = serde_json::from_value(serde_json::json!({
        "provider": "claude-headless", "jsonl": "/x/a.jsonl", "session_key": "a",
        "history": {"queue": "/q/s.jsonl"},
    }))
    .unwrap();
    let req = info.history_request(Some(0)).unwrap();
    assert_eq!(req.provider, Provider::ClaudeHeadless);
    assert_eq!(req.jsonl, Path::new("/x/a.jsonl"));
    assert_eq!(req.queue.as_deref(), Some(Path::new("/q/s.jsonl")));
    assert_eq!((req.limit, req.tail_window), (None, TAIL_WINDOW));
    assert_eq!(info.history_request(Some(30)).unwrap().limit, Some(30));
    let pi: InternalInfo =
        serde_json::from_value(serde_json::json!({"provider": "pi", "jsonl": "/x/a.jsonl", "session_key": "a", "history": {}}))
            .unwrap();
    assert!(pi.history_request(None).is_none());
    let fresh: InternalInfo =
        serde_json::from_value(serde_json::json!({"provider": "codex", "jsonl": null, "session_key": "", "history": {}}))
            .unwrap();
    assert!(fresh.history_request(None).is_none());
}

// A Task 12 manda os dois para `spawn_blocking`: se deixarem de ser Send, quebra aqui e não lá.
#[test]
fn parser_and_request_cross_threads() {
    fn _assert_send<T: Send + 'static>() {}
    _assert_send::<LineParser>();
    _assert_send::<HistoryRequest>();
}
```

- [x] **Step 2: Rodar e ver falhar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-server --test contract_history`
Expected: FAIL de compilação, `unresolved imports hangar_server::transcript::history_etag, merged_history, HistoryRequest, InternalInfo, TAIL_WINDOW`.

- [x] **Step 3: Dependências**

`crates/Cargo.toml`, em `[workspace.dependencies]` (versão do `desktop-native/Cargo.lock`, conferido 2026-10-02: `tempfile` 3.27.0):

```toml
tempfile = "=3.27.0"
```

`crates/hangar-server/Cargo.toml`: em `[dependencies]` acrescentar `serde.workspace = true` (o `derive` do `InternalInfo`), e no fim do arquivo a seção nova (a Task 11 acrescenta o `reqwest` nela):

```toml
[dev-dependencies]
tempfile.workspace = true
```

- [x] **Step 4: Helpers em `py.rs` e o relógio do anexo em `claude.rs`**

Acrescentar ao fim de `py.rs`:

```rust
/// `isinstance(v, (int, float))`, que no Python inclui `bool`.
pub(crate) fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// `str.splitlines()`: além de \n e \r, quebra em \v, \f, \x1c-\x1e, \x85, U+2028 e U+2029.
pub(crate) fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|&(_, n)| n == '\n') {
                chars.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}
```

Em `claude.rs`, trocar a primeira linha por:

```rust
//! Porte de `parse_obj`, `RewriteFilter` e `silent_attachment_timestamp` (backend/app/transcript.py).
```

Acrescentar logo depois do `static ORIGINAL_PROMPT` (antes de `// transcript.py:57-72`):

```rust
// transcript.py:375-378
static ATTACHMENT_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r#"\A\{"parentUuid":(?:null|"[^"\\]*"),"isSidechain":(?:true|false),"attachment":\{"type":"([^"\\]*)""#)
});
const ATTACHMENT_TAIL: &str = r#"},"type":"attachment","uuid":""#;
static ATTACHMENT_TAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| py_re(r#"\A\},"type":"attachment","uuid":"[^"\\]*","timestamp":"([^"\\]*)""#));
```

E logo antes da função `parse_obj` (com o comentário `///` dela):

```rust
/// `silent_attachment_timestamp` (transcript.py:381): relógio de um anexo que nunca vira bolha,
/// lido sem json.
pub(crate) fn silent_attachment_timestamp(line: &str) -> Option<&str> {
    if !line.ends_with("}\n") {
        return None;
    }
    let head = ATTACHMENT_HEAD.captures(line)?;
    if matches!(&head[1], "queued_command" | "hook_additional_context") {
        return None;
    }
    let i = line.rfind(ATTACHMENT_TAIL)?;
    Some(ATTACHMENT_TAIL_RE.captures(&line[i..])?.get(1)?.as_str())
}
```

- [x] **Step 5: Implementar `history.rs`**

Regras portadas, todas de `pqueue.py`: janela de 256 KB que quadruplica até juntar `limit` eventos ou chegar ao início (1096-1103); `start_ts` lido do começo do arquivo só quando a janela não começa nele (1032); anexo sem bolha conta só o relógio (1067); `RewriteFilter` novo a cada janela, só no Claude (1040); `held:` repetido entra uma vez (1048); fila com confirmados pulados exceto saídas locais (1113-1118), absorvidos pelo texto ou pela legenda commitada depois do envio (1126-1129), podados antes do início da sessão com a folga do bastão (1135, 168); desempate `10**9` e ordenação estável por `(ts, i)` (1115, 1139). O `splitlines` do Python também quebra em U+2028 e U+0085, e uma entrada da fila com esse caractere cru some; o Rust repete isso para sair igual.

```rust
// crates/hangar-server/src/transcript/history.rs
//! Porte de `pqueue.merged_history` (backend/app/pqueue.py:977): transcript + fila de pendentes,
//! ordenados pelo relógio, com o corte por `limit` que a rota `/history` faz (api.py:2918).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Map, Value};

use super::py::{self, py_re, strip};
use super::{claude, codex, event, finish, pyjson, Provider, SKIPPED_LINES};

/// Janela inicial da leitura de trás para frente (pqueue.py:958).
pub const TAIL_WINDOW: u64 = 256 * 1024;
// pqueue.py:168
const HANDOFF_GRACE_S: f64 = 15.0 * 60.0;
// pqueue.py:1115: a fila cai depois dos eventos do transcript de mesmo relógio.
const QUEUE_ORDER: u64 = 1_000_000_000;

// pqueue.py:285, 294, 298
static ATTACH: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)(?:\s*—\s*)?📎\s*(?:imagem|arquivo):.*$"));
static IMG_PREFIX: LazyLock<Regex> = LazyLock::new(|| py_re(r"\A(?:\[Image #\d+\])+\s*"));
static IMG_SOURCE: LazyLock<Regex> = LazyLock::new(|| py_re(r"\[Image: source: ([^\]]+)\]"));

/// O que o Python devolve em `GET /internal/sessions/{name}/info`.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct InternalInfo {
    pub provider: String,
    #[serde(default)]
    pub jsonl: Option<PathBuf>,
    #[serde(default)]
    pub session_key: String,
    /// `{"queue": "<sidecar da fila>"}`.
    #[serde(default)]
    pub history: Value,
}

#[derive(Clone, Debug)]
pub struct HistoryRequest {
    pub provider: Provider,
    pub jsonl: PathBuf,
    /// Sidecar da fila (`PromptQueue(name).path`); ausente = sessão sem fila.
    pub queue: Option<PathBuf>,
    /// `None` = histórico inteiro, como `limit` ausente ou `<= 0` no Python.
    pub limit: Option<usize>,
    /// Janela inicial da leitura de trás para frente; a rota usa `TAIL_WINDOW`.
    pub tail_window: u64,
}

impl InternalInfo {
    /// None quando o provider não é lido pelo Rust ou a sessão ainda não tem transcript.
    pub fn history_request(&self, limit: Option<usize>) -> Option<HistoryRequest> {
        Some(HistoryRequest {
            provider: Provider::parse(&self.provider)?,
            jsonl: self.jsonl.clone()?,
            queue: self.history.get("queue").and_then(Value::as_str).map(PathBuf::from),
            limit: limit.filter(|&n| n > 0),
            tail_window: TAIL_WINDOW,
        })
    }
}

struct Parsed {
    items: Vec<(f64, u64, ChatEvent)>,
    committed: HashMap<String, f64>,
    prev_ts: f64,
    start_ts: f64,
}

/// `merged_history` + o corte `evs[-limit:]` da rota. Transcript ausente dá histórico vazio.
pub fn merged_history(req: &HistoryRequest) -> io::Result<Vec<ChatEvent>> {
    let mut parsed = match req.limit {
        Some(limit) => {
            let mut window = req.tail_window.max(1);
            loop {
                let off = tail_offset(&req.jsonl, window);
                let parsed = parse_from(req, off)?;
                if off == 0 || parsed.items.len() >= limit {
                    break parsed;
                }
                window = window.saturating_mul(4);
            }
        }
        None => parse_from(req, 0)?,
    };
    if let Some(queue) = &req.queue {
        merge_queue(queue, &mut parsed)?;
    }
    let mut items = parsed.items;
    // Estável como o sort do Python: empate em (ts, i) mantém a ordem de chegada.
    items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
    let mut evs: Vec<ChatEvent> = items.into_iter().map(|(_, _, ev)| ev).collect();
    if let Some(limit) = req.limit {
        if evs.len() > limit {
            evs.drain(..evs.len() - limit);
        }
    }
    Ok(evs)
}

/// `_tail_offset` (pqueue.py:961): primeira linha completa dentro dos últimos `window` bytes.
fn tail_offset(path: &Path, window: u64) -> u64 {
    let read = || -> io::Result<u64> {
        let mut f = File::open(path)?;
        let size = f.metadata()?.len();
        if size <= window {
            return Ok(0);
        }
        f.seek(SeekFrom::Start(size - window))?;
        let mut partial = Vec::new();
        let n = BufReader::new(f).read_until(b'\n', &mut partial)?;
        Ok(size - window + n as u64)
    };
    read().unwrap_or(0)
}

/// `_ts_of_obj` (pqueue.py:227).
fn ts_of_obj(obj: &Map<String, Value>) -> f64 {
    if let Some(t) = obj.get("timestamp").and_then(Value::as_str) {
        return py::iso_timestamp(&t.replace('Z', "+00:00")).unwrap_or(0.0);
    }
    if let Some(n) = obj.get("message").and_then(Value::as_object).and_then(|m| m.get("timestamp")).and_then(py::number) {
        return n / 1000.0;
    }
    ["time", "created_at"].iter().find_map(|k| obj.get(*k).and_then(py::number)).map_or(0.0, |n| n / 1000.0)
}

/// `_transcript_start_ts` (pqueue.py:258) com o `or 0.0` de quem só poda.
fn transcript_start_ts(path: &Path) -> f64 {
    let Ok(f) = File::open(path) else { return 0.0 };
    let mut rd = BufReader::new(f);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match rd.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => return 0.0,
            Ok(_) => {}
        }
        if let Some(Value::Object(obj)) = pyjson::loads_lossless(&String::from_utf8_lossy(&raw)) {
            let ts = ts_of_obj(&obj);
            if ts > 0.0 {
                return ts;
            }
        }
    }
}

/// `_parse_from` (pqueue.py:1021).
fn parse_from(req: &HistoryRequest, offset: u64) -> io::Result<Parsed> {
    let mut p = Parsed {
        items: Vec::new(),
        committed: HashMap::new(),
        prev_ts: 0.0,
        start_ts: if offset > 0 { transcript_start_ts(&req.jsonl) } else { 0.0 },
    };
    let Ok(file) = File::open(&req.jsonl) else { return Ok(p) };
    let mut rd = BufReader::new(file);
    rd.seek(SeekFrom::Start(offset))?;
    // Reescrita do `--resume`: só o Claude regrava o jsonl.
    let mut rewrite = (req.provider != Provider::Codex).then(claude::RewriteFilter::default);
    let mut held_ids = HashSet::new();
    let mut raw = Vec::new();
    let mut i = 0u64;
    loop {
        raw.clear();
        if rd.read_until(b'\n', &mut raw)? == 0 {
            break;
        }
        let idx = i;
        i += 1;
        // O Python lê em modo texto: "\r\n" chega como "\n".
        let mut line = String::from_utf8_lossy(&raw).into_owned();
        if line.ends_with("\r\n") {
            line.truncate(line.len() - 2);
            line.push('\n');
        }
        let silent = rewrite.as_ref().and_then(|_| claude::silent_attachment_timestamp(&line));
        let (line_ts, evs) = match silent {
            Some(att) => (py::iso_timestamp(&att.replace('Z', "+00:00")).unwrap_or(0.0), Vec::new()),
            None => {
                let Some(value) = pyjson::loads_lossless(&line) else {
                    if !strip(&line).is_empty() {
                        SKIPPED_LINES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    continue;
                };
                let Value::Object(obj) = &value else {
                    SKIPPED_LINES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    continue;
                };
                if let Some(rw) = rewrite.as_mut() {
                    if !rw.keep(obj) {
                        continue;
                    }
                }
                let evs = if rewrite.is_some() { claude::parse_obj(obj) } else { codex::parse_rollout_obj(&value) };
                (ts_of_obj(obj), evs)
            }
        };
        if line_ts > 0.0 {
            if p.start_ts == 0.0 {
                p.start_ts = line_ts;
            }
            p.prev_ts = line_ts;
        }
        if evs.is_empty() {
            continue;
        }
        let ts = if line_ts != 0.0 { line_ts } else { p.prev_ts };
        p.prev_ts = ts;
        absorb(&mut p, ts, idx, evs, &mut held_ids);
    }
    Ok(p)
}

/// `_absorve` (pqueue.py:1044).
fn absorb(p: &mut Parsed, ts: f64, i: u64, evs: Vec<ChatEvent>, held_ids: &mut HashSet<String>) {
    for ev in evs {
        // Cada reenvio de um prompt barrado repete o id "held:"; a lista leva um só.
        if ev.id.starts_with("held:") && !held_ids.insert(ev.id.clone()) {
            continue;
        }
        let ets = ev.ts.filter(|t| *t != 0.0).unwrap_or(ts);
        if matches!(ev.kind, ChatKind::UserMsg) {
            if let Some(text) = ev.text.as_deref().filter(|t| !t.is_empty()) {
                for ln in chaves_de_commit(text) {
                    if ets > p.committed.get(&ln).copied().unwrap_or(0.0) {
                        p.committed.insert(ln, ets);
                    }
                }
            }
        }
        p.items.push((ets, i, ev));
    }
}

fn strip_attach(text: &str) -> String {
    ATTACH.replace(text, "").into_owned()
}

/// `_chaves_de_commit` (pqueue.py:301). Repetição não importa: quem usa só compara o relógio.
fn chaves_de_commit(text: &str) -> Vec<String> {
    let t = strip(text);
    let base = IMG_PREFIX.replace(t, "").into_owned();
    let fonte = IMG_SOURCE.replace_all(t, |c: &regex::Captures| format!("📎 imagem: {}", &c[1])).into_owned();
    let mut out = Vec::new();
    for variant in [t.to_string(), base.clone(), strip_attach(t), strip_attach(&base), fonte] {
        let v = strip(&variant);
        if v.is_empty() {
            continue;
        }
        out.push(v.to_string());
        out.extend(v.split('\n').map(strip).filter(|ln| !ln.is_empty()).map(str::to_string));
    }
    out
}

/// `PromptQueue.load` (pqueue.py:875): `splitlines` do Python, linha que não é objeto JSON pula.
fn load_queue(path: &Path) -> io::Result<Vec<Map<String, Value>>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(py::splitlines(&text)
        .into_iter()
        .map(strip)
        .filter(|l| !l.is_empty())
        .filter_map(|l| match pyjson::loads_lossless(l) {
            Some(Value::Object(m)) => Some(m),
            _ => None,
        })
        .collect())
}

fn is_local_output(entry: &Map<String, Value>) -> bool {
    entry.get("papel").and_then(Value::as_str) == Some("assistant")
}

/// `_da_sessao_atual` (pqueue.py:171) com o relógio já resolvido.
fn from_current_session(entry: &Map<String, Value>, min_ts: f64, ts: f64) -> bool {
    let grace = if py::truthy(entry.get("pre_transcript")) { HANDOFF_GRACE_S } else { 0.0 };
    ts >= min_ts - grace
}

/// `_entry_event` (pqueue.py:210).
fn entry_event(entry: &Map<String, Value>) -> ChatEvent {
    let id = entry.get("id").map_or_else(|| "None".to_string(), py::py_str);
    let text = entry.get("text").and_then(Value::as_str).map(str::to_string);
    let mut ev = if is_local_output(entry) {
        ChatEvent { text, ..event(ChatKind::AssistantMsg, format!("local-{id}")) }
    } else {
        ChatEvent {
            text,
            queued_delivered: entry.get("delivered").and_then(Value::as_bool),
            desistiu: py::truthy(entry.get("desistiu")).then_some(true),
            queued_ts: entry.get("ts").and_then(py::number),
            ..event(ChatKind::UserMsg, format!("queued-{id}"))
        }
    };
    finish(&mut ev);
    ev
}

/// Junção da fila (pqueue.py:1107-1137).
fn merge_queue(path: &Path, p: &mut Parsed) -> io::Result<()> {
    for entry in load_queue(path)? {
        let Some(text) = entry.get("text").and_then(Value::as_str).map(strip) else { continue };
        if text.is_empty() {
            continue;
        }
        // `float(entry.get("ts") or prev_ts)`.
        // ponytail: texto que o float() do Python não lê derrubaria a rota; aqui vira prev_ts.
        let ts = match entry.get("ts") {
            Some(Value::String(s)) if !s.is_empty() => strip(s).parse().unwrap_or(p.prev_ts),
            v if py::truthy(v) => v.and_then(py::number).unwrap_or(p.prev_ts),
            _ => p.prev_ts,
        };
        if is_local_output(&entry) {
            if p.start_ts == 0.0 || from_current_session(&entry, p.start_ts, ts) {
                p.items.push((ts, QUEUE_ORDER, entry_event(&entry)));
            }
            continue;
        }
        if py::truthy(entry.get("confirmed")) {
            continue;
        }
        let cap = strip(&strip_attach(text)).to_string();
        let committed = |k: &str| p.committed.get(k).copied().unwrap_or(-1.0);
        let committed_at = committed(text).max(if cap.is_empty() { -1.0 } else { committed(&cap) });
        if committed_at >= ts {
            continue;
        }
        if p.start_ts != 0.0 && !from_current_session(&entry, p.start_ts, ts) {
            continue;
        }
        p.items.push((ts, QUEUE_ORDER, entry_event(&entry)));
    }
    Ok(())
}

fn stamp(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{}.{mtime}", meta.len()))
}

// Os mesmos bytes rendem outra resposta quando o parser muda: o binário novo invalida o ETag.
static CODE_MARK: LazyLock<u128> = LazyLock::new(|| {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
});

/// Validador do `/history` pelos metadados do transcript e da fila (o papel de `historico_etag`,
/// pqueue.py:1155). Formato próprio: o cliente só devolve o valor. Sem transcript, sem validador.
pub fn history_etag(req: &HistoryRequest) -> Option<String> {
    let transcript = stamp(&req.jsonl)?;
    let queue_stamp = req.queue.as_deref().and_then(stamp).unwrap_or_else(|| "-".into());
    let limit = req.limit.map_or_else(|| "None".to_string(), |n| n.to_string());
    Some(format!("\"rs-{transcript}-{queue_stamp}-{}-{limit}-{}\"", req.provider.as_str(), *CODE_MARK))
}
```

- [x] **Step 6: Declarar o módulo**

Em `mod.rs`, trocar `mod codex;` por:

```rust
mod codex;
mod history;
```

E trocar `pub use py::ts_of_iso;` por:

```rust
pub use history::{history_etag, merged_history, HistoryRequest, InternalInfo, TAIL_WINDOW};
pub use py::ts_of_iso;
```

- [x] **Step 7: Rodar e ver passar** (quando autorizado)

Run: `cd crates && cargo test -p hangar-server`
Expected: PASS (`contract_history` 6, `contract_tail` 5, `contract_pyjson` 3), sem warning.

- [x] **Step 8: Escrever o teste da rota `info`**

```python
# backend/tests/test_internal_api.py
from unittest.mock import AsyncMock, patch

import pytest
from fastapi.testclient import TestClient

import app.api as api_mod
from app import internal_api, pqueue
from app.models import SessionInfo

SECRET = "ab" * 32
ROUTE = "/internal/sessions/s1/info"


@pytest.fixture(autouse=True)
def _env(monkeypatch, tmp_path):
    internal_api.set_secret(SECRET)
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    yield
    internal_api.set_secret(None)


def _client(ip="127.0.0.1"):
    return TestClient(api_mod.app, client=(ip, 50000))


def _info(**kw):
    return SessionInfo(**{"name": "s1", "cwd": "/p", "jsonl": "/p/abc-123.jsonl", "provider": "claude", **kw})


def _get(client, headers=None, info=None):
    with patch("app.api._cached_info", AsyncMock(return_value=info or _info())):
        return client.get(ROUTE, headers=headers if headers is not None else {"X-Hangar-Internal": SECRET})


def test_info_has_what_hangar_server_needs(tmp_path):
    with patch("app.adapters.chave_de", lambda name, provider: "claude-headless"):
        r = _get(_client())
    assert r.status_code == 200
    assert r.json() == {"provider": "claude-headless", "jsonl": "/p/abc-123.jsonl", "session_key": "abc-123",
                        "history": {"queue": str(tmp_path / "s1.jsonl")}}


def test_codex_session_key_is_the_rollout_id():
    info = _info(provider="codex",
                 jsonl="/c/rollout-2026-09-02T14-00-00-019f0000-0000-7000-8000-000000000001.jsonl")
    r = _get(_client(), info=info)
    assert r.json()["provider"] == "codex"
    assert r.json()["session_key"] == "019f0000-0000-7000-8000-000000000001"


def test_session_without_transcript_yet():
    r = _get(_client(), info=_info(jsonl=None))
    assert r.status_code == 200
    assert (r.json()["jsonl"], r.json()["session_key"]) == (None, "")


def test_unknown_session_404():
    with patch("app.api._cached_info", AsyncMock(return_value=None)):
        r = _client().get(ROUTE, headers={"X-Hangar-Internal": SECRET})
    assert r.status_code == 404


@pytest.mark.parametrize("headers", [{}, {"X-Hangar-Internal": "errado"}, {"X-Hangar-Internal": ""}])
def test_wrong_secret_404(headers):
    assert _get(_client(), headers=headers).status_code == 404


def test_no_secret_404():
    internal_api.set_secret(None)
    assert _get(_client(), headers={"X-Hangar-Internal": ""}).status_code == 404


def test_secret_never_goes_to_environ(monkeypatch):
    monkeypatch.delenv("HANGAR_INTERNAL_SECRET", raising=False)
    internal_api.set_secret(SECRET)
    import os
    assert "HANGAR_INTERNAL_SECRET" not in os.environ
    assert _get(_client()).status_code == 200


def test_info_payload_is_what_the_route_returns(tmp_path):
    with patch("app.adapters.chave_de", lambda name, provider: "claude-headless"):
        assert internal_api.info_payload("s1", "claude", "/p/abc-123.jsonl") == _get(_client()).json()


def test_outside_loopback_404_even_with_secret():
    assert _get(_client("10.0.0.7")).status_code == 404


def test_left_out_of_the_api_schema():
    paths = api_mod.app.openapi()["paths"]
    assert "/api/sessions/{name}/history" in paths
    assert not any(p.startswith("/internal") for p in paths)
```

- [x] **Step 9: Rodar e ver falhar** (quando autorizado)

Run: `cd backend && uv run pytest tests/test_internal_api.py -v`
Expected: FAIL na coleta, `ImportError: cannot import name 'internal_api' from 'app'` (o módulo ainda não existe).

- [x] **Step 10: Implementar a rota**

```python
# backend/app/internal_api.py
"""Rotas internas que só o hangar-server, filho deste backend na mesma máquina, consome."""
import secrets

from fastapi import APIRouter, Depends, HTTPException, Request

from app.models import session_key

_LOOPBACK = {"127.0.0.1", "::1"}

# Só na memória: no os.environ ele iria para toda sessão que o backend sobe.
_secret: str | None = None


def set_secret(value: str | None) -> None:
    global _secret
    _secret = value or None


def require_internal(request: Request) -> None:
    # 404 em toda recusa: quem não é o hangar-server não descobre que a rota existe. O segredo nasce
    # a cada subida e o repasse do hangar-server manda o IP real no X-Forwarded-For, então quem
    # chega de fora pela porta pública cai no IP antes do segredo.
    secret = _secret
    ip = request.client.host if request.client else None
    given = request.headers.get("x-hangar-internal", "")
    if secret is None or ip not in _LOOPBACK or not secrets.compare_digest(given.encode(), secret.encode()):
        raise HTTPException(status_code=404)


def info_payload(name: str, provider: str, jsonl: str | None) -> dict:
    """O `InternalInfo` do Rust: a rota `info` e o evento `info` do side-events usam este mesmo."""
    from app.adapters import chave_de
    from app.pqueue import PromptQueue

    return {
        # Chave do adapter: o Claude sem terminal vem como "claude-headless".
        "provider": chave_de(name, provider),
        "jsonl": jsonl,
        "session_key": session_key(jsonl) if jsonl else "",
        # Tudo que o merged_history do Rust precisa além do transcript.
        "history": {"queue": str(PromptQueue(name).path)},
    }


router = APIRouter(prefix="/internal", dependencies=[Depends(require_internal)], include_in_schema=False)


@router.get("/sessions/{name}/info")
async def session_info(name: str) -> dict:
    # Import tardio: api.py importa este módulo no topo.
    from app import api

    info = await api._cached_info(name)
    if info is None:
        raise HTTPException(status_code=404)
    return info_payload(name, info.provider, info.jsonl)
```

Em `backend/app/api.py:29`, trocar `from app import external_pair_api, external_pairs` por:

```python
from app import external_pair_api, external_pairs, internal_api
```

E logo depois de `app.include_router(external_pair_api.router)` (`api.py:637`):

```python
app.include_router(internal_api.router)
```

O 404 sai sem `erro(code, msg)`: a rota não é lida por tela nenhuma, e um código novo exigiria chave em `messages/*.json` à toa.

- [x] **Step 11: Rodar e ver passar** (quando autorizado)

Run: `cd backend && uv run pytest tests/test_internal_api.py -v`
Expected: PASS, 12 testes.

- [x] **Step 12: Commit**

```bash
git add crates/Cargo.toml crates/Cargo.lock crates/hangar-server/Cargo.toml \
  crates/hangar-server/src/transcript/history.rs crates/hangar-server/src/transcript/py.rs \
  crates/hangar-server/src/transcript/claude.rs crates/hangar-server/src/transcript/mod.rs \
  crates/hangar-server/tests/contract_history.rs \
  backend/app/internal_api.py backend/app/api.py backend/tests/test_internal_api.py
git commit -m "feat(server): merged history in Rust and internal session info route"
```

---

#### Notas (dependências e riscos)

**Desvios e acréscimos ao contrato**
- `LineParser` nasce na Task 7 só com Claude/Claude sem terminal; o braço do Codex entra na Task 8. Entre as duas, `LineParser::new(Provider::Codex)` não gera evento.
- Acréscimos públicos em `transcript`: `pyjson::{dumps_unicode, loads_lossless, float_repr}`, `ts_of_iso`, `SKIPPED_LINES`, `TAIL_WINDOW`, `Provider::as_str`.
- `HistoryRequest { provider, jsonl, queue: Option<PathBuf>, limit: Option<usize>, tail_window: u64 }`. `merged_history` já devolve com o corte `evs[-limit:]` da rota. `history_request(Some(0))` vira histórico inteiro, como `limit <= 0` no Python (`pqueue.py:1096`, `api.py:2918`). A rota da frente 4 deve ler `limit` como inteiro com sinal e mandar `None` quando for `<= 0`: o Python aceita `limit=-5` como "sem limite", e um `usize` no axum responderia 400.
- `InternalInfo.history = {"queue": "<caminho do sidecar>"}`. `provider` é o `chave_de` (vem `claude-headless` para Claude sem terminal). `session_key` é `""` quando ainda não há `jsonl`. O dicionário sai de `info_payload(name, provider, jsonl)`, que a Task 10 reusa no evento `info`.
- O segredo interno mora em `internal_api._secret` (`set_secret`), não no `os.environ` como o brief previa: no ambiente ele vazaria para toda sessão que o backend sobe.
- `InternalInfo` tem `#[serde(default)]` em `jsonl`, `session_key` e `history`.
- O ETag do Rust tem formato próprio (`"rs-…"`) e nunca casa com o do Python, então a troca de servidor não serve 304 velho. A marca do código é o mtime do binário.
- A impressão digital do `RewriteFilter` usa `ensure_ascii=True` (o Python usa `False`). A comparação dá o mesmo resultado, e o md5 não sai do processo.

**Dependências entre Tasks**
- Task 6 depende da Task 1 (workspace e `serde_json` com `preserve_order`). Task 7 depende da Task 1 (`ChatEvent: Default`, `ChatKind` com as variantes da parte 1).
- 6 → 7 → 8 → 9 em série: as quatro mexem em `transcript/mod.rs`, `py.rs` e `Cargo.toml`, e 7/8 em `contract_tail.rs`.
- A frente 4 acrescenta `side-events` em `backend/app/internal_api.py` depois da Task 9. As frentes 4 e 5 também mexem em `crates/hangar-server/Cargo.toml`, `src/lib.rs` e `backend/app/api.py`; conferir conflito de linha nesses três.
- O gerador roda no Linux/macOS (`time.tzset`). O golden vai commitado e o CI só lê.

**Bugs do Python que o porte expôs (fora do escopo, não corrigidos)**
- Surrogate solto numa mensagem `user`/`assistant` do Claude com `timestamp`: `RewriteFilter.keep` levanta `UnicodeEncodeError` (`transcript.py:342`). O tail da sessão morre e o `/history` dá 500 (`pqueue.py:1074`). O Rust segue lendo.
- Linha JSON que não é objeto (`5`, `[]`) levanta `AttributeError` em `keep`/`parse_obj`/`_ts_of_obj`, com o mesmo efeito. O Rust pula e conta em `SKIPPED_LINES`.
- `PromptQueue.load` usa `splitlines` e perde a entrada cujo texto tem U+2028/U+0085 cru (`pqueue.py:879`). O Rust repete o comportamento para sair igual; a correção é nos dois lados.

**Diferenças aceitas (o golden não cobre)**
- Inteiro acima de `u64` vira `f64`, e `NaN`/`Infinity` não são lidos (o Python aceita os dois). Aninhamento acima de 128 níveis não é lido (limite do `serde_json`). `-0` inteiro é lido como `-0.0`. A saída é ligar `arbitrary_precision` se aparecer num transcript, mas esse recurso tem problema conhecido com `#[serde(flatten)]` e enum `untagged`.
- Relógio sem fuso: o Python lê como hora local no `RewriteFilter` e no `merged_history`, e o Rust lê como UTC. Claude e Codex sempre gravam o fuso. Datas por semana ISO (`2026-W40-5`) não são lidas.
- `\w` do Unicode difere em detalhes entre `re` e `regex`, e o `repr()` usa um `isprintable` aproximado. Os dois só aparecem em nome de ferramenta do Codex e em resultado que não é texto.
- `\r` solto no meio de uma linha: o modo texto do Python quebra a linha ali, e o Rust só trata `\r\n`.
- Um caractere real entre U+10F800 e U+10FFFF (área privada) seria lido como surrogate.
- Recado nativo sem prefixo `[de: …]`: o Rust mostra o título que veio no recado (`origin.name`/`from-name`), e o Python mostra o nome tmux do remetente.
- O aviso "entrada system parece recado" do `_blocked_prompt` não foi portado, porque levaria texto da conversa ao log.

**Riscos e pontos para a frente 4**
- `float_roundtrip` vale para o workspace inteiro (unificação de features). O efeito é só a leitura de float um pouco mais lenta.
- Os testes do Rust leem `../../backend/tests/fixtures/contract` a partir de `CARGO_MANIFEST_DIR`, então o CI do `server.yml` precisa do checkout inteiro, não só de `crates/`.

**Conferido antes de entregar o plano**
- Numa cópia descartável fora do repositório, o gerador rodou contra os parsers Python atuais.
- Também na cópia, `cargo test` (1.98.1, `--offline`) passou em cada etapa (fim das Tasks 6, 7, 8 e 9), sem warning de compilação.
- O `test_internal_api.py` passou numa cópia do `backend/` com as duas linhas de `api.py` aplicadas, junto com `test_share_api.py` e `test_guest_user_gate.py`.
- Mutações conferidas: o limiar do `float_repr`, o `\n` antes do fechamento do `pasted_content` e o `?` do md5 com surrogate fazem os testes falharem.
- O `clippy` não estava instalado e não rodou.

### Task 10: Conexão interna no Python (`side-events`)

**Files:**
- Modify: `backend/app/sse.py` — assinatura de `merged_events` (linhas 672-673), `tail_pump` (linhas 809-810), abertura do `try` (linhas 991-992), os dois `yield {"event": "reset", ...}` (linhas 1028 e 1063); helper novo `_info_event` logo depois de `_confirm_codex_queue` (linha 669)
- Modify: `backend/app/internal_api.py` — acrescenta a rota `GET /internal/sessions/{name}/side-events` (o arquivo nasce na Task 9)
- Test: `backend/tests/test_internal_side_events.py`

**Interfaces:**
- Consumes:
  - `app.internal_api.require_internal` (Task 9) — dependência FastAPI: só loopback + `X-Hangar-Internal`; sem segredo definido → 404. `app.internal_api.set_secret(value: str | None) -> None` (Task 9): o segredo mora na memória do módulo, nunca no `os.environ`.
  - `app.internal_api.info_payload(name: str, provider: str, jsonl: str | None) -> dict` (Task 9) — o MESMO dicionário que a rota `GET /internal/sessions/{name}/info` devolve (`provider`, `jsonl`, `session_key`, `history`), com o provider já passado por `chave_de`.
  - `app.sse.merged_events`, `app.api.registry.list`, `app.mensagens.erro`, `sse_starlette.sse.EventSourceResponse` (existentes).
- Produces:
  - `merged_events(name: str, jsonl: str, provider: str = "claude", start_offset: int | None = None, count_app: bool = True, side: bool = False)` — com `side=False` o comportamento do `/events` atual não muda em nada.
  - `app.sse._info_event(name: str, provider: str, jsonl: str | None) -> dict` → `{"event": "info", "data": "<json do info_payload>"}`.
  - Rota `GET /internal/sessions/{name}/side-events?app=0|1` (fora do catálogo) → SSE, na ordem: `info`; depois `state`, `suggest`, `ask_question`, `stats`, `preview`, `pensamento`, `ferramenta`, `nav`, `message`/`queue_confirmed` da fila e `ping`; `info` de novo a cada troca de transcript ou de provider (no lugar do `reset`). Nunca emite `message` do transcript.

A escolha de desenho: o modo lateral é um parâmetro de `merged_events`, não uma segunda função. Tudo que o `side-events` precisa (monitor compartilhado pelo `Difusor`, prévia com supressão do que já foi gravado, pensamento, ferramenta, fila, `nav`, `drain` na transição para entregável, `_confirm_codex_queue`, `plugin_bridge.app_entrou/app_saiu`, regras do `jsonl_watcher`) já mora nas 500 linhas de estado local de `merged_events` (sse.py:672-1173); copiar seria duas cópias para manter em par. O que muda com `side=True` são só três pontos: abre com `info`, troca `reset` por `info`, e o `tail_pump` segue o arquivo sem emitir. O `tail_pump` continua rodando para TODO provider, não só para o Codex: é ele que mantém `committed["text"]` (sse.py:795-798), sem o qual a prévia de um bloco já gravado voltaria a aparecer duplicada (sse.py:726-732, 910); e é ele que chama `_confirm_codex_queue` a cada `user_msg` do Codex (sse.py:793-794). O custo é um leitor Python por sessão, não por aparelho.

- [x] **Step 1: Escrever os testes**

```python
# backend/tests/test_internal_side_events.py
"""Conexão interna do hangar-server: as fontes compartilhadas de uma sessão, sem o transcript."""
import asyncio
import json

import pytest

from app import pqueue, plugin_bridge, sse
from app.adapters.preview_push import PushPreviewSource
from app.models import ChatEvent, SessionInfo, StateEvent, session_key

_TEXTO = "resposta já gravada no arquivo"


class _Adapter:
    provider = "claude"

    def __init__(self):
        self.drains = []

    async def _transcript(self, path, start_offset=None):
        yield ChatEvent(kind="assistant_msg", id="a1", text=_TEXTO)
        await asyncio.Event().wait()

    async def _estados(self):
        yield StateEvent(session="s", state="idle")
        await asyncio.Event().wait()

    def transcript_stream(self, path, start_offset=None):
        return self._transcript(path, start_offset)

    def state_monitor(self, name, sid_get, **kw):
        return self._estados()

    async def drain(self, name, path):
        self.drains.append((name, path))
        return 0


@pytest.fixture
def fila(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    return tmp_path


async def _coleta(gen, ate, limite=5.0):
    vistos = []
    try:
        async with asyncio.timeout(limite):
            async for ev in gen:
                vistos.append(ev)
                if ate(vistos):
                    return vistos
    finally:
        await gen.aclose()
    return vistos


async def test_side_comeca_por_info_nao_manda_transcript_e_ainda_drena(tmp_path, fila, monkeypatch):
    adapter = _Adapter()
    monkeypatch.setattr(sse, "get_adapter", lambda provider: adapter)
    jsonl = tmp_path / "abc123.jsonl"
    jsonl.write_text("")
    gen = sse.merged_events("lado-a", str(jsonl), side=True)
    vistos = await _coleta(gen, lambda v: any(e["event"] == "state" for e in v))
    assert vistos[0]["event"] == "info"
    info = json.loads(vistos[0]["data"])
    assert info["provider"] == "claude"
    assert info["jsonl"] == str(jsonl)
    assert info["session_key"] == session_key(str(jsonl))
    assert not any(e["event"] == "message" for e in vistos)
    await asyncio.sleep(0.05)   # o drain é fire-and-forget
    assert adapter.drains == [("lado-a", str(jsonl))]


async def test_side_ainda_suprime_previa_ja_gravada(tmp_path, fila, monkeypatch):
    # Sem o tail_pump no modo lateral, a prévia ficaria com o texto que já está no arquivo.
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    nome = "lado-previa"
    await PushPreviewSource.get(nome).push(_TEXTO)
    jsonl = tmp_path / "p.jsonl"
    jsonl.write_text("")
    gen = sse.merged_events(nome, str(jsonl), provider="codex", side=True)
    vistos = await _coleta(gen, lambda v: any(
        e["event"] == "preview" and json.loads(e["data"])["text"] == "" for e in v))
    assert any(e["event"] == "preview" and json.loads(e["data"])["text"] == "" for e in vistos)


async def test_side_troca_de_provider_emite_info_e_nunca_reset(tmp_path, fila, monkeypatch):
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    velho = tmp_path / "velho.jsonl"
    velho.write_text("")
    novo = tmp_path / "rollout-2026-10-02T10-00-00-0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000.jsonl"
    novo.write_text("")

    async def lista():
        return [SessionInfo(name="lado-troca", jsonl=str(novo), provider="codex")]

    monkeypatch.setattr(sse, "_cached_list", lista)
    gen = sse.merged_events("lado-troca", str(velho), side=True)
    vistos = await _coleta(gen, lambda v: sum(e["event"] == "info" for e in v) >= 2)
    infos = [json.loads(e["data"]) for e in vistos if e["event"] == "info"]
    assert infos[1]["provider"] == "codex"
    assert infos[1]["jsonl"] == str(novo)
    assert not any(e["event"] == "reset" for e in vistos)


async def test_side_repassa_a_fila(tmp_path, fila, monkeypatch):
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    entry = pqueue.PromptQueue("lado-fila").append("manda isso depois", delivered=False)
    jsonl = tmp_path / "f.jsonl"
    jsonl.write_text("")
    gen = sse.merged_events("lado-fila", str(jsonl), side=True)
    vistos = await _coleta(gen, lambda v: any(e["event"] == "message" for e in v))
    msg = next(json.loads(e["data"]) for e in vistos if e["event"] == "message")
    assert msg["id"] == f"queued-{entry['id']}"


async def test_rota_side_events_conta_app_so_com_app_1_e_recusa_sessao_inexistente(
        tmp_path, fila, monkeypatch):
    from fastapi import HTTPException
    from app import api, internal_api
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    jsonl = tmp_path / "r.jsonl"
    jsonl.write_text("")
    monkeypatch.setattr(api.registry, "list",
                        lambda: [SessionInfo(name="lado-rota", jsonl=str(jsonl), provider="claude")])
    with pytest.raises(HTTPException) as exc:
        await internal_api.side_events("outra", app=1)
    assert exc.value.status_code == 404

    antes = plugin_bridge._apps_abertos
    gen = (await internal_api.side_events("lado-rota", app=1)).body_iterator
    assert (await anext(gen))["event"] == "info"
    assert plugin_bridge._apps_abertos == antes + 1
    await gen.aclose()
    assert plugin_bridge._apps_abertos == antes

    gen0 = (await internal_api.side_events("lado-rota", app=0)).body_iterator
    assert (await anext(gen0))["event"] == "info"
    assert plugin_bridge._apps_abertos == antes
    await gen0.aclose()


def test_rota_side_events_exige_conexao_interna(monkeypatch):
    from fastapi.testclient import TestClient
    from app import api, internal_api
    internal_api.set_secret(None)
    r = TestClient(api.app).get("/internal/sessions/qualquer/side-events")
    assert r.status_code == 404
```

- [x] **Step 2: Rodar e ver falhar** (quando autorizado)

Run: `(cd backend && uv run pytest tests/test_internal_side_events.py -v)`
Expected: FAIL com `TypeError: merged_events() got an unexpected keyword argument 'side'` nos testes de `merged_events` e `AttributeError: module 'app.internal_api' has no attribute 'side_events'` no teste da rota.

- [x] **Step 3: Implementar o modo lateral em `sse.py`**

Helper novo, logo depois de `_confirm_codex_queue` (sse.py:661-669):

```python
def _info_event(name: str, provider: str, jsonl: str | None) -> dict:
    """`info` da conexão interna: o mesmo JSON da rota /internal/sessions/{name}/info."""
    from app.internal_api import info_payload   # internal_api importa este módulo
    return {"event": "info",
            "data": json.dumps(info_payload(name, provider, jsonl), ensure_ascii=False)}
```

Assinatura (sse.py:672-673), de:

```python
async def merged_events(name: str, jsonl: str, provider: str = "claude",
                        start_offset: int | None = None, count_app: bool = True):
```

para:

```python
async def merged_events(name: str, jsonl: str, provider: str = "claude",
                        start_offset: int | None = None, count_app: bool = True,
                        side: bool = False):
    # side=True: conexão interna do hangar-server (internal_api.side_events). Abre com `info` e o
    # repete no lugar do `reset`. A conversa o hangar-server lê do arquivo; aqui o transcript só é
    # seguido para a supressão da prévia e a baixa da fila do Codex.
```

No `tail_pump` (sse.py:809-810), de:

```python
                ev_id = f"{session_key(path)}:{ev.offset}" if ev.offset is not None else None
                await queue.put(("message", ev.model_dump_json(), ev_id))
```

para:

```python
                if side:
                    continue
                ev_id = f"{session_key(path)}:{ev.offset}" if ev.offset is not None else None
                await queue.put(("message", ev.model_dump_json(), ev_id))
```

Abertura do laço (sse.py:991-992), de:

```python
    try:
        while True:
```

para:

```python
    try:
        if side:
            # Dentro do try: quem desconecta já aqui ainda passa pelo finally (tarefas, app_saiu).
            yield _info_event(name, current_provider, current_jsonl)
        while True:
```

No ramo `__reprovider__` (sse.py:1027-1029), de:

```python
                tasks += [tail_task, stats_task, state_task, preview_task]
                yield {"event": "reset", "data": "{}"}
                continue
```

para:

```python
                tasks += [tail_task, stats_task, state_task, preview_task]
                yield (_info_event(name, current_provider, current_jsonl) if side
                       else {"event": "reset", "data": "{}"})
                continue
```

No ramo `__reset__` (sse.py:1062-1064), de:

```python
                tasks.append(state_task)
                yield {"event": "reset", "data": "{}"}
                continue
```

para:

```python
                tasks.append(state_task)
                yield (_info_event(name, current_provider, current_jsonl) if side
                       else {"event": "reset", "data": "{}"})
                continue
```

- [x] **Step 4: Implementar a rota em `internal_api.py`**

Imports no topo do arquivo (acrescentar os que a Task 9 ainda não trouxe):

```python
import asyncio

from fastapi import Depends, HTTPException
from sse_starlette.sse import EventSourceResponse

from app.mensagens import erro
from app.sse import merged_events
```

Rota, no fim do arquivo:

```python
@router.get("/sessions/{name}/side-events", dependencies=[Depends(require_internal)],
            include_in_schema=False)
async def side_events(name: str, app: int = 0):
    """Uma conexão por sessão para o hangar-server, que reparte estado, prévia, perguntas e fila
    entre os aparelhos. `app=1`: há ao menos um aparelho do dono no chat (push fica calado)."""
    from app import api as _api   # api monta este roteador
    sessions = await asyncio.to_thread(_api.registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente",
                                             "session or transcript not found"))
    return EventSourceResponse(
        merged_events(name, info.jsonl, provider=info.provider, count_app=bool(app), side=True),
        send_timeout=30)
```

A resolução da sessão é a mesma do `/events` (api.py:3349-3350, `registry.list` fresco, não o cache), e o `send_timeout=30` também (api.py:3379-3382).

- [x] **Step 5: Rodar e ver passar** (quando autorizado)

Run: `(cd backend && uv run pytest tests/test_internal_side_events.py tests/test_sse.py tests/test_codex_queue_confirmation.py -v)`
Expected: PASS em todos; os de `test_sse.py` e `test_codex_queue_confirmation.py` provam que o `/events` com `side=False` não mudou.

- [x] **Step 6: Commit**

```bash
git add backend/app/sse.py backend/app/internal_api.py backend/tests/test_internal_side_events.py
git commit -m "feat(backend): internal side-events stream for hangar-server"
```

---

### Task 11: Núcleo do `hangar-server` (config, log, auth, repasse, saúde)

**Files:**
- Modify: `crates/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `crates/hangar-server/Cargo.toml` (`[dependencies]`, `[dev-dependencies]`)
- Modify: `crates/hangar-server/src/lib.rs` (módulos novos e `init_log`)
- Modify: `crates/hangar-server/src/main.rs` (corpo inteiro)
- Create: `crates/hangar-server/src/config.rs`
- Create: `crates/hangar-server/src/auth.rs`
- Create: `crates/hangar-server/src/proxy.rs`
- Create: `crates/hangar-server/src/routes.rs`
- Create: `crates/hangar-server/tests/fake/mod.rs`
- Test: `crates/hangar-server/tests/proxy.rs` (+ testes de unidade dentro de `config.rs`, `auth.rs`, `routes.rs`)

**Interfaces:**
- Consumes: ambiente montado pela Task 13 só para o filho — `HANGAR_SERVER_LISTEN`, `HANGAR_SERVER_UPSTREAM`, `HANGAR_INTERNAL_SECRET`, `CP_AUTH_TOKEN`, `HANGAR_SERVER_LOG` e `CP_FORWARDED_ALLOW_IPS` (os dois `CP_*` lidos do `settings`); stdin = cano aberto pelo Python durante a vida do filho.
- Produces:
  - `hangar_server::config::Config { pub listen: SocketAddr, pub upstream: SocketAddr, pub internal_secret: String, pub auth_token: String, pub log_path: Option<PathBuf>, pub trusted: TrustedHosts }`; `Config::from_env() -> Result<Config, String>`; `Config::from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config, String>`.
  - `hangar_server::init_log(path: Option<&Path>)` — `tracing` em arquivo (ou stderr).
  - `hangar_server::auth::{Auth, TrustedHosts, presented_token, query_param, is_loopback}`:
    `Auth::new(token: &str) -> Auth`; `Auth::is_owner(&self, ip: &str, token: Option<&[u8]>) -> bool`; `Auth::record_fail(&self, ip: &str)`;
    `presented_token(headers: &HeaderMap, query: Option<&str>, method: &Method, https: bool) -> Option<Vec<u8>>`;
    `query_param(query: Option<&str>, key: &str) -> Option<String>`;
    `TrustedHosts::parse(raw: &str) -> TrustedHosts`; `contains(&self, host: &str) -> bool`; `client_from_xff(&self, xff: &str) -> String`; `resolve(&self, peer: IpAddr, headers: &HeaderMap) -> (String, bool)`.
  - `hangar_server::proxy::{HttpClient, client() -> HttpClient, Forward { client_ip: String, https: bool }, forward(http: &HttpClient, upstream: SocketAddr, req: Request, fwd: &Forward) -> Response}`.
  - `hangar_server::routes::{AppState, serve(listener: TcpListener, cfg: Config) -> std::io::Result<()>, router(state: Arc<AppState>) -> Router}`.
  - `hangar_server::INTERNAL_PROTOCOL: u32 = 1`; `hangar_server::parent_gone<R: AsyncRead + Unpin>(pipe: R)` (termina no fim ou erro do cano); `hangar_server::serve_until(listener: TcpListener, cfg: Config, stop: impl Future<Output = ()>) -> std::io::Result<()>`.
  - `GET /__hangar_server/health` sem auth → `{"ok":true,"version":"<CARGO_PKG_VERSION>","protocol":1}` (o brief previa só `ok`/`version`; `protocol` é o que o Python confere, Task 13).
  - Binário `hangar-server`: sai 2 com configuração inválida, 1 sem a porta pública, 0 quando o stdin fecha (o Python morreu).

As regras portadas do Python:
- Ordem do token: `Authorization: Bearer`, depois `?token=` (último valor, como o `QueryParams` do Starlette), depois cookie; cookie só em GET/HEAD; `__Host-cp_token` sempre, `cp_token` só fora de https (conferido 2026-10-02: backend/app/auth.py:79-85 e 133-147).
- Comparação em tempo constante, tamanho diferente sai na hora como o `compare_digest` (conferido 2026-10-02: backend/app/auth.py:158-160).
- 8 falhas em 30 s por origem, teto de 512 origens, loopback isento, acerto limpa a origem (conferido 2026-10-02: backend/app/auth.py:36-41, 88-117, 149-171).
- Cliente e esquema reais só vêm do `X-Forwarded-*` quando o vizinho é confiável, varrendo o `X-Forwarded-For` da direita para a esquerda (conferido 2026-10-02: backend/.venv/lib/python3.14/site-packages/uvicorn/middleware/proxy_headers.py:30-60, 143-162; backend/app/main.py:231-232).
- CORS `*`, sem credenciais, `ETag` exposto (conferido 2026-10-02: backend/app/api.py:602-614). Gzip só com `gzip` no `Accept-Encoding` e corpo ≥ 1024 bytes, nível 5, nunca em `text/event-stream` (conferido 2026-10-02: backend/app/api.py:615-616).

Como o 429 fica igual ao de hoje sem duplicar o formato do erro: quem não traz o token do dono é sempre repassado, e o Python conta a falha dele. O Rust anota cada 401 que o Python devolve para aquela origem; com 8 em 30 s, ele para de avaliar o token (nem o do dono abre o atalho) e repassa, e o Python responde o 429 pela própria conta. Assim o atalho do Rust nunca vira oráculo de token durante o bloqueio, e o corpo do 429 continua sendo o do Python.

- [x] **Step 1: Dependências**

Em `crates/Cargo.toml`, acrescentar ao fim de `[workspace.dependencies]` (nenhuma destas existe ainda; `serde`/`serde_json` vêm da Task 1, `tokio` da Task 4, `regex`/`md-5`/`sha1_smol`/`tempfile` das Tasks 7-9; versões iguais às do `desktop-native/Cargo.lock` quando a crate já está lá, o `axum` não está):

```toml
axum = { version = "=0.8.9", default-features = false, features = ["http1", "tokio", "query"] }
bytes = "=1.12.1"
flate2 = "=1.1.10"
form_urlencoded = "=1.2.2"
futures-util = "=0.3.34"
hyper = { version = "=1.11.1", features = ["client", "http1"] }
hyper-util = { version = "=0.1.20", features = ["client-legacy", "http1", "tokio"] }
reqwest = { version = "=0.13.4", default-features = false, features = ["stream"] }
subtle = "=2.6.1"
tracing = "=0.1.44"
tracing-subscriber = { version = "=0.3.23", default-features = false, features = ["fmt", "std"] }
```

Em `crates/hangar-server/Cargo.toml`, acrescentar ao fim de `[dependencies]` (que já tem `serde_json`, `hangar-api`, `regex`, `md-5`, `sha1_smol` e `serde` das Tasks 6-9):

```toml
axum = { workspace = true }
bytes = { workspace = true }
flate2 = { workspace = true }
form_urlencoded = { workspace = true }
futures-util = { workspace = true }
hyper = { workspace = true }
hyper-util = { workspace = true }
subtle = { workspace = true }
# io-std: o stdin é o cano que avisa a morte do Python.
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "net", "io-util", "io-std", "sync", "time"] }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
```

e ao fim de `[dev-dependencies]` (a seção nasce na Task 9, com `tempfile`):

```toml
reqwest = { workspace = true }
```

- [x] **Step 2: Escrever os testes de integração**

```rust
// crates/hangar-server/tests/fake/mod.rs
//! Python falso para os testes do hangar-server: rotas internas (info, side-events) e o resto
//! respondendo "from-python", com o que chegou anotado.
#![allow(dead_code)]

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use bytes::Bytes;
use futures_util::StreamExt;
use hangar_server::auth::TrustedHosts;
use hangar_server::config::Config;
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, broadcast};

pub const OWNER: &str = "dono-token";
pub const SECRET: &str = "segredo-interno";

pub struct Fake {
    info: Mutex<Value>,
    side_tx: broadcast::Sender<String>,
    side_conns: AtomicUsize,
    side_apps: Mutex<Vec<String>>,
    info_calls: AtomicUsize,
    hits: Mutex<Vec<(String, HeaderMap)>>,
    pub release: Notify,
}

impl Fake {
    /// O que /internal/.../info devolve e o que a conexão interna manda como primeiro `info`.
    /// `Value::Null` = sessão inexistente (404).
    pub fn set_info(&self, v: Value) {
        *self.info.lock().unwrap() = v;
    }
    pub fn push_side(&self, event: &str, data: &str) {
        let _ = self.side_tx.send(format!("event: {event}\r\ndata: {data}\r\n\r\n"));
    }
    pub fn side_conns(&self) -> usize {
        self.side_conns.load(SeqCst)
    }
    pub fn side_apps(&self) -> Vec<String> {
        self.side_apps.lock().unwrap().clone()
    }
    pub fn info_calls(&self) -> usize {
        self.info_calls.load(SeqCst)
    }
    pub fn hits_to(&self, path: &str) -> usize {
        self.hits.lock().unwrap().iter().filter(|(p, _)| p.split('?').next() == Some(path)).count()
    }
    pub fn last_hit(&self) -> (String, HeaderMap) {
        self.hits.lock().unwrap().last().cloned().expect("algum pedido repassado")
    }
}

pub async fn spawn_fake() -> (Arc<Fake>, SocketAddr) {
    let (side_tx, _) = broadcast::channel(64);
    let fake = Arc::new(Fake {
        info: Mutex::new(Value::Null),
        side_tx,
        side_conns: AtomicUsize::new(0),
        side_apps: Mutex::default(),
        info_calls: AtomicUsize::new(0),
        hits: Mutex::default(),
        release: Notify::new(),
    });
    let app = Router::new()
        .route("/internal/sessions/{name}/info", get(fake_info))
        .route("/internal/sessions/{name}/side-events", get(fake_side))
        .fallback(fake_python)
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (fake, addr)
}

fn internal_ok(h: &HeaderMap) -> bool {
    h.get("x-hangar-internal").is_some_and(|v| v.as_bytes() == SECRET.as_bytes())
}

fn status(s: StatusCode) -> Response {
    Response::builder().status(s).body(Body::empty()).unwrap()
}

async fn fake_info(State(f): State<Arc<Fake>>, headers: HeaderMap) -> Response {
    f.info_calls.fetch_add(1, SeqCst);
    let info = f.info.lock().unwrap().clone();
    if !internal_ok(&headers) || info.is_null() {
        return status(StatusCode::NOT_FOUND);
    }
    Response::builder()
        .header("content-type", "application/json")
        .body(Body::from(info.to_string()))
        .unwrap()
}

async fn fake_side(
    State(f): State<Arc<Fake>>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let info = f.info.lock().unwrap().clone();
    if !internal_ok(&headers) || info.is_null() {
        return status(StatusCode::NOT_FOUND);
    }
    let rx = f.side_tx.subscribe();
    f.side_apps.lock().unwrap().push(q.get("app").cloned().unwrap_or_default());
    f.side_conns.fetch_add(1, SeqCst);
    let first = futures_util::stream::once(async move {
        Ok::<_, Infallible>(Bytes::from(format!("event: info\r\ndata: {info}\r\n\r\n")))
    });
    let rest = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.ok().map(|s| (Ok(Bytes::from(s)), rx))
    });
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(first.chain(rest)))
        .unwrap()
}

async fn fake_python(State(f): State<Arc<Fake>>, mut req: Request) -> Response {
    let full = req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default();
    f.hits.lock().unwrap().push((full, req.headers().clone()));
    let path = req.uri().path().to_owned();
    match path.as_str() {
        "/redirect" => Response::builder().status(302).header("location", "/outro").body(Body::empty()).unwrap(),
        "/probe" => status(StatusCode::UNAUTHORIZED),
        "/stream" => {
            let f2 = f.clone();
            let s = futures_util::stream::once(async { Ok::<_, Infallible>(Bytes::from_static(b"um")) })
                .chain(futures_util::stream::once(async move {
                    f2.release.notified().await;
                    Ok(Bytes::from_static(b"dois"))
                }));
            Response::new(Body::from_stream(s))
        }
        "/ws" => {
            let upgrade = hyper::upgrade::on(&mut req);
            tokio::spawn(async move {
                let Ok(up) = upgrade.await else { return };
                let mut io = TokioIo::new(up);
                let mut buf = [0u8; 64];
                while let Ok(n) = io.read(&mut buf).await {
                    if n == 0 || io.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
            Response::builder()
                .status(101)
                .header("connection", "upgrade")
                .header("upgrade", "eco")
                .body(Body::empty())
                .unwrap()
        }
        _ => Response::new(Body::from("from-python")),
    }
}

pub fn config(upstream: SocketAddr, trusted: &str) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream,
        internal_secret: SECRET.into(),
        auth_token: OWNER.into(),
        log_path: None,
        trusted: TrustedHosts::parse(trusted),
    }
}

pub async fn spawn_server(cfg: Config) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(hangar_server::routes::serve(listener, cfg));
    addr
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}
```

```rust
// crates/hangar-server/tests/proxy.rs
//! Repasse, autenticação e saúde do hangar-server contra um Python falso.
mod fake;

use std::time::Duration;

use fake::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn health_answers_without_token_and_with_cors() {
    let (_fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client()
        .get(format!("http://{srv}/__hangar_server/health"))
        .header("origin", "http://outra.maquina")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    assert_eq!(r.headers()["access-control-expose-headers"], "ETag");
    let v: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    // `protocol` é o contrato com o Python (RUST_SERVER_PROTOCOL na Task 13): mudar exige os dois.
    assert_eq!(v, serde_json::json!({"ok": true, "version": env!("CARGO_PKG_VERSION"), "protocol": 1}));
    assert_eq!(hangar_server::INTERNAL_PROTOCOL, 1);
}

#[tokio::test]
async fn closed_stdin_stops_the_server() {
    let (_fake, up) = spawn_fake().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // O duplex faz o papel do stdin: soltar a ponta de escrita é o Python morrendo.
    let (parent, child_stdin) = tokio::io::duplex(64);
    let server = tokio::spawn(hangar_server::serve_until(
        listener,
        config(up, "127.0.0.1"),
        hangar_server::parent_gone(child_stdin),
    ));
    let r = client().get(format!("http://{addr}/__hangar_server/health")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    drop(parent);
    let ended = tokio::time::timeout(Duration::from_secs(5), server).await.expect("parou em 5 s");
    assert!(ended.unwrap().is_ok(), "fim pelo cano é saída limpa");
    assert!(tokio::net::TcpStream::connect(addr).await.is_err(), "a porta pública fechou");
}

#[tokio::test]
async fn proxy_sets_forwarded_headers_and_drops_internal_secret() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client()
        .get(format!("http://{srv}/api/qualquer?x=1"))
        .header("x-forwarded-for", "203.0.113.9")
        .header("x-forwarded-proto", "https")
        .header("x-hangar-internal", SECRET)
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    let (path, h) = fake.last_hit();
    assert_eq!(path, "/api/qualquer?x=1");
    assert_eq!(h["x-forwarded-for"], "203.0.113.9");
    assert_eq!(h["x-forwarded-proto"], "https");
    assert!(!h.contains_key("x-hangar-internal"));
}

#[tokio::test]
async fn untrusted_peer_cannot_rewrite_client_or_scheme() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "10.9.9.9")).await;
    client()
        .get(format!("http://{srv}/api/qualquer"))
        .header("x-forwarded-for", "203.0.113.9")
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    let (_, h) = fake.last_hit();
    assert_eq!(h["x-forwarded-for"], "127.0.0.1");
    assert_eq!(h["x-forwarded-proto"], "http");
}

#[tokio::test]
async fn proxy_streams_body_without_buffering() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let mut r = client().get(format!("http://{srv}/stream")).send().await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), r.chunk()).await.unwrap().unwrap().unwrap();
    assert_eq!(&first[..], b"um");
    fake.release.notify_one();
    let second = tokio::time::timeout(Duration::from_secs(5), r.chunk()).await.unwrap().unwrap().unwrap();
    assert_eq!(&second[..], b"dois");
}

#[tokio::test]
async fn proxy_never_follows_redirects() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client().get(format!("http://{srv}/redirect")).send().await.unwrap();
    assert_eq!(r.status(), 302);
    assert_eq!(r.headers()["location"], "/outro");
    assert_eq!(fake.hits_to("/outro"), 0);
}

#[tokio::test]
async fn proxy_passes_websocket_upgrade_both_ways() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "10.9.9.9")).await;
    let mut s = tokio::net::TcpStream::connect(srv).await.unwrap();
    s.write_all(
        b"GET /ws HTTP/1.1\r\nHost: hangar\r\nConnection: Upgrade\r\nUpgrade: eco\r\n\
          X-Forwarded-For: 203.0.113.9\r\nX-Hangar-Internal: segredo-interno\r\n\r\n",
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut byte)).await.unwrap().unwrap();
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    s.write_all(b"ola").await.unwrap();
    let mut got = [0u8; 3];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut got)).await.unwrap().unwrap();
    assert_eq!(&got, b"ola");
    // O aperto de mão também leva o cliente real e nunca o segredo vindo de fora.
    let (_, h) = fake.last_hit();
    assert_eq!(h["x-forwarded-for"], "127.0.0.1");
    assert!(!h.contains_key("x-hangar-internal"));
}

#[tokio::test]
async fn guest_token_is_always_passed_to_python() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/history"))
        .bearer_auth("token-de-convidado")
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    assert_eq!(fake.info_calls(), 0, "convidado nunca chega às rotas internas");
}

#[tokio::test]
async fn blocked_origin_never_gets_the_owner_shortcut() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let ip = "198.51.100.7";
    for _ in 0..8 {
        let r = client()
            .get(format!("http://{srv}/probe"))
            .header("x-forwarded-for", ip)
            .bearer_auth("errado")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
    }
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/history"))
        .header("x-forwarded-for", ip)
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    assert_eq!(fake.info_calls(), 0, "bloqueado: o token do dono nem é avaliado");
}
```

Os dois últimos já passam com o repasse puro desta Task; eles ganham o controle positivo na Task 12, quando o dono vindo do loopback passa a consultar a rota interna.

- [x] **Step 3: Rodar e ver falhar** (quando autorizado)

Run: `(cd crates && cargo test -p hangar-server --test proxy)`
Expected: FAIL de compilação: `unresolved import hangar_server::auth` / `hangar_server::config` / `hangar_server::routes`.

- [x] **Step 4: Implementar `config.rs` e o log**

```rust
// crates/hangar-server/src/config.rs
//! Configuração vinda do ambiente que o Python monta ao subir o filho.
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::auth::TrustedHosts;

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: SocketAddr,
    pub upstream: SocketAddr,
    pub internal_secret: String,
    pub auth_token: String,
    pub log_path: Option<PathBuf>,
    pub trusted: TrustedHosts,
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        Config::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
        let required = |k: &str| get(k).filter(|v| !v.is_empty()).ok_or_else(|| format!("{k} ausente"));
        let addr = |k: &str| -> Result<SocketAddr, String> {
            let v = required(k)?;
            v.parse().map_err(|_| format!("{k} inválido: {v}"))
        };
        Ok(Config {
            listen: addr("HANGAR_SERVER_LISTEN")?,
            upstream: addr("HANGAR_SERVER_UPSTREAM")?,
            internal_secret: required("HANGAR_INTERNAL_SECRET")?,
            auth_token: required("CP_AUTH_TOKEN")?,
            log_path: get("HANGAR_SERVER_LOG").filter(|v| !v.is_empty()).map(PathBuf::from),
            // Mesmo padrão do Python: só um proxy desta máquina reescreve o cliente.
            trusted: TrustedHosts::parse(
                &get("CP_FORWARDED_ALLOW_IPS").unwrap_or_else(|| "127.0.0.1".into()),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    const BASE: [(&str, &str); 4] = [
        ("HANGAR_SERVER_LISTEN", "0.0.0.0:8765"),
        ("HANGAR_SERVER_UPSTREAM", "127.0.0.1:41234"),
        ("HANGAR_INTERNAL_SECRET", "ab12"),
        ("CP_AUTH_TOKEN", "tok"),
    ];

    #[test]
    fn reads_env_with_defaults() {
        let cfg = Config::from_lookup(env(&BASE)).unwrap();
        assert_eq!(cfg.listen, "0.0.0.0:8765".parse().unwrap());
        assert_eq!(cfg.upstream, "127.0.0.1:41234".parse().unwrap());
        assert_eq!(cfg.log_path, None);
        assert!(cfg.trusted.contains("127.0.0.1"));
        assert!(!cfg.trusted.contains("192.0.2.1"));
    }

    #[test]
    fn missing_or_bad_values_are_errors() {
        let sem_segredo: Vec<_> = BASE.iter().copied().filter(|(k, _)| *k != "HANGAR_INTERNAL_SECRET").collect();
        assert!(Config::from_lookup(env(&sem_segredo)).unwrap_err().contains("HANGAR_INTERNAL_SECRET"));
        let mut torto = BASE.to_vec();
        torto[0] = ("HANGAR_SERVER_LISTEN", "nao-e-endereco");
        assert!(Config::from_lookup(env(&torto)).unwrap_err().contains("HANGAR_SERVER_LISTEN"));
    }
}
```

Em `crates/hangar-server/src/lib.rs`, ao lado do `pub mod transcript;` da frente 3:

```rust
pub mod auth;
pub mod config;
pub mod proxy;
pub mod routes;

/// Versão do contrato com o Python (rotas `/internal`, eventos do side-events, ambiente). O
/// Python (`RUST_SERVER_PROTOCOL`) recusa um binário de outra versão e atende sozinho.
pub const INTERNAL_PROTOCOL: u32 = 1;

/// Lê o cano até o fim ou erro. O Python segura a outra ponta; fechou = pai morreu.
pub async fn parent_gone<R: tokio::io::AsyncRead + Unpin>(mut pipe: R) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 64];
    while let Ok(n) = pipe.read(&mut buf).await {
        if n == 0 {
            return;
        }
    }
}

/// `routes::serve` até `stop` terminar. Sem esperar as conexões abertas: SSE nunca fecha sozinho.
/// `Ok(())` só quando parou por `stop`.
pub async fn serve_until(
    listener: tokio::net::TcpListener,
    cfg: config::Config,
    stop: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    tokio::select! {
        r = routes::serve(listener, cfg) => r,
        () = stop => Ok(()),
    }
}

/// Log em arquivo (HANGAR_SERVER_LOG) ou no stderr. Nunca recebe texto de conversa.
pub fn init_log(path: Option<&std::path::Path>) {
    let file = path.and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
    let builder = tracing_subscriber::fmt().with_target(false);
    let _ = match file {
        Some(f) => builder.with_writer(std::sync::Mutex::new(f)).try_init(),
        None => builder.with_writer(std::io::stderr).try_init(),
    };
}
```

- [x] **Step 5: Implementar `auth.rs`**

```rust
// crates/hangar-server/src/auth.rs
//! Quem é o dono, pela mesma regra do auth.py. Quem não traz o token do dono é repassado ao
//! Python, que decide (convidado, 401, 429).
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, Method, header};
use subtle::ConstantTimeEq;

const MAX_FAILS: usize = 8;
const WINDOW: Duration = Duration::from_secs(30);
const MAX_ORIGINS: usize = 512;
const COOKIE: &str = "cp_token";
const COOKIE_HOST: &str = "__Host-cp_token";

pub fn is_loopback(ip: &str) -> bool {
    matches!(ip, "127.0.0.1" | "::1" | "localhost")
}

pub struct Auth {
    token: Vec<u8>,
    fails: Mutex<HashMap<String, Vec<Instant>>>,
}

impl Auth {
    pub fn new(token: &str) -> Auth {
        Auth { token: token.as_bytes().to_vec(), fails: Mutex::new(HashMap::new()) }
    }

    /// true = token do dono. Origem bloqueada nem tem o token avaliado: o pedido segue ao
    /// Python, que responde o 429 pela conta dele.
    pub fn is_owner(&self, ip: &str, token: Option<&[u8]>) -> bool {
        if !is_loopback(ip) && self.blocked(ip) {
            return false;
        }
        // Fatias de tamanho diferente saem na hora, como o compare_digest.
        let ok = token.is_some_and(|t| bool::from(t.ct_eq(self.token.as_slice())));
        if ok {
            self.fails.lock().unwrap().remove(ip);
        }
        ok
    }

    fn blocked(&self, ip: &str) -> bool {
        let mut fails = self.fails.lock().unwrap();
        let now = Instant::now();
        let Some(hits) = fails.get_mut(ip) else { return false };
        hits.retain(|t| now.duration_since(*t) < WINDOW);
        let n = hits.len();
        if n == 0 {
            fails.remove(ip);
        }
        n >= MAX_FAILS
    }

    /// O Python respondeu 401 a um pedido repassado e contou a falha; a mesma conta aqui impede
    /// que o atalho do dono responda 200 a um palpite certo durante o bloqueio.
    pub fn record_fail(&self, ip: &str) {
        if is_loopback(ip) {
            return;
        }
        let mut fails = self.fails.lock().unwrap();
        let now = Instant::now();
        let hits = fails.entry(ip.to_string()).or_default();
        hits.retain(|t| now.duration_since(*t) < WINDOW);
        if hits.len() >= MAX_FAILS {
            return;
        }
        hits.push(now);
        if hits.len() == MAX_FAILS {
            tracing::warn!(%ip, "token errado {MAX_FAILS} vezes em 30 s; atalho do dono desligado para esta origem");
        }
        if fails.len() > MAX_ORIGINS {
            fails.retain(|_, h| h.last().is_some_and(|t| now.duration_since(*t) < WINDOW));
            while fails.len() > MAX_ORIGINS {
                let Some(oldest) = fails.iter().min_by_key(|(_, h)| h.last().copied()).map(|(k, _)| k.clone())
                else {
                    break;
                };
                fails.remove(&oldest);
            }
        }
    }
}

/// Token apresentado, na ordem do require_auth: Bearer, ?token=, cookie (só GET/HEAD).
pub fn presented_token(headers: &HeaderMap, query: Option<&str>, method: &Method, https: bool) -> Option<Vec<u8>> {
    if let Some(v) = headers.get(header::AUTHORIZATION) {
        if let Some(t) = v.as_bytes().strip_prefix(b"Bearer ") {
            return Some(t.to_vec());
        }
    }
    if let Some(q) = query_param(query, "token").filter(|q| !q.is_empty()) {
        return Some(q.into_bytes());
    }
    // O cookie vai junto em pedido de outra página do mesmo site: só serve para ler.
    if method != Method::GET && method != Method::HEAD {
        return None;
    }
    let jar = cookies(headers);
    // `__Host-` só nasce numa página https deste host; o `cp_token` sem prefixo outra máquina do
    // mesmo site consegue gravar, e em https ele não vale.
    jar.get(COOKIE_HOST)
        .filter(|v| !v.is_empty())
        .or_else(|| if https { None } else { jar.get(COOKIE).filter(|v| !v.is_empty()) })
        .map(|v| v.as_bytes().to_vec())
}

/// Valor de um parâmetro da query; repetido, vale o último (QueryParams do Starlette).
pub fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    form_urlencoded::parse(query?.as_bytes())
        .filter(|(k, _)| k == key)
        .last()
        .map(|(_, v)| v.into_owned())
}

/// cookie_parser do Starlette: `;` separa, `=` só no primeiro, aspas em volta saem.
fn cookies(headers: &HeaderMap) -> HashMap<String, String> {
    let mut jar = HashMap::new();
    let Some(raw) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) else { return jar };
    for chunk in raw.split(';') {
        let (k, v) = chunk.split_once('=').unwrap_or(("", chunk));
        let (k, v) = (k.trim(), v.trim());
        if !k.is_empty() || !v.is_empty() {
            let v = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
            jar.insert(k.to_string(), v.to_string());
        }
    }
    jar
}

/// `forwarded_allow_ips` do uvicorn: IPs, redes CIDR, `*` e nomes literais.
#[derive(Clone, Debug, Default)]
pub struct TrustedHosts {
    all: bool,
    hosts: Vec<IpAddr>,
    nets: Vec<(IpAddr, u8)>,
    literals: Vec<String>,
}

impl TrustedHosts {
    pub fn parse(raw: &str) -> TrustedHosts {
        let raw = raw.trim();
        if raw == "*" {
            return TrustedHosts { all: true, ..TrustedHosts::default() };
        }
        let mut t = TrustedHosts::default();
        for item in raw.split(',').map(str::trim).filter(|i| !i.is_empty()) {
            if let Some((addr, bits)) = item.split_once('/') {
                match (addr.parse::<IpAddr>(), bits.parse::<u8>()) {
                    (Ok(a), Ok(b)) if b <= if a.is_ipv4() { 32 } else { 128 } => t.nets.push((a, b)),
                    _ => t.literals.push(item.to_string()),
                }
            } else {
                match item.parse::<IpAddr>() {
                    Ok(a) => t.hosts.push(a),
                    Err(_) => t.literals.push(item.to_string()),
                }
            }
        }
        t
    }

    pub fn contains(&self, host: &str) -> bool {
        if self.all {
            return true;
        }
        if host.is_empty() {
            return false;
        }
        match host.parse::<IpAddr>() {
            Ok(ip) => self.hosts.contains(&ip) || self.nets.iter().any(|(n, b)| in_net(ip, *n, *b)),
            Err(_) => self.literals.iter().any(|l| l == host),
        }
    }

    /// Primeiro endereço não confiável da direita para a esquerda; todos confiáveis = o primeiro.
    pub fn client_from_xff(&self, xff: &str) -> String {
        let hosts: Vec<&str> = xff.split(',').map(str::trim).collect();
        if self.all {
            return host_of(hosts[0]).to_string();
        }
        for hp in hosts.iter().rev() {
            let h = host_of(hp);
            if !self.contains(h) {
                return h.to_string();
            }
        }
        host_of(hosts[0]).to_string()
    }

    /// (ip do cliente, https?) como o uvicorn resolve com proxy_headers: só um vizinho confiável
    /// reescreve o cliente e o esquema.
    pub fn resolve(&self, peer: IpAddr, headers: &HeaderMap) -> (String, bool) {
        let peer = peer.to_canonical().to_string();
        if !self.contains(&peer) {
            return (peer, false);
        }
        let proto = headers
            .get_all("x-forwarded-proto")
            .iter()
            .last()
            .and_then(|v| v.to_str().ok())
            .map(str::trim);
        let https = matches!(proto, Some("https" | "wss"));
        let xff: Vec<&str> =
            headers.get_all("x-forwarded-for").iter().filter_map(|v| v.to_str().ok()).collect();
        if xff.is_empty() {
            return (peer, https);
        }
        let host = self.client_from_xff(&xff.join(", "));
        (if host.is_empty() { peer } else { host }, https)
    }
}

fn in_net(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let m = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(a) & m == u32::from(n) & m
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let m = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(a) & m == u128::from(n) & m
        }
        _ => false,
    }
}

/// `_parse_host_port` do uvicorn, só a parte do host.
fn host_of(value: &str) -> &str {
    if let Some(rest) = value.strip_prefix('[') {
        return match rest.find(']') {
            None => value,
            Some(end) => {
                let after = &rest[end + 1..];
                if after.is_empty() || after.starts_with(':') { &rest[..end] } else { value }
            }
        };
    }
    if value.matches(':').count() == 1 {
        let (h, p) = value.rsplit_once(':').expect("um ':' contado acima");
        if p.trim().parse::<i64>().is_ok() {
            return h;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn h(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        m
    }

    #[test]
    fn bearer_wins_over_query_and_cookie() {
        let hs = h(&[("authorization", "Bearer convidado"), ("cookie", "cp_token=dono")]);
        assert_eq!(presented_token(&hs, Some("token=dono"), &Method::GET, false), Some(b"convidado".to_vec()));
    }

    #[test]
    fn query_before_cookie_and_last_value_wins() {
        let hs = h(&[("cookie", "cp_token=c")]);
        assert_eq!(presented_token(&hs, Some("token=a&token=b"), &Method::GET, false), Some(b"b".to_vec()));
        assert_eq!(presented_token(&hs, Some("token="), &Method::GET, false), Some(b"c".to_vec()));
    }

    #[test]
    fn cookie_only_on_get_and_head() {
        let hs = h(&[("cookie", "cp_token=dono")]);
        assert_eq!(presented_token(&hs, None, &Method::POST, false), None);
        assert_eq!(presented_token(&hs, None, &Method::HEAD, false), Some(b"dono".to_vec()));
    }

    #[test]
    fn https_prefers_host_cookie_and_ignores_plain_one() {
        let both = h(&[("cookie", "cp_token=x; __Host-cp_token=y")]);
        assert_eq!(presented_token(&both, None, &Method::GET, true), Some(b"y".to_vec()));
        let plain = h(&[("cookie", "cp_token=x")]);
        assert_eq!(presented_token(&plain, None, &Method::GET, true), None);
        assert_eq!(presented_token(&plain, None, &Method::GET, false), Some(b"x".to_vec()));
    }

    #[test]
    fn owner_compare_block_and_loopback() {
        let a = Auth::new("tok");
        assert!(a.is_owner("192.0.2.5", Some(b"tok")));
        assert!(!a.is_owner("192.0.2.5", Some(b"tok-maior")));
        assert!(!a.is_owner("192.0.2.5", None));
        for _ in 0..MAX_FAILS {
            a.record_fail("192.0.2.5");
        }
        assert!(!a.is_owner("192.0.2.5", Some(b"tok")), "bloqueado não avalia o token");
        for _ in 0..MAX_FAILS {
            a.record_fail("127.0.0.1");
        }
        assert!(a.is_owner("127.0.0.1", Some(b"tok")), "loopback é isento");
    }

    #[test]
    fn owner_success_clears_the_origin() {
        let a = Auth::new("tok");
        for _ in 0..MAX_FAILS - 1 {
            a.record_fail("192.0.2.6");
        }
        assert!(a.is_owner("192.0.2.6", Some(b"tok")));
        a.record_fail("192.0.2.6");
        assert!(a.is_owner("192.0.2.6", Some(b"tok")));
    }

    #[test]
    fn trusted_hosts_walk_xff_like_uvicorn() {
        let t = TrustedHosts::parse("127.0.0.1, 10.0.0.0/8");
        assert!(t.contains("10.2.3.4") && t.contains("127.0.0.1") && !t.contains("192.0.2.1"));
        assert_eq!(t.client_from_xff("203.0.113.9, 10.0.0.2"), "203.0.113.9");
        assert_eq!(t.client_from_xff("10.0.0.3, 10.0.0.2"), "10.0.0.3");
        assert_eq!(t.client_from_xff("[2001:db8::1]:443"), "2001:db8::1");
        assert_eq!(t.client_from_xff("198.51.100.4:5000"), "198.51.100.4");
        let all = TrustedHosts::parse("*");
        assert_eq!(all.client_from_xff("198.51.100.1, 198.51.100.2"), "198.51.100.1");
    }

    #[test]
    fn resolve_only_trusts_a_trusted_peer() {
        let t = TrustedHosts::parse("127.0.0.1");
        let hs = h(&[("x-forwarded-for", "198.51.100.7"), ("x-forwarded-proto", "https")]);
        assert_eq!(t.resolve("127.0.0.1".parse().unwrap(), &hs), ("198.51.100.7".to_string(), true));
        assert_eq!(t.resolve("192.0.2.9".parse().unwrap(), &hs), ("192.0.2.9".to_string(), false));
        assert_eq!(
            t.resolve("::ffff:127.0.0.1".parse().unwrap(), &HeaderMap::new()),
            ("127.0.0.1".to_string(), false)
        );
    }
}
```

- [x] **Step 6: Implementar `proxy.rs`**

```rust
// crates/hangar-server/src/proxy.rs
//! Repasse ao Python de tudo que o hangar-server não atende sozinho, inclusive WebSocket,
//! upload e streaming. Nunca segue redirect: a resposta volta como veio.
use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};

pub type HttpClient = Client<HttpConnector, Body>;

pub fn client() -> HttpClient {
    Client::builder(TokioExecutor::new()).build_http()
}

/// Cliente já resolvido pelo `TrustedHosts`. Vai ao Python como valor único do X-Forwarded-For:
/// o uvicorn confia em 127.0.0.1 (o Rust) e chega ao mesmo cliente que chegaria sem o Rust.
pub struct Forward {
    pub client_ip: String,
    pub https: bool,
}

const HOP: [&str; 7] = ["connection", "keep-alive", "proxy-connection", "transfer-encoding", "te", "trailer", "upgrade"];

pub async fn forward(http: &HttpClient, upstream: SocketAddr, mut req: Request, fwd: &Forward) -> Response {
    let upgrade = req.headers().contains_key(header::UPGRADE);
    let path = req.uri().path_and_query().map(|p| p.as_str().to_owned()).unwrap_or_else(|| "/".into());
    prepare(req.headers_mut(), fwd, upgrade);
    if upgrade {
        return forward_upgrade(upstream, req, &path).await;
    }
    let Ok(uri) = format!("http://{upstream}{path}").parse::<Uri>() else {
        return (StatusCode::BAD_REQUEST, "hangar-server: caminho inválido").into_response();
    };
    *req.uri_mut() = uri;
    match http.request(req).await {
        Ok(resp) => {
            let (mut parts, body) = resp.into_parts();
            for name in HOP {
                parts.headers.remove(name);
            }
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => bad_gateway(&e),
    }
}

fn prepare(h: &mut HeaderMap, fwd: &Forward, upgrade: bool) {
    for name in HOP {
        // No aperto de mão do WebSocket, Connection e Upgrade são o próprio pedido.
        if upgrade && (name == "connection" || name == "upgrade") {
            continue;
        }
        h.remove(name);
    }
    // Só o próprio hangar-server fala com as rotas internas.
    h.remove("x-hangar-internal");
    h.remove("x-forwarded-for");
    h.remove("x-forwarded-proto");
    if let Ok(v) = HeaderValue::from_str(&fwd.client_ip) {
        h.insert("x-forwarded-for", v);
    }
    h.insert("x-forwarded-proto", HeaderValue::from_static(if fwd.https { "https" } else { "http" }));
}

/// Conexão própria por WebSocket: o pedido vai em forma de origem e, com o 101, os dois lados
/// viram um cano de bytes.
async fn forward_upgrade(upstream: SocketAddr, mut req: Request, path: &str) -> Response {
    let Ok(uri) = path.parse::<Uri>() else {
        return (StatusCode::BAD_REQUEST, "hangar-server: caminho inválido").into_response();
    };
    *req.uri_mut() = uri;
    let client_side = hyper::upgrade::on(&mut req);
    let stream = match tokio::net::TcpStream::connect(upstream).await {
        Ok(s) => s,
        Err(e) => return bad_gateway(&e),
    };
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await {
        Ok(x) => x,
        Err(e) => return bad_gateway(&e),
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    let mut resp = match sender.send_request(req).await {
        Ok(r) => r,
        Err(e) => return bad_gateway(&e),
    };
    if resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        let (parts, body) = resp.into_parts();
        return Response::from_parts(parts, Body::new(body));
    }
    let upstream_side = hyper::upgrade::on(&mut resp);
    tokio::spawn(async move {
        let (Ok(c), Ok(u)) = tokio::join!(client_side, upstream_side) else { return };
        let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(c), &mut TokioIo::new(u)).await;
    });
    let (parts, _) = resp.into_parts();
    Response::from_parts(parts, Body::empty())
}

fn bad_gateway(e: &dyn std::fmt::Display) -> Response {
    tracing::warn!("repasse ao Python falhou: {e}");
    (StatusCode::BAD_GATEWAY, "hangar-server: o backend não respondeu").into_response()
}
```

- [x] **Step 7: Implementar `routes.rs` e `main.rs`**

```rust
// crates/hangar-server/src/routes.rs
//! Rotas do hangar-server. As de conversa entram na Task 12; o resto é repasse.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::net::TcpListener;

use crate::auth::{self, Auth};
use crate::config::Config;
use crate::proxy::{self, Forward, HttpClient};

pub struct AppState {
    pub cfg: Config,
    pub auth: Auth,
    pub http: HttpClient,
}

pub async fn serve(listener: TcpListener, cfg: Config) -> std::io::Result<()> {
    let state = Arc::new(AppState { auth: Auth::new(&cfg.auth_token), http: proxy::client(), cfg });
    axum::serve(listener, router(state).into_make_service_with_connect_info::<SocketAddr>()).await
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/__hangar_server/health", get(health))
        .fallback(pass_any)
        .with_state(state)
}

async fn health(headers: HeaderMap) -> Response {
    let body = format!(
        "{{\"ok\":true,\"version\":\"{}\",\"protocol\":{}}}",
        env!("CARGO_PKG_VERSION"),
        crate::INTERNAL_PROTOCOL
    );
    let mut resp = ([(header::CONTENT_TYPE, "application/json")], body).into_response();
    cors(&headers, resp.headers_mut());
    resp
}

/// Quem pede e se é o dono, resolvidos uma vez por pedido.
pub(crate) fn gate(st: &AppState, peer: SocketAddr, req: &Request) -> (Forward, bool) {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    let token = auth::presented_token(req.headers(), req.uri().query(), req.method(), https);
    let owner = st.auth.is_owner(&client_ip, token.as_deref());
    (Forward { client_ip, https }, owner)
}

pub(crate) async fn pass(st: &AppState, req: Request, fwd: &Forward) -> Response {
    let resp = proxy::forward(&st.http, st.cfg.upstream, req, fwd).await;
    if resp.status() == StatusCode::UNAUTHORIZED {
        st.auth.record_fail(&fwd.client_ip);
    }
    resp
}

async fn pass_any(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    pass(&st, req, &Forward { client_ip, https }).await
}

/// CORS das respostas do próprio Rust, como o CORSMiddleware do Python: `*`, sem credenciais,
/// ETag legível pelo JS. Preflight não chega aqui: vai sem token e é repassado.
pub(crate) fn cors(req: &HeaderMap, resp: &mut HeaderMap) {
    if req.contains_key(header::ORIGIN) {
        resp.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        resp.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("ETag"));
    }
}

/// Gzip só quando o cliente pede e o corpo passa de 1 KB. Nunca chamado para text/event-stream.
pub(crate) fn maybe_gzip(req: &HeaderMap, resp: &mut HeaderMap, body: Vec<u8>) -> Vec<u8> {
    let wants = req
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("gzip"));
    if !wants || body.len() < 1024 {
        return body;
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(5));
    std::io::Write::write_all(&mut enc, &body).expect("escrita em memória");
    resp.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    resp.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    enc.finish().expect("escrita em memória")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn cors_only_with_origin() {
        let mut resp = HeaderMap::new();
        cors(&HeaderMap::new(), &mut resp);
        assert!(resp.is_empty());
        let mut req = HeaderMap::new();
        req.insert(header::ORIGIN, HeaderValue::from_static("http://outra"));
        cors(&req, &mut resp);
        assert_eq!(resp[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(resp[header::ACCESS_CONTROL_EXPOSE_HEADERS], "ETag");
    }

    #[test]
    fn gzip_only_when_asked_and_large() {
        let big = vec![b'a'; 2048];
        let mut resp = HeaderMap::new();
        assert_eq!(maybe_gzip(&HeaderMap::new(), &mut resp, big.clone()), big);
        let mut req = HeaderMap::new();
        req.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate"));
        assert_eq!(maybe_gzip(&req, &mut resp, b"curto".to_vec()), b"curto");
        assert!(resp.get(header::CONTENT_ENCODING).is_none());
        let packed = maybe_gzip(&req, &mut resp, big.clone());
        assert_eq!(resp[header::CONTENT_ENCODING], "gzip");
        assert_eq!(resp[header::VARY], "Accept-Encoding");
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&packed[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, big);
    }
}
```

```rust
// crates/hangar-server/src/main.rs
//! hangar-server: sobe como filho do Python (app/main.py), na porta pública.
#[tokio::main]
async fn main() {
    let cfg = match hangar_server::config::Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("hangar-server: configuração inválida: {e}");
            std::process::exit(2);
        }
    };
    hangar_server::init_log(cfg.log_path.as_deref());
    let listener = match tokio::net::TcpListener::bind(cfg.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(listen = %cfg.listen, "porta pública indisponível: {e}");
            eprintln!("hangar-server: porta {} indisponível: {e}", cfg.listen);
            std::process::exit(1);
        }
    };
    tracing::info!(listen = %cfg.listen, upstream = %cfg.upstream, version = env!("CARGO_PKG_VERSION"), "hangar-server de pé");
    // O Python segura o cano do stdin; fechou = pai morreu. Vale igual em Linux, Windows e macOS.
    let stop = hangar_server::parent_gone(tokio::io::stdin());
    match hangar_server::serve_until(listener, cfg, stop).await {
        Ok(()) => {
            tracing::info!("stdin fechou: o backend saiu, hangar-server sai junto");
            std::process::exit(0);
        }
        Err(e) => {
            tracing::error!("hangar-server parou: {e}");
            std::process::exit(1);
        }
    }
}
```

- [x] **Step 8: Rodar e ver passar** (quando autorizado)

Run: `(cd crates && cargo test -p hangar-server --lib -- config auth routes && cargo test -p hangar-server --test proxy)`
Expected: PASS em todos.

- [x] **Step 9: Commit**

```bash
git add crates/Cargo.toml crates/Cargo.lock crates/hangar-server/Cargo.toml crates/hangar-server/src/lib.rs crates/hangar-server/src/main.rs crates/hangar-server/src/config.rs crates/hangar-server/src/auth.rs crates/hangar-server/src/proxy.rs crates/hangar-server/src/routes.rs crates/hangar-server/tests/fake/mod.rs crates/hangar-server/tests/proxy.rs
git commit -m "feat(server): hangar-server core with owner auth, reverse proxy and health"
```

---

### Task 12: `/history` e `/events` no `hangar-server`

**Files:**
- Modify: `crates/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `crates/hangar-server/Cargo.toml`
- Modify: `crates/hangar-server/src/lib.rs` (`pub mod side;`, `pub mod tail;`)
- Modify: `crates/hangar-server/src/routes.rs` (substitui o arquivo da Task 11 inteiro)
- Create: `crates/hangar-server/src/tail.rs`
- Create: `crates/hangar-server/src/side.rs`
- Modify: `crates/hangar-server/tests/fake/mod.rs` (acrescenta os ajudantes de transcript e SSE)
- Test: `crates/hangar-server/tests/conversations.rs` (+ testes de unidade em `tail.rs` e `side.rs`)

**Interfaces:**
- Consumes:
  - frente 3, `hangar_server::transcript`: `Provider { Claude, ClaudeHeadless, Codex }` + `Provider::parse(&str) -> Option<Provider>`; `LineParser::new(Provider)` + `LineParser::feed(&mut self, line: &[u8], offset: u64) -> Vec<ChatEvent>`; `InternalInfo { provider, jsonl, session_key, history }` (`Deserialize + Clone`); `InternalInfo::history_request(&self, Option<usize>) -> Option<HistoryRequest>`; `merged_history(&HistoryRequest) -> io::Result<Vec<ChatEvent>>` (já com o corte `evs[-limit:]`); `history_etag(&HistoryRequest) -> Option<String>`; `SKIPPED_LINES: AtomicU64` (Task 7); `LineParser: Send` e `HistoryRequest: Send + 'static` (presos por teste na Task 9).
  - Task 10: `GET /internal/sessions/{name}/side-events?app=1` e o formato do evento `info`.
  - Task 9: `GET /internal/sessions/{name}/info`.
  - Task 11: `routes::{AppState, gate, pass, cors, maybe_gzip}`, `auth::query_param`, `proxy::HttpClient`.
- Produces:
  - `hangar_server::tail::{BACKFILL_LINES, sse_frame(event: &str, data: &str, id: Option<&str>) -> Bytes, ping_frame() -> Bytes, reset_frame() -> Bytes, comment_frame() -> Bytes, read_frames(path: &Path, from: u64, to: Option<u64>, parser: &mut LineParser, key: &str) -> io::Result<(Vec<Bytes>, u64)>, tail_offset(path: &Path, max_lines: usize) -> u64, backfill(path: &Path, key: &str, provider: Provider, resume: Option<&str>, cut: u64) -> Vec<Bytes>, log_skipped(key: &str, before: u64), Watchers, FileTail, TailState}` (`log_skipped` põe no log, com `tracing::warn!`, quantas linhas o `SKIPPED_LINES` da Task 7 contou numa leitura).
  - `hangar_server::side::{Out, Binding, Hub, Hubs, Lease, SideCtx, Attach, InfoCache, INFO_TTL, remember_info}`.
  - Rotas `GET /api/sessions/{name}/history` e `GET /api/sessions/{name}/events` atendidas pelo Rust para o dono em sessão `claude`/`claude-headless`/`codex`; qualquer outro caso, inclusive outro método nessas rotas, é repassado.

Regras portadas, com a origem:
- `/history`: `ETag`/`If-None-Match` → 304 com o `ETag`; corte final `evs[-limit:]` só com `limit > 0`; `limit <= 0` é o histórico inteiro, nunca 400 (conferido 2026-10-02: backend/app/api.py:2901-2920). `limit` lido como inteiro com sinal; o que não é inteiro fica com o FastAPI (422 dele).
- `/events`: `?last_event_id=` vence o `Last-Event-ID`; stem tem que ser o `session_key` do transcript atual; `0 ≤ offset ≤ tamanho`, senão cauda (conferido 2026-10-02: backend/app/api.py:3366-3373; backend/app/transcript.py:896-904). Cauda de 200 linhas lida de trás para frente em janelas de 256 KB×4ⁿ (conferido 2026-10-02: backend/app/transcript.py:21, 32, 859-886). Offset do INÍCIO da linha no `id`, linha sem `\n` fica para depois (conferido 2026-10-02: backend/app/transcript.py:796-812). `id:` só nas mensagens do transcript (conferido 2026-10-02: backend/app/sse.py:799-810).
- `ping` `{}` na abertura e a cada 10 s (conferido 2026-10-02: backend/app/sse.py:748-758); comentário `: ping - <data UTC>` a cada 15 s, `Cache-Control: no-store`, `Connection: keep-alive`, `X-Accel-Buffering: no`, separador `\r\n`, sem `retry:` (conferido 2026-10-02: backend/.venv/lib/python3.14/site-packages/sse_starlette/sse.py:259-260, 310-313, 435-447 e event.py:32-58); envio preso por 30 s fecha a conexão (`send_timeout=30`, api.py:3382).
- Quem chega depois recebe o último valor de cada fonte compartilhada, como o `Difusor` entrega o retrato ao ouvinte novo (conferido 2026-10-02: backend/app/difusor.py:47-50).
- `?diag_req`/`x-hangar-req` (32 caracteres, `[A-Za-z0-9_-]`) vão ao log (conferido 2026-10-02: backend/app/api.py:563-569).

Como o hub funciona (um por sessão, vivo enquanto houver aparelho):
- O primeiro aparelho do dono cria o hub com o `info` que buscou: um leitor do jsonl (`FileTail`, começa no fim das linhas completas) e a conexão interna (`run_side`). Cada aparelho faz a própria cauda com parser próprio, do ponto pedido até o ponto em que o leitor compartilhado está, e daí em diante recebe os quadros do `broadcast`. Assinar o canal e ler esse ponto acontecem sob a trava do leitor, que também é quem envia: nenhuma linha cai entre a cauda e o ao vivo.
- `info` novo da conexão interna com outro jsonl ou provider: o hub troca o leitor (geração nova), apaga o retrato das fontes e manda `Rebind`; cada aparelho manda `reset` e refaz a cauda no arquivo novo. Quadro de leitor de geração velha é descartado. Provider fora do Rust (ou 404): `Close`, cada aparelho manda `reset` e encerra; ao reconectar, o `info` em cache já diz que não é do Rust e o pedido vai ao Python.
- Aparelho novo com `info` diferente do hub (sessão recriada com o mesmo nome): troca o leitor na hora e religa a conexão interna, cujo primeiro `info` confirma ou corrige. Nunca serve a cauda do transcript morto a quem acabou de chegar.
- Conexão interna caída: religa em 1, 2, 4… 30 s; os aparelhos seguem recebendo a conversa do arquivo.

- [x] **Step 1: Dependências**

Em `crates/Cargo.toml`, acrescentar ao fim de `[workspace.dependencies]` (`chrono` já veio da Task 4 e `tempfile` da Task 9, nas mesmas versões do `desktop-native/Cargo.lock`):

```toml
eventsource-stream = "=0.2.3"
http-body-util = "=0.1.5"
notify = "=7.0.0"
percent-encoding = "=2.3.2"
```

Em `crates/hangar-server/Cargo.toml`, acrescentar ao fim de `[dependencies]` (o `tempfile` de `[dev-dependencies]` já está lá desde a Task 9):

```toml
chrono = { workspace = true }
eventsource-stream = { workspace = true }
http-body-util = { workspace = true }
notify = { workspace = true }
percent-encoding = { workspace = true }
```

Em `crates/hangar-server/src/lib.rs`, junto dos módulos da Task 11:

```rust
pub mod side;
pub mod tail;
```

- [x] **Step 2: Escrever os testes de integração**

Acrescentar ao fim de `crates/hangar-server/tests/fake/mod.rs` (com os `use` novos no topo do arquivo):

```rust
// topo do arquivo, junto dos outros `use`
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures_util::stream::BoxStream;
use serde_json::json;
```

```rust
/// Linha sintética no formato do Claude (nunca conversa real): user com texto e relógio crescente.
pub fn claude_line(i: usize) -> String {
    format!(
        "{{\"type\":\"user\",\"uuid\":\"u{i}\",\"timestamp\":\"2026-10-02T10:{m:02}:{s:02}.000Z\",\"message\":{{\"role\":\"user\",\"content\":\"linha {i}\"}}}}\n",
        m = i / 60 % 60,
        s = i % 60
    )
}

/// Acrescenta as linhas e devolve o offset do início de cada uma.
pub fn append_lines(path: &Path, range: std::ops::Range<usize>) -> Vec<u64> {
    let mut f = OpenOptions::new().create(true).append(true).open(path).unwrap();
    let mut at = f.metadata().unwrap().len();
    let mut offs = Vec::new();
    for i in range {
        let line = claude_line(i);
        f.write_all(line.as_bytes()).unwrap();
        offs.push(at);
        at += line.len() as u64;
    }
    offs
}

pub fn append_raw(path: &Path, s: &str) {
    OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(s.as_bytes()).unwrap();
}

/// Campo `history` do `info` nos testes: tem de casar com o formato que a Task 9 grava (ver as
/// Notas para o integrador).
pub fn history_field(jsonl: &Path) -> Value {
    json!({"queue": jsonl.with_extension("fila.jsonl")})
}

pub fn info_json(provider: &str, jsonl: &Path) -> Value {
    json!({
        "provider": provider,
        "jsonl": jsonl,
        "session_key": jsonl.file_stem().unwrap().to_str().unwrap(),
        "history": history_field(jsonl),
    })
}

pub type Events = BoxStream<'static, Result<Event, EventStreamError<reqwest::Error>>>;

pub async fn open_events(srv: SocketAddr, name: &str, query: &str, headers: &[(&str, &str)]) -> reqwest::Response {
    let mut url = format!("http://{srv}/api/sessions/{name}/events?token={OWNER}");
    if !query.is_empty() {
        url.push('&');
        url.push_str(query);
    }
    let mut req = client().get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.send().await.unwrap()
}

pub fn sse(resp: reqwest::Response) -> Events {
    resp.bytes_stream().eventsource().boxed()
}

pub async fn next_any(es: &mut Events) -> Event {
    tokio::time::timeout(Duration::from_secs(5), es.next())
        .await
        .expect("evento em 5 s")
        .expect("stream aberto")
        .expect("SSE válido")
}

pub async fn next_non_ping(es: &mut Events) -> Event {
    loop {
        let ev = next_any(es).await;
        if ev.event != "ping" {
            return ev;
        }
    }
}

pub async fn next_named(es: &mut Events, name: &str) -> Event {
    loop {
        let ev = next_any(es).await;
        if ev.event == name {
            return ev;
        }
    }
}

pub async fn messages(es: &mut Events, n: usize) -> Vec<Event> {
    let mut out = Vec::new();
    while out.len() < n {
        out.push(next_named(es, "message").await);
    }
    out
}

pub async fn stream_ends(es: &mut Events) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), es.next()).await {
            Ok(None) => return true,
            Ok(Some(Ok(ev))) if ev.event == "ping" => continue,
            _ => return false,
        }
    }
}

pub fn id_of(ev: &Event) -> String {
    serde_json::from_str::<Value>(&ev.data).unwrap()["id"].as_str().unwrap().to_owned()
}

pub async fn wait_until(cond: impl Fn() -> bool) {
    for _ in 0..250 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condição não chegou em 5 s");
}
```

```rust
// crates/hangar-server/tests/conversations.rs
//! Histórico e chat ao vivo servidos pelo hangar-server, com o Python falso no lugar do backend.
mod fake;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fake::*;
use hangar_server::transcript::{InternalInfo, merged_history};
use serde_json::{Value, json};

async fn setup(lines: std::ops::Range<usize>, stem: &str) -> (tempfile::TempDir, PathBuf, Vec<u64>, Arc<Fake>, SocketAddr) {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join(format!("{stem}.jsonl"));
    let offs = append_lines(&jsonl, lines);
    let (fake, up) = spawn_fake().await;
    fake.set_info(info_json("claude", &jsonl));
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    (dir, jsonl, offs, fake, srv)
}

#[tokio::test]
async fn events_open_with_ping_headers_and_backfill_of_200() {
    let (_dir, jsonl, offs, _fake, srv) = setup(0..250, "sess-a").await;

    let mut raw = open_events(srv, "s", "", &[("origin", "http://outra")]).await;
    assert_eq!(raw.headers()["content-type"], "text/event-stream; charset=utf-8");
    assert_eq!(raw.headers()["cache-control"], "no-store");
    assert_eq!(raw.headers()["x-accel-buffering"], "no");
    assert_eq!(raw.headers()["access-control-allow-origin"], "*");
    let first = tokio::time::timeout(Duration::from_secs(5), raw.chunk()).await.unwrap().unwrap().unwrap();
    assert!(first.starts_with(b"event: ping\r\ndata: {}\r\n\r\n"), "{:?}", first);
    assert!(!String::from_utf8_lossy(&first).contains("retry:"));
    drop(raw);

    let mut es = sse(open_events(srv, "s", "", &[]).await);
    assert_eq!(next_any(&mut es).await.event, "ping");
    let msgs = messages(&mut es, 200).await;
    assert_eq!(msgs[0].id, format!("sess-a:{}", offs[50]));
    assert_eq!(id_of(&msgs[0]), "u50");
    assert_eq!(id_of(&msgs[199]), "u249");

    let more = append_lines(&jsonl, 250..251);
    let live = messages(&mut es, 1).await;
    assert_eq!(live[0].id, format!("sess-a:{}", more[0]));
    assert_eq!(id_of(&live[0]), "u250");
}

#[tokio::test]
async fn events_resume_query_beats_header_and_bad_cursor_falls_back_to_tail() {
    let (_dir, _jsonl, offs, _fake, srv) = setup(0..250, "sess-b").await;

    let q = format!("last_event_id=sess-b:{}", offs[240]);
    let header = format!("sess-b:{}", offs[10]);
    let mut es = sse(open_events(srv, "s", &q, &[("last-event-id", header.as_str())]).await);
    let msgs = messages(&mut es, 10).await;
    assert_eq!(id_of(&msgs[0]), "u240");
    assert_eq!(id_of(&msgs[9]), "u249");

    let mut so_header = sse(open_events(srv, "s", "", &[("last-event-id", header.as_str())]).await);
    assert_eq!(id_of(&messages(&mut so_header, 1).await[0]), "u10");

    let mut outro = sse(open_events(srv, "s", "last_event_id=outro:0", &[]).await);
    assert_eq!(id_of(&messages(&mut outro, 1).await[0]), "u50");

    let mut alem = sse(open_events(srv, "s", "last_event_id=sess-b:999999999", &[]).await);
    assert_eq!(id_of(&messages(&mut alem, 1).await[0]), "u50");
}

#[tokio::test]
async fn partial_line_is_delivered_once_when_completed() {
    let (_dir, jsonl, _offs, _fake, srv) = setup(0..3, "sess-c").await;
    let off3 = std::fs::metadata(&jsonl).unwrap().len();
    let line3 = claude_line(3);
    let (head, tail) = line3.split_at(20);
    append_raw(&jsonl, head);

    let mut a = sse(open_events(srv, "s", "", &[]).await);
    let got = messages(&mut a, 3).await;
    assert_eq!(id_of(&got[2]), "u2");

    // Reconecta no meio da gravação, retomando da última mensagem recebida.
    let mut b = sse(open_events(srv, "s", &format!("last_event_id={}", got[2].id), &[]).await);
    assert_eq!(id_of(&messages(&mut b, 1).await[0]), "u2");

    append_raw(&jsonl, tail);
    for es in [&mut a, &mut b] {
        let m = messages(es, 1).await;
        assert_eq!(id_of(&m[0]), "u3");
        assert_eq!(m[0].id, format!("sess-c:{off3}"));
    }
    append_lines(&jsonl, 4..5);
    assert_eq!(id_of(&messages(&mut a, 1).await[0]), "u4", "a linha 3 não pode vir de novo");
}

#[tokio::test]
async fn devices_share_one_internal_connection_and_late_one_gets_snapshot() {
    let (_dir, _jsonl, _offs, fake, srv) = setup(0..1, "sess-d").await;
    let mut a = sse(open_events(srv, "s", "", &[]).await);
    messages(&mut a, 1).await;
    wait_until(|| fake.side_conns() == 1).await;

    let state = r#"{"session":"s","state":"idle"}"#;
    let queued = r#"{"kind":"user_msg","id":"queued-7","text":"na fila"}"#;
    fake.push_side("state", state);
    fake.push_side("message", queued);
    assert_eq!(next_named(&mut a, "state").await.data, state);
    assert_eq!(id_of(&next_named(&mut a, "message").await), "queued-7");

    let mut b = sse(open_events(srv, "s", "", &[]).await);
    assert_eq!(id_of(&messages(&mut b, 1).await[0]), "u0");
    assert_eq!(next_named(&mut b, "state").await.data, state);
    assert_eq!(id_of(&next_named(&mut b, "message").await), "queued-7");

    assert_eq!(fake.side_conns(), 1);
    assert_eq!(fake.side_apps(), vec!["1".to_string()]);
}

#[tokio::test]
async fn new_info_resets_every_device_and_follows_new_file() {
    let (dir, _a, _offs, fake, srv) = setup(0..2, "sess-e").await;
    let b_path = dir.path().join("sess-e2.jsonl");
    append_lines(&b_path, 100..102);

    let mut a = sse(open_events(srv, "s", "", &[]).await);
    let mut b = sse(open_events(srv, "s", "", &[]).await);
    messages(&mut a, 2).await;
    messages(&mut b, 2).await;
    wait_until(|| fake.side_conns() == 1).await;

    let novo = info_json("claude", &b_path);
    fake.set_info(novo.clone());
    fake.push_side("info", &novo.to_string());
    for es in [&mut a, &mut b] {
        assert_eq!(next_non_ping(es).await.event, "reset");
        let m = messages(es, 2).await;
        assert_eq!(id_of(&m[0]), "u100");
        assert!(m[0].id.starts_with("sess-e2:"));
    }
    assert_eq!(fake.side_conns(), 1, "a troca vem pela conexão que já existe");
}

#[tokio::test]
async fn provider_outside_rust_resets_and_next_connection_goes_to_python() {
    let (_dir, jsonl, _offs, fake, srv) = setup(0..1, "sess-f").await;
    let mut a = sse(open_events(srv, "s", "", &[]).await);
    messages(&mut a, 1).await;
    wait_until(|| fake.side_conns() == 1).await;

    let pi = json!({"provider": "pi", "jsonl": jsonl, "session_key": "sess-f", "history": {}});
    fake.set_info(pi.clone());
    fake.push_side("info", &pi.to_string());
    assert_eq!(next_non_ping(&mut a).await.event, "reset");
    assert!(stream_ends(&mut a).await);

    let r = open_events(srv, "s", "", &[]).await;
    assert_eq!(r.text().await.unwrap(), "from-python");
}

#[tokio::test]
async fn truncated_transcript_resets_and_rereads_from_start() {
    let (_dir, jsonl, _offs, _fake, srv) = setup(0..3, "sess-g").await;
    let mut a = sse(open_events(srv, "s", "", &[]).await);
    messages(&mut a, 3).await;

    std::fs::write(&jsonl, claude_line(9)).unwrap();
    assert_eq!(next_non_ping(&mut a).await.event, "reset");
    let m = messages(&mut a, 1).await;
    assert_eq!(id_of(&m[0]), "u9");
    assert_eq!(m[0].id, "sess-g:0");
}

#[tokio::test]
async fn same_name_with_new_transcript_never_serves_the_dead_one() {
    let (dir, _a, _offs, fake, srv) = setup(0..1, "sess-morta").await;
    let mut a = sse(open_events(srv, "s", "", &[]).await);
    assert_eq!(id_of(&messages(&mut a, 1).await[0]), "u0");
    wait_until(|| fake.side_conns() == 1).await;

    // A sessão morreu e nasceu outra com o mesmo nome; o cache de info já venceu.
    let nova = dir.path().join("sess-nova.jsonl");
    append_lines(&nova, 100..101);
    fake.set_info(info_json("claude", &nova));
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let mut b = sse(open_events(srv, "s", "", &[]).await);
    let first = next_non_ping(&mut b).await;
    assert_eq!(first.event, "message");
    assert_eq!(id_of(&first), "u100");
    assert!(first.id.starts_with("sess-nova:"));

    assert_eq!(next_non_ping(&mut a).await.event, "reset");
    assert_eq!(id_of(&messages(&mut a, 1).await[0]), "u100");
    wait_until(|| fake.side_conns() == 2).await;
}

#[tokio::test]
async fn events_without_owner_token_goes_to_python() {
    let (_dir, _jsonl, _offs, fake, srv) = setup(0..1, "sess-h").await;
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/events?token=errado"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    assert_eq!(fake.info_calls(), 0);
}

#[tokio::test]
async fn history_is_served_by_rust_with_etag_limit_gzip_and_cors() {
    let (_dir, jsonl, _offs, fake, srv) = setup(0..40, "sess-i").await;
    let info: InternalInfo = serde_json::from_value(info_json("claude", &jsonl)).unwrap();
    let req = info.history_request(None).expect("history_field tem de casar com o formato da Task 9");
    let expected = serde_json::to_value(merged_history(&req).unwrap()).unwrap();
    let url = format!("http://{srv}/api/sessions/s/history");

    let r = client().get(&url).bearer_auth(OWNER).header("origin", "http://outra").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/json");
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    assert_eq!(r.headers()["access-control-expose-headers"], "ETag");
    let etag = r.headers().get("etag").expect("etag").to_str().unwrap().to_owned();
    let got: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(got, expected);
    assert_eq!(fake.hits_to("/api/sessions/s/history"), 0, "não passou pelo Python");

    let r304 = client().get(&url).bearer_auth(OWNER).header("if-none-match", &etag).send().await.unwrap();
    assert_eq!(r304.status(), 304);
    assert_eq!(r304.headers()["etag"], etag.as_str());

    let rz = client().get(&url).bearer_auth(OWNER).header("accept-encoding", "gzip").send().await.unwrap();
    assert_eq!(rz.headers()["content-encoding"], "gzip");

    let rl = client().get(format!("{url}?limit=5")).bearer_auth(OWNER).send().await.unwrap();
    let tail: Value = serde_json::from_str(&rl.text().await.unwrap()).unwrap();
    let all = expected.as_array().unwrap();
    assert_eq!(tail.as_array().unwrap().as_slice(), &all[all.len() - 5..]);
}

#[tokio::test]
async fn history_negative_limit_is_the_whole_history_like_python() {
    let (_dir, jsonl, _offs, fake, srv) = setup(0..40, "sess-k").await;
    let info: InternalInfo = serde_json::from_value(info_json("claude", &jsonl)).unwrap();
    let expected = serde_json::to_value(merged_history(&info.history_request(None).unwrap()).unwrap()).unwrap();
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/history?limit=-5"))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let got: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(got, expected);
    assert_eq!(fake.hits_to("/api/sessions/s/history"), 0, "não passou pelo Python");
}

#[tokio::test]
async fn history_without_owner_or_supported_provider_goes_to_python() {
    let (_dir, jsonl, _offs, fake, srv) = setup(0..2, "sess-j").await;
    let url = format!("http://{srv}/api/sessions/s/history");

    let r = client().get(&url).bearer_auth("convidado").send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    assert_eq!(fake.info_calls(), 0);

    let r = client().get(format!("{url}?limit=abc")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");

    fake.set_info(json!({"provider": "pi", "jsonl": jsonl, "session_key": "sess-j", "history": {}}));
    let r = client().get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    assert_eq!(fake.info_calls(), 1, "o dono vindo do loopback consulta a rota interna");
}
```

- [x] **Step 3: Rodar e ver falhar** (quando autorizado)

Run: `(cd crates && cargo test -p hangar-server --test conversations)`
Expected: FAIL: com as rotas ainda caindo no repasse, o corpo é `from-python`, os testes de `/events` param em `evento em 5 s` e os de `/history` em `assert_eq!(fake.hits_to(...), 0)`.

- [x] **Step 4: Implementar `tail.rs`**

```rust
// crates/hangar-server/src/tail.rs
//! Leitura do transcript para o chat ao vivo: um observador por pasta, um leitor por jsonl e o
//! mesmo quadro SSE já serializado para todos os aparelhos daquele chat.
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use bytes::Bytes;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{Notify, broadcast};

use crate::side::Out;
use crate::transcript::{LineParser, Provider, SKIPPED_LINES};

/// Linhas da cauda de quem conecta sem posição.
pub const BACKFILL_LINES: usize = 200;
const TAIL_WINDOW: u64 = 256 * 1024;
/// Releitura sem aviso do sistema de arquivos: cobre aviso perdido e a fresta entre ler e armar
/// o observador, como o relógio do awatch no Python.
const HEARTBEAT: Duration = Duration::from_secs(5);
/// Pasta que ainda não existe (sessão nascendo): nova tentativa a cada segundo, aviso uma vez.
const DIR_RETRY: Duration = Duration::from_secs(1);
const DIR_WARN_AFTER: u32 = 30;

/// Quadro no formato do sse_starlette: id, event, data, linha em branco, tudo com `\r\n`.
pub fn sse_frame(event: &str, data: &str, id: Option<&str>) -> Bytes {
    let mut s = String::with_capacity(data.len() + event.len() + 48);
    if let Some(id) = id {
        s.push_str("id: ");
        s.push_str(id);
        s.push_str("\r\n");
    }
    s.push_str("event: ");
    s.push_str(event);
    s.push_str("\r\n");
    // Quebra de linha no dado vira várias linhas `data:`, como no sse_starlette.
    for line in data.replace("\r\n", "\n").split(['\r', '\n']) {
        s.push_str("data: ");
        s.push_str(line);
        s.push_str("\r\n");
    }
    s.push_str("\r\n");
    Bytes::from(s)
}

pub fn ping_frame() -> Bytes {
    sse_frame("ping", "{}", None)
}

pub fn reset_frame() -> Bytes {
    sse_frame("reset", "{}", None)
}

/// Keep-alive em comentário, que o EventSource ignora.
pub fn comment_frame() -> Bytes {
    Bytes::from(format!(": ping - {}\r\n\r\n", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.6f+00:00")))
}

/// Linhas completas de `from` até `to` (ou o fim) em quadros `message` com `id: <key>:<início da
/// linha>`. Linha sem `\n` fica para a próxima leitura; arquivo ausente não é erro.
pub fn read_frames(
    path: &Path,
    from: u64,
    to: Option<u64>,
    parser: &mut LineParser,
    key: &str,
) -> std::io::Result<(Vec<Bytes>, u64)> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), from)),
        Err(e) => return Err(e),
    };
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(from))?;
    let mut frames = Vec::new();
    let mut at = from;
    let mut line = Vec::new();
    let skipped_before = SKIPPED_LINES.load(Ordering::Relaxed);
    while to.is_none_or(|t| at < t) {
        line.clear();
        let n = reader.read_until(b'\n', &mut line)?;
        if n == 0 || line.last() != Some(&b'\n') {
            break;
        }
        let id = format!("{key}:{at}");
        for ev in parser.feed(&line, at) {
            let data = serde_json::to_string(&ev).map_err(std::io::Error::other)?;
            frames.push(sse_frame("message", &data, Some(&id)));
        }
        at += n as u64;
    }
    log_skipped(key, skipped_before);
    Ok((frames, at))
}

/// Registra as linhas que o parser pulou desde `before`: só a conta e a chave, nunca o texto.
// ponytail: o contador é global; leitura simultânea de outra sessão pode entrar na conta. Um
// contador por leitura, se o log precisar apontar o arquivo exato.
pub fn log_skipped(key: &str, before: u64) {
    let skipped = SKIPPED_LINES.load(Ordering::Relaxed).saturating_sub(before);
    if skipped > 0 {
        tracing::warn!(key, skipped, "linhas ilegíveis do transcript puladas");
    }
}

fn read_window(f: &mut File, size: u64, window: u64) -> Option<(u64, Vec<u8>)> {
    let start = size.saturating_sub(window);
    let mut buf = Vec::new();
    f.seek(SeekFrom::Start(start)).ok()?;
    f.take(size - start).read_to_end(&mut buf).ok()?;
    Some((start, buf))
}

/// Fim da última linha completa: onde o leitor compartilhado começa.
fn complete_end(path: &Path) -> u64 {
    let Ok(mut f) = File::open(path) else { return 0 };
    let Ok(size) = f.seek(SeekFrom::End(0)) else { return 0 };
    let mut window = TAIL_WINDOW;
    loop {
        let Some((start, buf)) = read_window(&mut f, size, window) else { return 0 };
        if let Some(i) = buf.iter().rposition(|&b| b == b'\n') {
            return start + i as u64 + 1;
        }
        if start == 0 {
            return 0;
        }
        window *= 4;
    }
}

/// Início da `max_lines`-ésima linha contada do fim; poucas linhas, arquivo vazio ou ausente → 0.
/// Só conta `\n`: cauda sem `\n` (gravação em curso) não entra.
pub fn tail_offset(path: &Path, max_lines: usize) -> u64 {
    let Ok(mut f) = File::open(path) else { return 0 };
    let Ok(size) = f.seek(SeekFrom::End(0)) else { return 0 };
    let mut window = TAIL_WINDOW;
    loop {
        let Some((start, buf)) = read_window(&mut f, size, window) else { return 0 };
        if buf.iter().filter(|&&b| b == b'\n').count() > max_lines {
            let mut idx = buf.len();
            for _ in 0..=max_lines {
                idx = buf[..idx].iter().rposition(|&b| b == b'\n').expect("contado acima");
            }
            return start + idx as u64 + 1;
        }
        if start == 0 {
            return 0;
        }
        // Janela curta, ou uma linha gigante (imagem em base64): cresce.
        window *= 4;
    }
}

/// Cauda de UM aparelho, de onde ele pediu até `cut`, com parser próprio (cada leitor tem o seu).
/// `resume` só vale com o stem desta sessão e dentro do arquivo; senão, as últimas 200 linhas.
pub fn backfill(path: &Path, key: &str, provider: Provider, resume: Option<&str>, cut: u64) -> Vec<Bytes> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let start = resume
        .and_then(|raw| {
            let (stem, off) = raw.rsplit_once(':')?;
            if stem.is_empty() || stem != key {
                return None;
            }
            off.trim().parse::<u64>().ok().filter(|o| *o <= size)
        })
        .unwrap_or_else(|| tail_offset(path, BACKFILL_LINES));
    if start >= cut {
        return Vec::new();
    }
    let mut parser = LineParser::new(provider);
    match read_frames(path, start, Some(cut), &mut parser, key) {
        Ok((frames, _)) => frames,
        Err(e) => {
            tracing::warn!(key, "cauda do transcript falhou: {e}");
            Vec::new()
        }
    }
}

/// Um observador por pasta, compartilhado por todos os leitores de arquivos dela.
#[derive(Clone, Default)]
pub struct Watchers(Arc<Mutex<HashMap<PathBuf, DirWatch>>>);

struct DirWatch {
    _watcher: RecommendedWatcher,
    subs: Vec<(OsString, Weak<Notify>)>,
}

impl Watchers {
    /// Inscreve o arquivo no observador da pasta dele. false = pasta inexistente ou observador
    /// que não armou; quem chama relê no relógio e tenta de novo.
    pub fn subscribe(&self, file: &Path, wake: &Arc<Notify>) -> bool {
        let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else { return false };
        let mut map = self.0.lock().unwrap();
        if !map.contains_key(dir) {
            let shared = Arc::downgrade(&self.0);
            let key = dir.to_path_buf();
            let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let (Ok(ev), Some(shared)) = (res, shared.upgrade()) else { return };
                let map = shared.lock().unwrap();
                let Some(d) = map.get(&key) else { return };
                // Escrita de arquivo irmão (subagente) não acorda este leitor.
                for p in &ev.paths {
                    let Some(n) = p.file_name() else { continue };
                    for (sub, w) in &d.subs {
                        if sub == n {
                            if let Some(w) = w.upgrade() {
                                w.notify_one();
                            }
                        }
                    }
                }
            });
            let Ok(mut watcher) = watcher else { return false };
            if watcher.watch(dir, RecursiveMode::NonRecursive).is_err() {
                return false;
            }
            map.insert(dir.to_path_buf(), DirWatch { _watcher: watcher, subs: Vec::new() });
        }
        let d = map.get_mut(dir).expect("inserido acima");
        d.subs.retain(|(_, w)| w.strong_count() > 0);
        d.subs.push((name.to_os_string(), Arc::downgrade(wake)));
        true
    }

    pub fn unsubscribe(&self, file: &Path, wake: &Arc<Notify>) {
        let Some(dir) = file.parent() else { return };
        let removed = {
            let mut map = self.0.lock().unwrap();
            let Some(d) = map.get_mut(dir) else { return };
            d.subs.retain(|(_, w)| w.strong_count() > 0 && !std::ptr::eq(w.as_ptr(), Arc::as_ptr(wake)));
            if d.subs.is_empty() { map.remove(dir) } else { None }
        };
        // Solta o observador fora da trava: o callback dele também a pega.
        drop(removed);
    }
}

pub struct TailState {
    path: PathBuf,
    key: String,
    provider: Provider,
    gen: u64,
    pos: Option<u64>,
    parser: LineParser,
}

impl TailState {
    /// Ponto até onde o leitor compartilhado já leu; na primeira consulta, o fim das linhas
    /// completas (a cauda de cada aparelho cobre o que vem antes).
    pub fn cut(&mut self) -> u64 {
        *self.pos.get_or_insert_with(|| complete_end(&self.path))
    }

    /// Lê o que chegou e manda a todos, sob a trava. Arquivo menor que a posição = truncado:
    /// `reset` e releitura do início com parser novo.
    fn poll(&mut self, tx: &broadcast::Sender<Out>) {
        let Some(pos) = self.pos else {
            self.cut();
            return;
        };
        let Ok(meta) = std::fs::metadata(&self.path) else { return };
        if meta.len() < pos {
            tracing::info!(key = %self.key, "transcript encolheu; recomeça do início");
            self.parser = LineParser::new(self.provider);
            self.pos = Some(0);
            let _ = tx.send(Out::Tail(self.gen, reset_frame()));
        }
        let from = self.pos.unwrap_or(0);
        match read_frames(&self.path, from, None, &mut self.parser, &self.key) {
            Ok((frames, end)) => {
                self.pos = Some(end);
                for f in frames {
                    let _ = tx.send(Out::Tail(self.gen, f));
                }
            }
            Err(e) => tracing::warn!(key = %self.key, "leitura do transcript falhou: {e}"),
        }
    }
}

/// Leitor compartilhado de um jsonl. Some ao ser solto (troca de transcript ou último aparelho).
pub struct FileTail {
    pub state: Arc<tokio::sync::Mutex<TailState>>,
    task: tokio::task::AbortHandle,
}

impl Drop for FileTail {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FileTail {
    pub fn spawn(
        path: PathBuf,
        key: String,
        provider: Provider,
        gen: u64,
        tx: broadcast::Sender<Out>,
        watchers: Watchers,
    ) -> Arc<FileTail> {
        let state = Arc::new(tokio::sync::Mutex::new(TailState {
            path: path.clone(),
            key,
            provider,
            gen,
            pos: None,
            parser: LineParser::new(provider),
        }));
        let task = tokio::spawn(run(path, state.clone(), tx, watchers)).abort_handle();
        Arc::new(FileTail { state, task })
    }
}

/// Inscrição que sai junto com a tarefa: o abort derruba o future e roda o Drop.
struct Subscription {
    watchers: Watchers,
    path: PathBuf,
    wake: Arc<Notify>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.watchers.unsubscribe(&self.path, &self.wake);
    }
}

async fn run(path: PathBuf, state: Arc<tokio::sync::Mutex<TailState>>, tx: broadcast::Sender<Out>, watchers: Watchers) {
    let sub = Subscription { watchers, path, wake: Arc::new(Notify::new()) };
    let mut watching = false;
    let mut misses = 0u32;
    loop {
        if !watching {
            watching = sub.watchers.subscribe(&sub.path, &sub.wake);
            if !watching {
                misses += 1;
                if misses == DIR_WARN_AFTER {
                    tracing::warn!(path = %sub.path.display(), "pasta do transcript ausente ou sem observador; segue relendo a cada segundo");
                }
            }
        }
        let st = state.clone();
        let tx2 = tx.clone();
        if tokio::task::spawn_blocking(move || st.blocking_lock().poll(&tx2)).await.is_err() {
            return;
        }
        let wait = if watching { HEARTBEAT } else { DIR_RETRY };
        tokio::select! {
            _ = sub.wake.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, lines: std::ops::Range<usize>, tail: &str) -> Vec<u64> {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        let mut at = f.metadata().unwrap().len();
        let mut offs = Vec::new();
        for i in lines {
            let l = format!(
                "{{\"type\":\"user\",\"uuid\":\"u{i}\",\"timestamp\":\"2026-10-02T10:{m:02}:{s:02}.000Z\",\"message\":{{\"role\":\"user\",\"content\":\"linha {i}\"}}}}\n",
                m = i / 60 % 60,
                s = i % 60
            );
            f.write_all(l.as_bytes()).unwrap();
            offs.push(at);
            at += l.len() as u64;
        }
        f.write_all(tail.as_bytes()).unwrap();
        offs
    }

    fn first_line(f: &Bytes) -> String {
        String::from_utf8_lossy(f).split("\r\n").next().unwrap().to_owned()
    }

    #[test]
    fn frame_matches_sse_starlette() {
        assert_eq!(
            &sse_frame("message", "{\"a\":1}", Some("k:5"))[..],
            b"id: k:5\r\nevent: message\r\ndata: {\"a\":1}\r\n\r\n"
        );
        assert_eq!(&sse_frame("x", "a\r\nb\nc", None)[..], b"event: x\r\ndata: a\r\ndata: b\r\ndata: c\r\n\r\n");
        let c = comment_frame();
        assert!(c.starts_with(b": ping - ") && c.ends_with(b"+00:00\r\n\r\n"));
    }

    #[test]
    fn tail_offset_counts_complete_lines_from_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        let offs = write(&p, 0..5, "{\"parcial\":");
        assert_eq!(tail_offset(&p, 2), offs[3]);
        assert_eq!(tail_offset(&p, 5), 0);
        assert_eq!(tail_offset(&dir.path().join("nao-existe.jsonl"), 2), 0);
    }

    #[test]
    fn read_frames_stops_before_partial_line_and_ids_line_start() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        let offs = write(&p, 0..3, "{\"parcial\":");
        let mut parser = LineParser::new(Provider::Claude);
        let (frames, end) = read_frames(&p, 0, None, &mut parser, "k").unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(first_line(&frames[1]), format!("id: k:{}", offs[1]));
        assert_eq!(end, complete_end(&p));
        assert!(end < std::fs::metadata(&p).unwrap().len());
    }

    #[test]
    fn backfill_honours_only_own_stem_inside_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        let offs = write(&p, 0..250, "");
        let cut = complete_end(&p);
        let own = backfill(&p, "k", Provider::Claude, Some(&format!("k:{}", offs[245])), cut);
        assert_eq!(own.len(), 5);
        assert_eq!(first_line(&own[0]), format!("id: k:{}", offs[245]));
        let foreign = backfill(&p, "k", Provider::Claude, Some(&format!("outro:{}", offs[245])), cut);
        assert_eq!(foreign.len(), 200);
        assert_eq!(first_line(&foreign[0]), format!("id: k:{}", offs[50]));
        assert_eq!(backfill(&p, "k", Provider::Claude, Some("k:999999999"), cut).len(), 200);
        assert_eq!(backfill(&p, "k", Provider::Claude, None, offs[10]).len(), 0, "cauda começa depois do corte");
    }
}
```

- [x] **Step 5: Implementar `side.rs`**

```rust
// crates/hangar-server/src/side.rs
//! Uma conexão interna por sessão (Python → hangar-server) e o hub que reparte, entre os
//! aparelhos daquele chat, o que vem dela e o que o leitor do transcript produz.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::StatusCode;
use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use http_body_util::BodyDataStream;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::sync::broadcast;

use crate::proxy::HttpClient;
use crate::tail::{self, FileTail, Watchers, sse_frame};
use crate::transcript::{InternalInfo, Provider};

#[derive(Clone, Debug)]
pub enum Out {
    /// Quadro do leitor do transcript, marcado com a geração da ligação que o leu.
    Tail(u64, Bytes),
    /// Quadro da conexão interna (estado, prévia, fila…); vale em qualquer geração.
    Side(Bytes),
    /// O transcript ou o provider trocou: cada aparelho manda `reset` e refaz a cauda.
    Rebind,
    /// Provider fora do Rust ou sessão sumida: `reset` e fim; ao reconectar, vai ao Python.
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub provider: Provider,
    pub jsonl: PathBuf,
    pub key: String,
}

impl Binding {
    /// Mesmo critério de `InternalInfo::history_request`: provider lido pelo Rust e com jsonl.
    pub fn from_info(info: &InternalInfo) -> Option<Binding> {
        Some(Binding {
            provider: Provider::parse(&info.provider)?,
            jsonl: info.jsonl.clone()?,
            key: info.session_key.clone(),
        })
    }
}

pub const INFO_TTL: Duration = Duration::from_secs(1);
pub type InfoCache = Arc<Mutex<HashMap<String, (Instant, Option<InternalInfo>)>>>;

/// Guarda o `info` mais recente; o da conexão interna também entra, para quem chega logo depois
/// de uma troca não religar o hub com um `info` velho.
pub fn remember_info(cache: &InfoCache, name: &str, info: Option<InternalInfo>) {
    let mut m = cache.lock().unwrap();
    if m.len() > 256 {
        m.retain(|_, (at, _)| at.elapsed() < INFO_TTL);
    }
    m.insert(name.to_string(), (Instant::now(), info));
}

/// Eventos cujo último valor vale para quem chega depois: o Python só os manda na mudança.
/// `nav` fica de fora: repetir um pedido já atendido reabriria o navegador; quem chega depois o
/// recebe pela lista de sessões.
const LATEST: [&str; 7] = ["state", "suggest", "ask_question", "stats", "preview", "pensamento", "ferramenta"];
const CHANNEL: usize = 1024;
const SIDE_CONNECT: Duration = Duration::from_secs(10);
/// O Python manda `ping` a cada 10 s; três calados = conexão morta.
const SIDE_IDLE: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct SideCtx {
    pub upstream: SocketAddr,
    pub secret: String,
    pub http: HttpClient,
    pub watchers: Watchers,
    pub hubs: Hubs,
    pub infos: InfoCache,
}

#[derive(Default)]
struct SideCache {
    latest: [Option<Bytes>; 7],
    queue: Vec<(String, Bytes)>,
}

impl SideCache {
    fn record(&mut self, event: &str, data: &str, frame: &Bytes) {
        if let Some(i) = LATEST.iter().position(|e| *e == event) {
            self.latest[i] = Some(frame.clone());
            return;
        }
        if event != "message" && event != "queue_confirmed" {
            return;
        }
        let id = serde_json::from_str::<serde_json::Value>(data)
            .ok()
            .and_then(|v| v.get("id")?.as_str().map(str::to_owned));
        let Some(id) = id else { return };
        match self.queue.iter_mut().find(|(k, _)| *k == id) {
            Some(slot) => slot.1 = frame.clone(),
            None => self.queue.push((id, frame.clone())),
        }
    }

    fn replay(&self) -> Vec<Bytes> {
        self.latest.iter().flatten().cloned().chain(self.queue.iter().map(|(_, f)| f.clone())).collect()
    }
}

#[derive(Clone)]
struct Bound {
    binding: Binding,
    gen: u64,
    tail: Arc<FileTail>,
}

pub struct Hub {
    pub name: String,
    pub tx: broadcast::Sender<Out>,
    ctx: SideCtx,
    bound: Mutex<Option<Bound>>,
    cache: Mutex<SideCache>,
    side: Mutex<Option<tokio::task::AbortHandle>>,
}

/// O que um aparelho recebe ao entrar: cauda + retrato, e o canal para o resto.
pub struct Attach {
    pub gen: u64,
    pub rx: broadcast::Receiver<Out>,
    pub frames: Vec<Bytes>,
}

impl Hub {
    fn start(name: &str, binding: Binding, ctx: SideCtx) -> Arc<Hub> {
        let (tx, _) = broadcast::channel(CHANNEL);
        let tail = FileTail::spawn(
            binding.jsonl.clone(),
            binding.key.clone(),
            binding.provider,
            0,
            tx.clone(),
            ctx.watchers.clone(),
        );
        let hub = Arc::new(Hub {
            name: name.to_string(),
            tx,
            ctx,
            bound: Mutex::new(Some(Bound { binding, gen: 0, tail })),
            cache: Mutex::default(),
            side: Mutex::new(None),
        });
        hub.restart_side();
        hub
    }

    /// Aparelho novo com `info` diferente (sessão recriada com o mesmo nome): troca o leitor já,
    /// para ele nunca receber a cauda do transcript morto, e religa a conexão interna, cujo
    /// primeiro `info` confirma ou corrige.
    fn ensure_current(self: &Arc<Self>, binding: &Binding) {
        let same = self.bound.lock().unwrap().as_ref().is_some_and(|b| b.binding == *binding);
        if !same {
            self.rebind(binding.clone());
            self.restart_side();
        }
    }

    fn restart_side(self: &Arc<Self>) {
        let mut side = self.side.lock().unwrap();
        if let Some(h) = side.take() {
            h.abort();
        }
        *side = Some(tokio::spawn(run_side(self.clone())).abort_handle());
    }

    fn stop(&self) {
        if let Some(h) = self.side.lock().unwrap().take() {
            h.abort();
        }
        self.bound.lock().unwrap().take();
    }

    fn rebind(&self, binding: Binding) {
        let mut bound = self.bound.lock().unwrap();
        let Some(cur) = bound.as_ref() else { return };
        let gen = cur.gen + 1;
        let tail = FileTail::spawn(
            binding.jsonl.clone(),
            binding.key.clone(),
            binding.provider,
            gen,
            self.tx.clone(),
            self.ctx.watchers.clone(),
        );
        *bound = Some(Bound { binding, gen, tail });
        self.cache.lock().unwrap().latest = Default::default();
        let _ = self.tx.send(Out::Rebind);
    }

    fn close(self: &Arc<Self>) {
        self.ctx.hubs.evict(&self.name, self);
        self.bound.lock().unwrap().take();
        let _ = self.tx.send(Out::Close);
    }

    /// Assina o canal e lê o ponto do leitor sob a trava dele (que é quem envia): nenhuma linha
    /// cai entre a cauda e o ao vivo. Troca de ligação no meio = tenta de novo.
    pub async fn attach(&self, resume: Option<String>) -> Option<Attach> {
        loop {
            let b = self.bound.lock().unwrap().clone()?;
            let guard = b.tail.state.clone().lock_owned().await;
            let rx = self.tx.subscribe();
            if self.bound.lock().unwrap().as_ref().map(|x| x.gen) != Some(b.gen) {
                continue;
            }
            let cached = self.cache.lock().unwrap().replay();
            let binding = b.binding.clone();
            let resume = resume.clone();
            let mut frames = tokio::task::spawn_blocking(move || {
                let mut g = guard;
                let cut = g.cut();
                drop(g);
                tail::backfill(&binding.jsonl, &binding.key, binding.provider, resume.as_deref(), cut)
            })
            .await
            .ok()?;
            frames.extend(cached);
            return Some(Attach { gen: b.gen, rx, frames });
        }
    }
}

enum SideEnd {
    Gone,
    Retry,
}

async fn run_side(hub: Arc<Hub>) {
    let mut attempt = 0u32;
    loop {
        match side_once(&hub, &mut attempt).await {
            SideEnd::Gone => {
                hub.close();
                return;
            }
            SideEnd::Retry => {}
        }
        // Religa com espera crescente: 1, 2, 4… até 30 s.
        let delay = (1u64 << attempt.min(5)).min(30);
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

async fn side_once(hub: &Arc<Hub>, attempt: &mut u32) -> SideEnd {
    let url = format!(
        "http://{}/internal/sessions/{}/side-events?app=1",
        hub.ctx.upstream,
        utf8_percent_encode(&hub.name, NON_ALPHANUMERIC)
    );
    let Ok(req) = axum::http::Request::get(url).header("x-hangar-internal", &hub.ctx.secret).body(Body::empty())
    else {
        return SideEnd::Gone;
    };
    let resp = match tokio::time::timeout(SIDE_CONNECT, hub.ctx.http.request(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(session = %hub.name, "conexão interna falhou: {e}");
            return SideEnd::Retry;
        }
        Err(_) => {
            tracing::warn!(session = %hub.name, "conexão interna sem resposta");
            return SideEnd::Retry;
        }
    };
    if resp.status() == StatusCode::NOT_FOUND {
        remember_info(&hub.ctx.infos, &hub.name, None);
        return SideEnd::Gone;
    }
    if !resp.status().is_success() {
        tracing::warn!(session = %hub.name, status = %resp.status(), "conexão interna recusada");
        return SideEnd::Retry;
    }
    let events = BodyDataStream::new(resp.into_body()).eventsource();
    let mut events = std::pin::pin!(events);
    let mut first = true;
    loop {
        let ev = match tokio::time::timeout(SIDE_IDLE, events.next()).await {
            Ok(Some(Ok(ev))) => ev,
            Ok(Some(Err(e))) => {
                tracing::warn!(session = %hub.name, "conexão interna: quadro inválido: {e}");
                return SideEnd::Retry;
            }
            Ok(None) => return SideEnd::Retry,
            Err(_) => {
                tracing::warn!(session = %hub.name, "conexão interna calada há 30 s; religa");
                return SideEnd::Retry;
            }
        };
        match ev.event.as_str() {
            "info" => {
                let info = match serde_json::from_str::<InternalInfo>(&ev.data) {
                    Ok(i) => i,
                    Err(e) => {
                        tracing::warn!(session = %hub.name, "info interna inválida: {e}");
                        return SideEnd::Retry;
                    }
                };
                remember_info(&hub.ctx.infos, &hub.name, Some(info.clone()));
                if first {
                    // Conexão nova manda a fila inteira de novo: o retrato anterior sai.
                    hub.cache.lock().unwrap().queue.clear();
                    *attempt = 0;
                    first = false;
                }
                let Some(binding) = Binding::from_info(&info) else {
                    tracing::info!(session = %hub.name, provider = %info.provider, "provider fora do Rust; aparelhos voltam ao Python");
                    return SideEnd::Gone;
                };
                let same = hub.bound.lock().unwrap().as_ref().is_some_and(|b| b.binding == binding);
                if !same {
                    tracing::info!(session = %hub.name, "transcript ou provider trocou; reset nos aparelhos");
                    hub.rebind(binding);
                }
            }
            "ping" => {}
            event => {
                let frame = sse_frame(event, &ev.data, None);
                // Retrato antes do envio: quem assina entre os dois recebe repetido, nunca nada.
                hub.cache.lock().unwrap().record(event, &ev.data, &frame);
                let _ = hub.tx.send(Out::Side(frame));
            }
        }
    }
}

/// Hubs vivos por nome de sessão, com a contagem de aparelhos.
#[derive(Clone, Default)]
pub struct Hubs(Arc<Mutex<HashMap<String, (Arc<Hub>, usize)>>>);

pub struct Lease {
    hubs: Hubs,
    pub hub: Arc<Hub>,
}

impl Hubs {
    pub fn acquire(&self, name: &str, binding: Binding, ctx: &SideCtx) -> Lease {
        let mut map = self.0.lock().unwrap();
        let hub = match map.get_mut(name) {
            Some((hub, n)) => {
                *n += 1;
                hub.clone()
            }
            None => {
                let hub = Hub::start(name, binding, ctx.clone());
                map.insert(name.to_string(), (hub.clone(), 1));
                return Lease { hubs: self.clone(), hub };
            }
        };
        drop(map);
        hub.ensure_current(&binding);
        Lease { hubs: self.clone(), hub }
    }

    fn evict(&self, name: &str, hub: &Arc<Hub>) {
        let mut map = self.0.lock().unwrap();
        if map.get(name).is_some_and(|(h, _)| Arc::ptr_eq(h, hub)) {
            map.remove(name);
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut map = self.hubs.0.lock().unwrap();
        let Some((h, n)) = map.get_mut(&self.hub.name) else { return };
        if !Arc::ptr_eq(h, &self.hub) {
            return;
        }
        *n -= 1;
        if *n == 0 {
            let (h, _) = map.remove(&self.hub.name).expect("presente acima");
            drop(map);
            // Último aparelho saiu: fecha a conexão interna (o Python roda app_saiu) e o leitor.
            h.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keeps_last_value_and_queue_by_id_without_nav() {
        let mut c = SideCache::default();
        let rec = |c: &mut SideCache, e: &str, d: &str| c.record(e, d, &sse_frame(e, d, None));
        rec(&mut c, "state", "{\"state\":\"working\"}");
        rec(&mut c, "state", "{\"state\":\"idle\"}");
        rec(&mut c, "message", "{\"id\":\"queued-1\"}");
        rec(&mut c, "queue_confirmed", "{\"id\":\"queued-1\",\"queued_confirmed\":true}");
        rec(&mut c, "nav", "{\"url\":\"http://x\"}");
        let r: Vec<String> = c.replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
        assert_eq!(r.len(), 2);
        assert!(r[0].starts_with("event: state") && r[0].contains("idle"));
        assert!(r[1].starts_with("event: queue_confirmed"));
    }
}
```

- [x] **Step 6: Implementar as rotas (`routes.rs`, substitui o da Task 11)**

```rust
// crates/hangar-server/src/routes.rs
//! Rotas do hangar-server: saúde, histórico e chat ao vivo do Claude e do Codex para o dono;
//! todo o resto é repasse ao Python.
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use http_body_util::BodyExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

use crate::auth::{self, Auth};
use crate::config::Config;
use crate::proxy::{self, Forward, HttpClient};
use crate::side::{Binding, Hubs, INFO_TTL, Lease, Out, SideCtx, remember_info};
use crate::tail::{self, Watchers};
use crate::transcript::{InternalInfo, SKIPPED_LINES, history_etag, merged_history};

const INFO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(10);
const COMMENT_EVERY: Duration = Duration::from_secs(15);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

pub struct AppState {
    pub cfg: Config,
    pub auth: Auth,
    pub http: HttpClient,
    pub side: SideCtx,
}

impl AppState {
    /// `info` da sessão com cache curto: várias telas abrindo juntas viram uma consulta só.
    async fn info(&self, name: &str) -> Option<InternalInfo> {
        if let Some((at, v)) = self.side.infos.lock().unwrap().get(name) {
            if at.elapsed() < INFO_TTL {
                return v.clone();
            }
        }
        let v = fetch_info(&self.http, self.cfg.upstream, &self.cfg.internal_secret, name).await;
        remember_info(&self.side.infos, name, v.clone());
        v
    }
}

async fn fetch_info(http: &HttpClient, upstream: SocketAddr, secret: &str, name: &str) -> Option<InternalInfo> {
    let url = format!("http://{upstream}/internal/sessions/{}/info", utf8_percent_encode(name, NON_ALPHANUMERIC));
    let req = axum::http::Request::get(url).header("x-hangar-internal", secret).body(Body::empty()).ok()?;
    let resp = tokio::time::timeout(INFO_TIMEOUT, http.request(req)).await.ok()?.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = tokio::time::timeout(INFO_TIMEOUT, resp.into_body().collect()).await.ok()?.ok()?.to_bytes();
    match serde_json::from_slice(&body) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(session = %name, "info interna inválida: {e}");
            None
        }
    }
}

pub async fn serve(listener: TcpListener, cfg: Config) -> std::io::Result<()> {
    let http = proxy::client();
    let side = SideCtx {
        upstream: cfg.upstream,
        secret: cfg.internal_secret.clone(),
        http: http.clone(),
        watchers: Watchers::default(),
        hubs: Hubs::default(),
        infos: Default::default(),
    };
    let state = Arc::new(AppState { auth: Auth::new(&cfg.auth_token), http, side, cfg });
    axum::serve(listener, router(state).into_make_service_with_connect_info::<SocketAddr>()).await
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/__hangar_server/health", get(health))
        // Outro método nessas rotas (preflight OPTIONS, HEAD) segue ao Python.
        .route("/api/sessions/{name}/history", get(history).fallback(pass_any))
        .route("/api/sessions/{name}/events", get(events).fallback(pass_any))
        .fallback(pass_any)
        .with_state(state)
}

async fn health(headers: HeaderMap) -> Response {
    let body = format!(
        "{{\"ok\":true,\"version\":\"{}\",\"protocol\":{}}}",
        env!("CARGO_PKG_VERSION"),
        crate::INTERNAL_PROTOCOL
    );
    let mut resp = ([(header::CONTENT_TYPE, "application/json")], body).into_response();
    cors(&headers, resp.headers_mut());
    resp
}

/// Quem pede e se é o dono, resolvidos uma vez por pedido.
pub(crate) fn gate(st: &AppState, peer: SocketAddr, req: &Request) -> (Forward, bool) {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    let token = auth::presented_token(req.headers(), req.uri().query(), req.method(), https);
    let owner = st.auth.is_owner(&client_ip, token.as_deref());
    (Forward { client_ip, https }, owner)
}

pub(crate) async fn pass(st: &AppState, req: Request, fwd: &Forward) -> Response {
    let resp = proxy::forward(&st.http, st.cfg.upstream, req, fwd).await;
    if resp.status() == StatusCode::UNAUTHORIZED {
        st.auth.record_fail(&fwd.client_ip);
    }
    resp
}

async fn pass_any(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    pass(&st, req, &Forward { client_ip, https }).await
}

/// Id de correlação do diário: cabeçalho x-hangar-req; o EventSource só manda ?diag_req.
fn diag_req(req: &Request) -> String {
    let h: String = req
        .headers()
        .get("x-hangar-req")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(32)
        .collect();
    if !h.is_empty() {
        return h;
    }
    auth::query_param(req.uri().query(), "diag_req")
        .filter(|c| (1..=32).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
        .unwrap_or_default()
}

async fn history(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    req: Request,
) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    let name = match path {
        Ok(Path(n)) if owner && req.method() == Method::GET => n,
        _ => return pass(&st, req, &fwd).await,
    };
    let limit = match auth::query_param(req.uri().query(), "limit") {
        None => None,
        Some(v) => match v.parse::<i64>() {
            // Como o Python: `limit <= 0` é o histórico inteiro (api.py:2918), nunca um 400.
            Ok(n) if n > 0 => usize::try_from(n).ok(),
            Ok(_) => None,
            // Texto ou vazio fica com o FastAPI e o 422 dele.
            Err(_) => return pass(&st, req, &fwd).await,
        },
    };
    let Some(hreq) = st.info(&name).await.and_then(|i| i.history_request(limit)) else {
        return pass(&st, req, &fwd).await;
    };
    tracing::info!(session = %name, req = %diag_req(&req), "history");
    let inm = req.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let log_name = name.clone();
    let done = tokio::task::spawn_blocking(move || -> std::io::Result<(Option<String>, Option<Vec<u8>>)> {
        let etag = history_etag(&hreq);
        if etag.is_some() && etag == inm {
            return Ok((etag, None));
        }
        let before = SKIPPED_LINES.load(Ordering::Relaxed);
        // Já vem com o corte `evs[-limit:]` da rota (Task 9).
        let evs = merged_history(&hreq)?;
        tail::log_skipped(&log_name, before);
        Ok((etag, Some(serde_json::to_vec(&evs)?)))
    })
    .await;
    let (etag, body) = match done {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::warn!(session = %name, "history no Rust falhou; repassa: {e}");
            return pass(&st, req, &fwd).await;
        }
        Err(e) => {
            tracing::warn!(session = %name, "history no Rust caiu; repassa: {e}");
            return pass(&st, req, &fwd).await;
        }
    };
    let mut resp = match body {
        None => StatusCode::NOT_MODIFIED.into_response(),
        Some(body) => {
            let mut r = Response::new(Body::empty());
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let body = maybe_gzip(req.headers(), r.headers_mut(), body);
            *r.body_mut() = Body::from(body);
            r
        }
    };
    if let Some(v) = etag.and_then(|e| HeaderValue::from_str(&e).ok()) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    cors(req.headers(), resp.headers_mut());
    resp
}

async fn events(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    req: Request,
) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    let name = match path {
        Ok(Path(n)) if owner && req.method() == Method::GET => n,
        _ => return pass(&st, req, &fwd).await,
    };
    let Some(binding) = st.info(&name).await.as_ref().and_then(Binding::from_info) else {
        return pass(&st, req, &fwd).await;
    };
    // A query vence: o app recria o EventSource a cada queda, e objeto novo não manda o cabeçalho.
    let resume = auth::query_param(req.uri().query(), "last_event_id")
        .filter(|v| !v.is_empty())
        .or_else(|| req.headers().get("last-event-id").and_then(|v| v.to_str().ok()).map(str::to_owned));
    tracing::info!(session = %name, req = %diag_req(&req), retomada = resume.is_some(), "events: abriu");
    let lease = st.side.hubs.acquire(&name, binding, &st.side);
    let (tx, rx) = mpsc::channel::<Bytes>(64);
    tokio::spawn(client_loop(lease, resume, tx));
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|b| (Ok::<Bytes, Infallible>(b), rx))
    });
    let mut resp = Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    cors(req.headers(), h);
    resp
}

async fn client_loop(lease: Lease, resume: Option<String>, out: mpsc::Sender<Bytes>) {
    let hub = lease.hub.clone();
    // O front dá 10 s para o primeiro quadro: o ping sai antes de qualquer leitura.
    if !push(&out, tail::ping_frame()).await {
        return;
    }
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + PING_EVERY, PING_EVERY);
    let mut comment = tokio::time::interval_at(start + COMMENT_EVERY, COMMENT_EVERY);
    let mut resume = resume;
    loop {
        let Some(att) = hub.attach(resume.take()).await else {
            let _ = push(&out, tail::reset_frame()).await;
            return;
        };
        for f in att.frames {
            if !push(&out, f).await {
                return;
            }
        }
        let (gen, mut rx) = (att.gen, att.rx);
        let rebind = loop {
            tokio::select! {
                _ = out.closed() => return,
                _ = ping.tick() => if !push(&out, tail::ping_frame()).await { return },
                _ = comment.tick() => if !push(&out, tail::comment_frame()).await { return },
                msg = rx.recv() => match msg {
                    Ok(Out::Tail(g, f)) if g == gen => if !push(&out, f).await { return },
                    Ok(Out::Tail(..)) => {}
                    Ok(Out::Side(f)) => if !push(&out, f).await { return },
                    Ok(Out::Rebind) => break true,
                    Ok(Out::Close) | Err(broadcast::error::RecvError::Closed) => break false,
                    // Atrasado demais para o canal: fecha, e o aparelho retoma pelo último id.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        tracing::info!(session = %hub.name, "aparelho atrasado; conexão fechada para retomar");
                        return;
                    }
                },
            }
        };
        if !push(&out, tail::reset_frame()).await || !rebind {
            return;
        }
    }
}

/// Envio preso por 30 s fecha a conexão, como o send_timeout do Python.
async fn push(out: &mpsc::Sender<Bytes>, frame: Bytes) -> bool {
    matches!(tokio::time::timeout(SEND_TIMEOUT, out.send(frame)).await, Ok(Ok(())))
}

/// CORS das respostas do próprio Rust, como o CORSMiddleware do Python: `*`, sem credenciais,
/// ETag legível pelo JS. Preflight não chega aqui: vai sem token e é repassado.
pub(crate) fn cors(req: &HeaderMap, resp: &mut HeaderMap) {
    if req.contains_key(header::ORIGIN) {
        resp.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        resp.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("ETag"));
    }
}

/// Gzip só quando o cliente pede e o corpo passa de 1 KB. Nunca chamado para text/event-stream.
pub(crate) fn maybe_gzip(req: &HeaderMap, resp: &mut HeaderMap, body: Vec<u8>) -> Vec<u8> {
    let wants = req
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("gzip"));
    if !wants || body.len() < 1024 {
        return body;
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(5));
    std::io::Write::write_all(&mut enc, &body).expect("escrita em memória");
    resp.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    resp.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    enc.finish().expect("escrita em memória")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn cors_only_with_origin() {
        let mut resp = HeaderMap::new();
        cors(&HeaderMap::new(), &mut resp);
        assert!(resp.is_empty());
        let mut req = HeaderMap::new();
        req.insert(header::ORIGIN, HeaderValue::from_static("http://outra"));
        cors(&req, &mut resp);
        assert_eq!(resp[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(resp[header::ACCESS_CONTROL_EXPOSE_HEADERS], "ETag");
    }

    #[test]
    fn gzip_only_when_asked_and_large() {
        let big = vec![b'a'; 2048];
        let mut resp = HeaderMap::new();
        assert_eq!(maybe_gzip(&HeaderMap::new(), &mut resp, big.clone()), big);
        let mut req = HeaderMap::new();
        req.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate"));
        assert_eq!(maybe_gzip(&req, &mut resp, b"curto".to_vec()), b"curto");
        assert!(resp.get(header::CONTENT_ENCODING).is_none());
        let packed = maybe_gzip(&req, &mut resp, big.clone());
        assert_eq!(resp[header::CONTENT_ENCODING], "gzip");
        assert_eq!(resp[header::VARY], "Accept-Encoding");
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&packed[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, big);
    }

    #[test]
    fn diag_req_prefers_header_then_valid_query() {
        let r = axum::http::Request::builder()
            .uri("/api/sessions/s/events?diag_req=abc_1-Z")
            .body(Body::empty())
            .unwrap();
        assert_eq!(diag_req(&r), "abc_1-Z");
        let r = axum::http::Request::builder()
            .uri("/api/sessions/s/events?diag_req=tem%20espaco")
            .body(Body::empty())
            .unwrap();
        assert_eq!(diag_req(&r), "");
        let long = "x".repeat(40);
        let r = axum::http::Request::builder()
            .uri("/api/sessions/s/events?diag_req=q")
            .header("x-hangar-req", long.as_str())
            .body(Body::empty())
            .unwrap();
        assert_eq!(diag_req(&r), "x".repeat(32));
    }
}
```

- [x] **Step 7: Rodar e ver passar** (quando autorizado)

Run: `(cd crates && cargo test -p hangar-server --lib -- tail side routes && cargo test -p hangar-server --test conversations --test proxy)`
Expected: PASS em todos. O `blocked_origin_never_gets_the_owner_shortcut` e o `guest_token_is_always_passed_to_python` da Task 11 agora exercitam o atalho de verdade: `history_without_owner_or_supported_provider_goes_to_python` prova que o dono vindo do loopback consulta a rota interna (`info_calls == 1`).

- [x] **Step 8: Commit**

```bash
git add crates/Cargo.toml crates/Cargo.lock crates/hangar-server/Cargo.toml crates/hangar-server/src/lib.rs crates/hangar-server/src/routes.rs crates/hangar-server/src/tail.rs crates/hangar-server/src/side.rs crates/hangar-server/tests/fake/mod.rs crates/hangar-server/tests/conversations.rs
git commit -m "feat(server): serve history and live events for Claude and Codex in hangar-server"
```

#### Notas (dependências e riscos)

**Dependências entre Tasks**
- Task 10 depende da Task 9 (`require_internal`, `set_secret`, `info_payload`). A rota `info` e o evento `info` saem da mesma função; se divergissem no provider (`claude` × `claude-headless`), o Rust veria dois bindings e mandaria um `reset` à toa a cada aparelho novo.
- Task 11 depende das Tasks 1, 4 e 6-9 (workspace, `tokio` no workspace, `transcript`, `[dev-dependencies]`). Só acrescenta linhas no `Cargo.toml` e no `lib.rs` e substitui o `main.rs`. O módulo de teste desta frente é `tests/fake/mod.rs`: `tests/common/mod.rs` é da Task 6 e tem os ajudantes do golden.
- Task 12 depende da frente 3 (parsers, `InternalInfo`, `merged_history`, `history_etag`, `SKIPPED_LINES`), da Task 10 e da Task 11. O `routes.rs` dela substitui o da Task 11 inteiro (a saúde com `protocol` está nas duas versões).
- O `transcript sintético da Task 6` não é usado: os testes geram linhas sintéticas próprias (`claude_line`), para não depender de nomes de arquivo de outra frente.

**Desvios do contrato ou do pedido**
- `broadcast` de `Out` com `Bytes`, não de `Arc<Bytes>`. O `Bytes` já é contado por referência; um `Arc` por cima seria uma segunda contagem. O enum carrega a geração do leitor (`Tail(gen, …)`), que é o que impede quadro do transcript velho de chegar depois do `reset`.
- `/events` decide se é do Rust por `Provider::parse` + `jsonl` (`Binding::from_info`), e não por `history_request(None)`. Pelo contrato, os dois critérios são equivalentes. Usar `history_request` aqui faria o chat ao vivo depender do formato de `history`. O `/history` usa `history_request`.
- O `side-events` continua rodando o `tail_pump` para todo provider, e não só para o Codex. Sem ele, `committed["text"]` nunca muda, e a prévia de um bloco já gravado volta a aparecer duplicada (sse.py:726-732, 795-798, 910). O custo é um leitor Python por sessão, não por aparelho.
- `app=1` sempre: o Rust só atende o dono, então todo aparelho do hub conta como app aberto (api.py:3381). O parâmetro `app` continua no Python, e `app=0` está testado.
- O 429 não sai do Rust. Ele anota os 401 que o Python devolve; com 8 em 30 s, para de avaliar o token e repassa, e o Python responde o 429 dele. Assim o atalho nunca responde 200 a um palpite durante o bloqueio, e o corpo do erro continua vindo de um lugar só. Um acerto do dono atendido pelo Rust zera só a conta do Rust; a do Python vence em 30 s.
- `offset > tamanho` volta para a cauda de 200 linhas, como o Python faz (transcript.py:896-904), e não "para o fim". A spec diz "volta para o fim"; o código atual diz cauda.
- Arquivo truncado: o Rust manda `reset` e relê do zero. O Python só relê do zero, sem `reset` (transcript.py:789-793). O `reset` foi pedido na Task e só faz o front recarregar o histórico.
- `nav` não entra no retrato de quem chega depois: repetir um pedido já confirmado reabriria o navegador. O aparelho que conecta depois recebe o `nav` pelo stream da lista (`/api/sessions/events`), que continua no Python (sse.py:631-643).
- Aparelho novo com `info` diferente do hub faz `rebind` na hora e religa a conexão interna. Os aparelhos já conectados recebem um `reset`. Se o `info` novo era uma oscilação da resolução, o primeiro `info` da conexão nova corrige com mais um `reset`, o mesmo que o `jsonl_watcher` faria numa conexão nova do Python.

- `X-Forwarded-For` ao Python: o Rust põe sempre (HTTP e WebSocket) um valor único, o cliente já resolvido pelo `TrustedHosts`, em vez de acrescentar ao que veio. Acrescentar ao valor de um vizinho não confiável deixaria o cliente forjar o IP, porque o uvicorn interno confia em `127.0.0.1` e seguiria varrendo para a esquerda. O efeito para o Python é o mesmo de acrescentar com vizinho confiável.

**Contrato com a Task 13**
- `CP_AUTH_TOKEN` e `CP_FORWARDED_ALLOW_IPS` vêm do `settings` no ambiente do filho (o `backend/.env` não chega ao `os.environ`, config.py:159). O uvicorn interno confia sempre em `127.0.0.1`.
- `protocol` da saúde casa com `RUST_SERVER_PROTOCOL` do Python; mudou o contrato interno, sobem os dois.
- Códigos de saída do binário: 2 = configuração inválida (não adianta religar), 1 = porta pública ocupada ou servidor parou, 0 = stdin fechou (o Python saiu).

**Riscos de comportamento (aceitos)**
- `RewriteFilter`: o leitor compartilhado guarda o estado desde que o hub nasceu, enquanto a cauda de cada aparelho usa parser novo. Só diverge numa reescrita do `--resume` cuja janela de 60 s caia entre os dois. Hoje cada conexão Python já tem o próprio estado.
- Aparelho que atrasa mais de 1024 quadros é desconectado e retoma pelo último id. Não perde nada, mas reconecta.
- Pasta do transcript apagada e recriada: o `notify` não rearma o observador, e o leitor segue pelo relógio de 5 s. Uma conexão interna que recebe 404 durante uma falha momentânea do tmux fecha o chat de todos os aparelhos, e eles reconectam pelo Python.
- O log (`HANGAR_SERVER_LOG`) não tem rotação.

### Task 13: Python sobe e vigia o `hangar-server`

**Files:**
- Create: `backend/app/rust_server.py`
- Modify: `backend/app/main.py:20` (import), `backend/app/main.py:231-248` (escolha entre Python sozinho e `hangar-server`)
- Modify: `backend/app/config.py:251` (campo `rust_server` logo depois de `forwarded_allow_ips`), `backend/app/config.py:344` (descrição do campo)
- Modify: `messages/pt.json:1893`, `messages/en.json:1893` (chave `config_server_env_rust_server` logo depois de `config_server_env_deploy_secret`)
- Modify: `scripts/windows-tasks.ps1:69` (o filho `hangar-server.exe` entra entre os processos do serviço)
- Test: `backend/tests/test_rust_server.py`
- Test: `scripts/test-windows-tasks.ps1:79` (dois cenários novos logo depois do assert "Precisao de WMI")

**Interfaces:**
- Consumes:
  - `app.rust_bins.find_bin(name: str, env_var: str) -> Path | None` (frente 2).
  - `GET /__hangar_server/health` → `{"ok":true,"version":"<CARGO_PKG_VERSION>","protocol":1}`, sem auth, na porta pública (Task 11, `hangar_server::INTERNAL_PROTOCOL`).
  - `app.internal_api.set_secret(value: str | None) -> None` (Task 9): o `require_internal` lê o segredo da memória do módulo a cada pedido.
  - O binário sai sozinho, com código 0, quando o stdin chega ao fim (Task 11, `hangar_server::parent_gone`).
- Produces:
  - `rust_server.RUST_SERVER_PROTOCOL = 1`, `rust_server.HEALTH_PATH = "/__hangar_server/health"`, `START_TIMEOUT = 10.0`, `CRASH_WINDOW = 60.0`, `MAX_CRASHES = 3`
  - `rust_server.wanted_binary(enabled: bool) -> Path | None`
  - `rust_server.listen_addr(host: str, port: int) -> str` (`"0.0.0.0:8765"`, `"[::]:8765"`)
  - `rust_server.trust_loopback(ips: str) -> str`
  - `rust_server.server_log_path() -> Path` (`log_paths.base()/privado/hangar-server.log`)
  - `rust_server._spawn(binary: Path, env: dict[str, str]) -> subprocess.Popen` (com `stdin=PIPE`; o `Popen` guardado segura a ponta de escrita enquanto o filho vive)
  - `async rust_server.serve(server: uvicorn.Server, sockets: list[socket.socket], binary: Path, kw: dict, token: str, bind_public: Callable[[], socket.socket]) -> bool` (`False` = porta pública ficou sem dono)
  - `rust_server.run(app: str, kw: dict, binary: Path, token: str, sockets: list[socket.socket], bind_public: Callable[[], socket.socket]) -> int` (código de saída: 0, 1 ou 3)
  - `Settings.rust_server: bool = True` (`CP_RUST_SERVER`, lido do ambiente **e** do `backend/.env`)
  - Ambiente do filho (só no dicionário passado ao `Popen`, nunca no `os.environ`): `HANGAR_SERVER_LISTEN`, `HANGAR_SERVER_UPSTREAM`, `HANGAR_INTERNAL_SECRET` (novo a cada subida do filho, entregue ao Python por `internal_api.set_secret`), `CP_AUTH_TOKEN` e `CP_FORWARDED_ALLOW_IPS` (os dois do `settings`, ver Notas), `HANGAR_SERVER_LOG`. O brief previa o segredo também no `os.environ` do Python; ali ele vazaria para toda sessão que o backend sobe.
  - Diário: `hangar_server.de_pe` (ok), `hangar_server.caiu` (aviso, `retorno`, `tentativa`), `hangar_server.protocolo` (erro, `esperado`, `recebido`), `hangar_server.reserva` (`codigo` = `sem_binario` | `sem_resposta` | `protocolo` | `quedas` | `erro` | `porta_ocupada`).

Regras herdadas que esta Task cobra:
- Um `uvicorn.Server` com lifespan por processo: "dois Server rodariam watchers e hooks em dobro" (conferido 2026-10-02: `backend/app/main.py:237`). O segundo servidor, o da porta pública na reserva, nasce com `lifespan="off"`, cuja subida é vazia (conferido 2026-10-02: `uvicorn/lifespan/off.py` `LifespanOff.startup` só tem `pass`; `uvicorn/config.py:57` mapeia `"off"` para ele e `uvicorn/server.py:105` chama `self.lifespan.startup()`). O app usa `app.state` e não estado de lifespan (conferido 2026-10-02: `backend/app/api.py:469` é `yield` sem valor; `api.py:449-460` grava em `app.state`), então a reserva atende igual.
- Um `uvicorn.Config` novo reconfigura o logging do uvicorn e tira os handlers do diário de `uvicorn.error` (conferido 2026-10-02: `uvicorn/config.py:284` chama `configure_logging()` no `__init__`; `backend/app/api.py:294-295` reinstala no lifespan, que a reserva não roda). Por isso a reserva chama `diag_logging.instalar()` de novo.
- Sinal com dois servidores: o último `capture_signals` recebe o SIGTERM, encerra, devolve os handlers do anterior e relança o sinal para ele (conferido 2026-10-02: `uvicorn/server.py:322-339`).
- **Código de retorno no Windows não se lê como falha** (`docs/decisoes/windows.md:27`): o código de saída do filho só vai para o diário (`retorno`), nunca decide nada; o que decide é "o processo terminou" e "a saúde respondeu".
- **`monkeypatch.setattr(os, "name", …)` leva o `pathlib` junto** (`docs/decisoes/windows.md:25`): o módulo ramifica por `sys.platform` no import e os testes não simulam plataforma; o ramo Windows é conferido na VM (Task 15).
- **Falha de decode em `subprocess` morre numa thread** (`docs/decisoes/windows.md:23`): o filho nasce com `stdout=DEVNULL` e stderr herdado; o Python nunca decodifica saída dele (o log do binário vai para `HANGAR_SERVER_LOG`).
- Flag do Windows como literal, nunca `subprocess.CREATE_NO_WINDOW`, que só existe no Windows (conferido 2026-10-02: `backend/app/atualizar.py:58-66`).
- **Conexão abortada no accept não pode fechar o listener** (`docs/decisoes/windows.md:54`): `resilient_accept.install()` continua antes de qualquer servidor (conferido 2026-10-02: `backend/app/main.py:224`), e o laço vem de `config.get_loop_factory()`, o mesmo que `server.run` usa (conferido 2026-10-02: `uvicorn/server.py:75`), então o Proactor do Windows continua o mesmo.
- **A tarefa Windows acompanha o processo; o reinício controlado não encerra a árvore inteira** (`docs/decisoes/windows.md:41`, `docs/decisoes/instalacao.md` "A tarefa Windows acompanha o processo até ele terminar"): nada de job object em volta do Python; o `hangar-server.exe` sai sozinho quando o Python morre, porque o cano do stdin fecha. E o `Restart-HangarTask` recusa reiniciar quando a porta é de "outro processo" (conferido 2026-10-02: `scripts/windows-tasks.ps1:70-74`). Com o filho segurando a 8765 no instante da parada, todo Atualizar no Windows pararia ali; o Step 6 reconhece o `hangar-server.exe` filho do backend.
- **O filho morre com o pai por um mecanismo só, em todo sistema**: o Python abre o stdin do filho como cano e o segura enquanto o filho vive; o binário lê o stdin até o fim e sai (Task 11). Morte dura do Python (SIGKILL, falha no Windows) fecha a ponta dele e o filho sai. Sem `preexec_fn`: ele roda entre `fork` e `exec` e não é seguro com threads vivas, e o backend já tem threads nessa hora.
- Guarda do token `change-me` em bind fora do loopback e saída 3 quando o servidor não subiu ficam como estão (conferido 2026-10-02: `backend/app/main.py:29-39,247-248`).
- `--reload` continua sem `hangar-server` (conferido 2026-10-02: `backend/app/main.py:233-236`): o reload recria o processo e deixaria o filho órfão segurando a porta; e a regra "Reiniciar o backend: sem `--reload`" (`docs/decisoes/instalacao.md:40`) já tira esse modo do uso real.
- Com o `hangar-server` na frente, todo pedido chega ao uvicorn interno de `127.0.0.1`. Sem confiar nesse endereço no `forwarded_allow_ips`, o `X-Forwarded-For` é ignorado, o limite de tentativas isenta loopback e o `require_loopback` aceita a LAN (conferido 2026-10-02: `backend/app/auth.py:148`, `auth.py:174-187`, padrão `config.py:251`). `trust_loopback` garante o `127.0.0.1` na lista do uvicorn interno, sempre que o `hangar-server` sobe. A porta pública (o filho, pelo `CP_FORWARDED_ALLOW_IPS`, ou a reserva em Python) segue com a lista do dono, como hoje.

- [x] **Step 1: Escrever os testes**

```python
# backend/tests/test_rust_server.py
"""main.py com o hangar-server: sobe, espera a saúde, religa, e o Python assume a porta pública.

O binário é um falso em Python que escuta a porta pública e responde a saúde como o de verdade,
e sai quando o stdin fecha, também como o de verdade; o modo (`FAKE_MODE`) decide se ele fica
(`ok`), cai logo depois de subir (`cai`) ou nunca responde (`mudo`), e `FAKE_PROTOCOL` muda o
`protocol` da saúde (`sem` = campo ausente). Linux só: o falso é um script com shebang e a morte
do filho é lida em /proc.
"""
import asyncio
import json
import os
import re
import socket
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import pytest
import uvicorn

from app import diag, internal_api, main, rust_server
from app.config import Settings

pytestmark = pytest.mark.skipif(not sys.platform.startswith("linux"),
                                reason="binário falso com shebang e /proc")

FAKE = r'''
import json, os, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

with open(os.environ["FAKE_LOG"], "a") as f:
    f.write(json.dumps({"pid": os.getpid(), "listen": os.environ["HANGAR_SERVER_LISTEN"],
                        "upstream": os.environ["HANGAR_SERVER_UPSTREAM"],
                        "secret": os.environ["HANGAR_INTERNAL_SECRET"],
                        "token": os.environ["CP_AUTH_TOKEN"],
                        "forwarded": os.environ["CP_FORWARDED_ALLOW_IPS"],
                        "log": os.environ["HANGAR_SERVER_LOG"]}) + "\n")


def parent_gone():
    # Igual ao binário: o Python segura o cano do stdin; fechou = pai morreu.
    while os.read(0, 64):
        pass
    os._exit(0)


threading.Thread(target=parent_gone, daemon=True).start()
mode = os.environ.get("FAKE_MODE", "ok")
if mode == "mudo":
    time.sleep(60)
    sys.exit(0)
protocol = os.environ.get("FAKE_PROTOCOL", "1")


class Health(BaseHTTPRequestHandler):
    def do_GET(self):
        health = {"ok": True, "version": "0.0.0-test"}
        if protocol != "sem":
            health["protocol"] = int(protocol)
        body = json.dumps(health).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


host, port = os.environ["HANGAR_SERVER_LISTEN"].rsplit(":", 1)
server = ThreadingHTTPServer((host, int(port)), Health)
if mode == "cai":
    threading.Timer(0.3, lambda: os._exit(1)).start()
server.serve_forever()
'''


class _App:
    """ASGI mínimo: conta as subidas do lifespan e responde "python" em qualquer rota."""

    def __init__(self):
        self.startups = 0

    async def __call__(self, scope, receive, send):
        if scope["type"] == "lifespan":
            while True:
                message = await receive()
                if message["type"] == "lifespan.startup":
                    self.startups += 1
                    await send({"type": "lifespan.startup.complete"})
                elif message["type"] == "lifespan.shutdown":
                    await send({"type": "lifespan.shutdown.complete"})
                    return
        await send({"type": "http.response.start", "status": 200,
                    "headers": [(b"content-type", b"text/plain")]})
        await send({"type": "http.response.body", "body": b"python"})


@pytest.fixture
def fake_bin(tmp_path, monkeypatch):
    path = tmp_path / "hangar-server"
    path.write_text(f"#!{sys.executable}\n{FAKE}", encoding="utf-8")
    path.chmod(0o755)
    monkeypatch.setenv("FAKE_LOG", str(tmp_path / "spawns.jsonl"))
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.delenv("HANGAR_INTERNAL_SECRET", raising=False)
    monkeypatch.setattr(rust_server, "_POLL", 0.05)
    yield path
    internal_api.set_secret(None)


@pytest.fixture
def events(monkeypatch):
    got = []
    monkeypatch.setattr(diag, "registrar",
                        lambda evento, nivel="ok", **campos: got.append((evento, nivel, campos)))
    return got


@pytest.fixture
def relog(monkeypatch):
    calls = []
    monkeypatch.setattr(rust_server.diag_logging, "instalar", lambda *a: calls.append(a))
    return calls


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _setup(app, public_port):
    kw = dict(host="127.0.0.1", port=public_port, workers=1, proxy_headers=True,
              forwarded_allow_ips="10.0.0.2", log_level="warning")
    server = uvicorn.Server(uvicorn.Config(app, **kw))
    return server, main._tcp_socket("127.0.0.1", 0), kw


def _bind(port):
    return lambda: main._tcp_socket("127.0.0.1", port)


def _spawns(tmp_path) -> list[dict]:
    log = tmp_path / "spawns.jsonl"
    return [json.loads(l) for l in log.read_text().splitlines()] if log.exists() else []


def _get(port: int, path: str) -> bytes:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(f"http://127.0.0.1:{port}{path}", timeout=1) as r:
        return r.read()


async def _wait_get(port: int, path: str, accept, timeout: float = 15.0) -> bytes:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            body = await asyncio.to_thread(_get, port, path)
            if accept(body):
                return body
        except OSError:
            pass
        await asyncio.sleep(0.05)
    raise AssertionError(f"a porta {port} não respondeu {path}")


def _dead(pid: int) -> bool:
    try:
        with open(f"/proc/{pid}/stat") as f:
            return f.read().rsplit(") ", 1)[1].startswith("Z")
    except FileNotFoundError:
        return True


def test_child_takes_public_port_and_gets_the_contract_env(fake_bin, tmp_path, events):
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)
    upstream = internal.getsockname()[1]

    async def scenario():
        task = asyncio.create_task(
            rust_server.serve(server, [internal], fake_bin, kw, "tok", _bind(port)))
        await _wait_get(port, rust_server.HEALTH_PATH, lambda b: json.loads(b)["ok"] is True)
        server.should_exit = True
        return await task

    assert asyncio.run(scenario()) is True
    [spawn] = _spawns(tmp_path)
    assert spawn["listen"] == f"127.0.0.1:{port}"
    assert spawn["upstream"] == f"127.0.0.1:{upstream}"
    assert spawn["token"] == "tok"
    assert spawn["forwarded"] == "10.0.0.2"         # a lista do dono vale na porta pública
    assert re.fullmatch(r"[0-9a-f]{64}", spawn["secret"])
    assert internal_api._secret == spawn["secret"]
    assert "HANGAR_INTERNAL_SECRET" not in os.environ   # nunca vaza para as sessões do backend
    assert spawn["log"] == str(tmp_path / "home" / ".hangar" / "logs" / "privado" / "hangar-server.log")
    assert _dead(spawn["pid"])                      # o Python leva o filho junto ao sair
    assert ("hangar_server.de_pe", "ok", {}) in events
    assert app.startups == 1


def test_three_crashes_in_a_minute_hand_the_public_port_to_python(
        fake_bin, tmp_path, monkeypatch, events, relog):
    monkeypatch.setenv("FAKE_MODE", "cai")
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)

    async def scenario():
        task = asyncio.create_task(
            rust_server.serve(server, [internal], fake_bin, kw, "tok", _bind(port)))
        await _wait_get(port, "/", lambda b: b == b"python")
        server.should_exit = True
        return await task

    assert asyncio.run(scenario()) is True
    spawns = _spawns(tmp_path)
    assert len(spawns) == rust_server.MAX_CRASHES
    assert all(_dead(s["pid"]) for s in spawns)
    assert len({s["secret"] for s in spawns}) == rust_server.MAX_CRASHES   # segredo novo a cada subida
    assert [e for e in events if e[0] == "hangar_server.caiu"]
    assert ("hangar_server.reserva", "erro", {"codigo": "quedas"}) in events
    assert app.startups == 1                        # a porta pública não rodou o lifespan de novo
    assert relog                                    # o diário voltou a ouvir o uvicorn


def test_child_that_never_answers_is_killed_and_python_takes_over(
        fake_bin, tmp_path, monkeypatch, events, relog):
    monkeypatch.setenv("FAKE_MODE", "mudo")
    monkeypatch.setattr(rust_server, "START_TIMEOUT", 1.0)
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)

    async def scenario():
        task = asyncio.create_task(
            rust_server.serve(server, [internal], fake_bin, kw, "tok", _bind(port)))
        await _wait_get(port, "/", lambda b: b == b"python")
        server.should_exit = True
        return await task

    assert asyncio.run(scenario()) is True
    [spawn] = _spawns(tmp_path)
    assert _dead(spawn["pid"])
    assert ("hangar_server.reserva", "erro", {"codigo": "sem_resposta"}) in events


def test_public_port_taken_at_takeover_ends_the_process_with_failure(
        fake_bin, monkeypatch, events, relog):
    monkeypatch.setenv("FAKE_MODE", "cai")
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)

    def occupied():
        raise OSError(98, "Address already in use")

    assert asyncio.run(
        rust_server.serve(server, [internal], fake_bin, kw, "tok", occupied)) is False
    assert any(e[0] == "hangar_server.reserva" and e[2].get("codigo") == "porta_ocupada"
               for e in events)


_CHILD_ENV = dict(HANGAR_SERVER_LISTEN="127.0.0.1:1", HANGAR_SERVER_UPSTREAM="127.0.0.1:2",
                  HANGAR_INTERNAL_SECRET="s", CP_AUTH_TOKEN="t", CP_FORWARDED_ALLOW_IPS="127.0.0.1",
                  HANGAR_SERVER_LOG="l")


def test_closing_stdin_makes_the_child_exit(fake_bin, monkeypatch):
    # `mudo` dormiria 60 s: sair em 5 s com código 0 só pode ser o fim do cano.
    monkeypatch.setenv("FAKE_MODE", "mudo")
    proc = rust_server._spawn(fake_bin, {**os.environ, **_CHILD_ENV})
    assert proc.poll() is None
    proc.stdin.close()
    assert proc.wait(timeout=5) == 0


def test_child_dies_when_python_is_killed(fake_bin, tmp_path, monkeypatch):
    """O cano do stdin: um Python morto a SIGKILL fecha a ponta dele, e o filho sai."""
    monkeypatch.setenv("FAKE_MODE", "mudo")
    code = ("import json, os, sys\n"
            "from pathlib import Path\n"
            "from app import rust_server\n"
            "env = dict(os.environ, **json.loads(sys.argv[2]))\n"
            "p = rust_server._spawn(Path(sys.argv[1]), env)\n"
            "print(p.pid, flush=True)\n"
            "os.kill(os.getpid(), 9)\n")
    out = subprocess.run([sys.executable, "-c", code, str(fake_bin), json.dumps(_CHILD_ENV)],
                         cwd=Path(rust_server.__file__).resolve().parents[1],
                         capture_output=True, text=True, timeout=60)
    pid = int(out.stdout.split()[0])
    deadline = time.monotonic() + 5
    while not _dead(pid) and time.monotonic() < deadline:
        time.sleep(0.05)
    assert _dead(pid)


@pytest.mark.parametrize("answer,got", [("2", 2), ("sem", None)])
def test_other_protocol_means_python_alone_and_a_diary_line(
        fake_bin, tmp_path, monkeypatch, events, relog, answer, got):
    monkeypatch.setenv("FAKE_PROTOCOL", answer)
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)

    async def scenario():
        task = asyncio.create_task(
            rust_server.serve(server, [internal], fake_bin, kw, "tok", _bind(port)))
        await _wait_get(port, "/", lambda b: b == b"python")
        server.should_exit = True
        return await task

    assert asyncio.run(scenario()) is True
    [spawn] = _spawns(tmp_path)                     # protocolo errado não religa
    assert _dead(spawn["pid"])
    assert ("hangar_server.protocolo", "erro",
            {"esperado": rust_server.RUST_SERVER_PROTOCOL, "recebido": got}) in events
    assert ("hangar_server.reserva", "erro", {"codigo": "protocolo"}) in events


def test_run_trusts_loopback_inside_and_keeps_the_owner_list_outside(monkeypatch):
    seen = {}

    async def fake_serve(server, sockets, binary, kw, token, bind_public):
        seen["internal"] = server.config.forwarded_allow_ips
        seen["public"] = kw["forwarded_allow_ips"]
        return True

    monkeypatch.setattr(rust_server, "serve", fake_serve)
    kw = dict(host="127.0.0.1", port=1, workers=1, proxy_headers=True, forwarded_allow_ips="10.0.0.2")
    assert rust_server.run(_App(), kw, Path("x"), "tok", [], lambda: None) == 3   # nada subiu
    assert seen == {"internal": "10.0.0.2,127.0.0.1", "public": "10.0.0.2"}


def test_rust_server_off_never_looks_for_the_binary(monkeypatch):
    monkeypatch.setattr(rust_server.rust_bins, "find_bin",
                        lambda *a: pytest.fail("CP_RUST_SERVER=0 não procura binário"))
    assert rust_server.wanted_binary(False) is None


def test_missing_binary_means_python_alone_and_a_diary_line(monkeypatch, events):
    monkeypatch.setattr(rust_server.rust_bins, "find_bin", lambda name, env_var: None)
    assert rust_server.wanted_binary(True) is None
    assert ("hangar_server.reserva", "aviso", {"codigo": "sem_binario"}) in events


def test_cp_rust_server_zero_turns_it_off(monkeypatch):
    monkeypatch.setenv("CP_RUST_SERVER", "0")
    assert Settings(_env_file=None).rust_server is False
    monkeypatch.delenv("CP_RUST_SERVER")
    assert Settings(_env_file=None).rust_server is True


@pytest.mark.parametrize("ips,expected", [
    ("127.0.0.1", "127.0.0.1"),
    ("10.0.0.2", "10.0.0.2,127.0.0.1"),
    ("10.0.0.2, 127.0.0.1", "10.0.0.2, 127.0.0.1"),
    ("*", "*"),
])
def test_internal_server_trusts_the_loopback_proxy(ips, expected):
    assert rust_server.trust_loopback(ips) == expected


@pytest.mark.parametrize("host,expected", [
    ("0.0.0.0", "0.0.0.0:8765"), ("192.168.1.5", "192.168.1.5:8765"), ("::", "[::]:8765"),
])
def test_listen_addr(host, expected):
    assert rust_server.listen_addr(host, 8765) == expected
```

- [x] **Step 2: Rodar e ver falhar** (quando autorizado)

Run: `cd backend && uv run pytest tests/test_rust_server.py -v`
Expected: FAIL na coleta com `ImportError: cannot import name 'rust_server' from 'app'`.

- [x] **Step 3: Implementar `rust_server.py`**

```python
# backend/app/rust_server.py
"""Sobe e vigia o `hangar-server`, o processo Rust que atende a porta pública na frente do uvicorn.

Com o binário, o uvicorn escuta numa porta interna de loopback e o filho fica com a porta do app.
Sem binário, sem resposta em 10 s, com outro protocolo ou com 3 quedas em 60 s, o Python volta a
atender a porta pública no mesmo processo, sem rodar o lifespan de novo.
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import secrets
import socket
import subprocess
import sys
import time
import urllib.request
from collections.abc import Callable
from pathlib import Path

import uvicorn

from app import diag, diag_logging, log_paths, rust_bins

_log = logging.getLogger("hangar.rust_server")

HEALTH_PATH = "/__hangar_server/health"
# Versão do contrato interno (rotas /internal, side-events, ambiente). Tem de casar com o
# `protocol` da saúde (hangar_server::INTERNAL_PROTOCOL); outro número = o Python atende sozinho.
RUST_SERVER_PROTOCOL = 1
START_TIMEOUT = 10.0
CRASH_WINDOW = 60.0
MAX_CRASHES = 3
_POLL = 0.25

# Literal, e não `subprocess.CREATE_NO_WINDOW`: o atributo só existe no Windows.
_CREATE_NO_WINDOW = 0x08000000


def wanted_binary(enabled: bool) -> Path | None:
    """O binário a subir, ou None quando o Python atende a porta pública sozinho."""
    if not enabled:
        return None
    found = rust_bins.find_bin("hangar-server", "CP_RUST_SERVER_BIN")
    if found is None:
        diag.registrar("hangar_server.reserva", "aviso", codigo="sem_binario")
    return found


def listen_addr(host: str, port: int) -> str:
    return f"[{host}]:{port}" if ":" in host else f"{host}:{port}"


def trust_loopback(ips: str) -> str:
    """Todo pedido chega ao uvicorn interno vindo do hangar-server em 127.0.0.1: sem confiar nele,
    o IP real se perde e a LAN passa por loopback (isenta do limite de tentativas)."""
    parts = [p.strip() for p in ips.split(",")]
    return ips if "127.0.0.1" in parts or "*" in parts else f"{ips},127.0.0.1"


def server_log_path() -> Path:
    folder = log_paths.base() / "privado"
    folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    return folder / "hangar-server.log"


def _health(host: str, port: int) -> dict | None:
    """Corpo da saúde quando ela responde `ok`; None enquanto não responde."""
    # Quem escuta em todas as interfaces responde no loopback da mesma família.
    probe = {"0.0.0.0": "127.0.0.1", "::": "::1"}.get(host, host)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(f"http://{listen_addr(probe, port)}{HEALTH_PATH}", timeout=1) as r:
            body = json.loads(r.read() or b"{}")
    except (OSError, ValueError):
        return None
    return body if isinstance(body, dict) and body.get("ok") is True else None


def _spawn(binary: Path, env: dict[str, str]) -> subprocess.Popen:
    # stdin=PIPE: o Popen guardado segura a ponta de escrita enquanto o filho vive. Quando o
    # Python morre, de qualquer jeito e em qualquer sistema, ela fecha e o binário sai.
    kw: dict = {"creationflags": _CREATE_NO_WINDOW} if sys.platform == "win32" else {}
    return subprocess.Popen([str(binary)], env=env, stdin=subprocess.PIPE,
                            stdout=subprocess.DEVNULL, **kw)


def _close_stdin(proc: subprocess.Popen) -> None:
    # Nada é escrito no cano, então fechar não tem o que despejar e não falha.
    if proc.stdin is not None:
        proc.stdin.close()


class Supervisor:
    """Um filho por vez; religa a cada queda até desistir."""

    def __init__(self, binary: Path, host: str, port: int, upstream_port: int, token: str,
                 forwarded: str):
        self.binary = binary
        self.host = host
        self.port = port
        self.upstream_port = upstream_port
        self.token = token
        self.forwarded = forwarded
        self.proc: subprocess.Popen | None = None
        self.announced = False

    def _env(self) -> dict[str, str]:
        # Import tardio: internal_api puxa o app, que o uvicorn interno já carregou a esta altura.
        from app import internal_api

        secret = secrets.token_hex(32)
        # O segredo vai só no ambiente do filho; no os.environ ele vazaria para toda sessão que o
        # backend sobe. O require_internal lê da memória do módulo a cada pedido.
        internal_api.set_secret(secret)
        return {**os.environ,
                "HANGAR_SERVER_LISTEN": listen_addr(self.host, self.port),
                "HANGAR_SERVER_UPSTREAM": f"127.0.0.1:{self.upstream_port}",
                "HANGAR_INTERNAL_SECRET": secret,
                # Os dois podem estar só no backend/.env, que o pydantic lê sem exportar.
                "CP_AUTH_TOKEN": self.token,
                "CP_FORWARDED_ALLOW_IPS": self.forwarded,
                "HANGAR_SERVER_LOG": str(server_log_path())}

    async def _start(self) -> str:
        """`up`, `died` (morreu subindo), `silent` (vivo e calado até o prazo) ou `protocol`."""
        if self.proc is not None:
            _close_stdin(self.proc)                     # o anterior já saiu
        self.proc = _spawn(self.binary, self._env())
        deadline = time.monotonic() + START_TIMEOUT
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                return "died"
            health = await asyncio.to_thread(_health, self.host, self.port)
            if health is not None:
                got = health.get("protocol")
                if isinstance(got, bool) or got != RUST_SERVER_PROTOCOL:
                    _log.error("hangar-server fala o protocolo %r; este backend fala %d",
                               got, RUST_SERVER_PROTOCOL)
                    diag.registrar("hangar_server.protocolo", "erro",
                                   esperado=RUST_SERVER_PROTOCOL, recebido=got)
                    return "protocol"
                return "up"
            await asyncio.sleep(_POLL)
        return "silent"

    async def run(self) -> str:
        """Mantém o filho de pé. Só volta quando desiste, com o motivo que vai pro diário."""
        crashes: list[float] = []
        try:
            while True:
                state = await self._start()
                if state in ("silent", "protocol"):
                    # Religar não adianta: o mesmo binário volta calado ou com o mesmo protocolo.
                    await self.stop()
                    return "sem_resposta" if state == "silent" else "protocolo"
                if state == "up" and not self.announced:
                    self.announced = True
                    print(f"[hangar] hangar-server de pé em {listen_addr(self.host, self.port)}; "
                          f"o Python atende atrás dele em 127.0.0.1:{self.upstream_port}", flush=True)
                    diag.registrar("hangar_server.de_pe")
                while state == "up" and self.proc.poll() is None:
                    await asyncio.sleep(_POLL)
                now = time.monotonic()
                crashes = [t for t in crashes if now - t < CRASH_WINDOW] + [now]
                _log.warning("hangar-server saiu (código %s), queda %d em %ds",
                             self.proc.returncode, len(crashes), int(CRASH_WINDOW))
                diag.registrar("hangar_server.caiu", "aviso", retorno=self.proc.returncode,
                               tentativa=len(crashes))
                if len(crashes) >= MAX_CRASHES:
                    return "quedas"
        except Exception:                                # noqa: BLE001 — a porta pública não fica sem dono
            _log.exception("a vigia do hangar-server falhou")
            await self.stop()
            return "erro"

    async def stop(self) -> None:
        proc = self.proc
        if proc is None:
            return
        if proc.poll() is None:
            # Fechar o cano é o pedido de saída que o binário entende em todo sistema.
            _close_stdin(proc)
            try:
                await asyncio.to_thread(proc.wait, 5)
            except subprocess.TimeoutExpired:
                proc.kill()
                await asyncio.to_thread(proc.wait)
        _close_stdin(proc)


async def serve(server: uvicorn.Server, sockets: list[socket.socket], binary: Path, kw: dict,
                token: str, bind_public: Callable[[], socket.socket]) -> bool:
    """O uvicorn interno e o hangar-server juntos. `False` = a porta pública ficou sem dono.

    `kw` é o da porta pública: a lista `forwarded_allow_ips` do dono vai ao filho e à reserva."""
    serving = asyncio.create_task(server.serve(sockets=sockets))
    # O filho só nasce com o uvicorn interno de pé: antes disso todo repasse dele daria 502.
    while not server.started and not serving.done():
        await asyncio.sleep(0.05)
    if serving.done():
        await serving
        return True
    supervisor = Supervisor(binary, kw["host"], kw["port"], sockets[0].getsockname()[1], token,
                            kw["forwarded_allow_ips"])
    watch = asyncio.create_task(supervisor.run())
    await asyncio.wait({serving, watch}, return_when=asyncio.FIRST_COMPLETED)
    if serving.done():
        watch.cancel()
        await asyncio.gather(watch, return_exceptions=True)
        await supervisor.stop()
        await serving
        return True
    return await _take_over(server, serving, watch.result(), kw, bind_public)


async def _take_over(server: uvicorn.Server, serving: asyncio.Task, reason: str, kw: dict,
                     bind_public: Callable[[], socket.socket]) -> bool:
    """O Python passa a atender a porta pública até o fim do processo."""
    _log.error("hangar-server desligado (%s); o Python assume a porta %s", reason, kw["port"])
    diag.registrar("hangar_server.reserva", "erro", codigo=reason)
    try:
        sock = bind_public()
    except OSError as e:
        _log.error("a porta %s ficou sem dono: %s", kw["port"], e)
        diag.registrar("hangar_server.reserva", "erro", codigo="porta_ocupada", **diag.erro_campos(e))
        server.should_exit = True
        await serving
        return False
    # lifespan="off": ele já rodou no servidor interno, e rodar de novo duplicaria watchers e hooks.
    public = uvicorn.Server(uvicorn.Config(server.config.app, **{**kw, "lifespan": "off"}))
    # O Config novo refaz o logging do uvicorn e tira dele os handlers do diário.
    diag_logging.instalar()
    public_task = asyncio.create_task(public.serve(sockets=[sock]))
    await asyncio.wait({serving, public_task}, return_when=asyncio.FIRST_COMPLETED)
    server.should_exit = True
    public.should_exit = True
    await asyncio.gather(serving, public_task)
    return True


def run(app: str, kw: dict, binary: Path, token: str, sockets: list[socket.socket],
        bind_public: Callable[[], socket.socket]) -> int:
    """Bloqueia até o fim. Devolve o código de saída: 0, 1 (porta sem dono) ou 3 (não subiu)."""
    # Só o uvicorn interno confia no 127.0.0.1 (o hangar-server); a porta pública segue com o `kw`.
    config = uvicorn.Config(app, **{**kw, "forwarded_allow_ips": trust_loopback(kw["forwarded_allow_ips"])})
    server = uvicorn.Server(config)
    public_ok = asyncio.run(serve(server, sockets, binary, kw, token, bind_public),
                            loop_factory=config.get_loop_factory())
    if not server.started:
        return 3
    return 0 if public_ok else 1
```

- [x] **Step 4: `CP_RUST_SERVER` no `Settings`, com a descrição da tela de Avançado**

Em `backend/app/config.py`, logo depois de `forwarded_allow_ips: str = "127.0.0.1"` (linha 251):

```python
    # CP_RUST_SERVER: 0 desliga o hangar-server (Rust) na porta pública e o Python atende sozinho.
    # Campo do Settings, e não os.environ, pra valer também escrito no backend/.env.
    rust_server: bool = True
```

Em `DESCRICAO_DE_CAMPO`, logo depois de `"deploy_secret": "deploy_secret",` (linha 344):

```python
    "rust_server": "rust_server",
```

Em `messages/pt.json`, logo depois da linha 1893 (`config_server_env_deploy_secret`):

```json
  "config_server_env_rust_server": "Ligado (padrão), o hangar-server em Rust atende a porta do app e o Python fica atrás dele. `0` desliga e o Python atende sozinho. Sem o binário baixado, o Python atende sozinho de qualquer jeito.",
```

Em `messages/en.json`, logo depois da linha 1893:

```json
  "config_server_env_rust_server": "On (default), the Rust hangar-server serves the app port with Python behind it. `0` turns it off and Python serves alone. Without the downloaded binary, Python serves alone anyway.",
```

- [x] **Step 5: `main.py` escolhe entre o Python sozinho e o `hangar-server`**

Import (linha 20):

```python
from app import migracao_sidecars, orq_politica, resilient_accept, rust_server
```

Trocar `backend/app/main.py:237-248` (de `# Um Server com dois sockets…` até o `sys.exit(3)`) por:

```python
    rust_bin = rust_server.wanted_binary(settings.rust_server)
    try:
        main_sock = _tcp_socket(bind, settings.port)
    except OSError as e:
        print(f"[hangar] ERRO: porta {settings.port} indisponível ({e})", file=sys.stderr)
        sys.exit(1)
    extras = [s for s in (_guest_socket(), _connect_socket()) if s]
    if rust_bin is not None:
        # A porta pública fica com o hangar-server; o bind acima só provou que ela estava livre.
        main_sock.close()
        sys.exit(rust_server.run("app.api:app", kw, rust_bin, settings.auth_token,
                                 [_tcp_socket("127.0.0.1", 0)] + extras,
                                 lambda: _tcp_socket(bind, settings.port)))
    # Um Server com dois sockets: um lifespan só (dois Server rodariam watchers e hooks em dobro).
    config = uvicorn.Config("app.api:app", **kw)
    server = uvicorn.Server(config)
    server.run(sockets=[main_sock] + extras)
    if not server.started:
        sys.exit(3)                              # mesmo código do uvicorn.run: o systemd reinicia
```

O bloco de `kw` e do `--reload` (linhas 231-236) fica como está, antes deste trecho.

- [x] **Step 6: Windows — o `hangar-server.exe` filho do backend não trava o reinício**

Em `scripts/windows-tasks.ps1`, logo depois da linha 69 (`$candidates = @(Get-HangarTaskProcesses $rows $name $directory)`), dentro do `Restart-HangarTask`:

```powershell
        # O hangar-server e filho do backend e segura a porta publica: entra na parada depois do
        # pai (ele sai sozinho quando o cano do stdin fecha; se ainda estiver vivo, cai aqui).
        $candidates = @($candidates) + @($rows | Where-Object {
            $_.Name -eq 'hangar-server.exe' -and [int]$_.ParentProcessId -in @($candidates.ProcessId)
        })
```

Em `scripts/test-windows-tasks.ps1`, logo depois da linha 79 (`Assert ('start' -in $script:events) 'Precisao de WMI recusou o mesmo processo'`):

```powershell
    Reset-State
    $script:processRows = @($rows) + @(Row 12 11 'hangar-server.exe' 'C:\Users\u\.hangar\bin\hangar-server.exe')
    $filho = [pscustomobject]@{ Id = 12; StartTime = $old }
    $filho | Add-Member ScriptMethod WaitForExit { param($timeout) return $true }
    $script:live[12] = $filho
    $script:owner = 12
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
    Assert (($script:events -join ',') -eq 'stop:10,stop:11,stop:12,start') 'hangar-server do backend travou ou desordenou o reinicio'
    Reset-State
    $script:processRows = @($rows) + @(Row 13 40 'hangar-server.exe' 'C:\other\hangar-server.exe')
    $script:owner = 13
    $failed = $false
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' } catch { $failed = $true }
    Assert ($failed -and 'start' -notin $script:events) 'hangar-server de outro checkout tratado como do backend'
```

Os dois arquivos continuam só em ASCII e sem BOM (conferido 2026-10-02: nenhum byte fora do ASCII; `windows-tasks.ps1` começa em `# F`).

- [x] **Step 7: Rodar e ver passar** (quando autorizado)

Run: `cd backend && uv run pytest tests/test_rust_server.py tests/test_startup_guard.py tests/test_share_gate.py tests/test_config.py -v`
Expected: PASS (os 8 cenários com binário falso, incluindo stdin fechado, Python morto e os 2 de protocolo; a confiança do uvicorn interno no `run`; as 3 decisões de configuração; os parametrizados; guarda, `_tcp_socket` e config sem regressão).

Run: `pwsh -NoProfile -File scripts/test-windows-tasks.ps1` (precisa do PowerShell 7; esta máquina não tem `pwsh` instalado)
Expected: `OK: selecao de processos, recuperacao, falhas, espera, atualizacao, vigia e lancador`

- [x] **Step 8: Commit**

O `scripts/windows-tasks.ps1` cai no portão de passo de atualização (conferido 2026-10-02: `scripts/check-passo-de-atualizacao.sh:27-30`), mas não muda nada que a máquina precise instalar: o motor e a vigia leem o `.ps1` do disco depois do pull (conferido 2026-10-02: `backend/app/atualizar.py:655-665`, `docs/decisoes/instalacao.md` "No Windows quem reinicia é `Restart-HangarTasks`… lido do disco depois do pull"). Daí o escape documentado.

```bash
git add backend/app/rust_server.py backend/app/main.py backend/app/config.py \
  messages/pt.json messages/en.json scripts/windows-tasks.ps1 scripts/test-windows-tasks.ps1 \
  backend/tests/test_rust_server.py
HANGAR_SEM_PASSO=1 git commit -m "feat(server): run hangar-server in front of uvicorn and fall back to Python on failure"
```

---

### Task 14: Instalação e atualização baixam os binários

**Files:**
- Create: `backend/app/rust_release.py`
- Create: `docs/atualizacoes/2026-10-02-hangar-server-binarios.md`
- Modify: `backend/app/atualizar.py:46` (import), `backend/app/atualizar.py:561-565` (`_preparar`, dentro do `if dist:`)
- Modify: `install.sh:434` (bloco novo depois do "App nativo", antes do `# ── 5/8`)
- Modify: `install.ps1:1215` (bloco novo depois do "App nativo", antes do `# -- 5/8`)
- Test: `backend/tests/test_rust_release.py`
- Test: `backend/tests/test_atualizar.py:359-378` (`_preparo_gravado`), `:407-424` (`test_preparar_sem_marca…`), testes novos depois da linha 431

`scripts/install-native.sh` e `scripts/install-native.ps1` não mudam: eles instalam o app de desktop, com pacote, atalho e marca próprios. O download dos binários do servidor é um módulo Python só (`app.rust_release`), chamado pelos dois instaladores logo depois deles e pelo `atualizar.py` no mesmo processo. Uma implementação em vez de três (bash, PowerShell, Python) para o mesmo manifesto aninhado, que o `grep` dos scripts do nativo não lê.

**Interfaces:**
- Consumes: release `server-latest` (frente 1): `server-latest.json` = `{"commit":"<sha>","files":{"<plataforma>/<bin>":{"name":"<asset>","sha256":"<hex>"}}}`; assets `hangar-server-<plataforma>[.exe]`, `hangar-cano-<plataforma>[.exe]`; plataformas `linux-x86_64`, `windows-x86_64`, `macos-aarch64`.
- Produces:
  - `rust_release.RELEASE_URL = "https://github.com/jeffer1312/hangar/releases/download/server-latest"` (sobrescrita por `HANGAR_SERVER_RELEASE_URL`, como `HANGAR_NATIVE_UPDATE_URL` no nativo)
  - `rust_release.NAMES = ("hangar-server", "hangar-cano")`
  - `rust_release.platform_key() -> str | None`
  - `rust_release.bin_dir() -> Path` (`~/.hangar/bin`, o mesmo de `rust_bins.find_bin`)
  - `rust_release.fetch(base_url: str | None = None, dest: Path | None = None) -> list[str] | None` (`None` = sem build para a máquina; `[]` = tudo no lugar; senão, os avisos; nunca levanta)
  - `rust_release.main(argv: list[str]) -> int` e `python -m app.rust_release [--never-fail]`: sai 0 (no lugar), 1 (falhou), 2 (sem build); com `--never-fail`, sempre 0.
  - Diário: `hangar_server.baixar` (`etapa` = `manifesto` | `download` | `sha256` | `gravar`; `detalhe` = nome do binário; `codigo` = `trocado` | `sem_build`).

Regras herdadas que esta Task respeita (lidas inteiras em `docs/decisoes/instalacao.md` e `docs/decisoes/windows.md`, "Regras vigentes"):

1. **Instalador com portão de prova por etapa** (`instalacao.md:11`): extra que falha entra na lista e o fim nunca diz "Pronto". Os binários são extra (o Python atende sozinho), então a falha vai para `anota_problema` / `$script:pendencias`, nunca para `fail`/`Pare` (conferido 2026-10-02: `install.sh:66` `anota_problema`, `install.sh:698-715` portão; `install.ps1:1207-1214`, o mesmo tratamento do app nativo). Máquina sem build é `falta`/`Nota`, não pendência, como o nativo (conferido 2026-10-02: `install.sh:424-426`).
2. **No `--update` os extras seguem moles, por `##HANGAR-AVISO##`** (`instalacao.md`, "Instalador com portão de prova por etapa"): o `install.sh --update` emite a marca no fim a partir de `PROBLEMAS` (conferido 2026-10-02: `install.sh:705-709`); no `install.ps1` a linha sai no próprio bloco, como o nativo (conferido 2026-10-02: `install.ps1:1213`).
3. **O botão Atualizar NÃO roda o instalador; mudança em `install.*`/`scripts/` exige passo em `docs/atualizacoes/`** (`instalacao.md:24-28`; conferido 2026-10-02: `scripts/check-passo-de-atualizacao.sh:27-30`). O botão baixa os binários sozinho no `_preparar` (como o dist do CI, `atualizar.py:561-565`), e esta Task traz o passo `2026-10-02-hangar-server-binarios.md`.
4. **O motor carrega o `atualizar.py` antes do `git pull`** (`instalacao.md`, "No Windows o `npm ci` do botão…", último item): a atualização que entrega esta Task ainda roda o `_preparar` antigo. Quem baixa nessa primeira vez é o passo, que roda depois do pull.
5. **Passo só entra no registro depois da prova passar; falha do passo para a atualização** (`instalacao.md:21-23`; conferido 2026-10-02: `backend/app/atualizacoes.py:236-247`, `PassoFalhou`). Por isso o comando do passo usa `--never-fail` (sai 0 com a rede fora) e a prova é o próprio módulo (`backend/app/rust_release.py`), como o passo `2026-10-01-atualizar-windows-front-no-ar.md`, cuja prova é o script que ele chama.
6. **Passo destrutivo não roda na subida do backend** (`docs/atualizacoes/README.md`, campo `destrutivo`; conferido 2026-10-02: `backend/app/main.py:121-127` chama `aplicar_pendentes(incluir_destrutivos=False)`). O passo sobrescreve binários e baixa da rede (até 300 s), o que não pode segurar a subida: `destrutivo: true`.
7. **Passo com comando por sistema usa `comando_posix` e `comando_windows`** (`instalacao.md:31`); o Windows roda pelo `cmd.exe` com cwd na raiz (conferido 2026-10-02: `backend/app/atualizacoes.py:197-203`, `shell=True, cwd=REPO`).
8. **Atualizar não é irreversível** (`instalacao.md:21`): o binário só troca depois de o sha256 conferir, pelo `tmp` ao lado e troca atômica; sha errado, download quebrado ou gravação recusada deixam o anterior intacto. O rollback (`_voltar`) chama `_preparar(dist=False)` (conferido 2026-10-02: `atualizar.py:839`) e não baixa: a release é a mais nova, não a do commit de antes.
9. **Instalador guiado: sem terminal tudo é NÃO; o log nunca carrega o token** (`instalacao.md:19-20`): o bloco não pergunta nada e não imprime token (o módulo só imprime caminho e motivo).
10. **`Path.replace` É `os.replace`; toda troca atômica passa por `atomico.substituir`** (`windows.md:21`; guarda de AST em `backend/tests/test_atomico_call_sites.py`). Os renames para `<nome>.old*` e de volta usam `Path.rename`, fora do formato que a guarda caça; no Windows o `rename` não sobrescreve, por isso o nome `.old` é escolhido livre antes.
11. **O exe aberto no Windows não pode ser sobrescrito nem apagado, mas pode ser renomeado** (conferido 2026-10-02: `scripts/install-native.ps1:59-63`; `desktop-native/src/update.rs:70-118`, commits b9650562 e eaf66071). `hangar-cano.exe` fica rodando nas sessões sem terminal e `hangar-server.exe` roda enquanto o botão baixa. Como no `update.rs`: o novo vai para um nome temporário em `~/.hangar/bin/`; o atual sai da frente para `<nome>.old`, ou `<nome>.old-N` quando um `.old` anterior ainda está preso (inclusive o que nega até a leitura); o temporário vira o nome final; se esse último passo falhar, o atual volta para o lugar. Cada rodada varre os `<nome>.old*` sem olhar a caixa, e o que ainda estiver em uso fica para a próxima.
12. **`monkeypatch.setattr(os, "name", …)` leva o `pathlib` junto** (`windows.md:25`): o ramo Windows decide por `_E_WINDOWS`, constante de módulo que o teste troca, no mesmo padrão de `atualizar.py:51-56`.
13. **Código de retorno no Windows não se lê como falha; separe "comando não existe" de "comando falhou"** (`windows.md:27`): o `install.ps1` confere se o `python.exe` do venv existe antes de chamar e só então lê o `$LASTEXITCODE`, que aqui é o contrato do próprio módulo (0/1/2).
14. **Encoding é por interpretador** (`windows.md:31`): `install.ps1` tem BOM e já leva texto acentuado (conferido 2026-10-02: começa em `EF BB BF`); o bloco novo é ASCII, como o resto dos textos dele. `install.sh` segue UTF-8 sem BOM (começa em `#!`).
15. **Saída de programa nativo no PowerShell passa pelo `Nativo`** (conferido 2026-10-02: `install.ps1:340-360`): com `$ErrorActionPreference = 'Stop'`, uma linha no stderr viraria erro terminante.

- [x] **Step 1: Escrever os testes do download**

```python
# backend/tests/test_rust_release.py
"""Download do hangar-server e do hangar-cano da release `server-latest`, contra um HTTP falso."""
import hashlib
import json
import os
import socket
import threading
import types
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

import pytest

from app import diag, rust_release

SERVER = b"#!/bin/sh\necho server\n"
CANO = b"#!/bin/sh\necho cano\n"
REAL_PLATFORM_KEY = rust_release.platform_key


@pytest.fixture(autouse=True)
def _ambiente(monkeypatch):
    for var in ("http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"):
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setattr(rust_release, "platform_key", lambda: "linux-x86_64")
    monkeypatch.setattr(rust_release, "_E_WINDOWS", False)


@pytest.fixture
def events(monkeypatch):
    got = []
    monkeypatch.setattr(diag, "registrar",
                        lambda evento, nivel="ok", **campos: got.append((evento, nivel, campos)))
    return got


@pytest.fixture
def release(tmp_path):
    """Pasta servida como a release. Devolve (url, pasta, caminhos pedidos)."""
    pasta = tmp_path / "release"
    pasta.mkdir()
    pedidos = []

    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(pasta), **kwargs)

        def do_GET(self):
            pedidos.append(self.path)
            super().do_GET()

        def log_message(self, *args):
            pass

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{httpd.server_address[1]}", pasta, pedidos
    httpd.shutdown()
    httpd.server_close()


def _publish(pasta, binaries: dict[str, bytes], plat="linux-x86_64", wrong_sha=()):
    files = {}
    for name, data in binaries.items():
        asset = f"{name}-{plat}{'.exe' if plat.startswith('windows') else ''}"
        (pasta / asset).write_bytes(data)
        sha = "0" * 64 if name in wrong_sha else hashlib.sha256(data).hexdigest()
        files[f"{plat}/{name}"] = {"name": asset, "sha256": sha}
    (pasta / "server-latest.json").write_text(json.dumps({"commit": "abc", "files": files}))


def test_downloads_both_checked_and_executable(release, tmp_path):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO})
    dest = tmp_path / "bin"
    assert rust_release.fetch(url, dest) == []
    assert (dest / "hangar-server").read_bytes() == SERVER
    assert (dest / "hangar-cano").read_bytes() == CANO
    assert os.access(dest / "hangar-server", os.X_OK)
    assert not [p for p in dest.iterdir() if p.name.endswith(".tmp")]


def test_same_release_again_downloads_only_the_manifest(release, tmp_path):
    url, pasta, pedidos = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO})
    dest = tmp_path / "bin"
    rust_release.fetch(url, dest)
    pedidos.clear()
    assert rust_release.fetch(url, dest) == []
    assert pedidos == ["/server-latest.json"]


def test_wrong_sha_keeps_the_binary_that_was_there(release, tmp_path, events):
    url, pasta, _ = release
    dest = tmp_path / "bin"
    dest.mkdir()
    (dest / "hangar-server").write_bytes(b"velho")
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, wrong_sha={"hangar-server"})
    avisos = rust_release.fetch(url, dest)
    assert len(avisos) == 1 and "hangar-server" in avisos[0] and "sha256" in avisos[0]
    assert (dest / "hangar-server").read_bytes() == b"velho"
    assert (dest / "hangar-cano").read_bytes() == CANO
    assert ("hangar_server.baixar", "erro", {"etapa": "sha256", "detalhe": "hangar-server"}) in events


def test_release_offline_is_a_warning_never_an_exception(tmp_path, events):
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]                   # porta fechada ao sair do with
    avisos = rust_release.fetch(f"http://127.0.0.1:{port}", tmp_path / "bin")
    assert len(avisos) == 1 and "manifesto" in avisos[0]
    assert any(e[0] == "hangar_server.baixar" and e[2].get("etapa") == "manifesto" for e in events)


def test_platform_missing_from_manifest_warns_per_binary(release, tmp_path):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, plat="macos-aarch64")
    avisos = rust_release.fetch(url, tmp_path / "bin")
    assert len(avisos) == 2 and all("linux-x86_64" in a for a in avisos)


def test_machine_without_build_returns_none(monkeypatch, tmp_path, events):
    monkeypatch.setattr(rust_release, "platform_key", lambda: None)
    assert rust_release.fetch("http://127.0.0.1:1", tmp_path / "bin") is None
    assert ("hangar_server.baixar", "aviso", {"codigo": "sem_build"}) in events


@pytest.mark.parametrize("plat,machine,key", [
    ("linux", "x86_64", "linux-x86_64"),
    ("win32", "AMD64", "windows-x86_64"),
    ("darwin", "arm64", "macos-aarch64"),
    ("linux", "aarch64", None),
    ("darwin", "x86_64", None),
])
def test_platform_key(monkeypatch, plat, machine, key):
    # Troca os nomes do módulo, não o `sys`/`platform` globais (ver windows.md, os.name e pathlib).
    monkeypatch.setattr(rust_release, "sys", types.SimpleNamespace(platform=plat))
    monkeypatch.setattr(rust_release, "platform", types.SimpleNamespace(machine=lambda: machine))
    assert REAL_PLATFORM_KEY() == key


@pytest.fixture
def windows(monkeypatch, tmp_path):
    monkeypatch.setattr(rust_release, "platform_key", lambda: "windows-x86_64")
    monkeypatch.setattr(rust_release, "_E_WINDOWS", True)
    dest = tmp_path / "bin"
    dest.mkdir()
    (dest / "hangar-server.exe").write_bytes(b"rodando")
    return dest


def test_windows_moves_the_running_exe_aside_and_sweeps_it_next_time(release, windows):
    url, pasta, _ = release
    dest = windows
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, plat="windows-x86_64")
    assert rust_release.fetch(url, dest) == []
    assert (dest / "hangar-server.exe").read_bytes() == SERVER
    assert (dest / "hangar-server.exe.old").read_bytes() == b"rodando"
    # Release nova: o resto da troca anterior sai na varredura, com outra caixa no nome.
    (dest / "hangar-server.exe.old").rename(dest / "HANGAR-SERVER.EXE.OLD")
    _publish(pasta, {"hangar-server": SERVER + b"#2", "hangar-cano": CANO}, plat="windows-x86_64")
    assert rust_release.fetch(url, dest) == []
    assert (dest / "hangar-server.exe").read_bytes() == SERVER + b"#2"
    assert sorted(p.name for p in dest.iterdir()) == [
        "hangar-cano.exe", "hangar-server.exe", "hangar-server.exe.old"]


def test_windows_stuck_old_yields_its_name(release, windows):
    url, pasta, _ = release
    dest = windows
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, plat="windows-x86_64")
    # Diretório não sai com unlink: faz o papel do .old que ainda é a imagem de um processo vivo.
    stuck = dest / "hangar-server.exe.old"
    stuck.mkdir()
    assert rust_release.fetch(url, dest) == []
    assert stuck.is_dir()
    assert (dest / "hangar-server.exe.old-1").read_bytes() == b"rodando"
    assert (dest / "hangar-server.exe").read_bytes() == SERVER


def test_windows_failed_swap_puts_the_running_exe_back(release, windows, monkeypatch):
    url, pasta, _ = release
    dest = windows
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, plat="windows-x86_64")

    def recusa(origem, destino):
        raise PermissionError(5, "Acesso negado")

    monkeypatch.setattr(rust_release.atomico, "substituir", recusa)
    avisos = rust_release.fetch(url, dest)
    assert len(avisos) == 2 and all("gravar" in a for a in avisos)
    assert (dest / "hangar-server.exe").read_bytes() == b"rodando"
    assert sorted(p.name for p in dest.iterdir()) == ["hangar-server.exe"]


def test_permission_error_outside_windows_is_a_warning(release, tmp_path, monkeypatch):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO})

    def recusa(origem, destino):
        raise PermissionError(13, "Permission denied")

    monkeypatch.setattr(rust_release.atomico, "substituir", recusa)
    avisos = rust_release.fetch(url, tmp_path / "bin")
    assert len(avisos) == 2 and all("gravar" in a for a in avisos)


@pytest.mark.parametrize("result,argv,code", [
    ([], [], 0), (["x"], [], 1), (None, [], 2),
    (["x"], ["--never-fail"], 0), (None, ["--never-fail"], 0),
])
def test_cli_exit_codes(monkeypatch, result, argv, code):
    monkeypatch.setattr(rust_release, "fetch", lambda: result)
    assert rust_release.main(argv) == code
```

- [x] **Step 2: Estender os testes do `atualizar.py`**

Em `backend/tests/test_atualizar.py`, `_preparo_gravado` (linha 359) ganha o parâmetro `fetch` e o liga antes do `_preparar`:

```python
def _preparo_gravado(repo, monkeypatch, *, lock_igual: bool, node_modules: bool,
                     topologia: str = "systemd", fetch=lambda: []):
    chamadas = []
    class P:
        returncode = 0
        stdout = ""
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: (chamadas.append(args), P())[1])
    monkeypatch.setattr(atualizar, "_atualizar_dist", lambda: None)
    monkeypatch.setattr(atualizar.rust_release, "fetch", fetch)
    monkeypatch.setattr(atualizar.shutil, "which", lambda nome: f"/bin/{nome}")
```

(o resto da função fica igual). Em `test_preparar_sem_marca_assume_o_node_modules_do_instalador`, logo depois de `monkeypatch.setattr(atualizar, "_atualizar_dist", lambda: None)` (linha 416):

```python
    monkeypatch.setattr(atualizar.rust_release, "fetch", lambda: [])
```

Testes novos, depois de `test_preparar_sem_node_modules_nao_instala_front` (linha 431):

```python
def test_preparar_baixa_os_binarios_e_a_falha_vira_aviso(repo, monkeypatch):
    """Sem os binários o Python atende sozinho: o download que falha avisa e não para o `uv sync`."""
    cmds = _preparo_gravado(repo, monkeypatch, lock_igual=True, node_modules=True,
                            fetch=lambda: ["binários Rust não baixados: rede fora"])
    assert any(c.endswith("uv sync") for c in cmds)
    assert "binários Rust não baixados: rede fora" in atualizar.estado()["avisos"]


def test_preparar_sem_build_para_a_maquina_nao_avisa(repo, monkeypatch):
    _preparo_gravado(repo, monkeypatch, lock_igual=True, node_modules=True, fetch=lambda: None)
    assert not atualizar.estado().get("avisos")


def test_volta_para_a_versao_anterior_nao_troca_binario(repo, monkeypatch):
    """O `_voltar` roda `_preparar(dist=False)`: a release é a mais nova, não a do commit de antes."""
    class P:
        returncode = 0
        stdout = ""
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: P())
    monkeypatch.setattr(atualizar.shutil, "which", lambda nome: f"/bin/{nome}")
    monkeypatch.setattr(atualizar.rust_release, "fetch",
                        lambda: pytest.fail("a volta não baixa binário"))
    (repo / "backend").mkdir(exist_ok=True)
    atualizar._preparar("systemd", dist=False)
```

- [x] **Step 3: Rodar e ver falhar** (quando autorizado)

Run: `cd backend && uv run pytest tests/test_rust_release.py tests/test_atualizar.py -v`
Expected: FAIL — `ImportError: cannot import name 'rust_release' from 'app'` em `test_rust_release.py`, e `AttributeError: module 'app.atualizar' has no attribute 'rust_release'` nos testes de `_preparar`.

- [x] **Step 4: Implementar `rust_release.py`**

```python
# backend/app/rust_release.py
"""Baixa o hangar-server e o hangar-cano da release `server-latest` para ~/.hangar/bin/.

Um módulo só para os dois instaladores e o botão Atualizar. Sem os binários o Python atende
sozinho, então nada aqui levanta: cada falha vira aviso e linha no diário.
Uso: python -m app.rust_release [--never-fail]   (sai 0 no lugar, 1 falhou, 2 sem build)
"""
from __future__ import annotations

import hashlib
import json
import logging
import os
import platform
import secrets
import sys
import urllib.request
from pathlib import Path

from app import atomico, diag

_log = logging.getLogger("hangar.rust_release")

RELEASE_URL = "https://github.com/jeffer1312/hangar/releases/download/server-latest"
NAMES = ("hangar-server", "hangar-cano")

# Constante de módulo pelo motivo do atualizar.py: o teste troca a decisão sem mexer no os.name.
_E_WINDOWS = os.name == "nt"


def platform_key() -> str | None:
    machine = platform.machine().lower()
    if sys.platform.startswith("linux") and machine in ("x86_64", "amd64"):
        return "linux-x86_64"
    if sys.platform == "win32" and machine in ("amd64", "x86_64"):
        return "windows-x86_64"
    if sys.platform == "darwin" and machine == "arm64":
        return "macos-aarch64"
    return None


def bin_dir() -> Path:
    return Path.home() / ".hangar" / "bin"


def _get(url: str, timeout: float) -> bytes:
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return r.read()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _free(path: Path) -> bool:
    try:
        os.lstat(path)
    except FileNotFoundError:
        return True
    except OSError:
        return False                 # nega até a leitura (apagado mas ainda aberto): ocupa o nome
    return False


def _sweep_old(target: Path) -> None:
    """Apaga os `<nome>.old*` de trocas anteriores, sem olhar a caixa (o NTFS não diferencia)."""
    prefix = f"{target.name}.old".lower()
    try:
        entries = list(target.parent.iterdir())
    except OSError:
        return
    for entry in entries:
        if entry.name.lower().startswith(prefix):
            try:
                entry.unlink()
            except OSError:
                pass                 # ainda é a imagem de um processo vivo: sai numa próxima rodada


def _old_name(target: Path) -> Path:
    """`<nome>.old`, ou `<nome>.old-N` quando um anterior ainda preso ocupa o nome."""
    candidate = target.with_name(f"{target.name}.old")
    n = 0
    while not _free(candidate):
        n += 1
        candidate = target.with_name(f"{target.name}.old-{n}")
    return candidate


def _install(data: bytes, target: Path) -> None:
    tmp = target.with_name(f".{target.name}.{secrets.token_hex(4)}.tmp")
    try:
        tmp.write_bytes(data)
        tmp.chmod(0o755)
        if _E_WINDOWS and not _free(target):
            # Exe em uso não pode ser sobrescrito nem apagado, mas pode ser renomeado: o processo
            # vivo segue no nome velho e o próximo a subir pega o novo.
            old = _old_name(target)
            target.rename(old)
            try:
                atomico.substituir(tmp, target)
            except OSError:
                old.rename(target)   # sem desfazer, o caminho do binário ficaria vazio
                raise
        else:
            atomico.substituir(tmp, target)
    finally:
        tmp.unlink(missing_ok=True)


def _fetch_one(url: str, files: object, plat: str, name: str, target: Path) -> str | None:
    entry = files.get(f"{plat}/{name}") if isinstance(files, dict) else None
    if not isinstance(entry, dict) or not isinstance(entry.get("name"), str) \
            or not isinstance(entry.get("sha256"), str):
        diag.registrar("hangar_server.baixar", "erro", etapa="manifesto", detalhe=name)
        return f"{name}: a release não traz build para {plat}"
    sha = entry["sha256"].lower()
    if target.is_file() and _sha256_file(target) == sha:
        return None
    try:
        data = _get(f"{url}/{entry['name']}", 300)
    except OSError as e:
        diag.registrar("hangar_server.baixar", "erro", etapa="download", detalhe=name, **diag.erro_campos(e))
        return f"{name}: o download falhou ({e})"
    if hashlib.sha256(data).hexdigest() != sha:
        diag.registrar("hangar_server.baixar", "erro", etapa="sha256", detalhe=name)
        return f"{name}: o sha256 não confere com a release; o binário anterior ficou"
    try:
        _install(data, target)
    except OSError as e:
        diag.registrar("hangar_server.baixar", "erro", etapa="gravar", detalhe=name, **diag.erro_campos(e))
        return f"{name}: não consegui gravar em {target.parent} ({e})"
    diag.registrar("hangar_server.baixar", codigo="trocado", detalhe=name)
    return None


def fetch(base_url: str | None = None, dest: Path | None = None) -> list[str] | None:
    """Põe em `dest` os binários desta máquina, conferidos pelo sha256 do manifesto.

    `None` = a release não tem build para esta máquina; `[]` = tudo no lugar; senão, os avisos.
    """
    plat = platform_key()
    if plat is None:
        diag.registrar("hangar_server.baixar", "aviso", codigo="sem_build")
        return None
    url = (base_url or os.environ.get("HANGAR_SERVER_RELEASE_URL") or RELEASE_URL).rstrip("/")
    dest = dest or bin_dir()
    ext = ".exe" if plat.startswith("windows") else ""
    try:
        files = json.loads(_get(f"{url}/server-latest.json", 15))["files"]
        dest.mkdir(parents=True, exist_ok=True)
    except (OSError, ValueError, KeyError, TypeError) as e:
        diag.registrar("hangar_server.baixar", "erro", etapa="manifesto", **diag.erro_campos(e))
        return [f"binários Rust não baixados: não consegui ler o manifesto da release ({e})"]
    for name in NAMES:
        _sweep_old(dest / f"{name}{ext}")
    avisos = [aviso for name in NAMES
              if (aviso := _fetch_one(url, files, plat, name, dest / f"{name}{ext}"))]
    for aviso in avisos:
        _log.warning(aviso)
    return avisos


def main(argv: list[str]) -> int:
    avisos = fetch()
    if avisos is None:
        print("binários Rust: a release não tem build para esta máquina; o Python atende sozinho")
        code = 2
    elif avisos:
        for aviso in avisos:
            print(aviso)
        code = 1
    else:
        print(f"binários Rust em {bin_dir()}")
        code = 0
    # O passo de atualização não pode parar a atualização inteira por um extra.
    return 0 if "--never-fail" in argv else code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
```

- [x] **Step 5: O botão Atualizar baixa no `_preparar`**

Em `backend/app/atualizar.py:46`:

```python
from app import atomico, procinfo, rust_release, tmux
```

Em `_preparar`, dentro do `if dist:` (linhas 561-565), logo depois do bloco do aviso do dist:

```python
    if dist:
        aviso = _atualizar_dist()
        if aviso:
            _escrever(avisos=list(estado().get("avisos") or []) + [aviso])
            _log.warning(aviso)
        # Sem o hangar-server o Python atende sozinho: falha aqui é aviso, nunca etapa quebrada.
        # A volta (`dist=False`) não baixa: a release é a mais nova, não a do commit de antes.
        binarios = rust_release.fetch() or []
        if binarios:
            _escrever(avisos=list(estado().get("avisos") or []) + binarios)
```

(o `_log.warning` de cada aviso já sai do `fetch`).

- [x] **Step 6: `install.sh` baixa os binários**

Em `install.sh`, depois do `fi` que fecha o bloco "App nativo" (linha 434) e antes de `# ── 5/8 Wrappers do claude e do codex`:

```bash
# ── Binários Rust (hangar-server e hangar-cano, release server-latest) ───────
# Sem eles o backend em Python atende sozinho: falha aqui nunca para a instalação.
say "Binários Rust"
RUST_RC=0
(cd backend && uv run --quiet --no-sync python -m app.rust_release) || RUST_RC=$?
case "$RUST_RC" in
  0) ok "hangar-server e hangar-cano em ~/.hangar/bin" ;;
  2) falta "sem binários Rust para esta máquina — o backend em Python atende sozinho" ;;
  *) anota_problema "binários Rust não baixaram — o backend em Python atende sozinho (tente: cd backend && uv run python -m app.rust_release)" ;;
esac
```

- [x] **Step 7: `install.ps1` baixa os binários**

Em `install.ps1`, depois do `}` que fecha o bloco "App nativo" (linha 1215) e antes de `# -- 5/8 Wrapper do claude`:

```powershell
# -- Binarios Rust (hangar-server e hangar-cano, release server-latest) -------
# Sem eles o backend em Python atende sozinho: falha aqui nunca para a instalacao.
if (-not $SoChecar) {
    Titulo 'Binarios Rust'
    $pyRust = Join-Path $raiz 'backend\.venv\Scripts\python.exe'
    if (-not (Test-Path $pyRust)) {
        $rcRust = -1
    } else {
        Push-Location (Join-Path $raiz 'backend')
        $rcRust = Nativo $pyRust -m app.rust_release
        Pop-Location
    }
    if ($rcRust -eq 0) {
        Ok 'hangar-server e hangar-cano em ~\.hangar\bin'
    } elseif ($rcRust -eq 2) {
        Nota 'sem binarios Rust para esta maquina; o backend em Python atende sozinho'
    } else {
        $motivo = if ($rcRust -eq -1) { 'sem o python do venv' } else { 'download ou conferencia falhou' }
        Falta "binarios Rust nao baixaram ($motivo) - o backend em Python atende sozinho"
        $script:pendencias += 'binarios Rust'
        Write-Host '##HANGAR-AVISO## binarios Rust nao baixaram; o backend em Python atende sozinho'
    }
}
```

- [x] **Step 8: Passo de atualização**

```markdown
---
id: 2026-10-02-hangar-server-binarios
titulo: As conversas do Claude e do Codex passam a ser servidas por uma parte nova do servidor, em Rust
comando_posix: cd backend && uv run --no-sync python -m app.rust_release --never-fail
comando_windows: cd backend && .venv\Scripts\python.exe -m app.rust_release --never-fail
prova: backend/app/rust_release.py
destrutivo: true
---

O histórico e o chat ao vivo das conversas do Claude e do Codex passam a sair de um programa em
Rust que fica na frente do servidor de sempre. Sem ele (máquina sem versão publicada ou download
que falhou), o app continua funcionando como antes.
```

(arquivo `docs/atualizacoes/2026-10-02-hangar-server-binarios.md`)

- [x] **Step 9: Conferir instaladores e passo**

Run: `bash -n install.sh && echo ok`
Expected: `ok`

Run: `cd backend && uv run python -c "from app import atualizacoes; print(atualizacoes.invalidos()); print([p['id'] for p in atualizacoes.todos() if p['id'] == '2026-10-02-hangar-server-binarios'])"`
Expected: `[]` e `['2026-10-02-hangar-server-binarios']`

O parse do `install.ps1` é conferido pelo `scripts/test-windows-tasks.ps1` (assert "Instalador nao parseia", conferido 2026-10-02: `scripts/test-windows-tasks.ps1:133-134`), que roda no Step 10.

- [x] **Step 10: Rodar e ver passar** (quando autorizado)

Run: `cd backend && uv run pytest tests/test_rust_release.py tests/test_atualizar.py tests/test_atualizacoes.py tests/test_atomico_call_sites.py -v`
Expected: PASS.

Run: `pwsh -NoProfile -File scripts/test-windows-tasks.ps1` (PowerShell 7)
Expected: a linha `OK: …` do fim.

- [x] **Step 11: Commit**

```bash
git add backend/app/rust_release.py backend/app/atualizar.py install.sh install.ps1 \
  docs/atualizacoes/2026-10-02-hangar-server-binarios.md \
  backend/tests/test_rust_release.py backend/tests/test_atualizar.py
scripts/check-passo-de-atualizacao.sh --staged && git commit -m "feat(install): download hangar-server and hangar-cano from the server-latest release"
```

---

### Task 15: Documentação e verificação manual

**Files:**
- Modify: `CLAUDE.md:404` (regra nova no fim da seção "Plataforma", depois do item do Hangar Connect)
- Modify: `docs/decisoes/plataforma.md` (entrada nova no fim do arquivo)
- Modify: `docs/decisoes/harnesses.md:111-115` (regra do cano: o `hangar-cano` quando existe, o `cano.py` de reserva)

**Interfaces:**
- Consumes: tudo das Tasks 1–14 no ar nesta máquina (binários publicados na `server-latest` pelo `server.yml`).
- Produces: a regra no `CLAUDE.md`, a entrada com a linha de base e as medidas de depois em `docs/decisoes/plataforma.md`, e a regra do cano em `docs/decisoes/harnesses.md` atualizada.

- [x] **Step 1: Regra no `CLAUDE.md`**

Depois da linha 404 (fim do item "Hangar Connect entra por `127.0.0.1:8768`…"):

```markdown
- **A porta 8765 é do `hangar-server` (Rust); o Python escuta atrás, numa porta de loopback.**
  Ele atende sozinho só `/history` e `/events` de Claude/Codex com o token do dono; o resto,
  convidado incluído, é repassado com `X-Forwarded-For`. 8766 e 8768 ficam no Python. Sem binário
  (`CP_RUST_SERVER_BIN`, `crates/target/release`, `~/.hangar/bin`), com `CP_RUST_SERVER=0`, com
  `protocol` da saúde diferente de `RUST_SERVER_PROTOCOL` ou com 3 quedas em 60 s, um segundo
  `uvicorn.Server` com `lifespan="off"` assume a porta: nunca um segundo lifespan. O segredo
  interno nunca entra no `os.environ`. Formato de `ChatEvent`, ids de evento e contrato interno
  mudam nos dois lados no mesmo commit.
  Medidas e motivo em [plataforma.md](docs/decisoes/plataforma.md#hangar-server-a-porta-pública-em-rust-o-python-atrás).
```

- [x] **Step 2: Entrada em `docs/decisoes/plataforma.md` e regra do cano em `harnesses.md`**

No fim de `docs/decisoes/plataforma.md`:

```markdown
## hangar-server: a porta pública em Rust, o Python atrás

O serviço continua subindo `python -m app.main`. Com o binário e sem `CP_RUST_SERVER=0`, o
uvicorn escuta numa porta livre de `127.0.0.1` e o `hangar-server` assume a porta pública como
filho (`app/rust_server.py`), com a porta interna, o token, a lista `forwarded_allow_ips` do dono
e um segredo novo a cada subida. O segredo vai só no ambiente do filho; no Python ele mora na
memória de `internal_api` (`set_secret`), porque no `os.environ` vazaria para toda sessão que o
backend sobe. O filho morre com o Python por um mecanismo só, em Linux, Windows e macOS: o
Python segura o stdin dele como cano, e o binário sai quando o cano fecha. Sem `preexec_fn`, que
não é seguro com threads vivas. No Windows o `Restart-HangarTask` reconhece o `hangar-server.exe`
filho do backend: sem isso, a porta "de outro processo" barrava todo reinício.

A reserva é no mesmo processo, sem novo lifespan: dois lifespans rodariam watchers e hooks em
dobro. O motivo vai ao diário como `hangar_server.reserva` (`sem_binario`, `sem_resposta`,
`protocolo`, `quedas`, `erro`, `porta_ocupada`); cada queda, como `hangar_server.caiu`. A saúde
traz `protocol`, e o Python só aceita o mesmo `RUST_SERVER_PROTOCOL`: a `server-latest` é sempre a
mais nova, e uma máquina atrasada pode baixar um binário que fala outro contrato interno. Com o
Rust na frente, todo pedido chega ao uvicorn interno por `127.0.0.1`: o `forwarded_allow_ips`
dele sempre inclui esse endereço, senão a LAN passaria por loopback e escaparia do limite de
tentativas. O Rust põe no `X-Forwarded-For` o cliente já resolvido, nunca o que veio de fora.

Os binários vêm da release `server-latest` para `~/.hangar/bin/`, pelo instalador, pelo botão
Atualizar (`_preparar`) e pelo passo `2026-10-02-hangar-server-binarios`; falha vira aviso.

### Linha de base (02/10/2026, antes da troca)

Fonte: backend vivo (3 sessões, 4 conexões) e benchmarks avulsos em Python 3.14, nesta máquina
(16 núcleos, 31 GB).

| Métrica | Antes |
|---|---|
| Memória do backend | 232–312 MB de RSS, 247 MB anônimos, 20 threads |
| CPU média | ~1,1% de um núcleo |
| `cano.py` por sessão sem terminal | 22 MB de RSS, 6 threads, 22 ms para subir |
| `/history` (Claude 0,9 MB / Codex 3 MB / Codex 35 MB) | 5–8 ms / 15–20 ms / 80–110 ms |
| `/history` completo, Claude 300 MB / `limit=200` | 362 ms / 45 ms |
| Atraso do laço com 8 leituras de histórico em paralelo | p99 9,6 ms, máx. 16 ms |
| Recursos presos por chat aberto | 2 threads do pool do anyio (limite 200) e 2 inotify |

### Depois da troca

Preenchida na verificação manual com o dono, com os comandos dela.

| Métrica | Depois |
|---|---|
| Backend Python (RSS, anônimos, threads) | |
| `hangar-server` (RSS, threads) | |
| `hangar-cano` por sessão sem terminal (RSS, threads) | |
| `/history` (Claude 0,9 MB / Codex 3 MB / Codex 35 MB), mediana de 5 | |
| `/history` completo, Claude 300 MB / `limit=200` | |
| Threads e inotify do Python com 4 chats abertos, contra 0 abertos | |
```

Em `docs/decisoes/harnesses.md`, nas "Regras vigentes", trocar a regra das linhas 111-115, hoje:

```markdown
- **Claude sem terminal: o `claude` é filho do CANO, nunca do backend.** `cano.py` é stdlib, um
  por sessão, escuta em socket local, nasce no escopo transiente do systemd e sintetiza um
  snapshot do que está em aberto; o backend só reconecta. O adapter (que muda sempre) fica no
  backend. Leitura do socket com `limit=16 MB` e embrulhada — leitor pendurado é sessão presa.
  Órfão é cano sem sidecar, não cano de backend anterior.
```

por:

```markdown
- **Claude sem terminal: o `claude` é filho do CANO, nunca do backend.** O cano é o binário
  `hangar-cano` quando o `rust_bins.find_bin` o acha (`CP_RUST_CANO_BIN`, `crates/target/release`,
  `~/.hangar/bin`), com o `cano.py` (stdlib) de reserva; os dois falam o mesmo protocolo. Um por
  sessão, escuta em socket local, nasce no escopo transiente do systemd e sintetiza um
  snapshot do que está em aberto; o backend só reconecta. O adapter (que muda sempre) fica no
  backend. Leitura do socket com `limit=16 MB` e embrulhada — leitor pendurado é sessão presa.
  Órfão é cano sem sidecar, não cano de backend anterior.
```

- [x] **Step 3: Commit da documentação**

```bash
git add CLAUDE.md docs/decisoes/plataforma.md docs/decisoes/harnesses.md
git commit -m "docs(platform): record the hangar-server rule and its baseline"
```

- [ ] **Step 4: verificação manual — roteiro de uso real com o dono**

O app desta máquina roda do checkout `/home/jefferson/hangar`, não de `pessoal/hangar`: restart ou build aqui não aparecem no app. O caminho é push (com autorização) → esperar o `server.yml` publicar a `server-latest` daquele commit (`gh release view server-latest --json assets`) → botão Atualizar no app. A atualização que entrega o código roda o passo `2026-10-02-hangar-server-binarios` (o `_preparar` novo só vale da próxima em diante).

Com `TOKEN=$(grep '^CP_AUTH_TOKEN=' /home/jefferson/hangar/backend/.env | cut -d= -f2-)`:

1. **Está no ar:**
   `ls -l ~/.hangar/bin/` mostra `hangar-server` e `hangar-cano`;
   `ss -ltnp 'sport = :8765'` mostra `hangar-server`;
   `journalctl --user -u hangar-backend -n 50 | grep 'hangar-server de pé'` mostra a porta interna.
2. **Chats do Claude e do Codex** abertos no celular, no web e no nativo: histórico completo,
   resposta ao vivo, fila (mensagem mandada com o agente ocupado aparece como pendente e depois
   confirma), pergunta nativa (`AskUserQuestion`) e prévia ao vivo.
3. **Rede caindo:** no celular, modo avião por 30 s enquanto o agente responde; voltar. A conversa
   continua sem mensagem faltando nem repetida (comparar com o web aberto no mesmo chat e com
   um recarregar).
4. **Reinício com sessão sem terminal viva:** abrir uma sessão sem terminal pelo app, mandar uma
   tarefa longa, `systemctl --user restart hangar-backend.service`. A sessão continua respondendo
   e o chat volta sozinho. `pgrep -af hangar-cano` mostra o cano dela (sessão nova nasce no Rust).
5. **Filho morre com o Python:** o `MainPID` do serviço é o `uv`, não o Python; matar o filho dele:
   `kill -9 $(pgrep -P $(systemctl --user show -p MainPID --value hangar-backend.service))`;
   em até 1 s `pgrep -f .hangar/bin/hangar-server` não acha nada e o systemd sobe tudo de novo.
6. **Reserva:** `for i in 1 2 3; do pkill -9 -f .hangar/bin/hangar-server; sleep 3; done`.
   `ss -ltnp 'sport = :8765'` passa a mostrar o `python`; os chats seguem funcionando; o diário de
   hoje (`~/.hangar/logs/diario/uso-$(date +%F).jsonl`) tem `hangar_server.reserva` com
   `"codigo": "quedas"`. Reiniciar o serviço para voltar ao Rust.
7. **`CP_RUST_SERVER=0`:** acrescentar `CP_RUST_SERVER=0` ao `/home/jefferson/hangar/backend/.env`,
   reiniciar o serviço: a 8765 é do `python`, nenhum `hangar-server` rodando, chats funcionam como
   antes. Tirar a linha e reiniciar de novo.
8. **Windows (se a VM DELPHI-02 estiver disponível):** Atualizar pelo app; `Get-NetTCPConnection
   -LocalPort 8765 -State Listen` aponta o `hangar-server.exe`; o próximo Atualizar reinicia sem
   "Porta 8765 ocupada por outro processo"; matar o `python.exe` do backend no Gerenciador de
   Tarefas derruba o `hangar-server.exe` junto.
9. **Medidas** (repetir as da linha de base, mesma máquina, mesmas sessões):
   - Backend e filho: o `MainPID` é o `uv`; `PY=$(pgrep -P $(systemctl --user show -p MainPID --value hangar-backend.service))`,
     `RS=$(pgrep -f .hangar/bin/hangar-server)`, `ps -o pid,rss,nlwp,args -p $PY,$RS` (RSS em KB)
     e `grep RssAnon /proc/$PY/status`.
   - `hangar-cano`: `ps -o pid,rss,nlwp,args -p $(pgrep -f hangar-cano | head -1)`.
   - `/history` nas três sessões de tamanho conhecido (Claude ~0,9 MB, Codex ~3 MB, Codex ~35 MB;
     `ls -l` do `jsonl` confirma o tamanho), cinco vezes cada, mediana:
     `for i in 1 2 3 4 5; do curl -s -o /dev/null -w '%{time_total}\n' -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:8765/api/sessions/<nome>/history"; done`;
     e na sessão Claude de 300 MB, sem e com `?limit=200`, se ela ainda existir.
   - Chats abertos: com nenhum chat aberto, anotar `nlwp` do `$PY` e
     `ls -l /proc/$PY/fd | grep -c inotify`; abrir 4 chats (2 sessões × celular e web) e anotar de
     novo. Antes eram +2 threads e +2 inotify por chat; agora o Python só ganha a conexão interna
     por sessão.
10. **Sessão real no `hangar-cano` (Task 5):** abrir uma sessão Claude sem terminal; `pgrep -af hangar-cano`
    mostra o cano com a chave da sessão na linha de comando (e o `claude` filho dele, não do
    backend). Mandar uma tarefa que gere cartão de permissão e, com o cartão pendente,
    `systemctl --user restart hangar-backend.service`: o cartão volta no chat, e responder
    continua a tarefa. Fechar a sessão pelo app: o cano e o `claude` somem (`pgrep -af hangar-cano`
    vazio para ela). Reserva: `CP_RUST_CANO_BIN=/nao/existe` no `.env` + reinício, abrir sessão
    nova: sobe pelo `cano.py` e funciona igual. Tirar a linha depois.
11. **Contrato dos protocolos:** o diário da subida não tem `hangar_server.reserva` com
    `"codigo": "protocolo"`, e `RUST_SERVER_PROTOCOL` (Python) = `INTERNAL_PROTOCOL` (Rust) no
    commit que subiu.
12. **Download real da `server-latest` (Task 14):** com `~/.hangar/bin/` vazio, rodar o passo pelo
    botão Atualizar (ou `python -m app.rust_release` pelo checkout do app) e conferir os dois
    binários com o sha256 do `server-latest.json`; repetir com um binário em uso (troca por `.old`).
13. **Windows (VM DELPHI-02), além do item 8:** (a) `install.ps1` parseia (`pwsh` não existe nesta
    máquina; conferir na VM com `[scriptblock]::Create((Get-Content install.ps1 -Raw))`); (b) o
    `hangar-cano` sobe o `hangar-engine.CMD` (escape de `.cmd` do `std` do Rust) e encerra o
    `claude` ao fechar a sessão; (c) cenários do `windows-tasks.ps1` com o `hangar-server.exe`
    filho do backend (Stop e Restart sem "Porta 8765 ocupada", corrida do `Stop-Process`);
    (d) reserva no Windows: matar o `hangar-server.exe` 3 vezes e conferir que o `python.exe`
    reassume a 8765; (e) download real com troca por `.old` com o exe em uso.
14. **Uso real nos três clientes:** celular, web e app nativo, com chat de Claude e de Codex
    abertos (item 2), depois do Atualizar com o código desta branch na main.

O que falhar aqui volta para a Task dona antes do Step 5. Os testes automatizados rodam só se o
dono pedir.

- [ ] **Step 5: Anotar as medidas e commitar**

Preencher a coluna "Depois" de `docs/decisoes/plataforma.md` com os números do item 9 (valor e
unidade, com a sessão usada em cada `/history`), e dizer na mesma entrada o que não deu para
medir e por quê.

```bash
git add docs/decisoes/plataforma.md
git commit -m "docs(platform): record hangar-server measurements after the switch"
```

#### Notas (dependências e riscos)

**Desvios e esclarecimentos do contrato**

- `CP_AUTH_TOKEN` e `CP_FORWARDED_ALLOW_IPS` vão **explícitos** no ambiente do filho, lidos do `settings`, não herdados: os dois costumam estar só no `backend/.env`, que o pydantic lê sem exportar para `os.environ` (conferido 2026-10-02: `backend/app/config.py:159` `env_file=".env"`; `config.py:305-307` explica o mesmo para outras variáveis). O `CP_FORWARDED_ALLOW_IPS` do filho é a lista do dono, sem o `127.0.0.1` que só o uvicorn interno ganha.
- `CP_RUST_SERVER` virou campo do `Settings` (`rust_server: bool`), para valer também escrito no `backend/.env`; no ambiente do processo continua funcionando. `CP_RUST_SERVER_BIN` segue só no ambiente (é o `find_bin` da frente 2).
- `HANGAR_INTERNAL_SECRET` é novo a **cada subida do filho** (não só a cada boot do Python). Vai só no dicionário de ambiente do filho; no Python, `internal_api.set_secret` o troca antes de cada spawn. Desvio do brief, que o punha também no `os.environ`: ali ele vazaria para toda sessão que o backend sobe.
- O filho morre com o pai pelo cano do stdin (`stdin=PIPE`, guardado no `Popen` enquanto ele vive), em todo sistema; `PR_SET_PDEATHSIG`, `preexec_fn` e o job object do Windows saíram. Fechar o cano também é o pedido de parada do `Supervisor.stop` (com `kill` depois de 5 s).
- A saúde traz `protocol`; diferente de `RUST_SERVER_PROTOCOL` (ou ausente), o Python não religa, registra `hangar_server.protocolo` e assume a porta (`hangar_server.reserva`, `codigo` = `protocolo`).
- `HANGAR_SERVER_LISTEN` usa colchetes em IPv6 (`[::]:8765`); a frente 4 deve aceitar o formato do `SocketAddr` do Rust, que lê os dois.
- A saúde é sondada no endereço público: `0.0.0.0` → `127.0.0.1`, `::` → `::1`, IP de LAN → ele mesmo.
- Logs do filho: `HANGAR_SERVER_LOG` = `<log_paths.base()>/privado/hangar-server.log`; stdout do filho vai para `/dev/null` e o stderr é herdado (journald no Linux).
- O uvicorn interno ganha `127.0.0.1` em `forwarded_allow_ips` (`trust_loopback`, aplicado só no `Config` interno do `run`; a reserva da porta pública usa a lista do dono). O `hangar-server` (Task 11) sempre põe no `X-Forwarded-For` o cliente que ele mesmo resolveu e tira o `X-Hangar-Internal` de fora, também no WebSocket: um `X-Forwarded-For` forjado por vizinho não confiável não passa.
- `scripts/install-native.sh`/`.ps1` não mudam: o download dos binários é um módulo Python (`app.rust_release`) chamado pelos dois instaladores e pelo `atualizar.py`.

**Dependências entre Tasks**

- Task 13 importa `app.rust_bins.find_bin` (frente 2): sem ele, até `test_startup_guard.py` deixa de importar `app.main`.
- Task 13 depende da Task 9 (`internal_api.set_secret`; o teste lê `internal_api._secret`) e, em uso real, da saúde com `protocol` e do `parent_gone` da Task 11. O binário falso dos testes imita os dois.
- Task 14 em uso real depende da release `server-latest` publicada pelo `server.yml` (frente 1) com o manifesto do brief; os testes usam HTTP falso. Os nomes de asset (`hangar-server-<plataforma>[.exe]`) só importam pelo campo `name` do manifesto: o download segue o que o manifesto diz.
- Task 15 depende de todas, inclusive do `hangar-cano` (frente 2) para a medida por sessão.
- Tasks 13 e 14 não dividem arquivo nenhum e podem rodar em paralelo depois que `rust_bins.py` existir.

**Riscos**

- **O cano do stdin só fecha quando nenhum processo segura a ponta de escrita.** O `Popen` cria o cano sem herança (`O_CLOEXEC` no POSIX, handle não herdável no Windows), então as sessões que o backend sobe depois não a levam. Rodar o binário à mão exige stdin aberto (terminal ou cano): com `/dev/null` ele sai na hora.
- **Filho vivo mas travado depois da subida** não é detectado: a vigia só olha saída do processo (e a saúde só na subida). A vigia do Windows (`Test-HangarHttp`) e um 502 do proxy cobrem parte; vigiar saúde periódica fica para quando aparecer o caso.
- **Ramo Windows sem teste automático:** o `CREATE_NO_WINDOW`, o fim do cano no Windows e a troca por `.old` com exe em uso real só são conferidos na VM (Task 15, Step 4, item 8). O CI não roda pytest no Windows (conferido 2026-10-02: `docs/decisoes/windows.md:138-150`, último parágrafo).
- **Versão do binário x versão do Python:** a `server-latest` é sempre a mais nova da main; uma máquina atrasada pode baixar um `hangar-server` mais novo que o `/internal` do Python dela. O `protocol` da saúde barra o contrato diferente (o Python atende sozinho), mas só se quem mudar o contrato interno subir `INTERNAL_PROTOCOL` e `RUST_SERVER_PROTOCOL` juntos.
- **`_pid_do_servidor` no Windows** passa a devolver o pid do `hangar-server.exe` (quem escuta a porta). A prova "o pid mudou" continua valendo, porque o filho é novo a cada boot do Python.
- **Hooks e CLIs locais que falam com `127.0.0.1:8765`** passam pelo proxy; durante a religação de uma queda (sub-segundo) eles recebem conexão recusada.
