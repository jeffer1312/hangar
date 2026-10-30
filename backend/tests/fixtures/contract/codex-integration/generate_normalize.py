"""Captura normalização original de hooks e comandos POSIX/Windows sem executá-los."""
import ast, copy, itertools, json, re, shlex, subprocess
from pathlib import Path, PureWindowsPath
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_compat.py'],cwd=ROOT,text=True)
names={'_partes_shell','_split_windows','_nome_binario','_argv_rtk','wrapper_instalado','_comando','normalizar_hooks','normalizar_security_guidance'}
ns=dict(deepcopy=copy.deepcopy,Path=Path,PureWindowsPath=PureWindowsPath,re=re,json=json,shlex=shlex,subprocess=subprocess)
exec(compile(ast.Module(body=[node for node in ast.parse(source).body if isinstance(node,ast.FunctionDef) and node.name in names],type_ignores=[]),'codex_compat.py','exec'),ns)
commands=['rtk hook claude',"rtk hook claude --flag 'valor com espaço' ''","'/pasta com espaço/rtk' hook claude --flag 'a;b'",'rtk hook claude | python3 /antigo/codex-hook-allow.py',"rtk hook claude --flag 'a|b' | '/bin/python3.14' '/antigo com espaço/codex-hook-allow.py'",'/x/guard.sh','echo rtk hook claude','rtk hook other','rtk hook claude && echo ok','rtk hook claude | python3 outro.py','rtk hook claude; echo fim','rtk hook claude --flag $VAR','rtk hook claude --flag "$VAR"','rtk hook claude > arquivo','rtk hook claude | | python3 codex-hook-allow.py','rtk hook claude *','rtk hook claude # comentário','rtk hook claude ~/arquivo',"rtk hook claude 'aspas incompletas",r'"C:\Program Files\RTK\rtk.exe" hook claude "a;b"',r'rtk hook claude a\ b',r'rtk hook claude a\|b',r'rtk hook claude "a\\"',r'rtk hook claude %VAR%',r'rtk hook claude !VAR!',r'rtk hook claude a^b',r'rtk hook claude "a\"b"',"rtk hook claude '\u001c'",'','python /x/codex-hook-allow.py -- rtk hook claude']
cases=[]
for windows,command,event,timeout in itertools.product([False,True],commands,['PreToolUse','SessionEnd'],[None,20]):
 config={'outra_chave':{'x':True},'hooks':{event:[{'matcher':'Bash|exec_command','hooks':[{'type':'command','command':command,'timeout':timeout,'statusMessage':'Executando'}]}]}}
 normalized=ns['normalizar_hooks'](config,'/python com espaço',Path('/wrapper/codex-hook-allow.py'),windows=windows)
 security=ns['normalizar_security_guidance'](config,'/python com espaço',Path('/wrapper/codex-hook-json.py'),windows=windows)
 cases.append(dict(windows=windows,config=config,normalized=normalized,installed=ns['wrapper_instalado'](normalized,windows=windows),security=security))
Path(__file__).with_name('normalize.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_normalize.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
