"""Captura a guarda de marcador pendente sem importar configuração ou executar comando."""
import ast
import json
import re
import subprocess
from pathlib import Path
HERE=Path(__file__).resolve().parent
source=HERE.parents[3]/'app/shortcut_transfer.py'
nodes=[]
for node in ast.parse(source.read_text()).body:
    if isinstance(node,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='PLACEHOLDER_RE' for t in node.targets):nodes.append(node)
    if isinstance(node,ast.FunctionDef) and node.name=='has_placeholder':nodes.append(node)
ns={'re':re};exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),'exec'),ns)
inputs=['','echo normal','⟦SEGREDO:senha⟧','prefixo ⟦SEGREDO:token_2⟧ sufixo','⟦SEGREDO:a.b-c_01⟧',
        '⟦SEGREDO:primeiro⟧ ⟦SEGREDO:segundo⟧','⟦SEGREDO:⟧','⟦SEGREDO:á⟧','⟦SEGREDO:a b⟧',
        '⟦segredo:a⟧','[SEGREDO:a]','⟦SEGREDO:a','SEGREDO:a⟧','⟦SEGREDO:a/b⟧','⟦SEGREDO:a\nb⟧',
        '⟦SEGREDO:inválido⟧⟦SEGREDO:valido⟧','\x00⟦SEGREDO:ok⟧','⟦SEGREDO:..⟧']
result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_placeholder.py',
        'rows':[{'text':text,'result':ns['has_placeholder'](text)} for text in inputs]}
(HERE/'placeholder.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
print(f'{len(inputs)} marcadores capturados')
