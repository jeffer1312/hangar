"""Captura remoção do bloco antigo sem alterar instruções pessoais."""
import ast,json,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5];COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_compat.py'],cwd=ROOT,text=True);tree=ast.parse(source);selected=[node for node in tree.body if isinstance(node,ast.FunctionDef) and node.name=='remover_instrucao_de_leitura' or isinstance(node,ast.Assign) and any(isinstance(t,ast.Name) and t.id in ('INICIO_INSTRUCOES','FIM_INSTRUCOES') for t in node.targets)];ns={};exec(compile(ast.Module(body=selected,type_ignores=[]),'codex_compat_original','exec'),ns)
a=ns['INICIO_INSTRUCOES'];b=ns['FIM_INSTRUCOES'];block=a+'\nmanaged\n'+b;cases=[]
for text in ['', 'Pessoal\r\n𐐀',block,block+'\n\nPessoal',block+'\nPessoal',block+'\r\n\r\nPessoal','A'+block+'B'+block+'\n\nC',a,b,b+a,a+a+b,a+b+b,a+'𐐀'+b,'Antes\r\n'+block+'\n\nDepois\r\n']:
 try:result={'ok':ns['remover_instrucao_de_leitura'](text)}
 except Exception as error:result={'error':type(error).__name__,'message':str(error)}
 cases.append({'input':text,'result':result})
Path(__file__).with_name('legacy-instructions.json').write_text(json.dumps({'source_commit':COMMIT,'command':'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_legacy_instructions.py','cases':cases},ensure_ascii=True,indent=2)+'\n');print(len(cases))
