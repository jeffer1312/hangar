"""Transporte dos consumidores Python para os fatos e a linha Pi que pertencem ao Rust."""
import json
import os
import subprocess

from app import rust_bins


# Sem o Rust a linha da extensão caiu junto: sem linha digita as teclas, entrega incerta fica na fila.
_WITHOUT_RUST = {"line": {"line": None}, "question": {"value": None}, "deliver": {"disposition": "deferred"}}


def _rust_down(error) -> bool:
    reason = getattr(error.__cause__, "reason", error.__cause__)
    return error.code == "list_bridge_off" or isinstance(reason, ConnectionRefusedError)


def inbox_call(operation: str, **arguments):
    from app import list_bridge
    try:
        return list_bridge._request("providers.inbox", {"op": operation, **arguments}, timeout=10)
    except list_bridge.ListBridgeError as error:
        # Com o Rust de pé, prazo estourado não prova que não há linha: digitar duplicaria a entrega.
        if operation == "deliver" or _rust_down(error):
            return _WITHOUT_RUST[operation]
        raise


def local(command: str, operation: str | None = None, **arguments):
    binary = rust_bins.find_bin("hangar-server", "CP_RUST_SERVER_BIN")
    if binary is None:
        raise RuntimeError("providers_need_rust_server")
    result = subprocess.run(
        [str(binary), command, *([operation] if operation is not None else [])],
        input=json.dumps(arguments, ensure_ascii=False), capture_output=True,
        text=True, encoding="utf-8", errors="replace", timeout=30,
        env={key: value for key, value in os.environ.items()
             if key not in {"HANGAR_INTERNAL_SECRET", "HANGAR_RUNTIME_INSTANCE", "HANGAR_AUTH_CONTEXT", "CP_AUTH_TOKEN"}},
    )
    try:
        response = json.loads(result.stdout)
    except ValueError:
        raise RuntimeError("providers_rust_invalid_response") from None
    if not isinstance(response, dict):
        raise RuntimeError("providers_rust_invalid_response")
    if result.returncode or response.get("ok") is not True:
        raise ValueError(response.get("error") or "providers_rust_failed")
    if "result" not in response:
        raise RuntimeError("providers_rust_invalid_response")
    return response["result"]
