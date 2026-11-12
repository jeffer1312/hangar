import json

from app import codex_contas


def create_account(name: str) -> codex_contas.Account:
    """Pasta com o marcador e a credencial em arquivo, no formato que o app lê."""
    name = codex_contas._validate_name(name)
    target = codex_contas._account_home(name)
    target.mkdir(mode=0o700)
    (target / codex_contas.MARKER).write_text(
        json.dumps({"version": codex_contas._MARKER_VERSION, "id": name}) + "\n", encoding="utf-8")
    (target / "config.toml").write_text('cli_auth_credentials_store = "file"\n', encoding="utf-8")
    return codex_contas._account(name, target)
