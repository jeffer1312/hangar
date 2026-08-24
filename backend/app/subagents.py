import json
import logging
import os
from datetime import datetime, timezone
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


# ── Kimi Code ────────────────────────────────────────────────────────────────
# O Kimi tambem delega (tools `Agent` e `AgentSwarm`), mas o layout e outro: cada subagente tem uma
# PASTA propria, <sessao>/agents/agent-N/wire.jsonl, ao lado do agents/main/wire.jsonl que e a
# conversa. Enquanto eles rodam o wire do main nao recebe uma linha (esta no CLAUDE.md: "o main fica
# MUDO quando delega"), entao sem ler estas pastas o app so tinha o cartao "Executando…" — o
# terminal do Kimi, ao lado, desenha uma linha por subagente com o que cada um esta fazendo.


def _kimi_agents_dir(jsonl: str) -> Path | None:
    """A pasta agents/ quando `jsonl` e o wire do main do Kimi; None em qualquer outro layout."""
    p = Path(jsonl)
    if p.name == "wire.jsonl" and p.parent.name == "main" and p.parent.parent.name == "agents":
        return p.parent.parent
    return None


def _sem_git_context(texto: str) -> str:
    """Tira o bloco <git-context>…</git-context> que o Kimi prega na frente do prompt do subagente.

    Ele e igual em todos os subagentes da sessao (branch, arquivos sujos, commits recentes), entao
    ficaria como TITULO de todos eles na lista — e o titulo e a unica coisa que distingue um do
    outro ali."""
    fim = texto.find("</git-context>")
    return texto[fim + len("</git-context>"):].strip() if fim >= 0 else texto.strip()


def _ms_iso(ms: object) -> str:
    """Epoch em MILISSEGUNDOS (o que o wire grava) -> ISO, que e o que o formato do Claude usa."""
    if not isinstance(ms, (int, float)) or ms <= 0:
        return ""
    return datetime.fromtimestamp(ms / 1000, tz=timezone.utc).isoformat()


def _read_agent_kimi(d: Path, tail: int) -> dict | None:
    """Uma passada no wire de um subagente do Kimi, no MESMO dicionario do formato do Claude.

    Mesmo shape de proposito: a lista e o painel de Atividade ja sabem desenhar isto, e um segundo
    formato so pro Kimi seria uma segunda UI pra mesma coisa."""
    f = d / "wire.jsonl"
    try:
        linhas = f.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError as e:
        _log.warning("subagents: não consegui ler %s: %s", f, e)
        return None

    prompt: str | None = None
    tipo: str | None = None
    tools: dict[str, int] = {}
    calls: list[dict] = []
    last_text = ""
    started = ""
    updated = ""
    terminou = False

    for line in linhas:
        try:
            r = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            continue
        t = r.get("type")
        quando = _ms_iso(r.get("time") or r.get("created_at"))
        if quando:
            if not started:
                started = quando
            updated = quando
        if t == "profile.bind":
            v = r.get("profileName")
            if isinstance(v, str):
                tipo = v
        elif t == "turn.prompt" and prompt is None:
            partes = [b.get("text", "") for b in (r.get("input") or [])
                      if isinstance(b, dict) and b.get("type") == "text"]
            texto = _sem_git_context(" ".join(partes))
            if texto:
                prompt = texto
        elif t == "turn.ended":
            terminou = True
        ev = r.get("event")
        if not isinstance(ev, dict):
            continue
        if ev.get("type") == "tool.call":
            n = ev.get("name") or "?"
            tools[n] = tools.get(n, 0) + 1
            calls.append({"name": n, "target": _tool_target(n, ev.get("args"))[:200]})
        elif ev.get("type") == "content.part":
            parte = ev.get("part") or {}
            if isinstance(parte, dict) and parte.get("type") == "text":
                texto = (parte.get("text") or "").strip()
                if texto:
                    last_text = texto

    try:
        mtime = f.stat().st_mtime
    except OSError:
        mtime = 0.0

    return {
        "agentId": d.name,
        "agentType": tipo,
        "prompt": prompt,
        "startedAt": started,
        "updatedAt": updated,
        "mtime": mtime,
        "toolCalls": sum(tools.values()),
        "tools": [{"name": n, "count": c} for n, c in sorted(tools.items(), key=lambda kv: -kv[1])],
        "recent": calls[-tail:],
        "lastText": last_text[:2000],
        # So o Kimi sabe dizer isto sozinho (`turn.ended` no proprio wire). No Claude quem decide se
        # o agente terminou e o tool_result no transcript do PAI, e a UI cruza os dois — aqui o dado
        # esta na mao, e sem ele os quatro subagentes de um AgentSwarm ficam "em execucao" pra
        # sempre depois que o lote fecha.
        "finished": terminou,
    }


def _mtime(f: Path) -> float:
    try:
        return f.stat().st_mtime
    except OSError:
        return 0.0


def _ilegivel(agent_id: str, mtime: float) -> dict:
    """Item pra um subagente que EXISTE mas cujo transcript nao deu pra ler agora.

    Some da lista era a saida antiga (o `if a:` descartava o None do OSError), e ai a resposta ficava
    `[]` — palavra por palavra o mesmo que "esta sessao nao delegou nada". A UI le `[]` como "nada
    rolando agora", entao um subagente com o arquivo temporariamente ilegivel (permissao, disco
    lento, escrita concorrente) sumia da tela com o log so no servidor, que ninguem le do celular.
    Mesma disciplina que `get_subagent` ja aplicava aos EVENTOS de um subagente."""
    return {
        "agentId": agent_id, "agentType": None, "prompt": None,
        "startedAt": "", "updatedAt": "", "mtime": mtime,
        "toolCalls": 0, "tools": [], "recent": [], "lastText": "",
        "ilegivel": True,
    }


def list_subagents(jsonl: str, tail: int = 12) -> list[dict]:
    """Todos os subagentes desta sessão, do mais recém-escrito pro mais antigo."""
    out: list[dict] = []
    kimi = _kimi_agents_dir(jsonl)
    if kimi is not None:
        # `iterdir` numa pasta que sumiu levanta FileNotFoundError, e o endpoint nao tem except: a
        # sessao morta virava 500 no lugar do `[]` honesto que o caminho do Claude ja devolvia.
        if not kimi.is_dir():
            return []
        for d in sorted(kimi.iterdir()):
            if not d.is_dir() or d.name == "main":
                continue
            out.append(_read_agent_kimi(d, tail) or _ilegivel(d.name, _mtime(d / "wire.jsonl")))
        out.sort(key=lambda a: a["mtime"], reverse=True)
        return out
    d = _subagents_dir(jsonl)
    if not d.is_dir():
        return []
    for f in d.glob("agent-*.jsonl"):
        nome = f.stem[len("agent-"):] if f.stem.startswith("agent-") else f.stem
        out.append(_read_agent(f, tail) or _ilegivel(nome, _mtime(f)))
    out.sort(key=lambda a: a["mtime"], reverse=True)
    return out


def get_subagent(jsonl: str, agent_id: str, tail: int = 40, events: int = 0) -> dict | None:
    kimi = _kimi_agents_dir(jsonl)
    if kimi is not None:
        return _get_subagent_kimi(kimi, agent_id, tail, events)
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


def _get_subagent_kimi(agents: Path, agent_id: str, tail: int, events: int) -> dict | None:
    # `agent_id` vem da URL: so o nome de UMA pasta filha, nunca um caminho. Sem isto um
    # "../../.." abriria wire de outra sessao (o Path / faz a juncao sem reclamar).
    #
    # Quem decide e o caminho RESOLVIDO, nao uma lista de caracteres proibidos — mesmo padrao do
    # `serve_file` em api.py. Barrar "/" e "\" parece bastar e nao basta no Windows: "D:x" nao tem
    # separador nenhum e mesmo assim e um caminho DRIVE-RELATIVO, e `Path("...agents") / "D:x"`
    # devolve `D:x`, jogando a base inteira fora (medido). O realpath fecha essa e as proximas.
    if agent_id == "main":
        return None
    d = agents / agent_id
    if os.path.dirname(os.path.realpath(d)) != os.path.realpath(agents):
        return None
    if not (d / "wire.jsonl").is_file():
        return None
    a = _read_agent_kimi(d, tail)
    if a is not None and events:
        ev = _events_kimi(d / "wire.jsonl", events)
        if ev is not None:
            a["events"] = ev
    return a


def _events_kimi(f: Path, limit: int) -> list[dict] | None:
    """O mesmo que `_events`, com o parser do Kimi — o wire nao e o jsonl do Claude."""
    from app.adapters.kimi.transcript import parse_line as parse_kimi

    out: list[dict] = []
    try:
        for line in f.read_text(encoding="utf-8", errors="replace").splitlines():
            for ev in parse_kimi(line):
                out.append(ev if isinstance(ev, dict) else ev.__dict__)
    except OSError as e:
        _log.warning("subagents: não consegui ler os eventos de %s: %s", f, e)
        return None
    return out[-limit:]


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
