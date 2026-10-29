"""Transporte JSON dos catálogos Rust; compartilha a conexão privada do servidor."""
import http.client
import json
import urllib.error
import urllib.request

from app import diag, runtime_coordinator, transcription_bridge
from app.transcription_bridge import BridgeError


def request(operation: str, payload: dict) -> dict:
    config = transcription_bridge._config
    coordinator = runtime_coordinator.current()
    if config is None and coordinator is not None and coordinator.mode == "pending":
        # Queda curta do Rust: espera o filho novo como a transcrição, sem falhar na hora.
        transcription_bridge._ready.wait(runtime_coordinator.PENDING_WAIT_S)
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
            with transcription_bridge._opener.open(call, timeout=60) as response:
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
    except (OSError, ValueError, TypeError, http.client.HTTPException) as error:
        # Só classes: a mensagem pode trazer trecho do corpo, e o diário não leva texto ditado.
        # O URLError embrulha recusa e timeout de conexão; a causa interna separa os dois.
        reason = getattr(error, "reason", None)
        diag.registrar("dictation.bridge_failed", "aviso", codigo="dictation_rust_unavailable",
                       etapa=operation, causa=type(error).__name__,
                       causa_interna=type(reason).__name__ if isinstance(reason, BaseException) else None)
        raise BridgeError(503, "dictation_rust_unavailable", "Não foi possível confirmar a organização no Rust; a transcrição foi preservada.") from None
