import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import pytest


@pytest.mark.parametrize("stage,expected", [("etapa_sem_claude","ready"),("etapa_sem_codex","partial")])
async def test_primeiro_preparo_aceita_fonte_claude_ausente_mas_recusa_codex_ausente(tmp_path,monkeypatch,stage,expected):
    from types import SimpleNamespace
    from app import account_bridge, account_lifecycle, api, codex_contas, codex_preparo, runtime_coordinator
    config = tmp_path/".codex"
    config.mkdir()
    monkeypatch.setenv("HOME",str(tmp_path))
    monkeypatch.setenv("USERPROFILE",str(tmp_path))
    monkeypatch.setattr(codex_contas,"_DEFAULT_HOME",config)
    monkeypatch.setattr(runtime_coordinator,"current",lambda:SimpleNamespace(instance="synthetic"))
    monkeypatch.setattr(codex_preparo.shutil,"which",lambda _:"/fake/codex")
    class Native:
        def __init__(self,*args,**kwargs):pass
        async def __aenter__(self):return self
        async def __aexit__(self,*args):pass
        async def request(self,*args):return {"data":[]}
    monkeypatch.setattr(codex_preparo,"CodexNativo",Native)
    async def source(_force):
        return {"estado":"indisponivel","etapa":{"codigo":stage}}
    monkeypatch.setattr(api.app.state,"codex_contas_login",SimpleNamespace(atualizar_principal=source),raising=False)
    key = account_lifecycle.AccountKey.new("codex",config)
    body = {"operation":"1"*32,"instance":"synthetic","key":{"provider":"codex","canonical_home":str(config)},
            "account_id":"default","seed":False,"force":False,"cwd":None}
    record = account_lifecycle.default_lock_root()/(key.digest+".prepare.json")
    record.parent.mkdir(parents=True)
    record.write_text(json.dumps(body))
    jobs = account_bridge.PreparationJobs()
    await jobs.start(body)
    try:
        result = await asyncio.wait_for(jobs.jobs[body["operation"]][1],5)
    finally:
        await asyncio.gather(*(job[1] for job in jobs.jobs.values()),return_exceptions=True)
    assert result["status"] == expected,result
    assert (config/"hooks.json").exists() == (expected == "ready")


async def test_reparo_mantem_travas_ate_terminar_mesmo_cancelado(tmp_path,monkeypatch):
    from app import account_lifecycle, codex_contas, codex_preparo
    from app.codex_arquivos import exclusivo
    config = tmp_path/".codex"
    config.mkdir()
    monkeypatch.setenv("HOME",str(tmp_path))
    monkeypatch.setenv("USERPROFILE",str(tmp_path))
    monkeypatch.setattr(codex_contas,"_DEFAULT_HOME",config)
    monkeypatch.setattr(codex_preparo.shutil,"which",lambda _:"/fake/codex")
    entered,release = asyncio.Event(),asyncio.Event()
    class Native:
        def __init__(self,*args,**kwargs):pass
        async def __aenter__(self):return self
        async def __aexit__(self,*args):pass
        async def request(self,*args):
            entered.set()
            await release.wait()
            return {"data":[]}
    monkeypatch.setattr(codex_preparo,"CodexNativo",Native)
    task = asyncio.create_task(codex_preparo.reparar_conta(codex_contas.resolve_account("default")))
    try:
        await asyncio.wait_for(entered.wait(),5)
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done()
        key = account_lifecycle.AccountKey.new("codex",config)
        with pytest.raises(account_lifecycle.AccountLockError,match="account_busy"):
            account_lifecycle.acquire(key,account_lifecycle.GuardMode.EXCLUSIVE,deadline=time.monotonic())
        async def competing_sync():
            async with exclusivo(config/".hangar-integracao.lock"):
                pass
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(competing_sync(),0.15)
    finally:
        release.set()
        await asyncio.gather(task,return_exceptions=True)
    assert task.cancelled()
    with account_lifecycle.acquire(key,account_lifecycle.GuardMode.EXCLUSIVE):
        pass


@pytest.mark.parametrize("state", [None,"parcial","ok"])
async def test_reparo_adota_so_conta_que_ja_estava_pronta(tmp_path,monkeypatch,state):
    from app import codex_contas, codex_preparo
    from app.codex_integracao import IntegracaoCodex
    config = tmp_path/".codex"
    config.mkdir()
    monkeypatch.setenv("HOME",str(tmp_path))
    monkeypatch.setenv("USERPROFILE",str(tmp_path))
    monkeypatch.setenv("CODEX_HOME",str(config))
    monkeypatch.setattr(codex_contas,"_DEFAULT_HOME",config)
    monkeypatch.setattr(codex_preparo.shutil,"which",lambda _:"/fake/codex")
    class Native:
        def __init__(self,*args,**kwargs):pass
        async def __aenter__(self):return self
        async def __aexit__(self,*args):pass
        async def request(self,*args):return {"data":[]}
    monkeypatch.setattr(codex_preparo,"CodexNativo",Native)
    if state:
        saved = IntegracaoCodex(tmp_path,config).raiz/"estado.json"
        saved.parent.mkdir(parents=True)
        saved.write_text(json.dumps({"status":{"estado":state}}))
    await codex_preparo.reparar_todas()
    marker = config/".hangar-prepared-v1"
    assert marker.exists() == (state == "ok")
    before = (config/"hooks.json").read_bytes()
    await codex_preparo.reparar_todas()
    assert marker.exists() == (state == "ok")
    assert (config/"hooks.json").read_bytes() == before


def test_reparo_de_maquina_conclui_sem_codex_e_nao_marca_conta_pronta(tmp_path):
    config = tmp_path / ".codex"
    config.mkdir()
    script = Path(__file__).resolve().parents[2]/"scripts/repair-codex-accounts.py"
    bin_dir = tmp_path/"bin"
    bin_dir.mkdir()
    env = {**os.environ,"HOME":str(tmp_path),"USERPROFILE":str(tmp_path),"CODEX_HOME":str(config),
           "PATH":str(bin_dir),"GIT_CONFIG_GLOBAL":os.devnull,"GIT_CONFIG_NOSYSTEM":"1"}
    result = subprocess.run([sys.executable,str(script)],env=env,cwd=tmp_path,
                            capture_output=True,text=True,timeout=15)
    assert result.returncode == 0,result.stderr
    proof = tmp_path/".hangar/codex-prepare-once-v1.json"
    assert json.loads(proof.read_text())["version"] == 1
    assert not (config/".hangar-prepared-v1").exists()
    hooks = json.loads((config/"hooks.json").read_text())
    assert any("guard_tmux.py" in hook.get("command","")
               for group in hooks["hooks"]["PreToolUse"] for hook in group["hooks"])
