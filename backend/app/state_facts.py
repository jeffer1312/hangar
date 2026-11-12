"""Só referência: o `Monitor` do Rust lê plugin, modo e vida no próprio Store e a rota do retrato
saiu, então nenhum interesse é registrado e `notify` não envia nada. Eram os fatos do estado de uma
sessão Claude com terminal que só o Python sabia (plugin, troca, operação de permissão controlada);
o empurrão ia pela ponte privada da lista só para sessão com interesse e só quando um valor mudava.
Instante monotônico daqui não vale no Rust: tudo que vence ia como idade em ms."""
import logging
import threading
import time

from app import diag

_log = logging.getLogger("hangar.state_facts")

# O Rust relê o retrato antes disso; sem releitura o interesse vence e os envios param.
INTEREST_S = 30.0
# Renovação sem mudança de valor (batida do /ask): no máximo um envio por sessão neste prazo.
REFRESH_S = 25.0
# O que cada aviso pede: só mudança de valor, renovação limitada ou envio certo.
CHANGE, REFRESH, FORCE = 0, 1, 2

_lock = threading.Lock()
_wake = threading.Condition(_lock)
_interest: dict[str, float] = {}
_seq: dict[str, int] = {}
# Último envio que chegou: (chave dos valores, quando).
_sent: dict[str, tuple[tuple, float]] = {}
# Sessão → o maior pedido pendente; o worker junta os avisos que chegam enquanto envia.
_pending: dict[str, int] = {}
_worker: threading.Thread | None = None
# Rust fora: o diário recebe a falha uma vez por queda, não uma por envio.
_down = False
# Retrato e envio montam e numeram sob a mesma trava: sequência maior é sempre fato mais novo.
_build = threading.Lock()
_threaded = True
_clock = time.monotonic


def _facts(name: str) -> dict:
    from app import permission_mode, plugin_bridge
    from app.adapters.claude_headless import sessions
    out = plugin_bridge.plugin_facts(name)
    out["in_transfer_ms"] = sessions.troca_restante_ms(name)
    out["permission_op"] = permission_mode.operacao_em_curso(name)
    return out


def _key(facts: dict) -> tuple:
    """Os valores sem as idades, que mudam sozinhas."""
    plugin = facts["plugin_state"]
    question = facts["question"]
    return (None if plugin is None else (plugin["state"], plugin["reason"]),
            facts["waiter_open"],
            None if question is None else (question["id"], repr(question["questions"]), question["tool"],
                                           question["resumo"]),
            facts["suggestion"], facts["body_columns"], facts["band_anchor"], facts["in_transfer_ms"] > 0,
            facts["permission_op"])


def snapshot(name: str) -> dict:
    """Retrato inteiro, com a sequência do último envio; registra (ou renova) o interesse."""
    with _lock:
        now = _clock()
        _interest[name] = now + INTEREST_S
        for gone in [n for n, until in _interest.items() if until <= now]:
            del _interest[gone]
    _ensure_worker()
    with _build:
        seq = _seq.get(name, 0)
        facts = _facts(name)
    return {**facts, "seq": seq}


def notify(name: str, mode: int = CHANGE) -> None:
    """Um fato desta sessão pode ter mudado. `REFRESH`: renovação sem mudança de valor (batida do
    /ask), no máximo uma vez por `REFRESH_S`; `FORCE`: anúncio do plugin, que renova a validade do
    estado no Rust mesmo repetido. Não bloqueia: quem envia é o worker."""
    with _lock:
        until = _interest.get(name)
        if until is None or until <= _clock():
            return
        _pending[name] = max(_pending.get(name, CHANGE), mode)
        _wake.notify()


def flush() -> None:
    """Envia o que está pendente. Chamado pelo worker; nos testes, direto."""
    with _lock:
        pending = dict(_pending)
        _pending.clear()
    for name, mode in pending.items():
        try:
            _push(name, mode)
        except Exception as exc:
            # Ler os fatos falhou: o worker segue vivo para as outras sessões, e o Rust relê o retrato.
            _log.exception("state facts build failed session=%s", name)
            diag.registrar("estado.fatos_leitura", "erro", sessao=name, codigo=type(exc).__name__)


def _push(name: str, mode: int) -> None:
    global _down
    with _build:
        facts = _facts(name)
        key = _key(facts)
        with _lock:
            now = _clock()
            until = _interest.get(name)
            last = _sent.get(name)
            due = (mode == FORCE or last is None or last[0] != key
                   or mode == REFRESH and now - last[1] >= REFRESH_S)
            if until is None or until <= now or not due:
                return
            seq = _seq[name] = _seq.get(name, 0) + 1
    try:
        _send(name, {**facts, "seq": seq})
    except Exception as exc:
        # Sequência gasta sem chegar: o Rust vê o pulo e relê o retrato.
        code = getattr(exc, "code", None) or type(exc).__name__
        with _lock:
            first = not _down
            _down = True
        if first:
            _log.warning("state facts push failed session=%s code=%s", name, code)
            diag.registrar("estado.fatos_envio", "aviso", sessao=name, codigo=code)
        return
    with _lock:
        _sent[name] = (key, now)
        _down = False


def _send(name: str, body: dict) -> None:
    from app import list_bridge
    list_bridge.push_state_facts(name, body)


def _run() -> None:
    while True:
        with _lock:
            while not _pending:
                _wake.wait()
        flush()


def _ensure_worker() -> None:
    global _worker
    if not _threaded:
        return
    with _lock:
        if _worker is not None:
            return
        _worker = threading.Thread(target=_run, name="state-facts", daemon=True)
    _worker.start()


def reset() -> None:
    """Para os testes."""
    global _down
    with _lock:
        _interest.clear()
        _seq.clear()
        _sent.clear()
        _pending.clear()
        _down = False
