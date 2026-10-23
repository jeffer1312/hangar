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
import tempfile
from contextlib import contextmanager
from pathlib import Path

from app import atomico
from app.names import sanitize_session_name

_log = logging.getLogger("hangar.codex.sessions")
_pretrust_lock = threading.Lock()
_write_lock = threading.Lock()


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


def primary_thread(thread: dict, cwd: str) -> bool:
    return (thread.get("cwd") == cwd and thread.get("source") in ("cli", "vscode")
            and not thread.get("parentThreadId") and thread.get("threadSource") != "subagent"
            and thread.get("canAcceptDirectInput") is not False
            and bool(thread.get("id") and thread.get("path")))


def switch_thread(name: str, thread: dict, previous: str, *, endpoint: str, app_pid: int) -> bool:
    with _locked(name):
        meta = load(name)
        if not meta or meta.get("endpoint") != endpoint or meta.get("app_pid") != app_pid \
                or not primary_thread(thread, meta.get("cwd")):
            return False
        if meta["thread_id"] == thread["id"]:
            return True
        if meta["thread_id"] != previous:
            return False
        _write(name, _merge_conversation(meta, {"thread_id": thread["id"], "rollout_path": thread["path"]}))
        return True


@contextmanager
def _locked(*names: str):
    _dir().mkdir(parents=True, exist_ok=True)
    # A trava de arquivo primeiro: esperar o Rust numa sessão não segura as gravações das outras.
    with atomico.locked(*(_path(name).with_suffix(".lock") for name in names)), _write_lock:
        yield


def _write(name: str, meta: dict) -> None:
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=_dir(), suffix=".tmp", delete=False) as tmp:
        json.dump(meta, tmp)
    try:
        atomico.substituir(tmp.name, _path(name))
    finally:
        Path(tmp.name).unlink(missing_ok=True)


def save(name: str, thread_id: str | None, rollout_path: str, cwd: str,
         model: str | None = None, effort: str | None = None,
         endpoint: str | None = None, app_pid: int | None = None,
         codex_home: str | Path | None = None,
         codex_account: str | None = None,
         headless: bool = False, key: str | None = None,
         permission_mode: str | None = None, jev: bool = False,
         transfer_id: str | None = None, tool_output_token_limit: int | None = None,
         previous_non_plan: str | None = None, launching: bool = False,
         service_tier: str | None = None) -> None:
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
    meta = {
        "name": name,
        "provider": "codex",
        "thread_id": thread_id,
        "rollout_path": rollout_path,
        "cwd": cwd,
        "model": model,
        "effort": effort,
        "endpoint": endpoint,
        "app_pid": app_pid,
    }
    if service_tier is not None:
        meta["service_tier"] = service_tier
    if codex_home is not None:
        meta["codex_home"] = str(Path(codex_home).expanduser().absolute())
    if codex_account is not None:
        meta["codex_account"] = codex_account
    if key is not None:
        meta["key"] = key
    if transfer_id is not None:
        meta["transfer_id"] = transfer_id
    if permission_mode is not None:
        meta["permission_mode"] = permission_mode
    if previous_non_plan is not None:
        meta["previous_non_plan"] = previous_non_plan
    if launching:
        meta["launching"] = True
    if tool_output_token_limit is not None:
        if type(tool_output_token_limit) is not int or tool_output_token_limit <= 0:
            raise ValueError("limite de resultado de ferramenta inválido")
        meta["tool_output_token_limit"] = tool_output_token_limit
    if headless:
        # Sem terminal: o app-server roda atrás de um cano (sem_terminal.py). `key` é a chave do
        # cano (varredura de órfãos) e `permission_mode` o modo do app, que vira sandbox/approval.
        meta.update(headless=True, permission_mode=permission_mode, cano=None, jev=jev)
    with _locked(name):
        previous = load(name) or {}
        same_thread = previous.get("thread_id") == thread_id
        for field in ("key", "transfer_id", "tool_output_token_limit", "codex_home", "codex_account",
                      "permission_mode", "previous_non_plan", "jev", "service_tier"):
            if field not in meta and field in previous and (field == "key" or same_thread):
                meta[field] = previous[field]
        _write(name, meta)


def _merge_conversation(meta: dict, fields: dict) -> dict:
    if (meta.get("thread_id") and "thread_id" in fields
            and fields["thread_id"] != meta["thread_id"]):
        meta = {k: v for k, v in meta.items() if k not in {"transfer_id", "tool_output_token_limit", "service_tier"}}
    return {**meta, **fields}


def update(name: str, **campos) -> dict | None:
    """Mescla campos no sidecar existente e devolve o meta novo. None sem sidecar."""
    with _locked(name):
        meta = load(name)
        if meta is None:
            return None
        meta = _merge_conversation(meta, campos)
        _write(name, meta)
        return meta


def update_model(name: str, model: str | None, effort: str | None) -> None:
    """Atualiza SO a escolha de modelo/effort no sidecar existente, preservando thread_id/
    rollout_path/cwd (re-le e regrava via save()). No-op silencioso se o sidecar nao existe
    (nome desconhecido) -- quem chama (CodexAdapter.set_model) ja mantem a copia em memoria."""
    with _locked(name):
        meta = load(name)
        if meta is not None:
            _write(name, {**meta, "model": model, "effort": effort})


def update_service_tier(name: str, thread_id: str, service_tier: str | None) -> bool:
    """Grava a escolha confirmada sem deixar evento de outra conversa sobrescrever o sidecar."""
    with _locked(name):
        meta = load(name)
        if meta is None:
            return True
        if meta.get("thread_id") != thread_id:
            return False
        _write(name, {**meta, "service_tier": service_tier})
        return True


def update_app_pid(name: str, app_pid: int) -> None:
    """O lancador ressubiu o app-server na mesma porta: so o dono muda. No-op sem sidecar."""
    with _locked(name):
        meta = load(name)
        if meta is not None:
            _write(name, {**meta, "app_pid": app_pid})


def load(name: str) -> dict | None:
    """Le o sidecar de uma sessao (ou None se nao existe / corrompido)."""
    try:
        return json.loads(atomico.read(_path(name)))
    except (OSError, ValueError):
        return None


def delete(name: str) -> None:
    """Remove o sidecar (idempotente)."""
    try:
        if _path(name).exists():
            with _locked(name):
                _path(name).unlink(missing_ok=True)
    except OSError:
        pass


def delete_if(name: str, antes=None, **vida) -> bool:
    """Apaga o sidecar só se ainda é da vida esperada (campos iguais); `antes` roda sob a trava."""
    with _locked(name):
        meta = load(name)
        if meta is None or any(meta.get(k) != v for k, v in vida.items()):
            return False
        if antes is not None:
            antes()
        _path(name).unlink(missing_ok=True)
        return True


def list_all(*, include_incomplete: bool = False) -> list[dict]:
    """Sidecars publicados; o registry inclui preparações para mostrar a origem em recuperação."""
    out: list[dict] = []
    try:
        files = sorted(_dir().glob("*.json"))
    except OSError:
        return out
    for f in files:
        try:
            raw = f.read_text(encoding="utf-8")
            meta = json.loads(raw)
        except (OSError, ValueError):
            continue
        if not isinstance(meta, dict):
            continue
        if not include_incomplete:
            # A preparação não é uma sessão Codex publicada e não pode ser aquecida.
            from app.conversation_transfer import TransferPhase, transfer_for_thread, load_transfer
            record = (load_transfer(meta["transfer_id"]) if meta.get("transfer_id") else
                      transfer_for_thread(meta["codex_home"], meta["thread_id"])
                      if meta.get("codex_home") and meta.get("thread_id") else None)
            if meta.get("transfer_id") and record is None:
                raise RuntimeError("sidecar aponta para transferência ausente")
            if record and record.phase != TransferPhase.COMPLETE:
                continue
        out.append(meta)
    return out


def exists(name: str) -> bool:
    return _path(name).exists()


# Onde o bloco que a ponte de skills gerencia COMECA no ~/.codex/config.toml (app/agentes_sync).
# A entrada de confianca entra ANTES dele: o Codex apenda `[hooks.state."..."]` no fim do arquivo,
# e o fim, hoje, esta dentro daquele bloco — escrever la seria pendurar a nossa tabela no meio de
# um trecho que outro codigo reescreve inteiro.
_MARCA_BLOCO = "# >>> hangar:"


def pretrust_cwd(cwd: str, codex_home: str | Path | None = None) -> None:
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

    if codex_home is None:
        cfg = Path.home() / ".codex" / "config.toml"
    else:
        cfg = Path(codex_home).expanduser().absolute() / "config.toml"
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


def prepared_path(transfer_id: str) -> Path:
    import uuid
    return _dir() / "prepared" / f"{uuid.UUID(transfer_id)}.json"


def load_prepared(name: str, transfer_id: str) -> dict | None:
    """Runtime privado da adoção; nunca participa de list_all nem de descoberta."""
    path = prepared_path(transfer_id)
    try:
        meta = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    if not isinstance(meta, dict) or meta.get("name") != name or meta.get("transfer_id") != transfer_id:
        raise ValueError("session_transfer_runtime_identity_changed")
    return meta


def write_prepared(name: str, transfer_id: str, meta: dict) -> None:
    from app.conversation_transfer import _write_json
    if meta.get("name") != name or meta.get("transfer_id") != transfer_id:
        raise ValueError("session_transfer_runtime_identity_changed")
    with _locked(name):
        previous = load_prepared(name, transfer_id)
        if previous and any(previous.get(k) != meta.get(k) for k in ("key", "thread_id", "codex_home")):
            raise ValueError("session_transfer_runtime_identity_changed")
        _write_json(prepared_path(transfer_id), meta)


def transfer_runtime(name: str, transfer_id: str) -> dict | None:
    live = load(name)
    if live and live.get("transfer_id") == transfer_id:
        return live
    return load_prepared(name, transfer_id)


def update_transfer_runtime(name: str, transfer_id: str, **fields) -> dict:
    from app.conversation_transfer import _write_json
    with _locked(name):
        live = load(name)
        meta = live if live and live.get("transfer_id") == transfer_id else load_prepared(name, transfer_id)
        if not meta or any(k in fields and fields[k] != meta.get(k)
                           for k in ("name", "key", "thread_id", "codex_home", "transfer_id")):
            raise ValueError("session_transfer_runtime_identity_changed")
        if "processes" in fields:
            fields["processes"] = {**meta.get("processes", {}), **fields["processes"]}
        updated = {**meta, **fields}
        if meta is live:
            _write(name, updated)
        else:
            _write_json(prepared_path(transfer_id), updated)
        return updated


def publish_prepared(name: str, transfer_id: str) -> dict:
    with _locked(name):
        meta = load_prepared(name, transfer_id)
        if not meta:
            raise ValueError("session_transfer_runtime_missing")
        live = load(name)
        if live and any(live.get(k) != meta.get(k) for k in ("transfer_id", "key", "thread_id")):
            raise ValueError("session_transfer_runtime_identity_changed")
        _write(name, meta)
        return meta
