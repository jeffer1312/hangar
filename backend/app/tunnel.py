import json
import subprocess

# Preview de projeto local via `tailscale serve`. Sobe um proxy HTTPS numa porta-slot da tailnet
# apontando pra localhost:<port> do projeto que o Claude roda NESTA maquina -> o app embute a URL
# num iframe e voce ve o projeto rodando no celular/desktop. Ganhos vs abrir a porta crua: fica
# HTTPS (o app e HTTPS -> sem mixed-content) e alcancavel por qualquer device da tailnet, sem expor
# a porta na LAN. Usa uma porta-slot PROPRIA (10000), nunca a 443 do front -> zero risco de derrubar
# o app. Tudo via argv list -> sem shell, sem injecao (a porta ainda e validada como int).
_TIMEOUT = 20
_SLOT = 10000  # porta HTTPS da tailnet pro preview (Serve so aceita 443/8443/10000; 443 = front).


class TunnelError(Exception):
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def _run(*args: str) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(
            ["tailscale", *args],
            capture_output=True, text=True, timeout=_TIMEOUT,
        )
    except FileNotFoundError:
        raise TunnelError(500, "tailscale nao encontrado")
    except subprocess.TimeoutExpired:
        raise TunnelError(504, "tailscale timeout")
    except OSError as e:
        raise TunnelError(500, f"tailscale falhou: {e}")


def _dns_name() -> str:
    """Hostname .ts.net desta maquina (tira o trailing dot). Erro limpo se tailscale nao logado."""
    p = _run("status", "--json")
    if p.returncode != 0:
        raise TunnelError(503, (p.stderr or "tailscale status falhou").strip() or "tailscale status falhou")
    try:
        name = json.loads(p.stdout)["Self"]["DNSName"].rstrip(".")
    except (ValueError, KeyError, AttributeError, TypeError):
        raise TunnelError(503, "tailscale sem DNSName (logado?)")
    if not name:
        raise TunnelError(503, "tailscale sem DNSName (logado?)")
    return name


def _url(name: str) -> str:
    return f"https://{name}:{_SLOT}/"


def _port_from_proxy(proxy: str) -> int | None:
    # "http://localhost:9999" -> 9999. Best-effort: so pro app mostrar qual porta esta no ar.
    try:
        return int(proxy.rsplit(":", 1)[1].split("/", 1)[0])
    except (IndexError, ValueError):
        return None


def status() -> dict:
    """O que o slot de preview serve agora: {active, port, url}. Le sem sudo; sem config -> inativo."""
    p = _run("serve", "status", "--json")
    if p.returncode != 0 or not p.stdout.strip():
        return {"active": False, "port": None, "url": None}
    try:
        web = json.loads(p.stdout).get("Web") or {}
    except ValueError:
        return {"active": False, "port": None, "url": None}
    for host, cfg in web.items():
        if not host.endswith(f":{_SLOT}"):
            continue
        proxy = (((cfg.get("Handlers") or {}).get("/") or {}).get("Proxy")) or ""
        name = host.rsplit(":", 1)[0]
        return {"active": True, "port": _port_from_proxy(proxy), "url": _url(name)}
    return {"active": False, "port": None, "url": None}


def start(port: int) -> dict:
    """Sobe (ou reaponta, ao trocar de projeto) o proxy HTTPS do slot -> localhost:<port>.
    Retorna {url, port}. Requer operator setado; sem ele o serve nega -> 403 com a dica do fix."""
    if not (1 <= port <= 65535):
        raise TunnelError(422, "porta invalida")
    name = _dns_name()  # valida tailscale logado ANTES de mexer no serve
    p = _run("serve", "--bg", f"--https={_SLOT}", f"localhost:{port}")
    out = p.stdout + p.stderr
    if p.returncode != 0:
        if "Access denied" in out or "operator" in out:
            raise TunnelError(403, "backend sem permissao pro tailscale serve "
                                   "(rode uma vez: sudo tailscale set --operator=$USER)")
        raise TunnelError(502, (p.stderr or "tailscale serve falhou").strip() or "tailscale serve falhou")
    return {"url": _url(name), "port": port}


def stop() -> dict:
    """Derruba SO o proxy do slot (nao toca nos outros, ex: o front na 443). Idempotente:
    'handler does not exist' = ja estava desligado -> sucesso."""
    p = _run("serve", f"--https={_SLOT}", "off")
    out = p.stdout + p.stderr
    if p.returncode != 0 and "does not exist" not in out:
        raise TunnelError(502, (p.stderr or "falha ao desligar").strip() or "falha ao desligar")
    return {"active": False, "port": None, "url": None}


if __name__ == "__main__":
    # Self-check: sobe um dummy no slot, confere status, derruba, confere. Roda DE VERDADE nesta
    # maquina (precisa tailscale logado + operator) e sobrescreve o slot 10000 -> um preview em uso
    # cai durante o teste. Sem tailscale/login -> pula com aviso.
    try:
        _dns_name()
    except TunnelError as e:
        print(f"tunnel self-check PULADO ({e.detail})")
        raise SystemExit(0)

    for bad in (0, 70000, -1):
        try:
            start(bad)
            raise AssertionError(f"deveria rejeitar porta {bad}")
        except TunnelError as e:
            assert e.status == 422, e.status

    DUMMY = 59999
    r = start(DUMMY)
    assert r["url"].endswith(f":{_SLOT}/"), r
    assert r["port"] == DUMMY, r
    s = status()
    assert s["active"] is True and s["port"] == DUMMY, s
    stop()
    assert status()["active"] is False, "slot devia estar inativo pos-stop"
    stop()  # idempotente: segundo off nao pode levantar
    print("tunnel self-check OK")
