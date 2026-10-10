"""Apoio dos testes: cadastro e preparo de conta Codex como o Rust faz, para montar o cenário.

O app não cadastra nem prepara conta por conta própria (é do Rust); os testes que precisam de uma
conta adicional no disco ou de um preparo completo usam estas duas.
"""
import json

from app import codex_contas, codex_contas_sync
from app.account_lifecycle import AccountKey, acquire, complete_on_cancel


def create_account(name: str) -> codex_contas.Account:
    """Pasta com o marcador e a credencial em arquivo, no formato que o app lê."""
    name = codex_contas._validate_name(name)
    target = codex_contas._account_home(name)
    target.mkdir(mode=0o700)
    (target / codex_contas.MARKER).write_text(
        json.dumps({"version": codex_contas._MARKER_VERSION, "id": name}) + "\n", encoding="utf-8")
    (target / "config.toml").write_text('cli_auth_credentials_store = "file"\n', encoding="utf-8")
    return codex_contas._account(name, target)


async def prepare_account(account: codex_contas.Account, force: bool = False) -> dict:
    """Preparo sob a guarda da conta, como o Rust pede pela ponte."""
    import asyncio

    async def prepare_owned():
        guard = await asyncio.to_thread(acquire, AccountKey.new("codex", account.home))
        with guard:
            return await codex_contas_sync._prepare_account_guarded(account, force)
    return await complete_on_cancel(prepare_owned())
