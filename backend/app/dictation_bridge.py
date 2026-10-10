"""Transporte JSON para organização e catálogos Rust; compartilha a conexão privada do STT."""
import http.client
import json
import urllib.error
import urllib.request

from app import diag, transcription_bridge
from app.transcription_bridge import BridgeError


def request(operation: str, payload: dict) -> dict:
    config = transcription_bridge._config
    if config is None:
        raise BridgeError(503, "dictation_rust_unavailable", "O servidor Rust de organização está indisponível.")
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    if len(body) > 1024 * 1024:
        raise BridgeError(413, "dictation_text_too_large", "A transcrição excede o limite de organização.")
    call = urllib.request.Request(f"http://{config[0]}/__hangar_server/dictation/{operation}",
                                  data=body, method="POST", headers={"x-hangar-internal": config[1], "content-type": "application/json"})
    try:
        try:
            with transcription_bridge._opener.open(call, timeout=210 if operation == "organize" else 60) as response:
                body = response.read(4 * 1024 * 1024 + 1)
        except urllib.error.HTTPError as error:
            body = error.read(4 * 1024 * 1024 + 1)
        if len(body) > 4 * 1024 * 1024:
            raise ValueError("resposta grande demais")
        value = json.loads(body)
        if not isinstance(value, dict) or type(value.get("ok")) is not bool:
            raise ValueError("resposta inválida")
        if not value["ok"]:
            error = value.get("error")
            if not isinstance(error, dict) or type(error.get("status")) is not int or not 400 <= error["status"] <= 599:
                raise ValueError("erro inválido")
            raise BridgeError(error["status"], str(error.get("code") or "dictation_organization_failed"),
                              str(error.get("detail") or "Não foi possível confirmar a organização."))
        result = value.get("result")
        if not isinstance(result, dict):
            raise ValueError("resultado inválido")
        return result
    except BridgeError:
        raise
    except (OSError, ValueError, TypeError, http.client.HTTPException):
        diag.registrar("dictation.bridge_failed", "aviso", codigo="dictation_rust_unavailable")
        raise BridgeError(503, "dictation_rust_unavailable", "Não foi possível confirmar a organização no Rust; a transcrição foi preservada.") from None


def organize(raw: str, style: str | None = None, *, mode: str | None = None, session: str | None = None,
             generation: str | None = None, model: str | None = None, account: str | None = None, harness_allowed: bool = True,
             include_recent_messages: bool | None = None, recent_messages: list[dict] | None = None) -> dict:
    payload = {"raw": raw, "style": style, "mode": mode, "session": session,
               "generation": generation, "model": model, "account": account, "harness_allowed": harness_allowed,
               "include_recent_messages": include_recent_messages, "recent_messages": recent_messages}
    try:
        result = request("organize", payload)
        if not isinstance(result.get("text"), str) or result.get("raw") != raw:
            raise BridgeError(503, "dictation_rust_unavailable", "A resposta de organização não corresponde à transcrição; foi mantido o original.")
        return result
    except BridgeError as error:
        # O transporte pode cair depois do STT: o texto já obtido nunca vira um erro sem conteúdo.
        return {"text": raw, "raw": raw, "aviso": error.detail, "estilo_aplicado": "cru",
                "organization_mode": mode, "organization_code": error.code}
