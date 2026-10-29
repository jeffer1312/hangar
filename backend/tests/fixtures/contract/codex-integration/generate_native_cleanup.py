"""Captura cwd administrativo e nova tentativa de limpeza com Codex sintético."""
import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[5]
sys.path.insert(0, str(ROOT / 'backend'))

async def main():
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory)
        os.environ.update(HOME=str(home), USERPROFILE=str(home), XDG_CONFIG_HOME=str(home/'.config'), XDG_DATA_HOME=str(home/'.local/share'), XDG_STATE_HOME=str(home/'.local/state'), XDG_CACHE_HOME=str(home/'.cache'))
        from app.codex_importador import CodexNativo, CodexNativoErro
        from app import codex_importador
        script = home/'codex'
        script.write_text('#!'+sys.executable+'\nimport os,json,sys\nfor line in sys.stdin:\n msg=json.loads(line)\n if "id" in msg: print(json.dumps({"id":msg["id"],"result":{"cwd":os.getcwd(),"home":os.environ["HOME"]}}),flush=True)\n')
        script.chmod(0o700)
        native = CodexNativo(home,home/'.codex',binario=str(script))
        await native.__aenter__()
        observed = await native.request('fixture', {})
        work = Path(observed['cwd'])
        original = codex_importador.shutil.rmtree
        calls = []
        def remove(path, *args, **kwargs):
            if Path(path) == work:
                calls.append(str(path))
                if len(calls) == 1: raise PermissionError('fixture')
            return original(path, *args, **kwargs)
        codex_importador.shutil.rmtree = remove
        try:
            try:
                await native.close()
                first_error = False
            except CodexNativoErro:
                first_error = True
            retained = work.exists()
            await native.close()
        finally:
            codex_importador.shutil.rmtree = original
        result = dict(cwd_differs_from_home=observed['cwd']!=observed['home'],first_close_error=first_error,
                      directory_retained_after_error=retained,directory_removed_after_retry=not work.exists(),cleanup_calls=len(calls))
    Path(__file__).with_name('native-cleanup.json').write_text(json.dumps(dict(
        source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
        command='uv run python tests/fixtures/contract/codex-integration/generate_native_cleanup.py',result=result),indent=2)+'\n')
asyncio.run(main())
