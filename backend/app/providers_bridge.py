"""Transporte dos consumidores Python para os fatos e a linha Pi que pertencem ao Rust."""
import json
import os
import subprocess

from app import rust_bins


def inbox_call(operation: str, **arguments):
    from app import list_bridge
    return list_bridge._request("providers.inbox", {"op": operation, **arguments}, timeout=6)


def local(command: str, operation: str | None = None, **arguments):
    binary = rust_bins.find_bin("hangar-server", "CP_RUST_SERVER_BIN")
    if binary is None:
        raise RuntimeError("providers_need_rust_server")
    result = subprocess.run(
        [str(binary), command, *([operation] if operation is not None else [])],
        input=json.dumps(arguments, ensure_ascii=False), capture_output=True,
        text=True, encoding="utf-8", errors="replace", timeout=30,
        env={key: value for key, value in os.environ.items()
             if key not in {"HANGAR_INTERNAL_SECRET", "HANGAR_RUNTIME_INSTANCE", "CP_AUTH_TOKEN"}},
    )
    try:
        response = json.loads(result.stdout)
    except ValueError:
        raise RuntimeError("providers_rust_invalid_response") from None
    if result.returncode or response.get("ok") is not True:
        raise ValueError(response.get("error") or "providers_rust_failed")
    return response["result"]
