import json
import logging
from pathlib import Path

# Subagentes SOLTOS (tool Agent, fora de Workflow). O Claude Code grava o transcript de cada um em
#   <session-dir>/subagents/agent-<agentId>.jsonl   (+ agent-<agentId>.meta.json com o agentType)
# session-dir = o jsonl do transcript sem a extensão — o MESMO layout que workflows.py já lê pros
# agentes de workflow, só que um nível acima (sem o wf_<runId>/ no meio).
#
# Por que existe: o transcript do PAI só guarda o tool_use (descrição + prompt) e, no fim, o
# resultado. Enquanto o agente roda, o painel de Atividade só sabia dizer o título dele. As
# ferramentas que ele está chamando AGORA estão neste arquivo.

from app.workflows import _session_dir

_log = logging.getLogger("claude_pocket")


def _subagents_dir(jsonl: str) -> Path:
    return _session_dir(jsonl) / "subagents"


def _text_of(content) -> str:
    """Texto de um message.content (string ou lista de blocos)."""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return " ".join(
            b.get("text", "") for b in content if isinstance(b, dict) and b.get("type") == "text"
        ).strip()
    return ""


def _tool_target(name: str, inp) -> str:
    """Uma linha curta do que a tool está tocando: caminho, comando ou padrão."""
    if not isinstance(inp, dict):
        return ""
    for k in ("file_path", "path", "notebook_path"):
        v = inp.get(k)
        if isinstance(v, str):
            return v
    if name == "Bash":
        v = inp.get("command")
        return v if isinstance(v, str) else ""
    for k in ("pattern", "query", "url", "prompt", "description"):
        v = inp.get(k)
        if isinstance(v, str):
            return v
    return ""


def _read_agent(f: Path, tail: int) -> dict | None:
    """Uma passada no transcript do subagente: prompt, contagem de tools, últimas chamadas, texto."""
    try:
        lines = f.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError as e:
        # Sem log, um subagente ilegível (permissão, disco) sumia da lista igualzinho a um que
        # nunca existiu — e ninguém tinha como saber a diferença.
        _log.warning("subagents: não consegui ler %s: %s", f, e)
        return None

    agent_id = f.stem[len("agent-"):] if f.stem.startswith("agent-") else f.stem
    prompt: str | None = None
    tools: dict[str, int] = {}
    calls: list[dict] = []
    last_text = ""
    started = ""
    updated = ""

    for line in lines:
        try:
            r = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            continue
        ts = r.get("timestamp")
        if isinstance(ts, str):
            if not started:
                started = ts
            updated = ts
        msg = r.get("message") or {}
        content = msg.get("content")
        if r.get("type") == "user" and prompt is None:
            t = _text_of(content)
            if t:
                prompt = t
        elif r.get("type") == "assistant" and isinstance(content, list):
            for b in content:
                if not isinstance(b, dict):
                    continue
                if b.get("type") == "tool_use":
                    n = b.get("name") or "?"
                    tools[n] = tools.get(n, 0) + 1
                    calls.append({"name": n, "target": _tool_target(n, b.get("input"))[:200]})
                elif b.get("type") == "text":
                    t = (b.get("text") or "").strip()
                    if t:
                        last_text = t

    # `mtime` é o sinal de vida: o arquivo é append-only enquanto o agente trabalha. Quem decide se
    # ele TERMINOU é o transcript do pai (tool_result) — aqui só reportamos a última escrita, e a UI
    # cruza com o que ela já sabe. Sem isso um agente morto pareceria vivo pra sempre.
    try:
        mtime = f.stat().st_mtime
    except OSError:
        mtime = 0.0

    meta: dict = {}
    mf = f.parent / (f.stem + ".meta.json")
    if mf.is_file():
        try:
            meta = json.loads(mf.read_text(encoding="utf-8", errors="replace")) or {}
        except (OSError, json.JSONDecodeError, ValueError):
            meta = {}

    return {
        "agentId": agent_id,
        "agentType": meta.get("agentType"),
        "prompt": prompt,
        "startedAt": started,
        "updatedAt": updated,
        "mtime": mtime,
        "toolCalls": sum(tools.values()),
        "tools": [{"name": n, "count": c} for n, c in sorted(tools.items(), key=lambda kv: -kv[1])],
        # Cauda: as últimas chamadas, que é o "o que ele está fazendo agora" de fato.
        "recent": calls[-tail:],
        "lastText": last_text[:2000],
    }


def list_subagents(jsonl: str, tail: int = 12) -> list[dict]:
    """Todos os subagentes desta sessão, do mais recém-escrito pro mais antigo."""
    d = _subagents_dir(jsonl)
    if not d.is_dir():
        return []
    out: list[dict] = []
    for f in d.glob("agent-*.jsonl"):
        a = _read_agent(f, tail)
        if a:
            out.append(a)
    out.sort(key=lambda a: a["mtime"], reverse=True)
    return out


def get_subagent(jsonl: str, agent_id: str, tail: int = 40, events: int = 0) -> dict | None:
    f = _subagents_dir(jsonl) / f"agent-{agent_id}.jsonl"
    if not f.is_file():
        return None
    a = _read_agent(f, tail)
    if a is not None and events:
        ev = _events(f, events)
        # `None` = não deu pra ler AGORA. A chave fica ausente de propósito: uma lista vazia aqui
        # faria a UI afirmar "nenhuma ferramenta chamada", que é dizer que o agente está parado.
        if ev is not None:
            a["events"] = ev
    return a


def _events(f: Path, limit: int) -> list[dict] | None:
    """Transcript do subagente nos MESMOS ChatEvent do chat principal.

    O arquivo tem o formato de sempre, então a conversão é a `parse_line` do transcript — o app
    reusa a lista de mensagens inteira (bolhas, cartões de tool, markdown) em vez de uma lista
    própria de "últimas chamadas", que era um segundo jeito de desenhar a mesma coisa.
    """
    from app.transcript import parse_line

    out: list[dict] = []
    try:
        for line in f.read_text(encoding="utf-8", errors="replace").splitlines():
            for ev in parse_line(line):
                out.append(ev if isinstance(ev, dict) else ev.__dict__)
    except OSError as e:
        _log.warning("subagents: não consegui ler os eventos de %s: %s", f, e)
        return None
    return out[-limit:]
