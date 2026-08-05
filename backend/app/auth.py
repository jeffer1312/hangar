import logging
import secrets
import time

from fastapi import Request, HTTPException
from app.config import settings

_log = logging.getLogger(__name__)

# ── Backoff por origem ───────────────────────────────────────────────────────────────────────
# O token deixou de ser 96 bits sorteados (`openssl rand -hex 24`): o instalador agora ACEITA um
# valor digitado, com piso de 8 caracteres — quem digita no celular nao aguenta 48 hex. Escolhido
# por gente, ele cabe num dicionario; e quem passa daqui roda `claude` COMO o dono, sem sandbox.
# Sem penalidade nenhuma, qualquer aparelho da mesma rede (Wi-Fi de visita, IoT tomada) martela
# `GET /api/sessions?token=...` a vontade.
#
# Desenho: janela deslizante por IP. _MAX_FAILS erros dentro de _WINDOW segundos -> 429 ate os
# erros envelhecerem, e o token NEM E AVALIADO enquanto o bloqueio dura. Recusar a avaliacao e o
# unico freio que funciona: um atraso por requisicao o atacante contorna abrindo conexoes em
# paralelo, e "bloqueia o errado mas ainda aceita o certo" nao bloqueia nada — a diferenca entre
# 200 e 429 ja entrega qual palpite era o bom.
#
# _MAX_FAILS = 8 / _WINDOW = 30s:
#  - 8 fica acima da rajada que o app legitimo consegue emitir com token velho: no PRIMEIRO 401 o
#    front apaga a credencial e recarrega (`ensureOk`, frontend/src/lib/api.ts), entao so contam as
#    poucas chamadas ja em voo (lista + SSE + polls). Com 3 ou 4, uma troca de token viraria tela de
#    erro pro proprio dono; com 20, o atacante ganha uma rajada grande de graca.
#  - 30s e o pior caso de espera de quem errou e agora quer acertar — meio minuto no celular passa.
#    Segura o atacante em 16 tentativas/minuto (~23 mil/dia): mesmo um token fraco de 8 caracteres
#    nao cai em tempo humano, e o log abaixo avisa o dono muito antes. Janela maior nao aperta a
#    seguranca (a taxa e _MAX_FAILS/_WINDOW), so aumenta o castigo de quem digitou errado.
_MAX_FAILS = 8
_WINDOW = 30.0
# Teto de origens rastreadas. O dicionario e alimentado por QUALQUER um da rede — sem teto ele
# mesmo vira o ataque (memoria). 512 e muito mais que uma LAN domestica e custa ~50 KB; ao encher,
# primeiro caem os vencidos e depois as origens com a falha mais antiga.
_MAX_ORIGINS = 512

# ponytail: dict por processo, zera no restart. Nao e limitador distribuido — e um backend so.
_fails: dict[str, list[float]] = {}

# Loopback e o proprio dono na maquina: cp-send, cp-panel e os scripts locais batem aqui o tempo
# todo, e quem esta logado ali le o backend/.env sem esforco nenhum — o token nao defende disso.
# Bloquear loopback so quebraria a ferramenta local. Atras de proxy (tailscale serve, traefik) o
# IP real chega via X-Forwarded-For, que o uvicorn resolve por CP_FORWARDED_ALLOW_IPS.
_LOOPBACK = {"127.0.0.1", "::1", "localhost"}


def _blocked(ip: str, now: float) -> bool:
    hits = _fails.get(ip)
    if not hits:
        return False
    fresh = [t for t in hits if now - t < _WINDOW]
    if fresh:
        _fails[ip] = fresh
    else:
        _fails.pop(ip, None)
    return len(fresh) >= _MAX_FAILS


def _evict(now: float) -> None:
    for ip in [i for i, hits in _fails.items() if not hits or now - hits[-1] >= _WINDOW]:
        _fails.pop(ip, None)
    while len(_fails) > _MAX_ORIGINS:
        _fails.pop(min(_fails, key=lambda i: _fails[i][-1]))


def _record_fail(ip: str, now: float) -> None:
    hits = _fails.setdefault(ip, [])
    if len(hits) >= _MAX_FAILS:
        return  # ja bloqueado: nao guarda mais nada (limita a memoria por origem)
    hits.append(now)
    if len(hits) == _MAX_FAILS:
        # Log SO na virada pro bloqueio: a falha aparece pro dono sem virar uma linha por tentativa.
        _log.warning("[auth] %s errou o token %d vezes em %.0fs — bloqueado por %.0fs",
                     ip, _MAX_FAILS, _WINDOW, _WINDOW)
    if len(_fails) > _MAX_ORIGINS:
        _evict(now)


def reset_backoff() -> None:
    """Zera o estado do backoff (usado pelos testes pra nao vazar contagem entre casos)."""
    _fails.clear()


def require_auth(request: Request) -> None:
    auth = request.headers.get("Authorization", "")
    if auth.startswith("Bearer "):
        token = auth[7:]
    else:
        # A SSE (EventSource) nao consegue mandar header Authorization; cross-origin (multi-PC) o
        # cookie tb nao vai (SameSite) -> sobra o ?token= na URL. Aceitar a query e o que faltava
        # (era 401 em /events?token=...). Ordem: header -> query -> cookie (same-origin).
        token = request.query_params.get("token") or request.cookies.get("cp_token")
    ip = request.client.host if request.client else "?"
    local = ip in _LOOPBACK
    now = time.time()
    if not local and _blocked(ip, now):
        # 429, nunca 401: o front trata 401 como "token expirou", apaga a credencial salva e
        # recarrega pro login (`ensureOk`). Se o bloqueio respondesse 401, a tentativa de OUTRO
        # aparelho na mesma origem deslogaria o dono. 429 sobe como erro comum, sem mexer no token.
        raise HTTPException(status_code=429, detail="muitas tentativas — aguarde",
                            headers={"Retry-After": str(int(_WINDOW))})
    # compare_digest em bytes: `!=` de string sai fora na primeira letra diferente (canal lateral de
    # tempo) e o encode ainda evita o TypeError do compare_digest com string nao-ASCII.
    if not secrets.compare_digest((token or "").encode(), settings.auth_token.encode()):
        if not local:
            _record_fail(ip, now)
        raise HTTPException(status_code=401, detail="unauthorized")
    _fails.pop(ip, None)  # acerto limpa a origem na hora


def require_loopback(request: Request) -> None:
    """So responde pra quem esta na propria maquina. Usado pela paleta do desktop, que nao faz
    sentido no celular: sem isto o iPhone receberia 200 e pintaria com as cores do notebook.

    Nao e defesa contra atacante — a rota ja tem `require_auth` e o segredo e a cor de um papel de
    parede. E produto: um "nao" claro em vez de um sim errado.

    Sem `request.client` a resposta e NAO. Cuidado herdado do uvicorn: com `proxy_headers=True`
    (main.py) e `CP_FORWARDED_ALLOW_IPS` alargado, um proxy pode reescrever `request.client.host` —
    com o padrao (`127.0.0.1`, config.py:133) so um proxy da propria maquina consegue isso.
    """
    ip = request.client.host if request.client else None
    if ip not in _LOOPBACK:
        raise HTTPException(status_code=403, detail="so na maquina do backend")
