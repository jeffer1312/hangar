"""Sidecar DURAVEL das sessoes Codex. Cada sessao tem uma TUI no tmux e um AppServerClient
WebSocket que o backend segura em memoria (efemero). Se o backend reiniciar, o processo app-server
morre, mas a IDENTIDADE (name/thread_id/rollout_path/cwd) sobrevive para recriar o servidor, retomar
a thread e ligar uma nova TUI tmux sob demanda (resume lazy). Este modulo grava essa identidade.

O historico do chat SEMPRE persiste no rollout JSONL do proprio Codex (~/.codex/sessions/...); aqui
so guardamos o ponteiro pra ele + o thread_id necessario pro thread/resume.

Local: ~/.hangar/codex-sessions/<name>.json (mesma familia de ~/.hangar usada pelo
sync-vault). Global por usuario (sessao Codex nao pertence a um config-dir do Claude). Um arquivo
por sessao, keyed pelo NOME sanitizado da sessao."""
import json
import logging
import os
import threading
from pathlib import Path

from app import atomico
from app.names import sanitize_session_name

_log = logging.getLogger("hangar.codex.sessions")
_pretrust_lock = threading.Lock()


def _dir() -> Path:
    # NAO cria o dir aqui (load/list nao devem ter efeito colateral); save() cria sob demanda.
    return Path.home() / ".hangar" / "codex-sessions"


def _sanitize(name: str) -> str:
    # MESMA funcao do registry (app.names), nao uma copia da regra: a copia daqui nao recebeu a
    # correcao de acentuacao e teria trazido o bug de volta so no lado Codex no dia em que alguem
    # chamasse codex_sessions.* com um nome cru, sem passar pelo registry antes.
    return sanitize_session_name(name)


def _path(name: str) -> Path:
    return _dir() / f"{_sanitize(name)}.json"


def save(name: str, thread_id: str, rollout_path: str, cwd: str,
         model: str | None = None, effort: str | None = None,
         endpoint: str | None = None, app_pid: int | None = None) -> None:
    """Grava (ou sobrescreve) o sidecar duravel da sessao Codex. Escrita ATOMICA (tmp + replace,
    mesmo padrao de PromptQueue._write_atomic em pqueue.py) -- write_text direto podia corromper
    o sidecar em crash/concorrencia no meio da escrita.

    model/effort (Task C): escolha de modelo/reasoning effort da sessao, opcional -- None pra
    sessao nova (usa o default da thread) ou sidecar antigo (chave ausente = load().get() -> None,
    sem quebrar).

    endpoint/app_pid: o app-server DAQUELA sessao, escrito pelo lancador (scripts/hangar-codex-tui).
    Sao o que deixa o backend se conectar a um servidor que nao e filho dele. O pid anda junto do
    endpoint porque porta de loopback e reciclada: reconectar so pelo endereco pode cair num
    processo alheio que ja tomou a porta. Ausentes = sidecar do desenho antigo, em que o servidor
    era filho do backend."""
    _dir().mkdir(parents=True, exist_ok=True)
    p = _path(name)
    tmp = p.with_suffix(".json.tmp")
    tmp.write_text(json.dumps({
        "name": name,
        "provider": "codex",
        "thread_id": thread_id,
        "rollout_path": rollout_path,
        "cwd": cwd,
        "model": model,
        "effort": effort,
        "endpoint": endpoint,
        "app_pid": app_pid,
    }), encoding="utf-8")
    atomico.substituir(tmp, p)


def update_model(name: str, model: str | None, effort: str | None) -> None:
    """Atualiza SO a escolha de modelo/effort no sidecar existente, preservando thread_id/
    rollout_path/cwd (re-le e regrava via save()). No-op silencioso se o sidecar nao existe
    (nome desconhecido) -- quem chama (CodexAdapter.set_model) ja mantem a copia em memoria."""
    meta = load(name)
    if meta is None:
        return
    # endpoint/app_pid seguem juntos: sem eles aqui, trocar o modelo apagaria o endereco do
    # app-server e o backend passaria a tratar a sessao viva como sessao do desenho antigo.
    save(name, meta["thread_id"], meta["rollout_path"], meta["cwd"], model=model, effort=effort,
         endpoint=meta.get("endpoint"), app_pid=meta.get("app_pid"))


def load(name: str) -> dict | None:
    """Le o sidecar de uma sessao (ou None se nao existe / corrompido)."""
    try:
        return json.loads(_path(name).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def delete(name: str) -> None:
    """Remove o sidecar (idempotente)."""
    try:
        _path(name).unlink(missing_ok=True)
    except OSError:
        pass


def rename(old: str, new: str) -> None:
    """Move o sidecar junto com a sessao tmux, preservando a identidade da thread."""
    src, dst = _path(old), _path(new)
    if not src.exists():
        return
    dst.parent.mkdir(parents=True, exist_ok=True)
    atomico.substituir(src, dst)
    meta = load(new)
    if meta is not None:
        save(new, meta["thread_id"], meta["rollout_path"], meta["cwd"],
             model=meta.get("model"), effort=meta.get("effort"),
             endpoint=meta.get("endpoint"), app_pid=meta.get("app_pid"))


def list_all() -> list[dict]:
    """Todas as sessoes Codex gravadas (pula arquivos corrompidos). Usado pelo registry.list()."""
    out: list[dict] = []
    try:
        files = sorted(_dir().glob("*.json"))
    except OSError:
        return out
    for f in files:
        try:
            out.append(json.loads(f.read_text(encoding="utf-8")))
        except (OSError, ValueError):
            continue
    return out


def exists(name: str) -> bool:
    return _path(name).exists()


# Onde o bloco que a ponte de skills gerencia COMECA no ~/.codex/config.toml (app/agentes_sync).
# A entrada de confianca entra ANTES dele: o Codex apenda `[hooks.state."..."]` no fim do arquivo,
# e o fim, hoje, esta dentro daquele bloco — escrever la seria pendurar a nossa tabela no meio de
# um trecho que outro codigo reescreve inteiro.
_MARCA_BLOCO = "# >>> hangar:"


def pretrust_cwd(cwd: str) -> None:
    """Pre-confia a pasta no `~/.codex/config.toml`: sem isso, uma sessao criada pelo app numa
    pasta NOVA nasce presa no "Do you trust the contents of this directory?" da TUI do Codex.

    Medido em 30/08/2026 (codex-cli 0.151.0), e cada um destes fatos e o motivo de uma linha aqui:
    a TUI nao abre a thread enquanto a pergunta esta na tela, entao a sessao fica sem rollout e sem
    sidecar — invisivel no proprio app que a criou, e sem ninguem no celular pra responder;
    a confianca NAO e herdada por subpasta (com `/tmp` confiado, `/tmp/proj-x` ainda pergunta);
    e o override de linha de comando (`-c projects."...".trust_level="trusted"`) NAO vale para isto
    — a TUI pergunta do mesmo jeito, entao a unica via e o arquivo.

    Mesmo papel do `_pretrust_cwd` do Claude (.claude.json) e do `pretrust_cwd` do Kimi. Quem digita
    `codex` no terminal nao passa por aqui: ali a pergunta tem quem responda, e responder por ela
    seria decidir confianca no lugar da pessoa.

    Best-effort: nunca levanta — falha aqui so devolve o comportamento de sem-pretrust.
    """
    import tomllib
    # Import LOCAL: quem escreve neste arquivo ja tem a funcao certa pra isso (ela preserva o modo
    # e poe o pid no temporario, porque nome fixo com duas escritas simultaneas promove bytes
    # entrelacados). Local, e nao no topo, porque `scripts/hangar-codex` importa este modulo com o
    # `python3` do sistema e nunca chama pretrust — ele nao pode pagar por esta dependencia.
    from app.agentes_sync import _gravar_preservando

    cfg = Path.home() / ".codex" / "config.toml"
    alvo = os.path.abspath(os.path.expanduser(cwd))
    # Um create() por thread (registry.create roda em to_thread): dois nascendo juntos fariam
    # read-modify-write no MESMO arquivo e o ultimo apagaria a entrada do outro, calado. Mesmo
    # padrao do _pretrust_lock do lado Claude (registry._pretrust_cwd).
    with _pretrust_lock:
        try:
            raw = cfg.read_text(encoding="utf-8") if cfg.exists() else ""
            # tomllib, e nao um regex procurando `[projects."<alvo>"]`: escrita por texto so e
            # segura depois de uma leitura que entenda TOML de verdade. Uma tabela ja existente
            # escrita de outra forma (aspas simples, nome sem aspas) passaria batida pelo regex, e
            # apendar a nossa seria REDEFINIR a tabela — o arquivo inteiro pararia de abrir, pro
            # Codex e pra ponte.
            if alvo in (tomllib.loads(raw).get("projects") or {}):
                return
            entrada = f'[projects.{json.dumps(alvo)}]\ntrust_level = "trusted"\n\n'
            corte = raw.find(_MARCA_BLOCO)
            # Sem backup, ao contrario do _gravar_bloco_toml: aquele SUBSTITUI um bloco inteiro, e
            # aqui so se insere uma tabela nova — nao ha conteudo do usuario em risco de sumir.
            novo = raw + "\n" + entrada if corte < 0 else raw[:corte] + entrada + raw[corte:]
            cfg.parent.mkdir(parents=True, exist_ok=True)
            _gravar_preservando(cfg, novo)
        except (OSError, ValueError) as e:
            # Best-effort NAO e mudo: o unico sintoma de um pre-trust que nao foi gravado e a TUI
            # parada na pergunta de confianca, e sem esta linha nao ha nada no log ligando uma
            # coisa a outra.
            _log.warning("pretrust do codex falhou pra %s: %r", cwd, e)
