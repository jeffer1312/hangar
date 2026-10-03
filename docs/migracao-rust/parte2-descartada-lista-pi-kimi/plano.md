# hangar-server, parte 2 — Plano de implementação

> **Para os agentes de execução:** usar `superpowers:subagent-driven-development` ou
> `superpowers:executing-plans`, conforme escolha do dono. Marcar os Steps concluídos no plano.

**Goal:** servir as leituras Pi/omp/Kimi e distribuir lista/orq ao vivo no Rust, preservando os
contratos dos clientes e as fontes Python explicitamente mantidas pela spec.

**Architecture:** os parsers JSONL Pi/omp/Kimi ampliam o histórico e o tail da parte 1. A lista usa
uma conexão interna com o produtor Python existente, e orq usa um stream materializado Python
por sessão mais replay finito por aparelho. O histórico orq segue pelo repasse atual.

**Tech Stack:** Rust 1.98.1, edition 2024; crates e versões já fixadas no workspace; Python 3.14,
FastAPI, pytest e testes Cargo existentes. Nenhuma dependência nova.

**Spec:** `docs/superpowers/specs/2026-10-02-hangar-server-parte2-design.md`.

**Análise:** `docs/analise-hangar-server-parte2-2026-10-02.md`.

**Status:** aguardando aprovação da fronteira proposta. Este documento contém código proposto
completo dos módulos novos e das substituições centrais; **não foi aplicado nem compilado**.
Os passos de execução/geração/checagem/commit abaixo são futuros, não comandos desta etapa.

## Global Constraints

- Quadro e canvas fora; nenhum arquivo de UI precisa mudar; nenhuma dependência ou toolchain nova.
- Lista: descoberta, `list_with_state`, filtro owner e manutenção permanecem no Python. GET usa
  dados frescos; SSE usa assinatura estável. Não vender fanout como redução de CPU por tique.
- Orq: `orq_timeline.entry` é o parser único; `/history` permanece Python. Sem pane/processo novo.
- Só o gate do dono da parte 1 entra nos handlers Rust. Guest/share/outros métodos ficam no repasse.
  Peer usando token do dono não é identificável como peer; aprovação precisa aceitar essa distinção.
- Toda rota interna requer loopback e segredo; nunca expor segredo, texto de conversa ou erro serde
  cru em log. Paths internos vêm da resolução da sessão, nunca de parâmetro arbitrário do cliente.
- Protocol: base1; Task5 lista sobe ambos para2; Task6 orq sobe ambos para3 no respectivo commit.
  Se upstream avançar antes da execução, conferir ambos e usar o próximo valor, sem rebaixar.
- Sem Rust/desligado/incompatível/queda: reserva Python no mesmo processo, sem segundo lifespan.
  Nunca iniciar/reiniciar/parar backend ou rodar instalador desta máquina durante a execução.
- Tests-first: escrever teste e observar falha **quando autorizado**; Rodar só a pedido do dono.
  Geradores que chamam parser/oráculo também ficam nos Steps “quando autorizado”. Nenhuma fixture real.
- Identificadores novos em inglês; comentários/doc em pt-BR; não rodar formatter --write sem pedido.
- Commits descritivos, caminhos explícitos. Documentos `docs/superpowers/` ignorados ficam fora;
  não commitar estes documentos agora. `git status` antes/depois de cada commit, sem add -A/`.`.
- Plano do Superpowers será executado pelo Superpowers somente após aprovação e escolha do fluxo.

## Review Focus

- Pi no EOF/linha parcial e hook chegando em200ms: offset do código Python, timestamp original e
  corte consistente, sem texto de hook na fala (Tasks1–3).
- Owner com sessão de convidado `owner_sees=false`: recorte interno correto também em attach novo,
  nenhuma lista crua do produtor entregue pelo Rust (Task5).
- Nav confirmado enquanto outro aparelho entra: snapshot de pendências atual e timestamp por
  aparelho, sem replay de nav já atendido (Task5).
- Orq reconecta após450 linhas e replay ocorre junto de append: cursor por upstream e por aparelho,
  cut/geração separados, conversa não acumulada na fila de SideCache (Task6).
- Same name recriado/troca provider/receptor lento: reset e retomada pelo arquivo; não congelar o
  chat com ping de um leitor morto (Tasks3,4,6).

## Ordem e responsabilidade dos arquivos

Tasks1–3: fixtures/parsers/histórico/tail. Task4: rotas e uso do contrato com Python falso.
Task5: lista (`list.rs`, rotas internas, navegação interna). Task6: orq (`orq_read.rs`, replay/bridge).
Task7: contrato final, uso real e medições. Um escritor por árvore; esta ordem não pressupõe
execução em paralelo nem autoriza novas sessões. `routes.rs`, `internal_api.py`, `sse.py` e
`lib.rs` são compartilhados entre Tasks5/6: executar sequencialmente.

---

### Task 1: Fixar contratos sintéticos Pi/omp/Kimi

Status: ready-for-agent (execução depende da aprovação do dono)
Risk: high (risco alto de paridade e retomada)

**Files:**
- Create: `backend/tests/fixtures/contract/gen_provider_golden.py`
- Create: `backend/tests/fixtures/contract/transcripts/pi.jsonl`
- Create: `backend/tests/fixtures/contract/transcripts/kimi.jsonl`
- Create: `backend/tests/fixtures/contract/queue/pi-fixture.jsonl`
- Create: `backend/tests/fixtures/contract/queue/kimi-fixture.jsonl`
- Create: `backend/tests/fixtures/contract/golden/pi.tail.json`
- Create: `backend/tests/fixtures/contract/golden/pi.history.json`
- Create: `backend/tests/fixtures/contract/golden/omp.history.json`
- Create: `backend/tests/fixtures/contract/golden/kimi.tail.json`
- Create: `backend/tests/fixtures/contract/golden/kimi.history.json`
Nunca usar conversa real.
**Interfaces:** golden usa ChatEvent.model_dump completo, offset excluído no evento, incluído no envelope tail. Gerador executa SOMENTE quando dono autorizar teste/verificação. Até lá os arquivos gerados são passos pendentes, não podem ser alegados conferidos.

- [ ] **Step 1: Conferir e incorporar os consertos conhecidos da parte 1 na execução aprovada**

```bash
git status --short
git fetch origin
git log --oneline HEAD..origin/hangar-server-parte1
git diff --stat HEAD..origin/hangar-server-parte1
git merge --no-edit origin/hangar-server-parte1
```

Este passo só ocorre depois da aprovação do plano. Árvore suja interrompe o merge até resolver
o que pertence a quem. Na elaboração foram encontrados aa8eec36 e2eef31e8, apenas CI Zig e teste
de processo encerrado. Se aparecer mudança de contrato/implementação, ajustar o plano antes de
prosseguir. Não trocar de branch, rodar reset ou iniciar backend. Não inferir que o CI passou.

- [ ] **Step 2: Escrever gerador sintético do contrato existente**

Proposta completa do gerador:

```python
# backend/tests/fixtures/contract/gen_provider_golden.py
from pathlib import Path
import gen_golden as base
from app.adapters.pi.transcript import Stream
from app.adapters.kimi.transcript import parse_line as kimi_parse
from app.transcript import TranscriptTailer


def ms(sec):
    return int(base.epoch(base.T(sec)) * 1000)


def pi_message(node, role, content, sec, **fields):
    return base.j({"type": "message", "id": node,
                   "message": {"role": role, "content": content, "timestamp": ms(sec), **fields}})


PI = [
    base.j({"type": "session", "timestamp": base.T(0), "id": "sid-pi"}),
    pi_message("p1", "user", [{"type": "text", "text": "contexto do hook\n\noi"}], 1),
    base.j({"type": "custom_message", "customType": "claude-hook-context", "parentId": "p1",
            "content": "contexto do hook", "id": "hook-1"}),
    pi_message("p2", "assistant", [{"type": "thinking", "thinking": "penso"},
        {"type": "text", "text": "\x1b[32mOlá\x1b[0m"},
        {"type": "toolCall", "id": "pc1", "name": "read", "arguments": {"path": "/tmp/demo"}},
        {"type": "thinking", "thinking": 7}, {"type": "text", "text": " "},
        {"type": "toolCall", "id": "pc2", "name": "bash", "arguments": "malformado"}], 2,
        usage={"cacheRead": 42}),
    pi_message("p3", "toolResult", [{"type": "text", "text": "\x1b[31msaída\x1b[0m"},
        {"type": "image", "data": "NÃO ENVIAR", "mimeType": "image/png"},
        {"type": "image", "data": "NÃO ENVIAR"}], 3, toolCallId="pc1", isError=True),
    base.j({"type": "message", "id": "p4", "message": {"role": "toolResult", "timestamp": ms(4),
        "toolCallId": "pc2", "content": [{"type": "text", "text": "cauda \ud83d"}]}}, ascii=True),
    pi_message("p5", "user", [{"type": "image", "data": "x"}, {"type": "text", "text": "ok"}], 5),
    pi_message("p6", "assistant", [{"type": "text", "text": "feito"}], 6,
               usage={"cacheRead": True}),
    base.j({"type": "model_change", "id": "change", "timestamp": base.T(7)}),
    pi_message("p7", "user", [{"type": "text", "text": "só hook"}], 8),
    base.j({"type": "custom_message", "customType": "claude-hook-context", "parentId": "p7",
            "content": "só hook"}),
    pi_message("p8", "user", [{"type": "text", "text": "não cortar"}], 9),
    base.j({"type": "custom_message", "customType": "claude-hook-context", "parentId": "outro",
            "content": "não cortar"}),
    b'{"type":"message","message":',
    pi_message("p9", "user", [{"type": "text", "text": "mensagem no EOF"}], 10),
]


def kimi_user(node, text, sec, origin="user"):
    return base.j({"type": "context.append_message", "time": ms(sec), "message": {
        "id": node, "role": "user", "origin": {"kind": origin}, "content": text}})


def kimi_loop(ev, sec):
    return base.j({"type": "context.append_loop_event", "time": ms(sec), "event": ev})


KIMI = [
    base.j({"type": "metadata", "created_at": ms(0)}),
    kimi_user("k1", [{"type": "text", "text": "oi"}], 1),
    kimi_user("inject", [{"type": "text", "text": "não sou usuário"}], 1, "injection"),
    kimi_loop({"type": "content.part", "uuid": "k2", "part": {"type": "text", "text": "Olá"}}, 2),
    kimi_loop({"type": "content.part", "uuid": "k3", "part": {"type": "think", "think": "penso"}}, 3),
    kimi_loop({"type": "tool.call", "uuid": "k4", "toolCallId": "kc1", "name": "Bash", "args": {"cmd": "ls"}}, 4),
    kimi_loop({"type": "tool.result", "toolCallId": "kc1", "result": {"output": "saída", "isError": False}}, 5),
    kimi_loop({"type": "tool.call", "uuid": "k6", "toolCallId": "kc2", "name": "Read", "args": 7}, 6),
    kimi_loop({"type": "tool.result", "toolCallId": "kc2", "result": {"unicode": "ç", "float": 1.0, "isError": True}}, 7),
    kimi_loop({"type": "tool.result", "parentUuid": "parent-only", "result": "texto direto"}, 8),
    kimi_loop({"type": "tool.result", "uuid": "uuid-only", "result": None}, 8),
    base.j({"type": "usage.record", "time": ms(8), "usage": {"cacheRead": 999}}),
    base.j({"type": "config.update", "time": ms(8), "config": {"system": "X" * 2048}}),
    kimi_user("k9", [{"type": "image", "data": "x"}, {"type": "text", "text": "ok"}], 9),
    kimi_loop({"type": "content.part", "uuid": "k10", "part": {"type": "think", "think": 7}}, 10),
    b'{"type":"context.append_loop_event","event":',
]


PI_QUEUE = [
    {"id": "primeiro", "text": "oi", "ts": base.epoch(base.T(1)) - .1},
    {"id": "enquanto-retido", "text": "próximo", "ts": base.epoch(base.T(1)) + .1},
    {"id": "ok-absorvido", "text": "ok", "ts": base.epoch(base.T(5)) - .1},
    {"id": "segundo-ok", "text": "ok", "ts": base.epoch(base.T(5)) + .1},
    {"id": "confirmada", "text": "confirmada", "ts": base.epoch(base.T(6)), "confirmed": True},
]
KIMI_QUEUE = [
    {"id": "primeiro", "text": "oi", "ts": base.epoch(base.T(1)) - .1},
    {"id": "segundo", "text": "oi", "ts": base.epoch(base.T(1)) + .1},
    {"id": "próximo", "text": "próximo", "ts": base.epoch(base.T(10))},
]


def tail(path: Path, provider: str):
    parse = Stream().parse_line if provider == "pi" else kimi_parse
    events, _ = TranscriptTailer(path, parse_line=parse)._read_from(0)
    return [{"offset": ev.offset, "event": base.event_dict(ev)} for ev in events]


def main():
    for provider, rows, queue in (("pi", PI, PI_QUEUE), ("kimi", KIMI, KIMI_QUEUE)):
        path = base.TRANSCRIPTS / f"{provider}.jsonl"
        base.INVALID_LINES[path.name] = 1
        base.write_jsonl(path, rows)
        base.write_queue(base.QUEUE / f"{provider}-fixture.jsonl", queue)
        base.write_golden(f"{provider}.tail.json", tail(path, provider))
        base.write_golden(f"{provider}.history.json", base.history(path, provider, f"{provider}-fixture"))
    # omp usa exatamente o mesmo parser de Pi, mas a entrada pública do histórico é outra.
    base.write_golden("omp.history.json", base.history(base.TRANSCRIPTS / "pi.jsonl", "omp", "pi-fixture"))


if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Gerar os contratos Python (quando autorizado)**

Comandos, não executar enquanto falta autorização:
`(cd backend && uv run python tests/fixtures/contract/gen_provider_golden.py)`
Esperado: arquivos de contrato sintéticos gravados com as variantes full/limit/janela/fila. O gerador usa helpers do oráculo Python já existentes; nenhum parser Rust precisa estar disponível nesta Task. Gerar no Linux desta linha de base; nos outros sistemas o Cargo lê as fixtures versionadas.

- [ ] **Step 4: Commit seletivo dos contratos após geração autorizada**

`git add backend/tests/fixtures/contract/gen_provider_golden.py backend/tests/fixtures/contract/transcripts/pi.jsonl backend/tests/fixtures/contract/transcripts/kimi.jsonl backend/tests/fixtures/contract/queue/pi-fixture.jsonl backend/tests/fixtures/contract/queue/kimi-fixture.jsonl backend/tests/fixtures/contract/golden/pi.tail.json backend/tests/fixtures/contract/golden/pi.history.json backend/tests/fixtures/contract/golden/omp.history.json backend/tests/fixtures/contract/golden/kimi.tail.json backend/tests/fixtures/contract/golden/kimi.history.json`
`git commit -m "test(server): pin Pi, omp and Kimi conversation contracts"`
Fixtures geradas são o produto desta Task. Os testes Rust dessas fixtures serão escritos e executados nas Tasks2/3; não incluir arquivo Rust vermelho neste commit. Não executar commit durante planejamento.


### Task 2: Portar parsers Pi/omp/Kimi e despacho por provider

Status: ready-for-agent (execução depende da aprovação do dono)
Risk: high (risco alto de paridade e retomada)

**Files:** criar `crates/hangar-server/src/transcript/pi.rs` e `crates/hangar-server/src/transcript/kimi.rs`; alterar `crates/hangar-server/src/transcript/mod.rs`. Histórico/tail e habilitação pública pertencem à Task3.
**Interfaces:** parse_obj(&Value)->Vec<ChatEvent>; Pi Stream feed_obj/flush/has_held; LineParser feed/flush/has_held/seed. `history.parse_from` não usa espera; tail200ms só held no EOF. Sem orq::parse_obj nesta Task.

- [ ] **Step 1: Escrever o teste direto dos parsers antes de portá-los**

No final de `transcript/mod.rs`, antes da implementação dos módulos novos:

```rust
#[cfg(test)]
mod provider_tests {
    use super::*;
    use std::path::PathBuf;
    #[test]
    fn provider_parsers_match_python_without_queue() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../backend/tests/fixtures/contract");
        for (provider, stem) in [(Provider::Pi,"pi"),(Provider::Omp,"pi"),(Provider::Kimi,"kimi")] {
            let bytes = std::fs::read(root.join("transcripts").join(format!("{stem}.jsonl"))).unwrap();
            let mut parser = LineParser::new(provider);
            let mut offset = 0_u64;
            let mut events = Vec::new();
            for line in bytes.split_inclusive(|byte| *byte == b'\n') {
                events.extend(parser.feed(line,offset));
                offset += line.len() as u64;
            }
            events.extend(parser.flush(offset));
            let raw = std::fs::read_to_string(root.join("golden").join(format!("{stem}.history.json"))).unwrap();
            let golden = pyjson::loads_lossless(&raw).unwrap();
            let actual = serde_json::to_value(events).unwrap();
            assert_eq!(pyjson::dumps(&actual,true),pyjson::dumps(&golden["full"],true));
        }
    }
}
```

Rodar quando autorizado:
`cargo test --manifest-path crates/Cargo.toml -p hangar-server --lib transcript::provider_tests`
Antes do porte falha por variantes/flush ausentes; depois do porte compara com o oráculo Python.
Manter o teste focado verde antes de habilitar as rotas na Task3.

- [ ] **Step 2: Escrever parsers completos nos dois módulos**

```rust
// Proposta crates/hangar-server/src/transcript/pi.rs
use std::sync::LazyLock;
use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::Value;
use super::{event, finish, py};
static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;:?]*[ -/]*[@-~]").unwrap());
fn clean(text: &str) -> String {
    let mut s = ANSI.replace_all(text, "").into_owned();
    py::scrub_str(&mut s);
    s
}
fn sub_id(id: &str, k: usize) -> String {
    if k == 0 { id.to_owned() } else { format!("{id}:{k}") }
}
fn result_text(blocks: &[Value]) -> String {
    let mut parts = Vec::new();
    for b in blocks {
        let s = match b.get("type").and_then(Value::as_str) {
            Some("text") => clean(b.get("text").and_then(Value::as_str).unwrap_or("")),
            Some("image") => format!("[imagem {}]", b.get("mimeType").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("desconhecida")),
            _ => continue,
        };
        if !s.is_empty() { parts.push(s) }
    }
    parts.join("\n")
}
pub fn parse_obj(obj: &Value) -> Vec<ChatEvent> {
    if obj.get("type").and_then(Value::as_str) != Some("message") { return Vec::new() }
    let Some(msg) = obj.get("message").and_then(Value::as_object) else { return Vec::new() };
    let Some(blocks) = msg.get("content").and_then(Value::as_array) else { return Vec::new() };
    let id = obj.get("id").and_then(Value::as_str).unwrap_or("");
    let ts = msg.get("timestamp").and_then(py::number).map(|n| n / 1000.0);
    let role = msg.get("role").and_then(Value::as_str);
    let mut out = Vec::new();
    if role == Some("toolResult") {
        out.push(ChatEvent {
            tool_use_id: msg.get("toolCallId").and_then(Value::as_str).map(str::to_owned),
            result: Some(result_text(blocks)), is_error: Some(py::truthy(msg.get("isError"))), ts,
            ..event(ChatKind::ToolResult, id.to_owned())
        });
    } else if matches!(role, Some("user" | "assistant")) {
        let cache = msg.get("usage").and_then(|u| u.get("cacheRead"));
        let cache_read = cache.and_then(|v| match v {
            Value::Bool(b) => Some(u64::from(*b)),
            Value::Number(n) => n.as_u64(), _ => None,
        });
        for (k, b) in blocks.iter().enumerate() {
            if !b.is_object() { continue }
            let kind = b.get("type").and_then(Value::as_str);
            let ev = match kind {
                Some("text") => {
                    let Some(text) = b.get("text").and_then(Value::as_str).filter(|s| !py::strip(s).is_empty()) else { continue };
                    ChatEvent { text: Some(clean(text)), ts,
                        cache_read: if role == Some("assistant") { cache_read } else { None },
                        ..event(if role == Some("user") { ChatKind::UserMsg } else { ChatKind::AssistantMsg }, sub_id(id,k)) }
                }
                Some("thinking") if role == Some("assistant") => {
                    let Some(text) = b.get("thinking").and_then(Value::as_str).filter(|s| !py::strip(s).is_empty()) else { continue };
                    ChatEvent { text: Some(clean(text)), ts, ..event(ChatKind::Thinking, sub_id(id,k)) }
                }
                Some("toolCall") if role == Some("assistant") => ChatEvent {
                    tool_name: b.get("name").and_then(Value::as_str).map(str::to_owned),
                    tool_input: Some(b.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default()),
                    tool_use_id: b.get("id").and_then(Value::as_str).map(str::to_owned), ts,
                    ..event(ChatKind::ToolUse, sub_id(id,k)) },
                _ => continue,
            };
            out.push(ev);
        }
    }
    out.iter_mut().for_each(finish);
    out
}
#[derive(Default)]
pub struct Stream { held: Vec<ChatEvent>, held_id: String }
impl Stream {
    pub fn has_held(&self) -> bool { !self.held.is_empty() }
    pub fn flush(&mut self) -> Vec<ChatEvent> {
        self.held_id.clear();
        std::mem::take(&mut self.held)
    }
    pub fn feed_obj(&mut self, obj: &Value) -> Vec<ChatEvent> {
        if obj.get("type").and_then(Value::as_str) == Some("custom_message")
            && obj.get("customType").and_then(Value::as_str) == Some("claude-hook-context")
            && self.has_held()
            && obj.get("parentId").and_then(Value::as_str) == Some(self.held_id.as_str()) {
            let ctx = clean(obj.get("content").and_then(Value::as_str).unwrap_or(""));
            let ctx = py::strip(&ctx);
            if !ctx.is_empty() {
                self.held.retain_mut(|ev| {
                    let text = py::strip(ev.text.as_deref().unwrap_or(""));
                    if let Some(rest) = text.strip_prefix(ctx) {
                        ev.text = Some(py::strip(rest).to_owned());
                        return ev.text.as_ref().is_some_and(|s| !s.is_empty());
                    }
                    true
                });
            }
            return self.flush();
        }
        let mut out = self.flush();
        let mut evs = parse_obj(obj);
        let id = obj.get("id").and_then(Value::as_str).unwrap_or("");
        if !id.is_empty() && !evs.is_empty() && evs.iter().all(|ev| ev.kind == ChatKind::UserMsg) {
            self.held = evs;
            self.held_id = id.to_owned();
        } else { out.append(&mut evs) }
        out
    }
}
```

```rust
// Proposta crates/hangar-server/src/transcript/kimi.rs
use hangar_api::chat::{ChatEvent, ChatKind};
use serde_json::Value;
use super::{event, finish, py, pyjson};
pub fn parse_obj(obj: &Value) -> Vec<ChatEvent> {
    let ts = obj.get("time").and_then(py::number).map(|n| n/1000.0);
    let mut out = Vec::new();
    match obj.get("type").and_then(Value::as_str) {
        Some("context.append_message") => {
            let Some(msg) = obj.get("message").and_then(Value::as_object) else { return out };
            if msg.get("role").and_then(Value::as_str) != Some("user")
                || msg.get("origin").and_then(|o| o.get("kind")).and_then(Value::as_str) != Some("user") { return out }
            let Some(blocks) = msg.get("content").and_then(Value::as_array) else { return out };
            let id = msg.get("id").and_then(Value::as_str).unwrap_or("");
            for (k,b) in blocks.iter().enumerate() {
                if b.get("type").and_then(Value::as_str) != Some("text") { continue }
                let Some(text) = b.get("text").and_then(Value::as_str).filter(|s| !py::strip(s).is_empty()) else { continue };
                out.push(ChatEvent { text: Some(text.to_owned()), ts,
                    ..event(ChatKind::UserMsg, if k == 0 { id.to_owned() } else { format!("{id}:{k}") }) });
            }
        }
        Some("context.append_loop_event") => {
            let Some(ev) = obj.get("event").and_then(Value::as_object) else { return out };
            let id = ev.get("uuid").and_then(Value::as_str).unwrap_or("");
            match ev.get("type").and_then(Value::as_str) {
                Some("content.part") => {
                    let Some(part) = ev.get("part").and_then(Value::as_object) else { return out };
                    let (kind,key) = match part.get("type").and_then(Value::as_str) {
                        Some("text") => (ChatKind::AssistantMsg,"text"),
                        Some("think") => (ChatKind::Thinking,"think"), _ => return out,
                    };
                    let Some(text) = part.get(key).and_then(Value::as_str).filter(|s| !py::strip(s).is_empty()) else { return out };
                    out.push(ChatEvent { text: Some(text.to_owned()), ts, ..event(kind,id.to_owned()) });
                }
                Some("tool.call") => out.push(ChatEvent {
                    tool_name: ev.get("name").and_then(Value::as_str).map(str::to_owned),
                    tool_input: Some(ev.get("args").and_then(Value::as_object).cloned().unwrap_or_default()),
                    tool_use_id: ev.get("toolCallId").and_then(Value::as_str).map(str::to_owned), ts,
                    ..event(ChatKind::ToolUse,id.to_owned()) }),
                Some("tool.result") => {
                    let result = ev.get("result");
                    let text = match result {
                        Some(v @ Value::Object(_)) => v.get("output").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| pyjson::dumps_unicode(v,false)),
                        Some(Value::String(s)) => s.clone(), _ => String::new(),
                    };
                    let cid = ["toolCallId","parentUuid","uuid"].into_iter().find_map(|k| ev.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())).unwrap_or("");
                    if cid.is_empty() { tracing::warn!("kimi: resultado de ferramenta sem identificador"); }
                    out.push(ChatEvent {
                        tool_use_id: ev.get("toolCallId").and_then(Value::as_str).map(str::to_owned),
                        result: Some(text), is_error: Some(py::truthy(result.and_then(|r|r.get("isError")))), ts,
                        ..event(ChatKind::ToolResult,if cid.is_empty() { String::new() } else { format!("res:{cid}") }) });
                }
                _ => {}
            }
        }
        _ => {}
    }
    out.iter_mut().for_each(finish);
    out
}
```


- [ ] **Step 3: Ampliar Provider e LineParser sem aplicar filtro Claude aos outros providers**

Adicionar `mod pi; mod kimi;` em transcript/mod.rs. Substituir enum e impl Provider por:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider { Claude, ClaudeHeadless, Codex, Pi, Omp, Kimi }
impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(Self::Claude), "claude-headless" => Some(Self::ClaudeHeadless),
            "codex" => Some(Self::Codex), _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude", Self::ClaudeHeadless => "claude-headless", Self::Codex => "codex",
            Self::Pi => "pi", Self::Omp => "omp", Self::Kimi => "kimi",
        }
    }
}
```

Substituir struct+impl LineParser completos por:

```rust
pub struct LineParser {
    provider: Provider,
    rewrite: claude::RewriteFilter,
    peer_resolver: claude::PeerResolver,
    pi: pi::Stream,
}
impl LineParser {
    pub fn new(provider: Provider) -> Self {
        Self { provider, rewrite: claude::RewriteFilter::default(), peer_resolver: peer::name_of_pid,
               pi: pi::Stream::default() }
    }
    pub fn with_peer_resolver(mut self, resolver: fn(i64)->Option<String>) -> Self {
        self.peer_resolver=resolver;self
    }
    pub fn seed(&mut self,line:&[u8]) {
        let text=String::from_utf8_lossy(line);
        let Some(value @ Value::Object(_))=pyjson::loads_lossless(py::strip(&text)) else {return};
        match self.provider {
            Provider::Claude|Provider::ClaudeHeadless => { self.rewrite.keep(value.as_object().unwrap()); }
            Provider::Pi|Provider::Omp => { self.pi.feed_obj(&value); }
            Provider::Codex|Provider::Kimi => {}
        }
    }
    pub fn has_held(&self)->bool {
        matches!(self.provider,Provider::Pi|Provider::Omp)&&self.pi.has_held()
    }
    pub fn flush(&mut self,offset:u64)->Vec<ChatEvent> {
        let mut evs=if matches!(self.provider,Provider::Pi|Provider::Omp) {self.pi.flush()} else {Vec::new()};
        for ev in &mut evs {ev.offset=Some(offset)}
        evs
    }
    pub fn feed(&mut self,line:&[u8],offset:u64)->Vec<ChatEvent> {
        let Some(value)=line_value(line) else {return Vec::new()};
        let mut evs=match self.provider {
            Provider::Codex=>codex::parse_rollout_obj(&value),
            Provider::Pi|Provider::Omp=>self.pi.feed_obj(&value),
            Provider::Kimi=>kimi::parse_obj(&value),
            Provider::Claude|Provider::ClaudeHeadless=>match &value {
                Value::Object(obj) if self.rewrite.keep(obj)=>claude::parse_obj(obj,self.peer_resolver),
                _=>Vec::new(),
            },
        };
        for ev in &mut evs {ev.offset=Some(offset)}
        evs
    }
}
```

Nesta Task `Provider::parse` ainda não habilita Pi/Omp/Kimi em rotas públicas. Testes diretos usam os enum variants; manter os asserts existentes de provider não suportado até a Task3 integrar histórico/tail. Orq nunca vira parser Claude.

- [ ] **Step 4: Rodar o teste direto (quando autorizado)**

`cargo test --manifest-path crates/Cargo.toml -p hangar-server --lib transcript::provider_tests`
Esperado: paridade direta de todos os parsers com goldens sem fila; rotas continuam na reserva Python.

- [ ] **Step 5: Commit seletivo dos parsers**

`git add crates/hangar-server/src/transcript/pi.rs crates/hangar-server/src/transcript/kimi.rs crates/hangar-server/src/transcript/mod.rs`
`git commit -m "feat(server): parse Pi, omp and Kimi conversation events"`
Commit futuro após execução autorizada; testes de integração ainda dependem Task3.


### Task 3: Integrar histórico e tail dos providers

Status: ready-for-agent (execução depende da aprovação do dono)
Risk: high (risco alto de paridade e retomada)

**Files:** `crates/hangar-server/src/transcript/history.rs`, `crates/hangar-server/src/transcript/mod.rs`, `crates/hangar-server/src/tail.rs`, criar `crates/hangar-server/tests/contract_providers.rs`, atualizar `crates/hangar-server/tests/contract_history.rs` e `crates/hangar-server/tests/contract_tail.rs`.
**Interfaces:** histórico usa Pi Stream novo por janela, tail offset de soltura; espera de hook somente quando retido.

- [ ] **Step 1: Escrever os testes Rust de igualdade dos contratos**

```rust
// crates/hangar-server/tests/contract_providers.rs
mod common;
use common::{canon, contract, golden};
use hangar_server::{tail, transcript::{HistoryRequest, LineParser, Provider, TAIL_WINDOW, merged_history}};

fn check_tail(provider: Provider, filename: &str, fixture: &str) {
    let path = contract().join("transcripts").join(filename);
    let mut parser = LineParser::new(provider);
    let (frames, _) = tail::read_frames(&path,0,None,&mut parser,"fixture").unwrap();
    let got: Vec<_> = frames.iter().map(|b| {
        let s = String::from_utf8_lossy(b);
        let id = s.lines().find_map(|line| line.strip_prefix("id: ")).unwrap();
        let offset: u64 = id.rsplit(':').next().unwrap().parse().unwrap();
        let data = s.lines().find_map(|line| line.strip_prefix("data: ")).unwrap();
        (offset,canon(&serde_json::from_str::<serde_json::Value>(data).unwrap()))
    }).collect();
    let want: Vec<_> = golden(fixture).as_array().unwrap().iter()
        .map(|row| (row["offset"].as_u64().unwrap(),canon(&row["event"]))).collect();
    assert_eq!(got,want,"{}",provider.as_str());
}

fn check_history(provider: Provider, filename: &str, fixture: &str, queue: &str) {
    let expected = golden(fixture);
    for (name,limit,window) in [
        ("full",None,TAIL_WINDOW), ("limit2",Some(2),TAIL_WINDOW), ("limit200",Some(200),TAIL_WINDOW),
        ("limit2_w512",Some(2),512), ("limit9_w512",Some(9),512), ("limit200_w512",Some(200),512),
    ] {
        for (suffix,queue_file) in [("","sem-fila"),("+queue",queue)] {
            let req=HistoryRequest { provider, jsonl:contract().join("transcripts").join(filename),
                queue:Some(contract().join("queue").join(format!("{queue_file}.jsonl"))),limit,tail_window:window };
            let got:Vec<_>=merged_history(&req).unwrap().iter().map(|ev|canon(&serde_json::to_value(ev).unwrap())).collect();
            let want:Vec<_>=expected[format!("{name}{suffix}")].as_array().unwrap().iter().map(canon).collect();
            assert_eq!(got,want,"{} {name}{suffix}",provider.as_str());
        }
    }
}

#[test]
fn pi_and_omp_tail_match_python() {
    for p in [Provider::Pi,Provider::Omp] { check_tail(p,"pi.jsonl","pi.tail.json") }
}
#[test]
fn pi_and_omp_history_match_python() {
    check_history(Provider::Pi,"pi.jsonl","pi.history.json","pi-fixture");
    check_history(Provider::Omp,"pi.jsonl","omp.history.json","pi-fixture");
}
#[test]
fn kimi_matches_python() {
    check_tail(Provider::Kimi,"kimi.jsonl","kimi.tail.json");
    check_history(Provider::Kimi,"kimi.jsonl","kimi.history.json","kimi-fixture");
}
#[test]
fn partial_line_does_not_advance_cursor() {
    let dir=tempfile::tempdir().unwrap();
    let path=dir.path().join("wire.jsonl");
    let line=r#"{"type":"context.append_loop_event","time":1000,"event":{"type":"content.part","uuid":"a","part":{"type":"text","text":"fim"}}}"#;
    std::fs::write(&path,line).unwrap();
    let mut parser=LineParser::new(Provider::Kimi);
    let (frames,pos)=tail::read_frames(&path,0,None,&mut parser,"wire").unwrap();
    assert!(frames.is_empty());assert_eq!(pos,0);
    std::fs::write(&path,format!("{line}\n")).unwrap();
    let (frames,pos)=tail::read_frames(&path,0,None,&mut parser,"wire").unwrap();
    assert_eq!(frames.len(),1);assert_eq!(pos,line.len() as u64+1);
}
```

- [ ] **Step 2: Integrar Pi Stream e Kimi no histórico, preservando relógios e fila**

Em history.rs ampliar import para `super::{claude, codex, pi, kimi, event, finish, peer, pyjson, Provider, SKIPPED_LINES};`. Substituir parse_from inteiro:

```rust
fn parse_from(req:&HistoryRequest,offset:u64)->io::Result<Parsed> {
    let mut p=Parsed {items:Vec::new(),committed:HashMap::new(),prev_ts:0.0,
        start_ts:if offset>0 {transcript_start_ts(&req.jsonl)} else {0.0}};
    let file=match File::open(&req.jsonl) {
        Ok(f)=>f,
        Err(e) if e.kind()==io::ErrorKind::NotFound=>return Ok(p),
        Err(e)=>return Err(e),
    };
    let mut rd=BufReader::new(file);rd.seek(SeekFrom::Start(offset))?;
    let mut rewrite=matches!(req.provider,Provider::Claude|Provider::ClaudeHeadless)
        .then(claude::RewriteFilter::default);
    let mut pi_stream=matches!(req.provider,Provider::Pi|Provider::Omp).then(pi::Stream::default);
    let mut held_ids=HashSet::new();let mut raw=Vec::new();let mut i=0u64;
    loop {
        raw.clear();if rd.read_until(b'\n',&mut raw)?==0 {break}
        let idx=i;i+=1;
        let mut line=String::from_utf8_lossy(&raw).into_owned();
        if line.ends_with("\r\n") {line.truncate(line.len()-2);line.push('\n')}
        let silent=rewrite.as_ref().and_then(|_|claude::silent_attachment_timestamp(&line));
        let (line_ts,evs)=match silent {
            Some(att)=>(py::iso_timestamp(&att.replace('Z',"+00:00")).unwrap_or(0.0),Vec::new()),
            None=>{
                let Some(value)=pyjson::loads_lossless(&line) else {
                    if !strip(&line).is_empty() {SKIPPED_LINES.fetch_add(1,std::sync::atomic::Ordering::Relaxed);}
                    continue;
                };
                let Value::Object(obj)=&value else {
                    SKIPPED_LINES.fetch_add(1,std::sync::atomic::Ordering::Relaxed);continue;
                };
                if let Some(rw)=rewrite.as_mut() {if !rw.keep(obj) {continue}}
                let evs=match req.provider {
                    Provider::Claude|Provider::ClaudeHeadless=>claude::parse_obj(obj,peer::name_of_pid),
                    Provider::Codex=>codex::parse_rollout_obj(&value),
                    Provider::Pi|Provider::Omp=>pi_stream.as_mut().unwrap().feed_obj(&value),
                    Provider::Kimi=>kimi::parse_obj(&value),
                };
                (ts_of_obj(obj),evs)
            },
        };
        if line_ts>0.0 {
            if p.start_ts==0.0 {p.start_ts=line_ts}
            p.prev_ts=line_ts;
        }
        if evs.is_empty() {continue}
        let ts=if line_ts!=0.0 {line_ts} else {p.prev_ts};p.prev_ts=ts;
        absorb(&mut p,ts,idx,evs,&mut held_ids);
    }
    if let Some(stream)=pi_stream.as_mut() {
        let ts=p.prev_ts;absorb(&mut p,ts,i,stream.flush(),&mut held_ids);
    }
    Ok(p)
}
```

`ts_of_obj`, absorb, merged_history, merge_queue, ETag permanecem íntegros. `HistoryRequest` e `InternalInfo` já têm todo dado necessário; não ampliar protocolo para só esses parsers.

- [ ] **Step 3: Preservar atraso de hook e offset no tail compartilhado**

`read_frames` roda em spawn_blocking existente; espera200ms não prende o event loop. Limite `to` do backfill nunca lê além de corte. Os eventos soltos pelo próximo feed recebem offset dessa linha; EOF sozinho usa offset EOF como Python real. Substituir read_frames e acrescentar helper privado de quadros (não substituir sse_frame):

```rust
fn append_events(frames:&mut Vec<Bytes>,evs:Vec<hangar_api::chat::ChatEvent>,key:&str)->std::io::Result<()> {
    for ev in evs {
        let id=ev.offset.map(|at|format!("{key}:{at}"));
        let data=serde_json::to_string(&ev).map_err(std::io::Error::other)?;
        frames.push(sse_frame("message",&data,id.as_deref()));
    }
    Ok(())
}
fn feed_frames(frames:&mut Vec<Bytes>,parser:&mut LineParser,line:&[u8],at:u64,key:&str)->std::io::Result<()> {
    let evs=match std::panic::catch_unwind(std::panic::AssertUnwindSafe(||parser.feed(line,at))) {
        Ok(evs)=>evs,
        Err(_)=>{
            tracing::error!(key,offset=at,"linha do transcript derrubou o parser; pulada");
            SKIPPED_LINES.fetch_add(1,Ordering::Relaxed);Vec::new()
        },
    };
    append_events(frames,evs,key)
}
pub fn read_frames(path:&Path,from:u64,to:Option<u64>,parser:&mut LineParser,key:&str)->std::io::Result<(Vec<Bytes>,u64)> {
    let file=match File::open(path) {
        Ok(f)=>f,
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok((Vec::new(),from)),
        Err(e)=>return Err(e),
    };
    let mut reader=BufReader::new(file);reader.seek(SeekFrom::Start(from))?;
    let mut frames=Vec::new();let mut at=from;let mut line=Vec::new();let mut flush_at=from;
    let skipped_before=SKIPPED_LINES.load(Ordering::Relaxed);
    while to.is_none_or(|t|at<t) {
        flush_at=at;line.clear();let n=reader.read_until(b'\n',&mut line)?;
        if n==0||line.last()!=Some(&b'\n') {reader.seek(SeekFrom::Start(at))?;break}
        feed_frames(&mut frames,parser,&line,at,key)?;at+=n as u64;
        flush_at=at;
    }
    if parser.has_held()&&to.is_none() {
        std::thread::sleep(std::time::Duration::from_millis(200));
        loop {
            line.clear();let n=reader.read_until(b'\n',&mut line)?;
            if n==0||line.last()!=Some(&b'\n') {reader.seek(SeekFrom::Start(at))?;break}
            feed_frames(&mut frames,parser,&line,at,key)?;
            flush_at=at;at+=n as u64;
        }
    }
    append_events(&mut frames,parser.flush(flush_at),key)?;
    log_skipped(key,skipped_before);
    Ok((frames,at))
}
```

Para primeiro aparelho conectar justamente entre user completo e marcador hook, corte inicial tem que finalizar a mesma janela de200ms. Não basta backfill.flush imediato até corte sem preparar memória do leitor compartilhado. Substituir `TailState::cut` e `seed_rewrite`:

```rust
pub fn cut(&mut self)->std::io::Result<u64> {
    if let Some(p)=self.pos {return Ok(p)}
    #[cfg(test)]
    if self.key==PANIC_KEY {panic!("corte de teste")}
    let mut end=complete_end(&self.path).inspect_err(|e| {
        tracing::warn!(key=%self.key,path=%self.path.display(),"fim do transcript ilegível; tenta de novo: {e}");
    })?;
    self.seed_rewrite(end);
    if self.parser.has_held() {
        std::thread::sleep(std::time::Duration::from_millis(200));
        end=complete_end(&self.path)?;
        self.parser=LineParser::new(self.provider);
        self.seed_rewrite(end);
        self.parser.flush(end);
    }
    self.pos=Some(end);Ok(end)
}
fn seed_rewrite(&mut self,end:u64) {
    if !matches!(self.provider,Provider::Claude|Provider::ClaudeHeadless|Provider::Pi|Provider::Omp) {return}
    let Ok(start)=tail_offset(&self.path,BACKFILL_LINES) else {return};
    let Ok(file)=File::open(&self.path) else {return};
    let mut reader=BufReader::new(file);
    if start>=end||reader.seek(SeekFrom::Start(start)).is_err() {return}
    let mut at=start;let mut line=Vec::new();
    while at<end {
        line.clear();
        let n=match reader.read_until(b'\n',&mut line) {
            Ok(n) if n>0&&line.last()==Some(&b'\n')=>n as u64,_=>break,
        };
        let parser=&mut self.parser;
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(||parser.seed(&line))).is_err() {
            tracing::error!(key=%self.key,offset=at,"linha do transcript derrubou o parser na semeadura; pulada");
        }
        at+=n;
    }
}
```

Método histórico `seed_rewrite` passa a preparar também memória Pi; nome legado preservado, comentário existente precisa dizer isso. Kimi jamais usa RewriteFilter. `TailState::poll` truncamento já recria LineParser; backfill continua criando novo parser por aparelho.

- [ ] **Step 4: Acrescentar verificação de hook dividido entre lotes e retomada**

Adicionar ao contract_providers.rs:

```rust
#[test]
fn pi_hook_written_after_first_batch_is_removed_before_flush() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("pi.jsonl");
    let user=serde_json::json!({"type":"message","id":"u","message":{"role":"user","timestamp":1000,
        "content":[{"type":"text","text":"hook\n\noi"}]}}).to_string()+"\n";
    let hook=serde_json::json!({"type":"custom_message","customType":"claude-hook-context","parentId":"u","content":"hook"}).to_string()+"\n";
    std::fs::write(&path,&user).unwrap();
    let writer_path=path.clone();let writer=std::thread::spawn(move|| {
        use std::io::Write;
        std::thread::sleep(std::time::Duration::from_millis(40));
        std::fs::OpenOptions::new().append(true).open(writer_path).unwrap().write_all(hook.as_bytes()).unwrap();
    });
    let mut parser=LineParser::new(Provider::Pi);
    let (frames,_) = tail::read_frames(&path,0,None,&mut parser,"pi").unwrap();writer.join().unwrap();
    assert_eq!(frames.len(),1);
    let frame=String::from_utf8_lossy(&frames[0]);
    assert!(frame.lines().any(|s|s==format!("id: pi:{}",user.len())));
    let data=frame.lines().find_map(|s|s.strip_prefix("data: ")).unwrap();
    let ev:serde_json::Value=serde_json::from_str(data).unwrap();assert_eq!(ev["text"],"oi");
}
#[test]
fn pi_unmatched_user_at_eof_flushes_with_eof_offset() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("pi.jsonl");
    let user=serde_json::json!({"type":"message","id":"u","message":{"role":"user",
        "content":[{"type":"text","text":"oi"}]}}).to_string()+"\n";
    std::fs::write(&path,&user).unwrap();let mut parser=LineParser::new(Provider::Omp);
    let (frames,pos)=tail::read_frames(&path,0,None,&mut parser,"pi").unwrap();
    assert_eq!(pos,user.len() as u64);assert_eq!(frames.len(),1);
    assert!(String::from_utf8_lossy(&frames[0]).lines().any(|s|s==format!("id: pi:{}",user.len())));
}
```

Prova de concorrência atrasada não mede milissegundos de funcionamento; só exige irmão escrito dentro da janela. Na implementação revisar se o teste de40ms corre sob carga e o custo da janela é aceitável; o delay de200ms é do contrato atual, não ajuste arbitrário novo.

- [ ] **Step 5: Reconstituir a mensagem Pi anterior ao cursor e escrever os testes de retomada**

#### Evidência e regra

`Pi.Stream.feed` guarda uma mensagem exclusivamente de usuário até o PRÓXIMO objeto JSON válido e a libera no offset desse próximo objeto (`pi/transcript.py:245-259`, `TranscriptTailer:850-857`). O flush sem irmão usa EOF/parcial/última releitura (`transcript.py:841,876-895`). Um cursor na linha que libera sozinho não contém a mensagem retida. A parte1 `tail.backfill:197-200` cria parser vazio e retorna cedo em start>=cut. Portanto retomada nessa linha ou EOF pode perder o segundo bloco de uma mensagem Pi já parcialmente recebida.

Fortalecimento explícito desta migração: antes do cursor Pi/omp, preparar memória com o último objeto JSON COMPLETO válido anterior. O parser tem só uma linha de memória; objetos inválidos/vazios não o alteram e são ignorados nessa busca. O objeto encontrado é alimentado SEM publicar eventos. Nenhum id ou offset existente muda. Apenas blocos que o Python atual perdia reaparecem na retomada. Atualizar golden de retomada específico para comportamento corrigido; golden de leitura contínua permanece Python.

Não basta semear a última LINHA física: inválidas entre user e release precisam ser puladas. Não basta retornar cedo quando start==cut: held user no EOF precisa flush para recuperar irmãos. Arquivo com linha enorme exige janela crescente sem teto artificial, como os tail_offset existentes.

#### Código completo do ajuste mínimo

Adicionar em tail.rs (usa imports e `read_window`, `open_sized` já existentes):

```rust
fn seed_pi_before(path:&Path,from:u64,parser:&mut LineParser)->std::io::Result<()> {
    if from==0 {return Ok(())}
    let Some((mut file,size))=open_sized(path)? else {return Ok(())};
    let prefix=from.min(size);
    let mut window=TAIL_WINDOW;
    loop {
        let (base,buf)=read_window(&mut file,prefix,window)?;
        // Só objetos de linhas completas antes do cursor; prefixo cortado não serve como memória.
        let mut end=buf.iter().rposition(|&b|b==b'\n').map_or(0,|i|i+1);
        while end>0 {
            let begin=buf[..end-1].iter().rposition(|&b|b==b'\n').map_or(0,|i|i+1);
            if begin==0&&base>0 {break}
            let raw=&buf[begin..end];
            if crate::transcript::decode_line(raw).is_some_and(|value|value.is_object()) {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(||parser.seed(raw))).is_err() {
                    return Err(std::io::Error::other("parser caiu ao preparar retomada Pi"));
                }
                return Ok(());
            }
            end=begin;
        }
        if base==0 {return Ok(())}
        window=window.saturating_mul(4);
    }
}

pub fn backfill(path:&Path,key:&str,provider:Provider,resume:Option<&str>,cut:u64)->Vec<Bytes> {
    let size=std::fs::metadata(path).map(|m|m.len()).unwrap_or(0);
    let resumed=resume.and_then(|raw| {
        let (stem,off)=raw.rsplit_once(':')?;
        if stem.is_empty()||stem!=key {return None}
        off.trim().parse::<u64>().ok().filter(|o|*o<=size)
    });
    let start=match resumed.map_or_else(||tail_offset(path,BACKFILL_LINES),Ok) {
        Ok(s)=>s,
        Err(e)=>{
            tracing::warn!(key,path=%path.display(),"cauda do transcript falhou: {e}");
            return Vec::new();
        },
    };
    if start>cut {return Vec::new()}
    let mut parser=LineParser::new(provider);
    if matches!(provider,Provider::Pi|Provider::Omp) {
        if let Err(e)=seed_pi_before(path,start,&mut parser) {
            tracing::warn!(key,path=%path.display(),"retomada Pi não pôde preparar memória: {e}");
            return Vec::new();
        }
    }
    if start==cut&&!parser.has_held() {return Vec::new()}
    match read_frames(path,start,Some(cut),&mut parser,key) {
        Ok((frames,_))=>frames,
        Err(e)=>{
            tracing::warn!(key,"cauda do transcript falhou: {e}");
            Vec::new()
        },
    }
}
```

`LineParser::seed` proposto anteriormente já alimenta pi.feed_obj sem publicar. `read_frames` proposto já chama flush(flush_at), e start==cut chega ali com flush_at=from e nenhuma espera200ms porque to é Some(cut). O preparo do leitor compartilhado em `TailState::cut` continua usando seed+200ms só na própria sessão; a correção de retomada é do parser PRÓPRIO do backfill e não modifica estado compartilhado.

Não usar `decode_line` para alimentar o Pi diretamente: ela troca surrogate antes da comparação raw ids; o helper acima usa decode_line SOMENTE para conferir se objeto é válido, e `parser.seed(raw)` preserva a semântica crua original do loader.

#### Testes completos a acrescentar no contract_providers.rs

```rust
fn pi_resume_fixture(path:&std::path::Path,release:Option<&str>)->(u64,u64) {
    let user=serde_json::json!({"type":"message","id":"u1","message":{"role":"user","timestamp":1000,
        "content":[{"type":"text","text":"contexto\n\nprimeiro"},{"type":"text","text":"segundo"}]}})
        .to_string()+"\n";
    let release_offset=user.len() as u64;
    let mut bytes=user.into_bytes();
    if let Some(line)=release {bytes.extend_from_slice(line.as_bytes());bytes.push(b'\n')}
    std::fs::write(path,&bytes).unwrap();
    (release_offset,bytes.len() as u64)
}
fn data_events(frames:&[bytes::Bytes])->Vec<serde_json::Value> {
    frames.iter().map(|frame| {
        let text=String::from_utf8_lossy(frame);
        let data=text.lines().find_map(|line|line.strip_prefix("data: ")).unwrap();
        serde_json::from_str(data).unwrap()
    }).collect()
}
#[test]
fn pi_resume_release_metadata_recovers_both_user_blocks() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("pi.jsonl");
    let (release,cut)=pi_resume_fixture(&path,Some(r#"{"type":"model_change","id":"model"}"#));
    let mut parser=LineParser::new(Provider::Pi);
    let (all,_)=tail::read_frames(&path,0,None,&mut parser,"pi").unwrap();
    assert_eq!(all.len(),2);
    // O aparelho recebeu só primeiro quadro; SSE cursor é o início de model_change.
    let frames=tail::backfill(&path,"pi",Provider::Pi,Some(&format!("pi:{release}")),cut);
    let events=data_events(&frames);
    assert_eq!(events.len(),2);assert_eq!(events[0]["id"],"u1");assert_eq!(events[1]["id"],"u1:1");
    for frame in frames {assert!(String::from_utf8_lossy(&frame).lines().any(|line|line==format!("id: pi:{release}")))}
}
#[test]
fn omp_resume_release_hook_recovers_cleaned_user_blocks() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("omp.jsonl");
    let (release,cut)=pi_resume_fixture(&path,Some(r#"{"type":"custom_message","customType":"claude-hook-context","parentId":"u1","content":"contexto"}"#));
    let frames=tail::backfill(&path,"omp",Provider::Omp,Some(&format!("omp:{release}")),cut);
    let events=data_events(&frames);
    assert_eq!(events.len(),2);assert_eq!(events[0]["text"],"primeiro");assert_eq!(events[1]["text"],"segundo");
    assert_eq!(events[0]["id"],"u1");assert_eq!(events[1]["id"],"u1:1");
}
#[test]
fn pi_resume_eof_flush_recovers_missing_user_sibling() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("pi.jsonl");
    let (_,cut)=pi_resume_fixture(&path,None);
    let frames=tail::backfill(&path,"pi",Provider::Pi,Some(&format!("pi:{cut}")),cut);
    let events=data_events(&frames);
    assert_eq!(events.len(),2);assert_eq!(events[1]["id"],"u1:1");
    for frame in frames {assert!(String::from_utf8_lossy(&frame).lines().any(|line|line==format!("id: pi:{cut}")))}
}
#[test]
fn pi_resume_skips_invalid_lines_when_rebuilding_held_state() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("pi.jsonl");
    let (_,user_end)=pi_resume_fixture(&path,None);
    let invalid=b"{broken\n\n";
    use std::io::Write;
    let mut file=std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(invalid).unwrap();
    let release=user_end+invalid.len() as u64;
    let model=b"{\"type\":\"model_change\"}\n";
    file.write_all(model).unwrap();drop(file);
    let cut=release+model.len() as u64;
    let frames=tail::backfill(&path,"pi",Provider::Pi,Some(&format!("pi:{release}")),cut);
    let events=data_events(&frames);assert_eq!(events.len(),2);assert_eq!(events[1]["id"],"u1:1");
}
```

Commit da Task3 continua stage seletivo tail.rs e contract_providers.rs com history.rs. Esses testes adicionam a prova corrigida de retomada que o golden Python não pode fornecer porque reproduziria a perda.

Limite/custo: leitura reversa procura só o último objeto válido anterior; normalmente uma linha. Janela inicial256KB cresce4x para objeto maior ou sequência longa de lixo. Há parsing do objeto anterior na reconexão, nenhum parse extra por evento live e nenhum armazenamento de histórico em memória compartilhada. Sem benchmark executado; não quantificar ganho.


- [ ] **Step 6: Habilitar os providers somente após integrar histórico e tail**

Substituir somente `Provider::parse` no `transcript/mod.rs`:

```rust
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(Self::Claude), "claude-headless" => Some(Self::ClaudeHeadless),
            "codex" => Some(Self::Codex), "pi" => Some(Self::Pi), "omp" => Some(Self::Omp),
            "kimi" => Some(Self::Kimi), _ => None,
        }
    }
```

Atualizar `contract_history.rs::internal_info_becomes_history_request` para esperar
`assert_eq!(pi.history_request(None).unwrap().provider, Provider::Pi)`;
`contract_tail.rs::provider_from_python_name` deve conferir Pi/Omp/Kimi reconhecidos e orq/outro
desconhecidos. Não acrescentar `Provider::Orq`: ela usa a ponte materializada na Task6.

- [ ] **Step 7: Rodar contratos focados e verificar uso real (quando autorizado)**

Comandos focados (executar juntos só após pedido):
`cargo test --manifest-path crates/Cargo.toml -p hangar-server --test contract_providers --test contract_history --test contract_tail`
Backend focados já existentes `backend/tests/test_pi_transcript.py`, `test_kimi_transcript.py`, testes TranscriptTailer relacionados e testes orq separados da ponte.
Uso real com dono: abrir uma sessão Pi/omp/Kimi existente em dois aparelhos; texto, pensamento recolhido, ferramenta encerra, pergunta responde e não reabre; texto hook não aparece; reconectar/retomar/rebind/fork com conversa correta. NÃO criar, matar, reiniciar ou dirigir sessão viva sem dono autorizar o fluxo. Regra do projeto exige uso real, não só testes.

- [ ] **Step 8: Commit seletivo após verificações autorizadas**

`git add crates/hangar-server/src/transcript/history.rs crates/hangar-server/src/transcript/mod.rs crates/hangar-server/src/tail.rs crates/hangar-server/tests/contract_providers.rs crates/hangar-server/tests/contract_history.rs crates/hangar-server/tests/contract_tail.rs`
`git commit -m "feat(server): serve Pi, omp and Kimi history and shared tails"`
Não executar nesta etapa, nem push sem pedido. Plano/spec ignorados não são stageados.

#### Limitações explícitas destes trechos

Código de plano NÃO compilado. Orq não é variante do parser: a Task6 atende sua fonte Python materializada. Cache_read Rust usa u64 (Python int aceita negativo): documentos/fixtures devem fixar valores de contador válido; mudança de tipo exige contrato coordenado. Detalhe tail EOF offset segue implementação Python. A retomada Pi/omp prepara a mensagem retida anterior, conforme complemento completo em /tmp/parte2-provider-resume.md, para recuperar irmãos com ids/offsets preservados. Esse fortalecimento frente à perda do Python atual é declarado no contrato, sem mudar formato de id e sem esperar correção posterior.


#### Espera de hook e mutex — decisão adotada

Usar a proposta mínima acima:200ms sob mutex SOMENTE TailState da própria sessão via spawn_blocking. Não segura mapa de hubs nem event loop. Outra sessão segue lendo normalmente; attach da mesma sessão pode esperar até200ms enquanto finaliza o par user/hook, que mantém corte consistente. A janela só existe quando Stream tem evento retido, inclusive inicial cut entre as duas escritas. Isso evita estado adicional de corte publicado/cursor e é a arquitetura escolhida pelo pai para o plano. Deve constar como limite explícito, sem dizer que não existe espera sob mutex.

u64 cache_read: contrato Rust existente só admite>=0, Python int aceita negativos. Preserve contrato Rust público nesta Task e registre essa limitação; fixture legítima usa contagens>=0 e bool coerced. Não afirmar igualdade de todo JSON inválido ou contador negativo e não ampliar tipo/escopo silenciosamente. Fonte linha hangar-api/src/chat.rs cache_read e Pi.transcript.py:91. Se contador negativo real aparecer, endpoint deve cair no Python ou mudança coordenada do tipo exigir nova Task; benchmark não prova paridade desse domínio.


## Complemento obrigatório da Task3: retomada Pi/omp

Aplicar a proposta COMPLETA de /tmp/parte2-provider-resume.md ao plano final: seed_pi_before + backfill substituído + testes de metadata/hook/EOF/lixo anterior. Não tratar como opcional: sem ele critérios de retomada sem perda seriam contraditórios. O arquivo é documento temporário; incorporar o código antes de apagá-lo.

### Task 4: Provar as rotas e o compartilhamento Pi/omp/Kimi

**Files:**
- Modify: `crates/hangar-server/tests/conversations.rs`
- Modify: `crates/hangar-server/tests/contract_history.rs`
- Modify: `crates/hangar-server/tests/contract_tail.rs`

**Interfaces:**
- Consumes: `Provider::{Pi,Omp,Kimi}`, `InternalInfo::history_request`, `Binding::from_info` e
  `LineParser`/tail já ampliados nas Tasks2/3; `tests/fake/mod.rs` existente.
- Produces: teste HTTP/SSE que impede fallback silencioso ao Python para esses providers e prova
  dois clientes usando um side-events, com history/ETag e cursor de vários blocos.

- [ ] **Step 1: Escrever teste HTTP/SSE primeiro**

Adicionar no fim de `tests/conversations.rs`; reusar imports e helpers reais do arquivo:

```rust
#[tokio::test]
async fn provider_history_and_two_devices_use_rust_and_keep_sibling_cursor() {
    for provider in ["pi", "omp", "kimi"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider-a.jsonl");
        let obj = if provider == "kimi" {
            json!({"type":"context.append_message","time":1000,"message":{
                "id":"u1","role":"user","origin":{"kind":"user"},"content":[
                    {"type":"text","text":"um"},{"type":"text","text":"dois"}]}})
        } else {
            json!({"type":"message","id":"u1","message":{
                "role":"user","timestamp":1000,"content":[
                    {"type":"text","text":"um"},{"type":"text","text":"dois"}]}})
        };
        let mut bytes = serde_json::to_vec(&obj).unwrap();
        bytes.push(b'\n');
        // Uma linha seguinte solta a fala Pi com offset próprio, sem depender de tempo.
        let release_at = bytes.len() as u64;
        if provider != "kimi" {
            bytes.extend_from_slice(b"{\"type\":\"model_change\"}\n");
        }
        std::fs::write(&path, &bytes).unwrap();
        let (python, upstream) = spawn_fake().await;
        let info = info_json(provider, &path);
        python.set_info(info.clone());
        let server = spawn_server(config(upstream, "127.0.0.1")).await;
        let response = reqwest::Client::new()
            .get(format!("http://{server}/api/sessions/s/history?limit=2"))
            .bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        let tag = response.headers()["etag"].to_str().unwrap().to_owned();
        let payload: Value = response.json().await.unwrap();
        assert_eq!(payload[0]["id"], "u1");
        assert_eq!(payload[1]["id"], "u1:1");
        assert_eq!(python.hits_to("/api/sessions/s/history"), 0);
        let not_changed = reqwest::Client::new()
            .get(format!("http://{server}/api/sessions/s/history?limit=2"))
            .bearer_auth(OWNER).header("if-none-match", &tag).send().await.unwrap();
        assert_eq!(not_changed.status().as_u16(), 304);
        let mut a = sse(open_events(server, "s", "", &[]).await);
        let first = messages(&mut a, 2).await;
        assert_eq!(id_of(&first[0]), "u1");
        assert_eq!(id_of(&first[1]), "u1:1");
        assert_eq!(first[0].id, first[1].id);
        let offset = if provider == "kimi" { 0 } else { release_at };
        let key = info["session_key"].as_str().unwrap();
        assert_eq!(first[0].id, format!("{key}:{offset}"));
        let query = format!("last_event_id={}", first[0].id);
        let mut b = sse(open_events(server, "s", &query, &[]).await);
        let resumed = messages(&mut b, 2).await;
        assert_eq!(id_of(&resumed[0]), "u1");
        assert_eq!(id_of(&resumed[1]), "u1:1");
        wait_until(|| python.side_conns() == 1).await;
        assert_eq!(python.side_apps(), vec!["1".to_owned()]);
        assert_eq!(python.hits_to("/api/sessions/s/events"), 0);
    }
}
```

- [ ] **Step 2: Atualizar asserts de provider e conferir integração sem caminho alternativo**

Em `contract_history.rs:internal_info_becomes_history_request`, conferir o assert atualizado na Task3
por `assert_eq!(pi.history_request(None).unwrap().provider, Provider::Pi)`; manter assert da sessão
sem jsonl e provider realmente desconhecido. Em `contract_tail.rs:provider_from_python_name`, já atualizado na Task3,
conferir `parse("pi")=Some(Pi)`, `"omp"=Omp`, `"kimi"=Kimi`, `"orq"=None` e `"outro"=None`.
Não acrescentar handler duplicado: os handlers existentes escolhem pelo `InternalInfo` e Binding.
Se o teste mostrar que o dispatch não alcança a Task3, corrigir exatamente esse dispatch.

- [ ] **Step 3: Rodar os testes de rotas/contrato (quando autorizado)**

```bash
cargo test --manifest-path crates/Cargo.toml -p hangar-server --test conversations --test contract_history --test contract_tail --test contract_providers
```

Esperado: testes de providers novos e antigos passam; nenhum `from-python` para history/events
Pi/omp/Kimi com arquivo válido. Convidado/sem arquivo/provider desconhecido seguem Python.
O teste de rebind existente `new_info_resets_every_device_and_follows_new_file` deve continuar
verde; acrescentar cenários de troca Claude→Pi/Kimi e recriação em sua mesma infraestrutura,
com barreira de `info`, novos payloads acima e `assert_eq!(next_named(...,"reset").data,"{}")`.

- [ ] **Step 4: Commit da prova das rotas**

```bash
git status --short
git add crates/hangar-server/tests/conversations.rs
git commit -m "test(server): cover Pi omp and Kimi conversation routes"
git status --short
```


### Task 5: Leitura da lista e distribuição compartilhada em Rust

Status: ready-for-agent (execução depende da aprovação do dono)
Risk: high (risco alto de paridade e retomada)
Depends on: Tasks 1–4

**Files:**
- Modify: `backend/app/api.py` (`list_sessions`, extrair helper `_session_list`).
- Modify: `backend/app/internal_api.py` (rotas `/internal/sessions`, `/internal/list-events` e `/internal/list-nav`).
- Modify: `backend/app/sse.py` (`nav_snapshot`, argumento `internal_nav`, `nav_pump`).
- Modify: `backend/app/rust_server.py` (`RUST_SERVER_PROTOCOL = 2`).
- Modify: `crates/hangar-server/src/lib.rs` (`pub mod list`, `INTERNAL_PROTOCOL = 2`).
- Modify: `crates/hangar-server/src/routes.rs` (`AppState.list`, registrar duas rotas da lista).
- Create: `crates/hangar-server/src/list.rs` (código completo abaixo, incluindo testes).
- Create: `backend/tests/test_internal_list.py` (testes abaixo).

**Interfaces:**
- GET público `/api/sessions`: array `SessionInfo`, fresco via GET interno `/internal/sessions`; não usa o snapshot estável SSE como resposta GET.
- GET público `/api/sessions/events`: `sessions`, `list_error`, `shortcut_terminals`, `nav`, `ping`; mesmos conteúdos públicos. Cliente lento fecha e reconecta; não existe evento `reset` da lista.
- GET `/internal/sessions`: exige loopback+segredo existente; aplica a mesma função/filtros do GET público para o dono.
- GET `/internal/list-nav`: retrato atual de `nav_snapshot`, com o mesmo segredo interno. Nova anexação consulta esse mapa antes de aceitar o SSE; erro faz proxy.
- GET `/internal/list-events`: uma conexão Rust compartilhada; assina **o mesmo** `_ListRefresher` existente; `sessions` já filtrado para owner, `list_error`, `shortcut_terminals`, `nav_snapshot`, `ping`.
- Revisão de `sessions` e instante de navegação são independentes; anexação usa navegação fresca e ignora snapshot de origem observado antes desse GET. Falha da origem apaga o cache de navegação, mantendo sessões/atalhos.
- `nav_snapshot`: JSON `{"observed_at": <time.monotonic_ns()>, "pending": {name: {url, ts}}}`. É periódico a cada 1 s na ponte, mesmo igual, para garantir entrega do pedido surgido na janela GET/anexação. Instantes são produzidos apenas no Python; Rust não compara relógios de processos ou máquinas. Nunca sai no SSE público. Cada assinante transforma em `nav` `{name,url}` quando timestamp ainda não visto.
- Cada novo assinante Rust faz um GET fresco interno antes de aceitar o SSE. Usa esse corpo como seu primeiro `sessions`, evitando reaplicar um snapshot owner cacheado antes de uma mudança de `owner_sees`. Não é outro produtor: GET reutiliza `recent_list` e filtros existentes. Só atualizações de `sessions` da ponte substituem essa lista, não snapshots gerados apenas por atalhos/nav.
- Política de visibilidade durante a conexão segue a fonte atual: filtro reaplicado quando o produtor publica versão. Não prometer revogação instantânea sem versão/invalidação adicional. Peers com token do dono são indistinguíveis de aparelhos remotos do dono; rotas específicas de peering continuam no proxy, sem novo cabeçalho.
- Contrato interno nesta Task sobe **1 → 2 nos dois lados no mesmo commit**. A Task 6 sobe **2 → 3** quando adicionar suas próprias rotas.

- [ ] **Step 1: Escrever os testes da ponte Python antes de alterar suas rotas**

Criar `backend/tests/test_internal_list.py`:

```python
import asyncio
import json
from types import SimpleNamespace

import pytest

from app import api, guest_users, internal_api, sse
from app.models import SessionInfo


@pytest.mark.asyncio
async def test_owner_snapshot_keeps_current_visibility_filter(monkeypatch):
    infos = [SessionInfo(name="owner"), SessionInfo(name="private")]
    monkeypatch.setattr(sse, "recent_list", lambda age: infos)
    monkeypatch.setattr(guest_users, "has_claims", lambda: True)
    seen = []

    def visible(viewer, rows, name_of):
        seen.append(viewer)
        return [row for row in rows if name_of(row) != "private"]

    monkeypatch.setattr(guest_users, "filter_visible", visible)
    result = await internal_api.owner_sessions()
    assert [row.name for row in result] == ["owner"]
    assert seen == [None]


@pytest.mark.asyncio
async def test_internal_get_reuses_fresh_latest_not_stable_sse(monkeypatch):
    fresh = SessionInfo(name="owner", last_activity=42.0, status_line="fresh")
    monkeypatch.setattr(sse, "recent_list", lambda age: [fresh])
    monkeypatch.setattr(guest_users, "has_claims", lambda: False)
    result = await internal_api.owner_sessions()
    assert result[0].last_activity == 42.0
    assert result[0].status_line == "fresh"


@pytest.mark.asyncio
async def test_internal_get_recomputes_when_recent_is_invalid(monkeypatch):
    raw = SessionInfo(name="new", last_activity=None)
    monkeypatch.setattr(sse, "recent_list", lambda age: None)
    monkeypatch.setattr(api, "_guardar_snap", lambda: [raw])
    monkeypatch.setattr(guest_users, "has_claims", lambda: False)

    async def decorate(infos):
        assert infos[0] is not raw
        infos[0].last_activity = 64.0
        return infos

    monkeypatch.setattr(api.registry, "list_with_state", decorate)
    result = await internal_api.owner_sessions()
    assert result[0].last_activity == 64.0
    assert raw.last_activity is None


@pytest.mark.asyncio
async def test_internal_nav_route_reads_current_pending_map(monkeypatch):
    pending = {"new": {"url": "http://fixture.invalid", "ts": 12.0}}
    monkeypatch.setattr(sse, "nav_snapshot", lambda: json.dumps({"observed_at": 42, "pending": pending}))
    assert await internal_api.owner_list_nav() == {"observed_at": 42, "pending": pending}
    pending.clear()
    assert await internal_api.owner_list_nav() == {"observed_at": 42, "pending": {}}


def test_nav_snapshot_preserves_pending_timestamps(monkeypatch):
    pending = {"owner": {"url": "http://fixture.invalid", "ts": 42.0}}
    monkeypatch.setattr(sse, "nav_vivos", lambda: pending)
    monkeypatch.setattr(sse.time, "monotonic_ns", lambda: 64)
    assert json.loads(sse.nav_snapshot()) == {"observed_at": 64, "pending": pending}
    pending.clear()
    assert json.loads(sse.nav_snapshot()) == {"observed_at": 64, "pending": {}}


@pytest.mark.asyncio
async def test_internal_list_uses_existing_refresher_and_initial_nav_snapshot(monkeypatch):
    condition = asyncio.Condition()
    counts = {"acquire": 0, "release": 0, "enter": 0, "exit": 0}

    def acquire():
        counts["acquire"] += 1
        return condition

    def release():
        counts["release"] += 1

    refresher = SimpleNamespace(
        acquire=acquire, release=release, version=1,
        errored=False, data='[{"name":"owner"}]', shortcuts_data="[]",
    )
    monkeypatch.setattr(sse, "_list_refresher", refresher)
    monkeypatch.setattr(guest_users, "has_claims", lambda: False)
    monkeypatch.setattr(sse.diag, "registrar", lambda *args, **kwargs: None)
    monkeypatch.setattr(sse, "nav_snapshot", lambda: '{"observed_at":42,"pending":{"owner":{"url":"http://fixture.invalid","ts":9}}}')
    monkeypatch.setattr(sse.plugin_bridge, "app_entrou", lambda: counts.__setitem__("enter", counts["enter"] + 1))
    monkeypatch.setattr(sse.plugin_bridge, "app_saiu", lambda: counts.__setitem__("exit", counts["exit"] + 1))
    stream = sse.list_events(internal_nav=True)
    events = []
    try:
        while not any(event["event"] == "nav_snapshot" for event in events):
            events.append(await asyncio.wait_for(anext(stream), 1.0))
    finally:
        await stream.aclose()
    assert any(event["event"] == "sessions" for event in events)
    assert not any(event["event"] == "nav" for event in events)
    assert counts == {"acquire": 1, "release": 1, "enter": 1, "exit": 1}
```

- [ ] **Step 2: Rodar os testes Python novos (quando autorizado)**

```bash
(cd backend && uv run pytest tests/test_internal_list.py)
```

Antes da implementação, esperar falha pelos símbolos novos (`owner_sessions`, `nav_snapshot`, `internal_nav`); não interpretar erro de ambiente/importação como prova da mudança.

- [ ] **Step 3: Extrair o GET existente e adicionar somente as interfaces internas**

Em `backend/app/api.py`, substituir `list_sessions` pela função seguinte e helper. Manter o decorator `response_model=list[SessionInfo]` da rota pública; não mexer em `_guardar_snap`, `_invalidate_lists`, `registry.list` ou `list_with_state`:

```python
async def _session_list(guest=None, viewer=None) -> list[SessionInfo]:
    from app.sse import recent_list

    decorated = recent_list(2.0)
    if decorated is not None:
        if guest is not None:
            decorated = [i for i in decorated if guest.sees(i.name)]
        if viewer is not None or guest_users.has_claims():
            decorated = await asyncio.to_thread(
                guest_users.filter_visible, viewer, decorated, lambda i: i.name)
        return decorated if guest is None else [guest_safe(i, guest) for i in decorated]
    snap = await asyncio.to_thread(_guardar_snap)
    if guest is not None:
        snap = [i for i in snap if guest.sees(i.name)]
    if viewer is not None or guest_users.has_claims():
        snap = await asyncio.to_thread(
            guest_users.filter_visible, viewer, snap, lambda i: i.name)
    decorated = await registry.list_with_state([i.model_copy() for i in snap])
    return decorated if guest is None else [guest_safe(i, guest) for i in decorated]


@app.get("/api/sessions", dependencies=[Depends(require_auth)], response_model=list[SessionInfo])
async def list_sessions(request: Request):
    return await _session_list(guest_of(request), guest_users.current.get())
```

Em `backend/app/internal_api.py`, acrescentar imports `import json` e `from app.models import SessionInfo` (o import existente de `session_key` pode ficar na mesma linha) e as rotas depois de `router`:

```python
@router.get("/sessions", response_model=list[SessionInfo])
async def owner_sessions() -> list[SessionInfo]:
    from app import api

    return await api._session_list(guest=None, viewer=None)


@router.get("/list-events")
async def owner_list_events():
    from app.sse import list_events

    return EventSourceResponse(list_events(internal_nav=True), send_timeout=30)


@router.get("/list-nav")
async def owner_list_nav() -> dict:
    from app.sse import nav_snapshot

    return json.loads(nav_snapshot())
```

Em `backend/app/sse.py`, acrescentar depois de `nav_vivos`:

```python
def nav_snapshot() -> str:
    pending = {name: dict(marker) for name, marker in nav_vivos().items()}
    return json.dumps({"observed_at": time.monotonic_ns(), "pending": pending},
                      ensure_ascii=False, sort_keys=True)
```

Adicionar parâmetro no fim da assinatura existente:

```python
async def list_events(ping_secs: float = 8.0, only=None, viewer=None, token=None,
                      internal_nav: bool = False):
```

Substituir apenas `nav_pump` dentro dessa função; reader/ping/refcount/filtros continuam iguais:

```python
    async def nav_pump():
        vistos: dict[str, float] = {}
        try:
            while True:
                if internal_nav:
                    queue.put_nowait(("nav_snapshot", nav_snapshot()))
                else:
                    for nome, marc in nav_novos(vistos):
                        queue.put_nowait(("nav", json.dumps({"name": nome, "url": marc["url"]})))
                await asyncio.sleep(1.0)
        except asyncio.CancelledError:
            raise
        except Exception:
            _log.exception("sse: nav_pump da lista morreu")
```

O ramo interno emite `pending: {}` inicial para remover pendência cacheada que já foi confirmada. O ramo público preserva conteúdo/periodicidade, com primeiro exame imediato (até 1 s antes da implementação anterior); não altera confirmação, prazo ou arquivo.

Atualizar `RUST_SERVER_PROTOCOL = 2` em `backend/app/rust_server.py` e `INTERNAL_PROTOCOL: u32 = 2` em `crates/hangar-server/src/lib.rs` no mesmo commit; acrescentar `pub mod list;` em `lib.rs`.

- [ ] **Step 4: Escrever os testes Rust abaixo antes de implementar o módulo**

Criar primeiro o bloco `#[cfg(test)]` apresentado no final do módulo do Step 6 em `crates/hangar-server/src/list.rs`; declarar `pub mod list;` para que o comando focado descubra os testes. Os símbolos do módulo ainda ausentes devem fazer a compilação falhar. Não rodar nenhuma verificação sem autorização.

- [ ] **Step 5: Rodar os testes Rust novos (quando autorizado)**

```bash
(cd crates && cargo test -p hangar-server list::tests)
```

- [ ] **Step 6: Implementar o módulo completo e conectar as rotas**

Em `routes.rs`, adicionar `pub list: crate::list::Shared` a `AppState`; na construção final usar:

```rust
        AppState { auth: Auth::new(&cfg.auth_token), http, side, cfg, list: Default::default() }
```

Registrar antes das rotas de chat no `router` existente:

```rust
        .route("/api/sessions", get(crate::list::sessions).fallback(pass_any))
        .route("/api/sessions/events", get(crate::list::events).fallback(pass_any))
```

`gate`, `pass` e `cors` já são `pub(crate)`; não precisam ser duplicados nem expostos além do crate. Outras formas de autenticação e métodos HTTP continuam no proxy. Sem novo header de peer.

Código completo de `crates/hangar-server/src/list.rs`:

```rust
//! Leitura fresca e distribuição da lista; descoberta e decoração continuam no Python.
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use eventsource_stream::{EventStreamError, Eventsource};
use futures_util::StreamExt;
use http_body_util::{BodyDataStream, BodyExt};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};

use crate::proxy::HttpClient;
use crate::routes::{AppState, cors, gate, pass, maybe_gzip};
use crate::side::SideCtx;
use crate::tail::{comment_frame, ping_frame, sse_frame};

const CONNECT: Duration = Duration::from_secs(10);
const SOURCE_IDLE: Duration = Duration::from_secs(30);
const PING: Duration = Duration::from_secs(8);
const COMMENT: Duration = Duration::from_secs(15);
const SEND: Duration = Duration::from_secs(30);
const CHANNEL: usize = 64;

struct Fresh {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl Fresh {
    fn into_response(self, request_headers: &HeaderMap) -> Response {
        let mut headers = self.headers;
        headers.remove(header::CONTENT_LENGTH);
        let body = maybe_gzip(request_headers, &mut headers, self.body.to_vec());
        let mut response = Response::new(Body::from(body));
        *response.status_mut() = self.status;
        *response.headers_mut() = headers;
        response
    }
}

async fn fresh(st: &AppState) -> Option<Fresh> {
    let request = axum::http::Request::get(format!("http://{}/internal/sessions", st.cfg.upstream))
        .header("x-hangar-internal", &st.cfg.internal_secret)
        .body(Body::empty()).ok()?;
    let response = match tokio::time::timeout(CONNECT, st.http.request(request)).await {
        Ok(Ok(response)) => response,
        _ => {
            tracing::warn!("lista: GET interno sem resposta; repassa ao Python");
            return None;
        }
    };
    if !response.status().is_success() {
        tracing::warn!(status = response.status().as_u16(), "lista: GET interno recusado; repassa ao Python");
        return None;
    }
    let (parts, body) = response.into_parts();
    let body = match tokio::time::timeout(CONNECT, body.collect()).await {
        Ok(Ok(body)) => body.to_bytes(),
        _ => {
            tracing::warn!("lista: corpo do GET interno incompleto; repassa ao Python");
            return None;
        }
    };
    match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(serde_json::Value::Array(_)) => {}
        _ => {
            tracing::warn!("lista: GET interno inválido; repassa ao Python");
            return None;
        }
    }
    let mut headers = parts.headers;
    for name in ["connection", "keep-alive", "proxy-connection", "transfer-encoding", "te", "trailer", "upgrade"] {
        headers.remove(name);
    }
    Some(Fresh { status: parts.status, headers, body })
}

pub async fn sessions(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (forward, owner) = gate(&st, peer, &request);
    if !owner || request.method() != Method::GET {
        return pass(&st, request, &forward).await;
    }
    let Some(value) = fresh(&st).await else {
        return pass(&st, request, &forward).await;
    };
    let mut response = value.into_response(request.headers());
    cors(request.headers(), response.headers_mut());
    response
}

pub async fn events(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (forward, owner) = gate(&st, peer, &request);
    if !owner || request.method() != Method::GET {
        return pass(&st, request, &forward).await;
    }
    // Quem chega agora recebe o recorte atual do dono, nunca uma autorização cacheada.
    let Some(initial) = fresh(&st).await else {
        return pass(&st, request, &forward).await;
    };
    let Some(initial_nav) = fresh_nav(&st).await else {
        return pass(&st, request, &forward).await;
    };
    let initial = sse_frame("sessions", &String::from_utf8_lossy(&initial.body), None);
    let lease = st.list.acquire(&st.side);
    let (out, receiver) = mpsc::channel::<Bytes>(64);
    tokio::spawn(client_loop(lease, initial, initial_nav, out));
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|frame| (Ok::<Bytes, Infallible>(frame), receiver))
    });
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    cors(request.headers(), headers);
    response
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct PendingNav {
    url: String,
    ts: f64,
}

type NavSnapshot = BTreeMap<String, PendingNav>;

#[derive(Clone, Default, Deserialize)]
struct NavView {
    observed_at: u64,
    pending: NavSnapshot,
}

async fn fresh_nav(st: &AppState) -> Option<NavView> {
    let request = axum::http::Request::get(format!("http://{}/internal/list-nav", st.cfg.upstream))
        .header("x-hangar-internal", &st.cfg.internal_secret)
        .body(Body::empty()).ok()?;
    let result = tokio::time::timeout(CONNECT, async {
        let response = st.http.request(request).await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body = response.into_body().collect().await.ok()?.to_bytes();
        serde_json::from_slice(&body).ok()
    }).await;
    match result {
        Ok(Some(nav)) => Some(nav),
        _ => {
            tracing::warn!("lista: navegação interna indisponível; repassa ao Python");
            None
        }
    }
}

#[derive(Clone, Default)]
struct Snapshot {
    sessions: Option<Bytes>,
    sessions_revision: u64,
    shortcuts: Option<Bytes>,
    nav: NavView,
    errored: bool,
}

struct Hub {
    source: SocketAddr,
    secret: String,
    http: HttpClient,
    cache: Mutex<Snapshot>,
    tx: broadcast::Sender<Snapshot>,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Hub {
    fn new(ctx: &SideCtx) -> Arc<Self> {
        let (tx, _) = broadcast::channel(CHANNEL);
        Arc::new(Self {
            source: ctx.upstream,
            secret: ctx.secret.clone(),
            http: ctx.http.clone(),
            cache: Mutex::default(),
            tx,
            task: Mutex::new(None),
        })
    }

    fn start(self: &Arc<Self>) {
        *self.task.lock().unwrap() = Some(tokio::spawn(run_source(self.clone())).abort_handle());
    }

    fn stop(&self) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
    }

    fn attach(&self) -> (Snapshot, broadcast::Receiver<Snapshot>) {
        let cache = self.cache.lock().unwrap();
        let receiver = self.tx.subscribe();
        (cache.clone(), receiver)
    }

    fn record(&self, event: &str, data: &str) -> Result<(), ()> {
        let mut cache = self.cache.lock().unwrap();
        let changed = match event {
            "sessions" => {
                if !matches!(serde_json::from_str::<serde_json::Value>(data), Ok(serde_json::Value::Array(_))) {
                    return Err(());
                }
                let frame = sse_frame(event, data, None);
                let changed = cache.sessions.as_ref() != Some(&frame) || cache.errored;
                if changed {
                    cache.sessions = Some(frame);
                    cache.sessions_revision += 1;
                    cache.errored = false;
                }
                changed
            }
            "shortcut_terminals" => {
                if !matches!(serde_json::from_str::<serde_json::Value>(data), Ok(serde_json::Value::Array(_))) {
                    return Err(());
                }
                let frame = sse_frame(event, data, None);
                let changed = cache.shortcuts.as_ref() != Some(&frame);
                if changed {
                    cache.shortcuts = Some(frame);
                }
                changed
            }
            "nav_snapshot" => {
                cache.nav = serde_json::from_str::<NavView>(data).map_err(|_| ())?;
                true
            }
            "list_error" => {
                let changed = !cache.errored;
                cache.errored = true;
                changed
            }
            "ping" => false,
            _ => return Err(()),
        };
        if changed {
            // Assinar e copiar usam a mesma trava: não há intervalo entre o retrato e o canal.
            let _ = self.tx.send(cache.clone());
        }
        Ok(())
    }

    fn failed(&self) -> bool {
        let mut cache = self.cache.lock().unwrap();
        let first = !cache.errored;
        let changed = first || !cache.nav.pending.is_empty();
        cache.errored = true;
        cache.nav = NavView::default();
        if changed {
            let _ = self.tx.send(cache.clone());
        }
        first
    }
}

#[derive(Clone, Default)]
pub struct Shared(Arc<Mutex<Option<(Arc<Hub>, usize)>>>);

struct Lease {
    shared: Shared,
    hub: Arc<Hub>,
}

impl Shared {
    fn acquire(&self, ctx: &SideCtx) -> Lease {
        let mut slot = self.0.lock().unwrap();
        let hub = if let Some((hub, refs)) = slot.as_mut() {
            *refs += 1;
            hub.clone()
        } else {
            let hub = Hub::new(ctx);
            hub.start();
            *slot = Some((hub.clone(), 1));
            hub
        };
        Lease { shared: self.clone(), hub }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut slot = self.shared.0.lock().unwrap();
        if let Some((hub, refs)) = slot.as_mut() {
            if !Arc::ptr_eq(hub, &self.hub) {
                return;
            }
            *refs -= 1;
            if *refs == 0 {
                hub.stop();
                *slot = None;
            }
        }
    }
}

#[derive(Debug)]
enum SourceEnd {
    Request,
    Connect,
    Status(u16),
    ContentType,
    Idle,
    Utf8,
    Parser,
    Transport,
    Closed,
    Payload,
}

async fn run_source(hub: Arc<Hub>) {
    let mut delay = 1_u64;
    loop {
        let (reason, healthy) = source_once(&hub).await;
        if hub.failed() {
            // Só código e metadados, nunca o erro do parser nem o payload.
            match reason {
                SourceEnd::Status(status) => tracing::warn!(status, "lista: origem interna recusada; mantém a lista anterior"),
                _ => tracing::warn!(reason = ?reason, "lista: origem interna falhou; mantém a lista anterior"),
            }
        }
        if healthy {
            delay = 1;
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}

async fn source_once(hub: &Hub) -> (SourceEnd, bool) {
    let request = match axum::http::Request::get(format!("http://{}/internal/list-events", hub.source))
        .header("x-hangar-internal", &hub.secret)
        .body(Body::empty())
    {
        Ok(request) => request,
        Err(_) => return (SourceEnd::Request, false),
    };
    let response = match tokio::time::timeout(CONNECT, hub.http.request(request)).await {
        Ok(Ok(response)) => response,
        _ => return (SourceEnd::Connect, false),
    };
    if !response.status().is_success() {
        return (SourceEnd::Status(response.status().as_u16()), false);
    }
    if !response.headers().get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
    {
        return (SourceEnd::ContentType, false);
    }
    let events = BodyDataStream::new(response.into_body()).eventsource();
    let mut events = std::pin::pin!(events);
    let mut healthy = false;
    loop {
        let event = match tokio::time::timeout(SOURCE_IDLE, events.next()).await {
            Ok(Some(Ok(event))) => event,
            Ok(Some(Err(error))) => {
                let reason = match error {
                    EventStreamError::Utf8(_) => SourceEnd::Utf8,
                    EventStreamError::Parser(_) => SourceEnd::Parser,
                    EventStreamError::Transport(_) => SourceEnd::Transport,
                };
                return (reason, healthy);
            }
            Ok(None) => return (SourceEnd::Closed, healthy),
            Err(_) => return (SourceEnd::Idle, healthy),
        };
        if hub.record(&event.event, &event.data).is_err() {
            return (SourceEnd::Payload, healthy);
        }
        healthy = true;
    }
}

struct Client {
    sessions_revision: u64,
    nav_floor: u64,
    last_shortcuts: Option<Bytes>,
    seen_nav: BTreeMap<String, f64>,
    was_error: bool,
}

impl Client {
    fn new(sessions_revision: u64, nav_floor: u64) -> Self {
        Self { sessions_revision, nav_floor, last_shortcuts: None, seen_nav: BTreeMap::new(), was_error: false }
    }

    fn nav_frames(&mut self, pending: &NavSnapshot) -> Vec<Bytes> {
        let mut frames = Vec::new();
        for (name, nav) in pending {
            if self.seen_nav.get(name) != Some(&nav.ts) {
                self.seen_nav.insert(name.clone(), nav.ts);
                let data = serde_json::json!({"name": name, "url": nav.url}).to_string();
                frames.push(sse_frame("nav", &data, None));
            }
        }
        frames
    }

    fn frames(&mut self, snapshot: &Snapshot) -> Vec<Bytes> {
        let mut frames = Vec::new();
        if snapshot.errored {
            if !self.was_error {
                frames.push(sse_frame("list_error", "{}", None));
            }
            self.was_error = true;
        } else {
            if snapshot.sessions_revision != self.sessions_revision || self.was_error {
                if let Some(frame) = &snapshot.sessions {
                    frames.push(frame.clone());
                    self.sessions_revision = snapshot.sessions_revision;
                    self.was_error = false;
                }
            }
            if snapshot.shortcuts != self.last_shortcuts {
                if let Some(frame) = &snapshot.shortcuts {
                    frames.push(frame.clone());
                }
                self.last_shortcuts = snapshot.shortcuts.clone();
            }
        }
        if snapshot.nav.observed_at > self.nav_floor {
            frames.extend(self.nav_frames(&snapshot.nav.pending));
            self.nav_floor = snapshot.nav.observed_at;
        }
        frames
    }
}

async fn push(out: &mpsc::Sender<Bytes>, frame: Bytes) -> bool {
    matches!(tokio::time::timeout(SEND, out.send(frame)).await, Ok(Ok(())))
}

async fn client_loop(lease: Lease, initial: Bytes, initial_nav: NavView, out: mpsc::Sender<Bytes>) {
    let (snapshot, mut receiver) = lease.hub.attach();
    let mut client = Client::new(snapshot.sessions_revision, initial_nav.observed_at);
    if !push(&out, ping_frame()).await || !push(&out, initial).await {
        return;
    }
    let mut initial_frames = client.nav_frames(&initial_nav.pending);
    initial_frames.extend(client.frames(&snapshot));
    for frame in initial_frames {
        if !push(&out, frame).await {
            return;
        }
    }
    let now = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(now + PING, PING);
    let mut comment = tokio::time::interval_at(now + COMMENT, COMMENT);
    loop {
        tokio::select! {
            _ = out.closed() => return,
            _ = ping.tick() => if !push(&out, ping_frame()).await { return },
            _ = comment.tick() => if !push(&out, comment_frame()).await { return },
            next = receiver.recv() => match next {
                Ok(snapshot) => {
                    for frame in client.frames(&snapshot) {
                        if !push(&out, frame).await { return; }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    tracing::info!("lista: aparelho atrasado; fecha para reconectar");
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::TrustedHosts;
    use crate::config::Config;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn state(source: SocketAddr) -> AppState {
        AppState::new(Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            upstream: source,
            internal_secret: "fixture-secret".into(),
            auth_token: "owner-token".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        })
    }

    fn hub() -> Arc<Hub> {
        Hub::new(&state("127.0.0.1:1".parse().unwrap()).side)
    }

    #[test]
    fn subscribe_and_snapshot_have_no_gap() {
        let hub = hub();
        hub.record("sessions", r#"[{"name":"one"}]"#).unwrap();
        let (snapshot, mut receiver) = hub.attach();
        hub.record("sessions", r#"[{"name":"two"}]"#).unwrap();
        assert_eq!(snapshot.sessions_revision, 1);
        assert_eq!(receiver.try_recv().unwrap().sessions_revision, 2);
    }

    #[test]
    fn shortcuts_never_replay_cached_sessions_over_fresh_initial_list() {
        let hub = hub();
        hub.record("sessions", r#"[{"name":"old-private"}]"#).unwrap();
        let (snapshot, _) = hub.attach();
        let mut client = Client::new(snapshot.sessions_revision, snapshot.nav.observed_at);
        assert!(client.frames(&snapshot).is_empty());
        hub.record("shortcut_terminals", "[]").unwrap();
        let frames = client.frames(&hub.cache.lock().unwrap());
        assert_eq!(frames.len(), 1);
        assert!(String::from_utf8_lossy(&frames[0]).contains("event: shortcut_terminals"));
    }

    #[test]
    fn error_is_once_and_identical_sessions_recover() {
        let hub = hub();
        hub.record("sessions", "[]").unwrap();
        let mut client = Client::new(0, 0);
        assert_eq!(client.frames(&hub.cache.lock().unwrap()).len(), 1);
        assert!(hub.failed());
        let frames = client.frames(&hub.cache.lock().unwrap());
        assert_eq!(frames.len(), 1);
        assert!(String::from_utf8_lossy(&frames[0]).contains("list_error"));
        assert!(!hub.failed());
        assert!(client.frames(&hub.cache.lock().unwrap()).is_empty());
        hub.record("sessions", "[]").unwrap();
        assert_eq!(client.frames(&hub.cache.lock().unwrap()).len(), 1);
        assert!(!hub.cache.lock().unwrap().errored);
    }

    #[test]
    fn pending_nav_is_per_recipient_and_removed_nav_does_not_replay() {
        let hub = hub();
        hub.record("nav_snapshot", r#"{"observed_at":1,"pending":{"one":{"url":"http://fixture.invalid","ts":1.0}}}"#).unwrap();
        let mut first = Client::new(0, 0);
        let mut second = Client::new(0, 0);
        let snapshot = hub.cache.lock().unwrap().clone();
        assert_eq!(first.frames(&snapshot).len(), 1);
        assert!(first.frames(&snapshot).is_empty());
        assert_eq!(second.frames(&snapshot).len(), 1);
        hub.record("nav_snapshot", r#"{"observed_at":2,"pending":{}}"#).unwrap();
        assert!(Client::new(0, 0).frames(&hub.cache.lock().unwrap()).is_empty());
        hub.record("nav_snapshot", r#"{"observed_at":3,"pending":{"one":{"url":"http://fixture.invalid","ts":2.0}}}"#).unwrap();
        let frames = first.frames(&hub.cache.lock().unwrap());
        assert_eq!(frames.len(), 1);
        assert!(String::from_utf8_lossy(&frames[0]).contains("event: nav\r\n"));
        assert!(!String::from_utf8_lossy(&frames[0]).contains("nav_snapshot"));
    }

    #[test]
    fn buffered_nav_older_than_fresh_get_cannot_reopen_confirmed_request() {
        let hub = hub();
        let mut client = Client::new(0, 20);
        assert!(client.nav_frames(&NavSnapshot::new()).is_empty());
        hub.record("nav_snapshot", r#"{"observed_at":10,"pending":{"confirmed":{"url":"http://fixture.invalid","ts":1.0}}}"#).unwrap();
        assert!(client.frames(&hub.cache.lock().unwrap()).is_empty());
        hub.record("nav_snapshot", r#"{"observed_at":21,"pending":{}}"#).unwrap();
        assert!(client.frames(&hub.cache.lock().unwrap()).is_empty());
        hub.record("nav_snapshot", r#"{"observed_at":22,"pending":{"new":{"url":"http://fixture.invalid","ts":2.0}}}"#).unwrap();
        assert_eq!(client.frames(&hub.cache.lock().unwrap()).len(), 1);
        hub.record("nav_snapshot", r#"{"observed_at":23,"pending":{"new":{"url":"http://fixture.invalid","ts":2.0}}}"#).unwrap();
        assert!(client.frames(&hub.cache.lock().unwrap()).is_empty());
    }

    #[test]
    fn malformed_source_payload_preserves_previous_cache() {
        let hub = hub();
        hub.record("sessions", "[]").unwrap();
        let before = hub.cache.lock().unwrap().sessions.clone();
        assert!(hub.record("sessions", r#"{"error":"fixture"}"#).is_err());
        assert!(hub.record("nav_snapshot", r#"{"observed_at":1,"pending":{"one":{"url":"fixture","ts":"bad"}}}"#).is_err());
        assert_eq!(hub.cache.lock().unwrap().sessions, before);
    }

    #[test]
    fn failed_source_drops_pending_nav_but_keeps_sessions() {
        let hub = hub();
        hub.record("sessions", "[]").unwrap();
        hub.record("nav_snapshot", r#"{"observed_at":1,"pending":{"old":{"url":"http://fixture.invalid","ts":1.0}}}"#).unwrap();
        let previous = hub.cache.lock().unwrap().sessions.clone();
        assert!(hub.failed());
        let snapshot = hub.cache.lock().unwrap();
        assert!(snapshot.nav.pending.is_empty());
        assert_eq!(snapshot.sessions, previous);
    }

    #[test]
    fn fresh_attach_nav_is_not_replaced_by_stale_cache_or_shortcuts() {
        let hub = hub();
        hub.record("sessions", "[]").unwrap();
        hub.record("nav_snapshot", r#"{"observed_at":1,"pending":{"confirmed":{"url":"http://fixture.invalid","ts":1.0}}}"#).unwrap();
        let (snapshot, _) = hub.attach();
        let mut client = Client::new(snapshot.sessions_revision, snapshot.nav.observed_at);
        assert!(client.nav_frames(&NavSnapshot::new()).is_empty());
        assert!(client.frames(&snapshot).is_empty());
        hub.record("shortcut_terminals", "[]").unwrap();
        let frames = client.frames(&hub.cache.lock().unwrap());
        assert!(frames.iter().all(|frame| !String::from_utf8_lossy(frame).contains("event: nav\r\n")));
        hub.record("nav_snapshot", r#"{"observed_at":2,"pending":{}}"#).unwrap();
        assert!(client.frames(&hub.cache.lock().unwrap()).is_empty());
        hub.record("nav_snapshot", r#"{"observed_at":3,"pending":{"new":{"url":"http://fixture.invalid","ts":2.0}}}"#).unwrap();
        assert_eq!(client.frames(&hub.cache.lock().unwrap()).len(), 1);
        hub.record("nav_snapshot", r#"{"observed_at":3,"pending":{"new":{"url":"http://fixture.invalid","ts":2.0}}}"#).unwrap();
        assert!(client.frames(&hub.cache.lock().unwrap()).is_empty());
    }

    #[tokio::test]
    async fn leases_share_one_source_and_last_release_stops_it() {
        use axum::routing::get;
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let upstream = axum::Router::new().route("/internal/list-events", get(move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                let stream = futures_util::stream::unfold(false, |sent| async move {
                    if sent {
                        std::future::pending::<()>().await;
                    }
                    Some((Ok::<Bytes, Infallible>(sse_frame("sessions", "[]", None)), true))
                });
                let mut response = Response::new(Body::from_stream(stream));
                response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
                response
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let st = state(listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let first = st.list.acquire(&st.side);
        let second = st.list.acquire(&st.side);
        assert!(Arc::ptr_eq(&first.hub, &second.hub));
        let (_, mut receiver) = first.hub.attach();
        let snapshot = tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await.unwrap().unwrap();
        assert_eq!(snapshot.sessions_revision, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop(first);
        assert!(st.list.0.lock().unwrap().is_some());
        let remaining = second.hub.clone();
        drop(second);
        assert!(st.list.0.lock().unwrap().is_none());
        assert!(remaining.task.lock().unwrap().is_none());
        let third = st.list.acquire(&st.side);
        assert!(!Arc::ptr_eq(&third.hub, &remaining));
        drop(third);
        server.abort();
    }

    #[tokio::test]
    async fn get_uses_current_internal_response_not_sse_cache() {
        use axum::routing::get;
        let upstream = axum::Router::new()
            .route("/internal/sessions", get(|| async {
                axum::Json(serde_json::json!([{"name":"current","last_activity":42}]))
            }))
            .route("/internal/list-nav", get(|| async {
                axum::Json(serde_json::json!({"observed_at":64,"pending":{"current":{"url":"http://fixture.invalid","ts":9.0}}}))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let st = state(listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let value = fresh(&st).await.unwrap();
        let rows: serde_json::Value = serde_json::from_slice(&value.body).unwrap();
        assert_eq!(rows[0]["last_activity"], 42);
        assert_eq!(value.status, StatusCode::OK);
        let nav = fresh_nav(&st).await.unwrap();
        assert_eq!(nav.pending["current"].ts, 9.0);
        assert!(st.list.0.lock().unwrap().is_none(), "GET não cria produtor ou presença de app");
        server.abort();
    }
}
```

- [ ] **Step 7: Rodar os testes focados da Task (quando autorizado)**

```bash
(cd backend && uv run pytest tests/test_internal_list.py)
(cd crates && cargo test -p hangar-server list::tests)
```

Testes mínimos entregues pelo código acima: owner filtro, GET fresco/invalidação, mesma fonte Python, nav inicial com timestamps, assinatura/snapshot sem intervalo, erro/recuperação, nav por assinante, navegação confirmada durante falha e dado antigo em voo, payload inválido sem apagar dados, leases e reconexão, GET sem presença. Antes de declarar a Task pronta, ampliar apenas se os checks autorizados revelarem falha concreta ou caso material não coberto.

- [ ] **Step 8: Conferir no uso real — verificação manual**

Quando o dono autorizar a execução/serviço desta parte: com backend já usando o commit correspondente, abrir lista no nativo e celular, manter as duas abertas, fechar uma, conferir que a outra continua; mudar estado/provider/rename de sessão de teste pelo fluxo normal; abrir pedido de navegador fora da sessão ativa, abrir segundo aparelho e confirmar entrega a ele; conferir terminais de atalho e lista vazia versus erro. Não fechar/interromper sessões de trabalho reais. Conferir owner hidden com convidado de teste e anexação nova após política mudar. Com Rust desligado/incompatível, os mesmos clientes devem continuar pela reserva Python. Essa etapa é humana/ambiente autorizado, não é autorização para subir outro backend durante planejamento.

Limites a registrar: o GET por anexação renova o recorte antes de aceitar o SSE, mas não introduz epoch de política; visibilidade durante um stream já aberto conserva a cadência atual da versão Python. O Rust religa origem em 1,2,4,8,16,30 s e mantém ping público/última lista. Se `/internal/list-events` ficar recusando após GET bem-sucedido, o usuário vê `list_error` e tentativas continuam; o protocolo compartilhado evita mismatch instalado, e uma rota ausente é bug visível. Não apresentar esse caminho como fallback imediato do SSE depois que seus headers já saíram.

- [ ] **Step 9: Registrar o commit da Task (somente na execução autorizada)**

```bash
git status --short
git add backend/app/api.py backend/app/internal_api.py backend/app/sse.py backend/app/rust_server.py backend/tests/test_internal_list.py crates/hangar-server/src/lib.rs crates/hangar-server/src/routes.rs crates/hangar-server/src/list.rs
git commit -m "feat(server): serve session list through shared Rust stream"
git status --short
```

Não stagear o plano/spec ignorado, nem mudança de outra Task. Reportar hash/branch/estado real. Não fazer push nesta Task sem autorização do dono.

O relógio `observed_at` pertence ao mesmo processo Python e só delimita observações de navegação. Não é `id` SSE, não persiste em disco e não é comparado ao relógio Rust. Fonte observada antes do GET de navegação é ignorada pelo assinante novo; a próxima observação periódica garante nova consulta dos marcadores duráveis. `pending: {}` na reconexão apaga pendências confirmadas durante a falha.

### Task 6: conversa orq compartilhada sem duplicar o parser Python

**Files:**
- Modify: `backend/app/sse.py`
- Modify: `backend/app/internal_api.py`
- Create: `backend/tests/test_internal_orq.py`

**Interfaces:**
- `merged_events(..., bridge: bool=False)`; `side` mantém o significado atual, `bridge` habilita `info` sem suprimir transcript.
- `orq_replay_payload(name, jsonl, raw)` síncrono chamado por `asyncio.to_thread`.
- As duas rotas internas acima; sem mutação ou endpoint público novo.

- [ ] **Step 1: testes primeiro para ids, enriquecimento, corte e retomada maior que 200 linhas**

Adicionar a `backend/tests/test_internal_orq.py`:

```python
import json
from pathlib import Path

from app.adapters.orq.adapter import parse_obj
from app.internal_api import orq_replay_payload
from app.models import session_key


def timeline(tmp_path, count):
    path = tmp_path / "timeline-run-a.jsonl"
    rows = [{"kind": "notice", "text": f"T{i}: avanço", "ts": "2026-10-02T12:00:00+00:00"}
            for i in range(count)]
    chunks = [json.dumps(row, ensure_ascii=False).encode() + b"\n" for row in rows]
    path.write_bytes(b"".join(chunks))
    return path, rows, chunks


def test_replay_has_exact_parser_payload_cut_and_inclusive_resume(tmp_path):
    path, rows, chunks = timeline(tmp_path, 450)
    offset = len(chunks[0])
    replay = orq_replay_payload("g-orq", str(path), f"{session_key(str(path))}:{offset}")
    assert replay["cut"] == sum(map(len, chunks))
    assert len(replay["events"]) == 449
    wrapped = replay["events"][0]
    assert wrapped["offset"] == offset
    first = wrapped["event"]
    expected = parse_obj(rows[1], Path(path).parent)[0]
    expected.offset = offset
    assert first == expected.model_dump(mode="json")
    assert replay["info"]["provider"] == "orq"
    assert first["orq"] is not None


def test_replay_defaults_to_tail_and_excludes_partial_line(tmp_path):
    path, _, chunks = timeline(tmp_path, 450)
    complete = sum(map(len, chunks))
    with path.open("ab") as stream:
        stream.write(b'{"kind":"notice","text":"incompleta')
    replay = orq_replay_payload("g-orq", str(path), "outra-execucao:0")
    assert len(replay["events"]) == 200
    assert replay["cut"] == complete
    with path.open("ab") as stream:
        stream.write(b'"}\n')
    later = orq_replay_payload("g-orq", str(path),
                               f"{session_key(str(path))}:{complete}")
    assert len(later["events"]) == 1
    assert later["events"][0]["offset"] == complete
    assert "offset" not in later["events"][0]["event"]
```

Além destes testes finitos, adaptar a infraestrutura já existente de testes de `merged_events` para conferir `bridge=True`: primeiro evento `info`, notices com ids SSE, `bridge=False` inalterado, `side=True` ainda sem transcript, troca de provider/transcript manda `info` e nunca `reset` interno; contadores app entram uma vez e saem no cancelamento. Essas extensões exigem selecionar os helpers reais já existentes, não abrir serviço vivo.

- [ ] **Step 2: separar info interno da supressão da conversa**

Em `sse.py`, acrescentar `bridge: bool = False` após `side`; nas três expressões que escolhem `_info_event` usar `side or bridge`. Manter `if side: continue` do tail **inalterado**. Não converter `side` inteiro em `side or bridge`.

```python
async def merged_events(name: str, jsonl: str, provider: str = "claude",
                        start_offset: int | None = None, count_app: bool = True,
                        side: bool = False, bridge: bool = False):
```

Inicial:

```python
        if side or bridge:
            yield _info_event(name, current_provider, current_jsonl)
```

Ambos os rebinds:

```python
                yield (_info_event(name, current_provider, current_jsonl) if side or bridge
                       else {"event": "reset", "data": "{}"})
```

- [ ] **Step 3: acrescentar replay e rota full-stream internas**

Adicionar `import os` e `from pathlib import Path` no topo de `internal_api.py`; imports locais abaixo preservam a inicialização existente:

```python
def orq_start_offset(jsonl: str, raw: str | None) -> int | None:
    if not raw:
        return None
    key, _, offset = raw.rpartition(":")
    if key != session_key(jsonl):
        return None
    try:
        value = int(offset)
        size = os.stat(jsonl).st_size
    except (OSError, ValueError):
        return None
    return value if 0 <= value <= size else None


def orq_replay_payload(name: str, jsonl: str, raw: str | None) -> dict:
    from app.adapters.orq.adapter import line_parser
    from app.transcript import TranscriptTailer

    if not Path(jsonl).is_file():
        raise HTTPException(404)
    tail = TranscriptTailer(jsonl, parse_line=line_parser(Path(jsonl).parent))
    start = orq_start_offset(jsonl, raw)
    if start is None:
        start = tail._tail_offset(200)
    events, cut = tail._read_from(start)
    return {"info": info_payload(name, "orq", jsonl), "start": start, "cut": cut,
            "events": [{"offset": event.offset, "event": event.model_dump(mode="json")}
                       for event in events]}


async def require_orq_info(name: str, fresh: bool = False):
    from app import api
    if fresh:
        sessions = await asyncio.to_thread(api.registry.list)
        info = next((item for item in sessions if item.name == name), None)
    else:
        info = await api._cached_info(name)
    if info is None or not info.jsonl:
        raise HTTPException(404)
    if info.provider != "orq":
        raise HTTPException(409)
    return info


@router.get("/sessions/{name}/orq-replay")
async def orq_replay(name: str, last_event_id: str | None = None):
    info = await require_orq_info(name)
    payload = await asyncio.to_thread(orq_replay_payload, name, info.jsonl, last_event_id)
    if not Path(info.jsonl).is_file():
        raise HTTPException(404)
    after = await require_orq_info(name, fresh=True)
    if after.jsonl != info.jsonl:
        raise HTTPException(409)
    return payload


@router.get("/sessions/{name}/orq-events")
async def orq_events(name: str, request: Request, app: int = 0):
    info = await require_orq_info(name)
    raw = request.query_params.get("last_event_id") or request.headers.get("last-event-id")
    offset = await asyncio.to_thread(orq_start_offset, info.jsonl, raw)
    return EventSourceResponse(
        merged_events(name, info.jsonl, provider="orq", start_offset=offset if offset is not None else 0,
                      count_app=bool(app), bridge=True),
        send_timeout=30)
```

Falha de replay deve permanecer erro HTTP (não retornar `[]` fingindo sucesso). A verificação da identidade antes/depois da leitura será feita pelo Rust; sessão recriada troca key/caminho e invalida replay antigo.

- [ ] **Step 4: Rodar testes focados da ponte (quando autorizado)**

```bash
(cd backend && uv run pytest tests/test_internal_orq.py tests/test_internal_api.py tests/test_internal_side_events.py)
```

Os dois testes existentes de API interna e stream lateral foram conferidos no checkout.

#### Continuação da Task 6: HTTP/SSE compartilhado orq no Rust

**Files:**
- Create: `crates/hangar-server/src/orq_read.rs`
- Modify: `crates/hangar-server/src/routes.rs`
- Modify: `crates/hangar-server/src/side.rs` (visibilidade interna do retrato)
- Modify: `crates/hangar-server/src/lib.rs`

**Interfaces:**
- `orq_read::Hubs::acquire(name, &SideCtx) -> Lease`.
- `orq_read::client_loop(Lease, resume, mpsc::Sender<Bytes>)`.
- Não adicionar `Provider::Orq` ao parser Rust nem fazê-lo fingir que conhece orq.
- `SideCache`: `pub(crate)` e métodos `record`/`replay` `pub(crate)`. Nenhum notice do transcript passa em `record`: offset vem do SSE id validado, não do JSON público.

- [ ] **Step 5: teste de contrato primeiro**

Servidor interno fake deve ter rotas `info`, `orq-replay`, `orq-events`; contador de streams ativos. Dois clientes owner devem produzir um upstream, replay independente para cada cursor, id original no transcript e ausência de id no estado. Escrever deterministicamente barreiras `Notify`:

1. Assinatura do aparelho criada, replay parado antes de devolver o corte; mandar linha nova upstream durante o replay; ela aparece exatamente uma vez (replay ou live).
2. Derrubar upstream depois de id A; gravar 450 linhas; reabrir exige `last_event_id=A` e todas as linhas chegam, não só 200.
3. Aparelho novo com cursor B mais antigo que A recebe tudo desde B via replay; não herda cursor A do hub.
4. Mesmo nome recriado com key e caminho diferentes gera reset e fecha, sem notice antigo no replay da nova ligação.
5. Queda no broadcast fecha o aparelho; retomada recupera pelo arquivo, sem descartar gap.
6. Sair primeiro aparelho mantém upstream; sair último fecha upstream e contagem Python volta ao valor inicial.
7. Convidado ou qualquer pedido sem token do dono jamais entram no bridge, permanecem no proxy existente.
8. Inserir mais de 1024 notices: `SideCache` não cresce com eles, sua memória depende somente de estado/fila lateral.

#### Teste executável proposto: regressão de offset encerra hub e próxima entrada começa nova

Adicionar ao final de `orq_read.rs`, na execução autorizada da Task 6. Este teste sobe somente servidor fake efêmero dentro do próprio teste, não toca backend vivo.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use tokio::net::TcpListener;
    use crate::side::SideCtx;

    #[tokio::test]
    async fn regressed_notice_resets_and_next_attach_creates_a_new_hub() {
        let info = serde_json::json!({
            "provider": "orq", "jsonl": "/tmp/timeline-same.jsonl",
            "session_key": "timeline-same", "history": {}
        });
        let notice = serde_json::json!({
            "kind": "notice", "id": "orq:first", "text": "avanço", "orq": null
        });
        let mut frames = Vec::new();
        frames.extend_from_slice(&sse_frame("info", &info.to_string(), None));
        frames.extend_from_slice(&sse_frame("message", &notice.to_string(), Some("timeline-same:100")));
        // A retomada inclusiva pode repetir o mesmo offset sem reset.
        frames.extend_from_slice(&sse_frame("message", &notice.to_string(), Some("timeline-same:100")));
        frames.extend_from_slice(&sse_frame("message", &notice.to_string(), Some("timeline-same:0")));
        let app = Router::new().route("/internal/sessions/g/orq-events", get(move || {
            let frames = frames.clone();
            async move { ([("content-type", "text/event-stream")], frames) }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let ctx = SideCtx {
            upstream, secret: "test".into(), http: crate::proxy::client(),
            watchers: Default::default(), hubs: Default::default(), infos: Default::default(),
        };
        let hubs = Hubs::default();
        let first = hubs.acquire("g", &ctx);
        let mut rx = first.hub.tx.subscribe();
        let mut notices = 0;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match rx.recv().await.unwrap() {
                    Frame::Data { offset: Some(100), .. } => notices += 1,
                    Frame::Data { offset: Some(0), .. } => panic!("linha reescrita passou sem reset"),
                    Frame::Reset => break,
                    _ => {}
                }
            }
        }).await.unwrap();
        assert_eq!(notices, 2, "offset igual é replay inclusivo válido");
        assert!(first.hub.dead.load(Ordering::Acquire));
        let next = hubs.acquire("g", &ctx);
        assert!(!Arc::ptr_eq(&first.hub, &next.hub));
        assert_eq!(next.hub.ready.borrow().as_ref(), None, "novo hub não herda a ligação encerrada");
        drop(first);
        drop(next);
        server.abort();
    }
}
```

Adicionar ao teste Python de replay para conferir que o cursor inválido do arquivo antigo volta ao início da timeline pequena reescrita:

```python
def test_rewritten_timeline_replay_starts_from_zero(tmp_path):
    path, _, chunks = timeline(tmp_path, 450)
    old_offset = sum(map(len, chunks[:-1]))
    path.write_text(json.dumps({"kind": "notice", "text": "nova execução"}) + "\n")
    replay = orq_replay_payload("g-orq", str(path),
                               f"{session_key(str(path))}:{old_offset}")
    assert replay["start"] == 0
    assert replay["events"][0]["offset"] == 0
    assert replay["events"][0]["event"]["text"] == "nova execução"
```

O teste Rust comprova emissão de reset e substituição do hub; o Python comprova replay novo desde zero. Não foram executados nesta etapa de planejamento.

#### Segundo teste da Task 6 (no mesmo `mod tests`, antes da implementação)

Adicionar este teste junto do anterior no `mod tests` de `orq_read.rs`. Reutiliza imports do módulo e acrescenta tipos pelo caminho completo.

```rust
    #[tokio::test]
    async fn two_leases_share_upstream_replay_covers_the_gap_and_state_has_no_inherited_id() {
        use std::convert::Infallible;
        use axum::{body::Body, response::Response};
        use tokio::sync::Notify;

        let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sender = Arc::new(Mutex::new(None::<mpsc::Sender<Bytes>>));
        let replay_entered = Arc::new(Notify::new());
        let replay_release = Arc::new(Notify::new());
        let info = serde_json::json!({
            "provider": "orq", "jsonl": "/tmp/timeline-gap.jsonl",
            "session_key": "timeline-gap", "history": {}
        });
        let old = serde_json::json!({"kind": "notice", "id": "orq:old", "text": "antes"});
        let new = serde_json::json!({"kind": "notice", "id": "orq:new", "text": "durante"});
        let events = {
            let opened = opened.clone();
            let sender = sender.clone();
            let info = info.clone();
            let old = old.clone();
            get(move || {
                let opened = opened.clone();
                let sender = sender.clone();
                let info = info.clone();
                let old = old.clone();
                async move {
                    opened.fetch_add(1, Ordering::SeqCst);
                    let (tx, rx) = mpsc::channel(8);
                    tx.send(sse_frame("info", &info.to_string(), None)).await.unwrap();
                    tx.send(sse_frame("message", &old.to_string(), Some("timeline-gap:0"))).await.unwrap();
                    *sender.lock().unwrap() = Some(tx);
                    let body = futures_util::stream::unfold(rx, |mut rx| async move {
                        rx.recv().await.map(|bytes| (Ok::<Bytes, Infallible>(bytes), rx))
                    });
                    let mut response = Response::new(Body::from_stream(body));
                    response.headers_mut().insert("content-type", "text/event-stream".parse().unwrap());
                    response
                }
            })
        };
        let replay_route = {
            let entered = replay_entered.clone();
            let release = replay_release.clone();
            let info = info.clone();
            let old = old.clone();
            get(move || {
                let entered = entered.clone();
                let release = release.clone();
                let info = info.clone();
                let old = old.clone();
                async move {
                    entered.notify_one();
                    release.notified().await;
                    axum::Json(serde_json::json!({
                        "info": info, "start": 0, "cut": 100,
                        "events": [{"offset": 0, "event": old}]
                    }))
                }
            })
        };
        let app = Router::new()
            .route("/internal/sessions/g/orq-events", events)
            .route("/internal/sessions/g/orq-replay", replay_route);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let ctx = SideCtx {
            upstream, secret: "test".into(), http: crate::proxy::client(),
            watchers: Default::default(), hubs: Default::default(), infos: Default::default(),
        };
        let hubs = Hubs::default();
        let first = hubs.acquire("g", &ctx);
        let second = hubs.acquire("g", &ctx);
        assert!(Arc::ptr_eq(&first.hub, &second.hub));
        let (out, mut received) = mpsc::channel(64);
        let client = tokio::spawn(client_loop(first, None, out));
        tokio::time::timeout(Duration::from_secs(3), replay_entered.notified()).await.unwrap();
        assert_eq!(opened.load(Ordering::SeqCst), 1);
        let tx = sender.lock().unwrap().clone().unwrap();
        // A linha chega depois da assinatura e antes da resposta do replay.
        tx.send(sse_frame("message", &new.to_string(), Some("timeline-gap:100"))).await.unwrap();
        tx.send(sse_frame("state", "{\"state\":\"idle\"}", None)).await.unwrap();
        replay_release.notify_one();
        let frames = tokio::time::timeout(Duration::from_secs(3), async {
            let mut frames = Vec::new();
            loop {
                let frame = received.recv().await.unwrap();
                frames.push(String::from_utf8(frame.to_vec()).unwrap());
                if frames.iter().any(|frame| frame.contains("orq:new")) &&
                   frames.iter().any(|frame| frame.contains("event: state")) { return frames; }
            }
        }).await.unwrap();
        assert_eq!(frames.iter().filter(|frame| frame.contains("orq:old")).count(), 1);
        assert_eq!(frames.iter().filter(|frame| frame.contains("orq:new")).count(), 1);
        assert!(frames.iter().any(|frame| frame.contains("id: timeline-gap:100")));
        assert!(frames.iter().filter(|frame| frame.contains("event: state"))
                .all(|frame| !frame.contains("id:")));
        client.abort();
        let _ = client.await;
        assert!(!second.hub.worker.lock().unwrap().as_ref().unwrap().is_finished(),
                "sair primeiro aparelho mantém a conexão compartilhada");
        let reader = second.hub.worker.lock().unwrap().as_ref().unwrap().clone();
        drop(second);
        tokio::time::timeout(Duration::from_secs(3), async {
            while !reader.is_finished() { tokio::task::yield_now().await; }
        }).await.unwrap();
        assert!(hubs.0.lock().unwrap().is_empty());
        server.abort();
    }
```

Esse teste não foi executado. O cenário envolve duas leases no Rust e um aparelho consumidor; garante fonte única, janela replay/live, ausência de id herdado e fechamento só no último dono do hub. O outro aparelho precisa de teste de integração dos clientes quando a execução for autorizada; a fonte única já fica conferida pelo contador.

- [ ] **Step 6: módulo completo proposto**

O código abaixo é a referência da implementação, não código já aplicado ou compilado. Antes de executar, combinar com os imports/visibilidade das Tasks anteriores e adicionar os testes acima. Mantém o HttpClient já instalado, eventsource-stream e todos os limites do chat atual.

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use http_body_util::{BodyDataStream, BodyExt};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc, watch};

use crate::side::{SideCache, SideCtx};
use crate::tail::{comment_frame, ping_frame, reset_frame, sse_frame};
use crate::transcript::InternalInfo;

const CONNECT: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(30);
const SEND: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding { jsonl: String, key: String }
impl Binding {
    fn from_info(info: &InternalInfo) -> Option<Self> {
        if info.provider != "orq" { return None; }
        Some(Self {
            jsonl: info.jsonl.as_ref()?.to_string_lossy().into_owned(),
            key: info.session_key.clone(),
        })
    }
}

#[derive(Clone)]
enum Frame {
    Data { binding: Binding, offset: Option<u64>, bytes: Bytes },
    Reset,
    Close,
}

struct Hub {
    name: String,
    ctx: SideCtx,
    tx: broadcast::Sender<Frame>,
    ready: watch::Sender<Option<Binding>>,
    cache: Mutex<(Option<Binding>, SideCache)>,
    worker: Mutex<Option<tokio::task::AbortHandle>>,
    dead: AtomicBool,
}

#[derive(Clone, Default)]
pub struct Hubs(Arc<Mutex<HashMap<String, (Arc<Hub>, usize)>>>);
pub struct Lease { hubs: Hubs, hub: Arc<Hub> }

impl Hubs {
    pub fn acquire(&self, name: &str, ctx: &SideCtx) -> Lease {
        let mut map = self.0.lock().unwrap();
        if let Some((hub, refs)) = map.get_mut(name) {
            if !hub.dead.load(Ordering::Acquire) {
                *refs += 1;
                return Lease { hubs: self.clone(), hub: hub.clone() };
            }
        }
        let (tx, _) = broadcast::channel(1024);
        let (ready, _) = watch::channel(None);
        let hub = Arc::new(Hub {
            name: name.into(), ctx: ctx.clone(), tx, ready,
            cache: Mutex::new((None, SideCache::default())), worker: Mutex::default(), dead: AtomicBool::new(false),
        });
        let reader = hub.clone();
        *hub.worker.lock().unwrap() = Some(tokio::spawn(async move { upstream(reader).await }).abort_handle());
        map.insert(name.into(), (hub.clone(), 1));
        Lease { hubs: self.clone(), hub }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut map = self.hubs.0.lock().unwrap();
        if let Some((hub, refs)) = map.get_mut(&self.hub.name) {
            if Arc::ptr_eq(hub, &self.hub) {
                *refs -= 1;
                if *refs != 0 { return; }
                map.remove(&self.hub.name);
            }
        }
        drop(map);
        if let Some(task) = self.hub.worker.lock().unwrap().take() { task.abort(); }
    }
}

fn url(hub: &Hub, route: &str, resume: Option<&str>) -> String {
    let base = format!("http://{}/internal/sessions/{}/{route}", hub.ctx.upstream,
                       utf8_percent_encode(&hub.name, NON_ALPHANUMERIC));
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("app", "1");
    if let Some(id) = resume { query.append_pair("last_event_id", id); }
    format!("{base}?{}", query.finish())
}

async fn upstream(hub: Arc<Hub>) {
    let mut cursor: Option<String> = None;
    let mut binding: Option<Binding> = None;
    let mut attempt = 0_u32;
    loop {
        let request = axum::http::Request::get(url(&hub, "orq-events", cursor.as_deref()))
            .header("x-hangar-internal", &hub.ctx.secret).body(Body::empty());
        let response = match request {
            Ok(request) => tokio::time::timeout(CONNECT, hub.ctx.http.request(request)).await,
            Err(_) => break,
        };
        if let Ok(Ok(response)) = response {
            if response.status().as_u16() == 404 || response.status().as_u16() == 409 { break; }
            if response.status().is_success() {
                let stream = BodyDataStream::new(response.into_body()).eventsource();
                let mut stream = std::pin::pin!(stream);
                loop {
                    let event = match tokio::time::timeout(IDLE, stream.next()).await {
                        Ok(Some(Ok(event))) => event,
                        _ => break,
                    };
                    if event.event == "info" {
                        let Some(next) = serde_json::from_str::<InternalInfo>(&event.data).ok()
                            .and_then(|info| Binding::from_info(&info)) else {
                                hub.dead.store(true, Ordering::Release);
                                let _ = hub.tx.send(Frame::Close);
                                return;
                            };
                        if binding.as_ref().is_some_and(|old| old != &next) {
                            cursor = None;
                            *hub.cache.lock().unwrap() = (Some(next.clone()), SideCache::default());
                            let _ = hub.tx.send(Frame::Reset);
                        }
                        {
                            let mut snapshot = hub.cache.lock().unwrap();
                            snapshot.0 = Some(next.clone());
                        }
                        binding = Some(next.clone());
                        hub.ready.send_replace(Some(next));
                        attempt = 0;
                        continue;
                    }
                    if event.event == "ping" { continue; }
                    let Some(bound) = binding.as_ref() else { break };
                    // EventSource pode herdar o id anterior em evento sem id; só a mensagem
                    // notice do orq com id validado ganha posição no SSE público.
                    let is_notice = event.event == "message" &&
                        serde_json::from_str::<serde_json::Value>(&event.data).ok()
                            .is_some_and(|value| value.get("kind").and_then(|kind| kind.as_str()) == Some("notice"));
                    let offset = if is_notice {
                        event.id.rsplit_once(':').and_then(|(key, offset)| {
                            (key == bound.key).then(|| offset.parse::<u64>().ok()).flatten()
                        })
                    } else { None };
                    if is_notice && offset.is_none() { break; }
                    let id = offset.map(|_| event.id.clone());
                    let bytes = sse_frame(&event.event, &event.data, id.as_deref());
                    if let Some(offset) = offset {
                        let previous = cursor.as_deref().and_then(|cursor| {
                            cursor.rsplit_once(':').and_then(|(key, position)| {
                                (key == bound.key).then(|| position.parse::<u64>().ok()).flatten()
                            })
                        });
                        if previous.is_some_and(|position| offset < position) {
                            hub.dead.store(true, Ordering::Release);
                            let _ = hub.tx.send(Frame::Reset);
                            return;
                        }
                        cursor = id;
                    } else {
                        let mut snapshot = hub.cache.lock().unwrap();
                        if snapshot.0.as_ref() == Some(bound) {
                            snapshot.1.record(&event.event, &event.data, &bytes, false);
                        }
                    }
                    let _ = hub.tx.send(Frame::Data { binding: bound.clone(), offset, bytes });
                }
            }
        }
        tracing::warn!(session = %hub.name, "ponte orq interrompida; retoma pelo último id");
        let delay = (1_u64 << attempt.min(5)).min(30);
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
    hub.dead.store(true, Ordering::Release);
    let _ = hub.tx.send(Frame::Close);
}

#[derive(Deserialize)]
struct Replay { info: InternalInfo, start: u64, cut: u64, events: Vec<ReplayEvent> }
#[derive(Deserialize)]
struct ReplayEvent { offset: u64, event: serde_json::Value }

async fn replay(hub: &Hub, resume: Option<&str>) -> Option<Replay> {
    let request = axum::http::Request::get(url(hub, "orq-replay", resume))
        .header("x-hangar-internal", &hub.ctx.secret).body(Body::empty()).ok()?;
    let response = tokio::time::timeout(CONNECT, hub.ctx.http.request(request)).await.ok()?.ok()?;
    if !response.status().is_success() { return None; }
    let bytes = tokio::time::timeout(CONNECT, response.into_body().collect()).await.ok()?.ok()?.to_bytes();
    serde_json::from_slice(&bytes).ok()
}

async fn push(out: &mpsc::Sender<Bytes>, bytes: Bytes) -> bool {
    matches!(tokio::time::timeout(SEND, out.send(bytes)).await, Ok(Ok(())))
}

pub async fn client_loop(lease: Lease, resume: Option<String>, out: mpsc::Sender<Bytes>) {
    let hub = &lease.hub;
    if !push(&out, ping_frame()).await { return; }
    let mut ready = hub.ready.subscribe();
    let available = tokio::time::timeout(CONNECT, async {
        loop {
            if ready.borrow().is_some() { return true; }
            if ready.changed().await.is_err() { return false; }
        }
    }).await.unwrap_or(false);
    // Assinar antes da leitura finita fecha a fresta entre cauda e ao vivo.
    let mut rx = hub.tx.subscribe();
    let payload = if available { replay(hub, resume.as_deref()).await } else { None };
    let Some(payload) = payload else {
        tracing::warn!(session = %hub.name, "replay orq falhou; reset e reconexão");
        let _ = push(&out, reset_frame()).await;
        return;
    };
    let Some(binding) = Binding::from_info(&payload.info) else {
        let _ = push(&out, reset_frame()).await;
        return;
    };
    if hub.dead.load(Ordering::Acquire) || ready.borrow().as_ref() != Some(&binding) {
        let _ = push(&out, reset_frame()).await;
        return;
    }
    if payload.start > payload.cut {
        let _ = push(&out, reset_frame()).await;
        return;
    }
    for item in payload.events {
        let id = format!("{}:{}", binding.key, item.offset);
        if !push(&out, sse_frame("message", &item.event.to_string(), Some(&id))).await { return; }
    }
    if hub.dead.load(Ordering::Acquire) || ready.borrow().as_ref() != Some(&binding) {
        let _ = push(&out, reset_frame()).await;
        return;
    }
    let (cached_binding, cached) = {
        let snapshot = hub.cache.lock().unwrap();
        (snapshot.0.clone(), snapshot.1.replay())
    };
    if cached_binding.as_ref() != Some(&binding) {
        let _ = push(&out, reset_frame()).await;
        return;
    }
    for bytes in cached { if !push(&out, bytes).await { return; } }
    let mut floor = payload.cut;
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + Duration::from_secs(10), Duration::from_secs(10));
    let mut comment = tokio::time::interval_at(start + Duration::from_secs(15), Duration::from_secs(15));
    loop {
        tokio::select! {
            _ = out.closed() => return,
            _ = ping.tick() => if !push(&out, ping_frame()).await { return; },
            _ = comment.tick() => if !push(&out, comment_frame()).await { return; },
            frame = rx.recv() => match frame {
                Ok(Frame::Data { binding: source, offset, bytes }) if source == binding => {
                    if let Some(offset) = offset {
                        if offset < floor { continue; }
                        floor = offset.saturating_add(1);
                    }
                    if !push(&out, bytes).await { return; }
                }
                Ok(Frame::Data { .. }) => {}
                Ok(Frame::Reset | Frame::Close) => {
                    let _ = push(&out, reset_frame()).await;
                    return;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = push(&out, reset_frame()).await;
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}
```

- [ ] **Step 7: integrar a rota orq sem duplicar auth/HTTP**

`lib.rs`: `pub mod orq_read;`.

`side.rs`: `pub(crate) struct SideCache` e `pub(crate) fn record/replay`; nenhum campo vira público.

`routes.rs`: `AppState` ganha `pub orq: crate::orq_read::Hubs`; construtor inicializa `Default::default()`.

No `events`, depois de obter `info` e antes de exigir `Binding::from_info`, extrair `resume` com a mesma precedência query/header já existente. Se `info.provider == "orq" && info.jsonl.is_some()`, criar lease `st.orq.acquire(&name, &st.side)`, mpsc 64, e chamar `orq_read::client_loop`; montar **a mesma** resposta SSE (Content-Type, Cache-Control, Connection, X-Accel-Buffering, CORS) já existente. Extrair `sse_response(rx, headers)` do final atual para reutilizar a montagem, sem alterar autenticação/gate ou rotas HEAD/OPTIONS. `/history` continua passando pelo branch existente de `history_request(None)` que não conhece orq e chama `pass`.


Integração literal da resposta SSE:

Acrescentar em `routes.rs`:

```rust
fn sse_response(rx: mpsc::Receiver<Bytes>, request_headers: &HeaderMap) -> Response {
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|bytes| (Ok::<Bytes, Infallible>(bytes), rx))
    });
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    cors(request_headers, headers);
    response
}
```

No `events`, substituir do `let info = st.info(&name).await` até o retorno final pelo trecho abaixo. Gate, extração do path e owner antes desse trecho ficam iguais:

```rust
    let info = st.info(&name).await;
    let resume = auth::query_param(req.uri().query(), "last_event_id")
        .filter(|value| !value.is_empty())
        .or_else(|| req.headers().get("last-event-id").and_then(|value| value.to_str().ok()).map(str::to_owned));
    if info.as_ref().is_some_and(|value| value.provider == "orq" && value.jsonl.is_some()) {
        tracing::debug!(session = %name, req = %diag_req(&req), retomada = resume.is_some(), "events orq: abriu");
        let lease = st.orq.acquire(&name, &st.side);
        let (tx, rx) = mpsc::channel::<Bytes>(64);
        tokio::spawn(crate::orq_read::client_loop(lease, resume, tx));
        return sse_response(rx, req.headers());
    }
    let Some(binding) = info.as_ref().and_then(Binding::from_info) else {
        let response = pass(&st, req, &fwd).await;
        warn_if_internal_refused(&name, info.is_none(), response.status());
        return response;
    };
    tracing::debug!(session = %name, req = %diag_req(&req), retomada = resume.is_some(), "events: abriu");
    let lease = st.side.hubs.acquire(&name, binding, &st.side);
    let (tx, rx) = mpsc::channel::<Bytes>(64);
    tokio::spawn(client_loop(lease, resume, tx));
    sse_response(rx, req.headers())
```

Campo literal no `AppState`:

```rust
    pub orq: crate::orq_read::Hubs,
```

Retorno literal do construtor:

```rust
        AppState { auth: Auth::new(&cfg.auth_token), http, side, cfg, list: Default::default(), orq: Default::default() }
```


- [ ] **Step 8: Rodar contratos Rust e Python (quando autorizado)**

```bash
cargo test --manifest-path crates/Cargo.toml -p hangar-server --lib orq_read::tests
(cd backend && uv run pytest tests/test_internal_orq.py)
```

- [ ] **Step 9: verificação manual de orq com serviço autorizado pelo dono**

Dois aparelhos abertos na mesma orq; notices enriquecidos iguais ao Python; atualização da linha do tempo/painel mantida; fechar um aparelho mantém outro; desconectar rede e acumular mais de 200 linhas recupera todas desde o cursor; recriar execução não mostra notices antigos. Não criar/fechar sessão de trabalho real para fabricar a prova sem autorização.

#### Cuidados definidos para a revisão

1. Task 6 altera contrato 2→3 no mesmo commit Python+Rust; Task 5 altera 1→2.
2. `offset` no SSE e id do `ChatEvent` são identidades diferentes. Nunca usar `orq:<hash>` como cursor de arquivo.
3. O replay não retém notices no Rust, mas ainda parseia uma cauda ou gap no Python por aparelho. Ganho fica na fonte ao vivo compartilhada, não na história.
4. Snapshot de replay não é cache permanente nem estado derivado separado. Linha que chega durante replay aparece pelo corte mais broadcast; se o canal estoura, reconexão pelo arquivo recupera, nunca aceitar perda como “limite”.
5. Rebind pode ocorrer durante envio longo do replay. O código confere binding antes e depois, fecha em reset e captura retrato lateral+binding sob a mesma trava para impedir mistura de estado.
6. Logs do parser SSE não podem formatar `EventStreamError` com texto. O snippet loga somente o estado da ponte, sem dado de conversa.
7. `_read_from` existente trata arquivo ausente como lista vazia; a rota de replay confirma `Path(jsonl).is_file()` antes e precisa confirmar novamente após ler para cobrir remoção durante a chamada. Falhas reais de leitura propagam erro/diário; não mascarar como sucesso.
8. O replay faz resolução fresca ao terminar, evitando validar uma execução antiga contra o TTL de `_cached_info`. Após a Task5, reutilizar a resolução fresca dela quando houver esse contrato; não substituir por dados vencidos. Custo dessa leitura pertence ao primeiro attach/replay, não a todo tick/aparelho ativo.
9. Escritor timeline confirmado em `skills/orquestrar/scripts/orq.py:176`: append. Se chegar notice cujo offset regressa na mesma key, o hub é marcado encerrado e manda reset; o próximo attach cria hub novo, impedindo que floor esconda a timeline reescrita. Repetição inclusiva de offset igual ao último continua válida. Timeout finito de 10 s acompanha o timeout da resolução interna atual; falha manda reset e reconexão, nunca sucesso vazio.
10. Primeira ida interna no bridge começa no byte zero. Em timelines grandes isso pode reler muita coisa ao entrar primeiro aparelho; medir antes de trocar por 200. Otimização correta exigiria handshake com cursor de corte inicial, não perder mais de 200 linhas por conveniência.

- [ ] **Step 10: Subir a versão e commitar a ponte completa**

A Task 5 da lista já terá subido protocolo 1→2; esta Task muda 2→3. Alterar `RUST_SERVER_PROTOCOL = 3` em `backend/app/rust_server.py` e `pub const INTERNAL_PROTOCOL: u32 = 3` em `crates/hangar-server/src/lib.rs` no **mesmo commit**. 

Quando a execução e os testes pedidos estiverem autorizados e concluídos:

```bash
git add backend/app/internal_api.py backend/app/sse.py backend/app/rust_server.py backend/tests/test_internal_orq.py backend/tests/test_internal_side_events.py crates/hangar-server/src/orq_read.rs crates/hangar-server/src/routes.rs crates/hangar-server/src/side.rs crates/hangar-server/src/lib.rs
git commit -m "feat(server): share orq conversation streams through Rust"
```

Não executar agora; nenhum produto foi alterado por estas notas.



### Task 7: Conferência final e uso real — verificação manual

**Files:**
- Modify: `docs/decisoes/plataforma.md` (entrada hangar-server)
- Modify: `docs/decisoes/harnesses.md` (fronteira orq/parser único e formatos portados)
- Modify: `CLAUDE.md` (regra resumida da parte2)

**Interfaces:**
- Consumes: rotas e protocol finais das Tasks1–6, mesmo processo de reserva da parte1.
- Produces: evidência de contrato/uso real e medições antes/depois; não altera clientes/instalador.

- [ ] **Step 1: Conferir dependência da parte 1 e contrato final**

```bash
git fetch origin
git log --oneline HEAD..origin/hangar-server-parte1
git diff --stat HEAD...origin/hangar-server-parte1
rg -n 'RUST_SERVER_PROTOCOL|INTERNAL_PROTOCOL' backend/app/rust_server.py crates/hangar-server/src/lib.rs
git status --short
```

Esperado: mudanças posteriores relevantes foram incorporadas ao plano antes de execução, com
aprovação de qualquer merge necessário; ambos protocolos finais iguais a3 nesta base. Não
trocar de branch, resetar ou absorver a outra sessão sem autorização. Erro no contrato bloqueia
entrega, não é desvio para esconder problema no repasse.

- [ ] **Step 2: Rodar checks focados finais (quando autorizado)**

```bash
cargo test --manifest-path crates/Cargo.toml -p hangar-server --test contract_providers --test contract_history --test contract_tail --test conversations --test proxy
cargo test --manifest-path crates/Cargo.toml -p hangar-server --lib list::tests
cargo test --manifest-path crates/Cargo.toml -p hangar-server --lib orq_read::tests
cargo test --manifest-path crates/Cargo.toml -p hangar-server --lib transcript::provider_tests
(cd backend && uv run pytest tests/test_internal_api.py tests/test_internal_side_events.py tests/test_internal_list.py tests/test_internal_orq.py tests/test_rust_server.py)
```

Esperado: contrato existente e adicional verde. Todos os arquivos citados novos devem ter sido
criados na Task dona; não remover check só porque não foi escrito. Se falhar, repetir somente
os casos que falharam após corrigir. Cargo/build/pytest não foram executados no planejamento.
Build Linux glibc2.28 e plataformas continuam no CI existente; não instalar toolchain para
executar exemplos. Suíte completa e checks de frontend somente se o dono pedir completo.

- [ ] **Step 3: Conferir clientes e retomada no uso real — verificação manual**

O dono instala/seleciona o canal de testes existente. Não iniciar backend duplicado. Com sessão
real de cada provider, abrir lista/chat no celular/PWA e nativo; abrir segundo aparelho; conferir
texto/tool/result/thinking e perguntas/preview/fila. Orq deve mostrar cards enriquecidos e
continuar sem compositor. Quadro/canvas não entram no roteiro; preservar seus campos de lista.

Comparar GET e SSE: escrita sem mudança de estado atualiza atividade no GET e não produz
flicker no SSE. Convidado só vê suas sessões; sessão com owner_sees=false não vaza ao dono.
Solicitar nav, abrir segundo aparelho enquanto pendente, confirmar no primeiro e abrir terceiro:
o terceiro não reabre pedido confirmado. Atalhos vazios/ativos aparecem sem travar lista.

Cortar a rede dos aparelhos e retomar cada chat; orq produzir450 linhas durante o intervalo e
conferir os ids sem buraco. Fechar/recriar sessão de mesmo nome e trocar provider usando os
controles existentes; conferir reset/novo transcript. Erro/list_error mantém a lista anterior,
ping segue, recuperação limpa erro. Receptor lento reconecta pelo arquivo.

- [ ] **Step 4: Reserva e plataformas — verificação manual quando autorizado**

Somente com autorização específica do dono no canal de testes: verificar Python sozinho com
CP_RUST_SERVER=0, binário ausente/incompatível e quedas; sessão sem terminal permanece viva.
Não derrubar processos ou alterar serviço vivo desta máquina para demonstrar o caso. VM Windows
confere psmux, replay/cancelamento, caminhos UTF-8 e watcher/timers; ausência de VM deve aparecer
como não conferida, sem marcar este Step concluído. Linux glibc2.28 mantém build zigbuild existente.

- [ ] **Step 5: Repetir medidas e gravar regra/evidência**

Mesma máquina, registrar data/hora, population por provider, tamanho dos transcripts e requests.
20 GETs lista após um aquecimento; mediana/mín/máx e bytes. Cinco histories full/limit200 por
provider. Python/Rust RSS/threads com zero, um e dois aparelhos da mesma sessão; contar fontes
internas ativas, leitura/tail e watchers. Orq history é Python: não atribuir ganho a Rust.
Comparar CPU por ciclo da lista só se medido diretamente; não deduzir da latência GET cacheada.

Nova regra resumida em CLAUDE.md (texto exato para ajustar à entrega verificada):

```markdown
- **Lista e conversas extras na frente Rust:** dono usa a lista compartilhada por um produtor
  Python; Pi/omp/Kimi history e transcript são lidos pelo Rust; orq ao vivo é compartilhado,
  com parser único Python e histórico repassado. Convidados seguem Python. Gate não distingue
  peer que usa token do dono. Contrato interno sobe junto nos dois lados a cada alteração.
```

Em `plataforma.md`, registrar a tabela medida e os limites da fronteira, vinculando a análise
parte2; em `harnesses.md`, registrar preservação Stream Pi e parser orq único. Sem tabela de
“ganhos” estimados apresentada como medida. Campo não medido recebe motivo, não número inventado.

- [ ] **Step 6: Commit da evidência, sem publicar sem autorização**

```bash
git status --short
git add CLAUDE.md docs/decisoes/plataforma.md docs/decisoes/harnesses.md
git commit -m "docs(server): record session list and provider migration evidence"
git status --short
```

Não executar push nem abrir PR/MR neste plano sem pedido do dono. Reportar hash/branch e estado
real se a execução chegar ao commit. Esta etapa atual termina com documentos e aprovação pendente.
