"""Captura o filtro dos hooks exclusivos do Claude antes da importação Codex."""
import ast,json,re,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_integracao.py'],cwd=ROOT,text=True)
functions=[node for node in ast.parse(source).body if isinstance(node,ast.FunctionDef) and node.name in ('_skip_imported_hook','sem_hooks_do_app')]
ns={'re':re};exec(compile(ast.Module(body=functions,type_ignores=[]),'original-stage-hooks','exec'),ns)
commands=[None,0,False,{},[], '', 'echo keep','python backend/hooks/ask.py','python C:\\tool\\backend\\hooks\\ask.py']
for name in ['guard_tmux.py','skill-suggester.py','jev-command-gate.py','jev-answer-check.py']:
 for prefix,suffix in [('', ''),('/tmp/',''),('python /tmp/',' --arg'),('prefix',''),('','suffix'),('"','"'),('',';'),('','|'),('','&'),('\x1c','\x1f'),('\u00a0','\u0085')]:commands.append(prefix+name+suffix)
inputs=[None,0,[],"text",{}, {'Event':None},{'Event':[]},{'Event':[None,{}, {'hooks':None},{'hooks':[]}] }]
for command in commands:
 inputs.append({'Event':[{'matcher':'*','extra':{'keep':True},'hooks':[{'type':'command','command':command},{'type':'command','command':'echo keep'}]}]})
 inputs.append({'Event':[{'hooks':[{'type':'command','command':command}]}],'Other':[{'hooks':[]}]})
cases=[dict(input=value,result=ns['sem_hooks_do_app'](value)) for value in inputs]
Path(__file__).with_name('stage-hooks.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_stage_hooks.py',cases=cases),indent=2)+'\n')
print(len(cases))
