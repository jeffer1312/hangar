import json
import logging
import os
import re
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

_log = logging.getLogger("hangar")


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
    # Sem case-fold, o Pi (que chama a tool de `bash`, minúsculo) caía no `for` seguinte e não
    # achava chave nenhuma: o alvo vinha VAZIO justamente na tool mais usada dele.
    if name.lower() == "bash":
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


# ── Pi ───────────────────────────────────────────────────────────────────────
# Terceiro layout, e o mais parecido com o do Claude: cada subagente da sessao mora DENTRO da pasta
# que leva o nome do transcript do pai —
#   <transcript-sem-.jsonl>/<taskId>/run-<n>/session.jsonl
# Por ficar sob o proprio transcript, ele ja e por-sessao: nao precisa de cwd, nem de varrer o pai
# atras de id, nem corre risco de misturar subagente de outra conversa. E o arquivo nasce junto com
# o subagente e cresce enquanto ele trabalha, que e o caso que importa — enquanto ele roda, o
# transcript do pai NAO tem nada dele (o `toolResult` da tool `subagent` so aparece no fim).
#
# O formato de dentro e o MESMO do transcript de sessao do Pi (`type: "message"` + `message`),
# entao `adapters/pi/transcript` vale aqui sem adaptador nenhum.
#
# A pasta `<cwd>/.pi/subagents/artifacts/<taskId>_<agente>_<n>_*` (input.md, output.md, meta.json)
# e a MESMA execucao vista de outro angulo — o `taskId` daqui e o prefixo de la —, mas nao serve de
# fonte: e por CWD, entao junta meses de subagente de conversas que ninguem tem mais aberta
# (40 MB medidos numa dessas), e no meio de um run so tem o transcript, que ja e este arquivo.
_RUN_RE = re.compile(r"^run-(\d+)$")
# `session_info.name` = "subagent-<agente>-<uuid do run>-<n>" (medido no Pi 0.82.1). E a UNICA
# fonte do tipo do agente que existe DENTRO do filho; o resto so o pai sabe, e o pai fica mudo ate
# o run fechar.
_NOME_SUB_RE = re.compile(r"^subagent-(.+?)-[0-9a-f]{8}-[0-9a-f-]+-\d+$")
# `stopReason` que encerra o subagente. Levantado sobre os 305 transcripts de filho desta maquina
# (27/08/2026): `toolUse` aparece 4660 vezes e e "vou chamar mais uma ferramenta"; `stop` (297),
# `error` (12) e `length` (2) sao fim de linha — mal ou bem, aquele run nao continua.
_ACABOU = ("stop", "error", "length")


def _pi_agents_dir(jsonl: str) -> Path | None:
    """A pasta de subagentes quando `jsonl` e um transcript do Pi; None em qualquer outro layout."""
    from app.adapters.pi.sessions import sessions_root
    p = Path(jsonl)
    if p.suffix != ".jsonl":
        return None
    try:
        if not p.is_relative_to(sessions_root()):
            return None
    except (OSError, ValueError):
        return None
    d = p.with_suffix("")
    return d if d.is_dir() else None


def _pi_run_dir(task: Path) -> Path | None:
    """O `run-<n>` mais alto de uma task, ou None.

    So o ultimo: `run-1`/`run-2` sao RETENTATIVAS da mesma task (59 delas nesta maquina), e listar
    todas encheria o painel com a versao abandonada ao lado da que vale."""
    melhor: tuple[int, Path] | None = None
    try:
        filhos = list(task.iterdir())
    except OSError:
        return None
    for d in filhos:
        m = _RUN_RE.match(d.name)
        if m and (d / "session.jsonl").is_file() and (melhor is None or int(m.group(1)) > melhor[0]):
            melhor = (int(m.group(1)), d)
    return melhor[1] if melhor else None


def _read_agent_pi(task: Path, tail: int) -> dict | None:
    """Uma passada num subagente do Pi, no MESMO dicionario do formato do Claude."""
    run = _pi_run_dir(task)
    if run is None:
        return None
    f = run / "session.jsonl"
    try:
        linhas = f.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError as e:
        _log.warning("subagents: não consegui ler %s: %s", f, e)
        return None

    tools: dict[str, int] = {}
    calls: list[dict] = []
    last_text = ""
    started = ""
    updated = ""
    prompt: str | None = None
    tipo_agente: str | None = None
    # (papel, stopReason) da ULTIMA mensagem — e o que diz se o subagente acabou. Ver `_ACABOU`.
    fim: tuple[str | None, str | None] = (None, None)

    for line in linhas:
        try:
            r = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            continue
        quando = r.get("timestamp")
        if isinstance(quando, str):
            if not started:
                started = quando
            updated = quando
        if r.get("type") == "session_info" and tipo_agente is None:
            m = _NOME_SUB_RE.match(r.get("name") or "")
            if m:
                tipo_agente = m.group(1)
            continue
        if r.get("type") != "message":
            continue
        msg = r.get("message") or {}
        content = msg.get("content")
        if not isinstance(content, list):
            continue
        papel = msg.get("role")
        fim = (papel, msg.get("stopReason"))
        if papel == "user" and prompt is None:
            t = _text_of(content)
            if t:
                prompt = t
        elif papel == "assistant":
            for b in content:
                if not isinstance(b, dict):
                    continue
                if b.get("type") == "toolCall":
                    n = b.get("name") or "?"
                    tools[n] = tools.get(n, 0) + 1
                    calls.append({"name": n, "target": _tool_target(n, b.get("arguments"))[:200]})
                elif b.get("type") == "text":
                    t = (b.get("text") or "").strip()
                    if t:
                        last_text = t

    return {
        "agentId": task.name,
        "agentType": tipo_agente,
        "prompt": prompt,
        "startedAt": started,
        "updatedAt": updated,
        "mtime": _mtime(f),
        "toolCalls": sum(tools.values()),
        "tools": [{"name": n, "count": c} for n, c in sorted(tools.items(), key=lambda kv: -kv[1])],
        "recent": calls[-tail:],
        "lastText": last_text[:2000],
        # Como no Kimi, o dado esta na mao — e sem ele TODO subagente do Pi ficava "rodando" pra
        # sempre (visto na tela em 27/08/2026): pro Claude quem fecha o cartao e o `tool_result` no
        # transcript do pai, e o pai do Pi nunca cita o `taskId` que da nome a este subagente.
        "finished": fim[0] == "assistant" and fim[1] in _ACABOU,
    }


def _get_subagent_pi(agents: Path, agent_id: str, tail: int, events: int) -> dict | None:
    # `agent_id` vem da URL: so o nome de UMA pasta filha, nunca um caminho. Mesma guarda do Kimi
    # e pelo mesmo motivo — quem decide e o caminho RESOLVIDO, nao uma lista de caracteres
    # proibidos (no Windows "D:x" nao tem separador e ainda assim joga a base fora).
    d = agents / agent_id
    if os.path.dirname(os.path.realpath(d)) != os.path.realpath(agents):
        return None
    run = _pi_run_dir(d)
    if run is None:
        return None
    a = _read_agent_pi(d, tail) or _ilegivel(agent_id, _mtime(run / "session.jsonl"))
    if a.get("ilegivel"):
        return a
    if events:
        ev = _events_pi(run / "session.jsonl", events)
        if ev is not None:
            a["events"] = ev
    return a


def _events_pi(f: Path, limit: int) -> list[dict] | None:
    """O mesmo que `_events`, com o parser do Pi — o filho tem o formato de transcript do Pi, nao
    o jsonl do Claude."""
    from app.adapters.pi.transcript import parse_line as parse_pi

    out: list[dict] = []
    try:
        for line in f.read_text(encoding="utf-8", errors="replace").splitlines():
            for ev in parse_pi(line):
                out.append(ev if isinstance(ev, dict) else ev.__dict__)
    except OSError as e:
        _log.warning("subagents: não consegui ler os eventos de %s: %s", f, e)
        return None
    return out[-limit:]


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
    pi = _pi_agents_dir(jsonl)
    if pi is not None:
        # Mesmo cuidado do Kimi: a pasta pode sumir DEPOIS do is_dir e no meio da iteracao, e o
        # OSError subiria cru ate a rota, que nao tem except.
        try:
            tasks = sorted(p for p in pi.iterdir() if p.is_dir())
        except OSError:
            return []
        for t in tasks:
            run = _pi_run_dir(t)
            if run is None:
                continue      # pasta de task sem run nenhum: nao e subagente, e sobra de layout
            out.append(_read_agent_pi(t, tail) or _ilegivel(t.name, _mtime(run / "session.jsonl")))
        out.sort(key=lambda a: a["mtime"], reverse=True)
        return out
    kimi = _kimi_agents_dir(jsonl)
    if kimi is not None:
        # `iterdir` numa pasta que sumiu levanta FileNotFoundError, e o endpoint nao tem except: a
        # sessao morta virava 500 no lugar do `[]` honesto que o caminho do Claude ja devolvia.
        # Checar antes NAO basta: a pasta pode sumir DEPOIS do is_dir e no meio da iteracao, e o
        # OSError sobe cru ate a rota, que nao tem except — 500 no lugar do `[]` honesto.
        try:
            filhos = sorted(kimi.iterdir())
        except OSError:
            return []
        for d in filhos:
            if not d.is_dir() or d.name == "main":
                continue
            out.append(_read_agent_kimi(d, tail) or _ilegivel(d.name, _mtime(d / "wire.jsonl")))
        out.sort(key=lambda a: a["mtime"], reverse=True)
        return out
    d = _subagents_dir(jsonl)
    try:
        arquivos = sorted(d.glob("agent-*.jsonl"))
    except OSError:
        return []
    for f in arquivos:
        nome = f.stem[len("agent-"):] if f.stem.startswith("agent-") else f.stem
        out.append(_read_agent(f, tail) or _ilegivel(nome, _mtime(f)))
    out.sort(key=lambda a: a["mtime"], reverse=True)
    return out


def get_subagent(jsonl: str, agent_id: str, tail: int = 40, events: int = 0) -> dict | None:
    pi = _pi_agents_dir(jsonl)
    if pi is not None:
        return _get_subagent_pi(pi, agent_id, tail, events)
    kimi = _kimi_agents_dir(jsonl)
    if kimi is not None:
        return _get_subagent_kimi(kimi, agent_id, tail, events)
    f = _subagents_dir(jsonl) / f"agent-{agent_id}.jsonl"
    if not f.is_file():
        return None
    # O arquivo EXISTE (o is_file acima) — entao `None` aqui so pode ser falha de leitura, nunca
    # "nao existe". Devolver None juntava os dois no mesmo 404 "subagente inexistente", e a linha
    # que a lista acabou de marcar como ilegivel abria dizendo que o agente nem existe.
    a = _read_agent(f, tail) or _ilegivel(agent_id, _mtime(f))
    if a.get("ilegivel"):
        return a
    if events:
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
    # Mesma regra do caminho do Claude: o wire EXISTE, entao `None` aqui e falha de leitura, e o
    # 404 de "subagente inexistente" seria mentira.
    a = _read_agent_kimi(d, tail) or _ilegivel(agent_id, _mtime(d / "wire.jsonl"))
    if a.get("ilegivel"):
        return a
    if events:
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
