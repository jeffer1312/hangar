"""Uma entrada de cookie ASCII da fonte original, conservando os bytes do cabeçalho."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import types
SOURCE='ba2db1021ab7fb801eca50a50f9509dcb825ac7e'
REPO=Path(__file__).resolve().parents[5]

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--child',type=Path);parser.add_argument('--output',type=Path,default=Path('/tmp/hangar-browser-cookie-ascii.json'));args=parser.parse_args()
    if args.child:
        import http.cookies
        source=args.child/'guest_user_gate.py';module=types.ModuleType('original_guest_user_gate')
        exec(compile(source.read_text(),str(source),'exec'),module.__dict__)
        raw=b'cp_token=login-guest; \xc3\xaa=1'
        scope={'type':'http','method':'GET','scheme':'http','headers':[(b'cookie',raw)],'query_string':b''}
        observed={'raw_cookie_hex':raw.hex(),'token':module._token(scope),'simple_cookie_flags':int(http.cookies._CookiePattern.flags)}
        (args.child/'result.json').write_text(json.dumps(observed,ensure_ascii=False,indent=2)+'\n');return
    raw=subprocess.check_output(['git','show',f'{SOURCE}:backend/app/guest_user_gate.py'],cwd=REPO)
    with tempfile.TemporaryDirectory(prefix='hangar-browser-cookie-original-') as temp:
        root=Path(temp);home=root/'home';home.mkdir();(root/'guest_user_gate.py').write_bytes(raw)
        env={'PATH':'/usr/bin:/bin','HOME':str(home),'USERPROFILE':str(home),'XDG_CONFIG_HOME':str(home/'.config'),'XDG_CACHE_HOME':str(home/'.cache'),'XDG_DATA_HOME':str(home/'.local/share'),'CLAUDE_CONFIG_DIR':str(home/'.claude'),'CODEX_HOME':str(home/'.codex'),'PYTHONPATH':str(REPO/'backend'),'PYTHONDONTWRITEBYTECODE':'1','PYTHONNOUSERSITE':'1','CP_AUTH_TOKEN':'isolated-owner','CP_SCAN_ROOTS':str(root/'workspace'),'CP_PROJECTS_DIR':str(root/'workspace')}
        child=subprocess.run([sys.executable,str(Path(__file__).resolve()),'--child',str(root)],env=env,cwd=root,capture_output=True,text=True,timeout=5)
        if child.returncode:raise RuntimeError(child.stderr)
        observed=json.loads((root/'result.json').read_text())
    result={'source_commit':SOURCE,'source_sha256':hashlib.sha256(raw).hexdigest(),'generator':'/home/jefferson/Projetos/hangar/backend/.venv/bin/python /tmp/generate_hangar_browser_cookie_ascii.py','python_version':sys.version.split()[0],'isolation':'filho env_clear/HOME/config/cwd temporários; só _token original, nenhum servidor ou sessão real','limits':['Um cabeçalho real em bytes; não converte a string Latin1 de volta a UTF8.','A prova Rust envia esses bytes ao roteador público e usa guests.json temporário real.'],'observed':observed}
    args.output.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n');print(json.dumps(result,ensure_ascii=False))
if __name__=='__main__':main()
