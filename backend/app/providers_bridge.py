import json
import os
import subprocess

from app import rust_bins




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
