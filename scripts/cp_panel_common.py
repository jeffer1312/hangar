"""Resolução de servidor/token compartilhada pelos scripts do painel (cp-panel-data,
cp-panel-action). Módulo em vez de copiar: é o ponto que lê CREDENCIAL e monta a URL — duas
cópias divergindo aqui viraria bug de auth silencioso."""
import json
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

BACKEND = Path(__file__).resolve().parent.parent / "backend"
TIMEOUT = 8
# Operação de ESCRITA (commit/push) precisa de folga MAIOR que o timeout do subprocesso git do
# backend (git_ops._TIMEOUT = 20). Com o timeout curto, um push lento (link ruim, diff grande)
# estourava aqui e virava "servidor inacessível" — mentira: o push seguia rodando lá e podia
# até completar, deixando o usuário sem saber se caiu ou não.
WRITE_TIMEOUT = 30


class PanelError(Exception):
    """Erro já legível pro usuário final (sem traceback)."""


def env(key: str) -> str:
    try:
        for line in (BACKEND / ".env").read_text(encoding="utf-8").splitlines():
            if line.startswith(f"{key}="):
                return line.split("=", 1)[1].strip()
    except OSError:
        pass
    return ""


def peers() -> dict:
    """Mapa id -> {base_url, token, web_url?}. Ausente = máquina só-local (não é erro)."""
    try:
        return json.loads((BACKEND / "peers.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as e:
        raise PanelError(f"peers.json inválido: {e}") from e


def local_base() -> str:
    return f"http://127.0.0.1:{env('CP_PORT') or '8765'}"


def resolve(address: str) -> tuple[str, str, str]:
    """'sessao' ou 'servidor::sessao' -> (base_url, token, nome_da_sessao)."""
    if "::" not in address:
        return local_base(), env("CP_AUTH_TOKEN"), address
    server, _, name = address.partition("::")
    known = peers()
    cfg = known.get(server)
    if not cfg:
        raise PanelError(f"servidor '{server}' desconhecido (conhecidos: {', '.join(known) or 'nenhum'})")
    # `or ""`, não `.get(k, "")`: chave presente com valor null (typo comum ao editar à mão)
    # passava batido e estourava AttributeError/KeyError cru lá na frente.
    base, token = (cfg.get("base_url") or ""), (cfg.get("token") or "")
    if not base or not token:
        raise PanelError(f"peers.json: entrada '{server}' sem base_url ou token")
    return base.rstrip("/"), token, name


def api(address: str, method: str, path: str, body: dict | None = None, timeout: int = TIMEOUT):
    """Chama a API do servidor DONO da sessão. `path` usa {name} como placeholder do nome."""
    base, token, name = resolve(address)
    url = base + path.replace("{name}", urllib.parse.quote(name, safe=""))
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
                                 headers={"Authorization": f"Bearer {token}",
                                          "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode()
            try:
                return json.loads(raw) if raw else {}
            except ValueError as e:
                # 200 com corpo não-JSON (proxy devolvendo HTML, backend cuspindo texto):
                # sem isto o ValueError subia cru e quebrava o contrato "sempre imprime JSON".
                raise PanelError(f"resposta não-JSON de {base}: {e}") from e
    except urllib.error.HTTPError as e:
        # detail do FastAPI é a mensagem ÚTIL ("sessao nao encontrada"); sem extrair, o usuário
        # via só "HTTP 404" e não tinha como se corrigir.
        try:
            detail = json.loads(e.read().decode()).get("detail", "")
        except (ValueError, OSError):
            detail = ""
        raise PanelError(f"{e.code}: {detail or e.reason}") from e
    except (urllib.error.URLError, OSError) as e:
        raise PanelError(f"servidor inacessível: {e}") from e
