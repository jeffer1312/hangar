"""Convidado decidido pelo Rust: chega num cabeçalho assinado, nunca pelo token de convite."""
import base64
import binascii
import hashlib
import hmac
import json
import time

from app import internal_api

HEADER = b"x-hangar-guest"
# Conferido uma vez por pedido: o porteiro confere e as rotas leem o mesmo resultado do scope.
_SCOPE_KEY = "hangar.guest"


class GuestView:
    """Sessões e níveis que o Rust conferiu para este pedido."""

    def __init__(self, sessions: dict[str, str]):
        self.sessions = sessions

    def sees(self, name: str | None) -> bool:
        return name in self.sessions

    def kind_of(self, name: str | None) -> str | None:
        return self.sessions.get(name)


def guest_of(obj) -> GuestView | None:
    """Convidado deste pedido (scope, Request ou WebSocket), ou None para o dono."""
    scope = obj if isinstance(obj, dict) else getattr(obj, "scope", None)
    if not scope:
        return None
    if _SCOPE_KEY not in scope:
        scope[_SCOPE_KEY] = _verify(scope)
    return scope[_SCOPE_KEY]


def _verify(scope: dict) -> GuestView | None:
    secret = internal_api._secret
    if secret is None or (scope.get("client") or ("",))[0] not in internal_api._LOOPBACK:
        return None
    value = next((v for k, v in scope.get("headers") or [] if k == HEADER), None)
    raw_path = scope.get("raw_path")
    if value is None or raw_path is None:
        return None
    payload, _, mac = value.partition(b".")
    # Aperto de mão do WebSocket não tem método no scope: é um GET.
    method = scope.get("method", "GET").encode()
    expected = hmac.new(secret.encode(), method + b"\n" + raw_path + b"\n" + payload, hashlib.sha256).hexdigest()
    if not hmac.compare_digest(mac, expected.encode()):
        return None
    try:
        data = json.loads(base64.urlsafe_b64decode(payload + b"=" * (-len(payload) % 4)))
        sessions, exp = data["s"], data["exp"]
    except (binascii.Error, ValueError, KeyError, TypeError):
        return None
    if not isinstance(sessions, dict) or not isinstance(exp, int) or exp < time.time():
        return None
    return GuestView({str(n): str(k) for n, k in sessions.items()})
