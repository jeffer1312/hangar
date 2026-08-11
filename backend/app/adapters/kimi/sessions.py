"""Onde uma sessao do Kimi vive no disco.

Layout medido no Kimi 0.34.0 (e documentado em kimi.com/code/docs):

    ~/.kimi-code/sessions/<workDirKey>/<sessionId>/agents/main/wire.jsonl
    ~/.kimi-code/session_index.jsonl     {"sessionId","sessionDir","workDir"} por linha
    ~/.kimi-code/workspace-trust/<workDirKey>   {"root","trustedAt"} — sem ele a TUI trava no
                                                "Trust this folder?" (medido num boot real)

workDirKey = "wd_" + basename(cwd) + "_" + sha256(cwd)[:12] — hash confirmado por calculo
(/tmp/kimi-acp-probe -> 15ca61fc9ec9, /home/jefferson/Projetos/hangar -> 5112ff7a84e0).
O slug mantem hifen intacto (kimi-acp-probe); o que ele faz com espaco/acento ainda nao foi medido
— por isso a resolucao primaria e o INDICE (autoritativo) e a chave computada e so o fallback.

Duas diferencas pro layout do Claude que dirigem o desenho do adapter:
  1. O session-id NAO e escolhido pelo caller (nao existe --session-id; -S so resume). Quem liga
     pane->sessao e o bilhete que o kimi_state_hook.py escreve (registry.kimi_session_file).
  2. O transcript se chama wire.jsonl pra TODA sessao -> Path(jsonl).stem seria "wire" sempre.
     A chave de estado e o nome do sessionDir (state_key abaixo).
"""
import hashlib
import json
import os
import time
from pathlib import Path


def kimi_home() -> Path:
    # KIMI_CODE_HOME move TUDO (doc oficial de data locations); o CLI respeita, entao nos tambem.
    env = os.environ.get("KIMI_CODE_HOME")
    return Path(env) if env else Path.home() / ".kimi-code"


def workdir_key(cwd: str) -> str:
    resolved = os.path.abspath(os.path.expanduser(cwd))
    # ponytail: calibration knob — slug com espaco/acento nao medido; o indice cobre o desvio.
    slug = os.path.basename(resolved) or "root"
    h = hashlib.sha256(resolved.encode("utf-8")).hexdigest()[:12]
    return f"wd_{slug}_{h}"


def _wire_of(session_dir: str) -> str:
    return str(Path(session_dir) / "agents" / "main" / "wire.jsonl")


def transcript_path(cwd: str, session_id: str) -> str:
    """Caminho do wire.jsonl da sessao, ou "" se ela ainda nao existe em disco.

    O Kimi so cria a sessao no 1o prompt ("No session yet" da TUI) — "" e nao excecao, igual ao
    transcript_path do Pi. O indice e a fonte primaria; a chave computada e o fallback (cobre a
    janela entre o hook gravar o bilhete e o CLI flushar o session_index).
    """
    try:
        with open(kimi_home() / "session_index.jsonl", encoding="utf-8") as fh:
            for line in fh:
                try:
                    o = json.loads(line)
                except ValueError:
                    continue
                if isinstance(o, dict) and o.get("sessionId") == session_id and o.get("sessionDir"):
                    return _wire_of(o["sessionDir"])
    except OSError:
        pass
    d = kimi_home() / "sessions" / workdir_key(cwd) / session_id
    return _wire_of(str(d)) if d.is_dir() else ""


def is_subagent_wire(path: str) -> bool:
    """O wire e de um subagente (Agent tool do Kimi), nao da conversa principal?

    Subagente mora em agents/<nome>/wire.jsonl ao lado do agents/main/wire.jsonl (medido no layout
    documentado: agent-0, agent-1...)."""
    p = Path(path)
    return p.name == "wire.jsonl" and p.parent.parent.name == "agents" and p.parent.name != "main"


def root_wire(path: str) -> str:
    """Do wire de um subagente pro da conversa principal, ou "" se o path nao e de subagente."""
    if not is_subagent_wire(path):
        return ""
    return str(Path(path).parent.parent / "main" / "wire.jsonl")


def pretrust_cwd(cwd: str) -> None:
    """Pre-confia a pasta no workspace-trust do Kimi: sem o arquivo, uma sessao criada pelo app
    numa pasta NOVA nasce presa no "Trust this folder?" (medido num boot real — a TUI nao aceita
    input ate responder). Mesmo papel do _pretrust_cwd do Claude (.claude.json). Best-effort:
    nunca levanta — falha aqui so devolve o comportamento de sem-pretrust."""
    try:
        resolved = os.path.abspath(os.path.expanduser(cwd))
        d = kimi_home() / "workspace-trust"
        f = d / workdir_key(resolved)
        if f.exists():
            return
        d.mkdir(parents=True, exist_ok=True)
        tmp = f.with_suffix(".tmp")
        tmp.write_text(json.dumps({"root": resolved, "trustedAt": int(time.time() * 1000)}),
                       encoding="utf-8")
        os.replace(tmp, f)  # atomico, mesmo padrao dos marcadores
    except OSError:
        pass
