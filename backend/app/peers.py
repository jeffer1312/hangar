"""Resolução de servidores peer + chamadas outbound pro backend do outro server. peers.json
(backend/peers.json, gitignored — MESMO arquivo que o cp-send lê) mapeia server_id ->
{base_url, token}. Só o pareamento cross-server usa isto; recado 1:1 e --list seguem no cp-send
(bash). Sem httpx no hot path: urllib da stdlib (mesmo padrão de transcribe.py), chamado numa thread
pelo caller async — os POSTs de pareamento são ação de usuário, não hot path.

ponytail: só o que o pareamento precisa — resolver base/token e um POST/DELETE com erro claro.
Grupo cross-server de N não existe (o pareamento cross-server é 1:1); quando existir, isto não muda."""
import http.client
import json
import logging
import urllib.error
import urllib.request
from pathlib import Path

_log = logging.getLogger("claude_pocket")

# Ao lado de backend/.env e backend/peers.json (app/ -> backend/). Robusto a qual é o cwd do backend.
_PEERS_FILE = Path(__file__).resolve().parent.parent / "peers.json"


class PeerError(Exception):
    """Falha de transporte/HTTP falando com um peer — mensagem já legível pro usuário/Claude.

    transport=True: falha de REDE (URLError/timeout/corpo ilegível) — a requisição PODE ter chegado
    e sido processada no peer, só a resposta se perdeu; o estado remoto fica INCERTO (o caller
    compensa). transport=False: o peer respondeu !2xx (rejeitou limpo, não comitou)."""

    def __init__(self, msg: str, transport: bool = False):
        super().__init__(msg)
        self.transport = transport


def is_remote(name: str) -> bool:
    """Nome qualificado 'srv::sessao' (peer em OUTRA máquina) vs sessão local (nome cru)."""
    return "::" in name


def split_addr(qualified: str) -> tuple[str, str]:
    """'srv::sessao' -> ('srv', 'sessao'). Split no PRIMEIRO '::' (nome de sessão não tem '::')."""
    srv, _, sess = qualified.partition("::")
    return srv, sess


def _load() -> dict:
    try:
        data = json.loads(_PEERS_FILE.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}   # esperado: sem peers.json = cross-server desligado, não é erro
    except (OSError, json.JSONDecodeError, ValueError) as e:
        # Arquivo existe mas está ilegível/malformado: BUG de config, não "desligado". Sem este log
        # (journalctl --user -u claude-cockpit-backend) uma vírgula sobrando no peers.json fazia
        # TODOS os peers falharem com "não cadastrado" e o usuário caçava o problema errado.
        _log.warning("peers.json ilegível/malformado (%s): %r", _PEERS_FILE, e)
        return {}
    if not isinstance(data, dict):
        _log.warning("peers.json não é um objeto JSON (%s)", _PEERS_FILE)
        return {}
    return data


def peer_cfg(server_id: str) -> tuple[str, str] | None:
    """(base_url sem barra final, token) do peer, ou None se ausente/inválido no peers.json."""
    p = _load().get(server_id)
    if not isinstance(p, dict) or not p.get("base_url") or not p.get("token"):
        return None
    return p["base_url"].rstrip("/"), p["token"]


def call(server_id: str, method: str, path: str, body: dict | None = None, timeout: int = 8):
    """POST/DELETE num backend peer. Devolve (status, json|None). Levanta PeerError em qualquer
    falha (peer desconhecido, inacessível, ou !2xx) com o detail real do backend remoto — sem isto,
    quem chama (o iniciador) não teria como reportar por que o pareamento não fechou."""
    cfg = peer_cfg(server_id)
    if not cfg:
        raise PeerError(f"servidor '{server_id}' não está em peers.json (ou sem base_url/token)")
    base, token = cfg
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        base + path, data=data, method=method,
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status = r.status
            raw = r.read().decode()
    except urllib.error.HTTPError as e:
        # Peer respondeu !2xx — rejeitou de forma limpa, NÃO comitou. transport=False.
        raw = e.read().decode(errors="replace")
        try:
            detail = json.loads(raw).get("detail", raw)
        except (json.JSONDecodeError, ValueError, AttributeError):
            detail = raw
        raise PeerError(f"{server_id} respondeu HTTP {e.code}: {detail}", transport=False)
    except (urllib.error.URLError, http.client.IncompleteRead, OSError, TimeoutError) as e:
        # Falha de rede/leitura truncada — pode ter chegado no peer. Estado remoto INCERTO.
        raise PeerError(f"{server_id} inacessível: {e}", transport=True)
    try:
        return status, (json.loads(raw) if raw.strip() else None)
    except (json.JSONDecodeError, ValueError) as e:
        # 2xx com corpo ilegível (proxy retornando HTML em 200, leitura truncada): sem este catch a
        # exceção escapava crua, não virava PeerError, e o rollback do caller (que só pega PeerError)
        # NUNCA rodava — sidecar local comitava com estado remoto incerto. transport=True.
        raise PeerError(f"{server_id} respondeu corpo ilegível: {e}", transport=True)
