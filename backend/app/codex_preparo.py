"""Reparo pontual dos recursos de uma conta Codex, sem reinstalar a conta."""
import asyncio
import ipaddress
from pathlib import Path
import runpy
import shutil
from urllib.parse import urlparse

from app import account_lifecycle, install_root
from app.codex_arquivos import exclusivo, gravar, json_bytes, ler
from app.codex_compat import adaptar_hooks_plugin, edicoes_confianca_hooks
from app.codex_contas import Account, PREPARED_MARKER, list_accounts, resolve_account
from app.codex_hook_installer import ensure_codex_state_hook_installed
from app.codex_importador import CodexNativo


async def reparar_conta(account: Account, *, adotar_preparo: bool = False) -> bool | None:
    async def owned():
        key = account_lifecycle.AccountKey.new("codex",account.home)
        guard = await asyncio.to_thread(account_lifecycle.acquire,key)
        with guard:
            if resolve_account(account.id).home.resolve() != account.home.resolve():
                raise ValueError("A conta Codex mudou durante o reparo")
            async with exclusivo(account.home/".hangar-integracao.lock"):
                from app.codex_contas_sync import preparation_status
                from app.codex_integracao import IntegracaoCodex
                ready = (IntegracaoCodex(codex_home=account.home).status()["estado"] == "ok"
                         if account.is_default else preparation_status(account)["status"] == "ready")
                pending = await _reparar_conta_locked(account)
                if adotar_preparo and ready and pending is not None:
                    marker = account.home/PREPARED_MARKER
                    gravar(marker,json_bytes({"version":1}),ler(marker))
                return pending
    return await account_lifecycle.complete_on_cancel(owned())


async def _reparar_conta_locked(account: Account) -> bool | None:
    account.home.mkdir(parents=True, exist_ok=True)
    private = Path.home()/".hangar/codex-prepare-once"
    private.mkdir(parents=True, exist_ok=True, mode=0o700)
    hooks = account.home/"hooks.json"
    backup = private/"backups"/account.id/"hooks.json"
    if hooks.is_file() and not backup.exists():
        gravar(backup,hooks.read_bytes(),None)
    await account_lifecycle.complete_on_cancel(asyncio.to_thread(ensure_codex_state_hook_installed,account.home))
    if shutil.which("codex") is None:
        return None
    for root in sorted((account.home/"plugins/cache/ecc/ecc").glob("*/")):
        await account_lifecycle.complete_on_cancel(asyncio.to_thread(
            adaptar_hooks_plugin,root,account.home,private/"backups"/account.id,
            normalizar=lambda value:value,ao_alterar=lambda:None))
    async with CodexNativo(Path.home(),account.home,account=account) as native:
        data = await native.request("hooks/list",{"cwds":[]})
        edits, manual = edicoes_confianca_hooks(data,account.home)
        if edits:
            await native.request("config/batchWrite",{"edits":edits,"reloadUserConfig":True})
    return manual > 0


async def reparar_todas() -> None:
    accounts = [account for account in list_accounts() if account.home.is_dir()]
    for account in accounts:
        await reparar_conta(account,adotar_preparo=True)
    from app.adapters.codex import sessions
    from app.procinfo import pid_vivo
    reload_hooks = runpy.run_path(str(install_root.root()/"scripts/hangar-codex-tui"))["_confiar_hooks"]
    for session in sessions.list_all(include_incomplete=True):
        endpoint, home, pid = session.get("endpoint"),session.get("codex_home"),session.get("app_pid")
        if not isinstance(endpoint,str) or not isinstance(home,str) or not isinstance(pid,int) or not pid_vivo(pid):
            continue
        account = next((account for account in accounts if account.home.resolve()==Path(home).resolve()),None)
        if account is None:
            continue
        host = urlparse(endpoint).hostname
        if host != "localhost" and (not host or not ipaddress.ip_address(host).is_loopback):
            raise ValueError("O app-server da conta não está em loopback")
        confirmed = await account_lifecycle.complete_on_cancel(asyncio.to_thread(
            reload_hooks,endpoint,session.get("cwd") or str(Path.home()),
            recarregar=True,codex_home=account.home))
        if not confirmed:
            raise RuntimeError("O Codex não confirmou a recarga dos hooks reparados")
    proof = Path.home()/".hangar/codex-prepare-once-v1.json"
    gravar(proof,json_bytes({"version":1,"accounts":[account.id for account in accounts]}),ler(proof))
