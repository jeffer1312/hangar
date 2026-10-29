"""Executa o leitor e escritor Python de apelidos em diretório temporário."""
import ast
import json
import logging
import os
from pathlib import Path
import subprocess
import tempfile
import threading
from types import SimpleNamespace

ROOT=Path(__file__).resolve().parents[5]
source=ROOT/"backend/app/apelidos.py"
names={"ler","definir"}
nodes=[node for node in ast.parse(source.read_text()).body if getattr(node,"name",None) in names]
inputs=[("missing",None,"chave:a","Nome"),("broken","broken","chave:a","Novo"),
        ("list","[]","chave:a","Novo"),("filter",'{"a":" A ","b":7,"c":" ","d":null}',"e","E"),
        ("strip-controls",'{"a":"\\u001cA\\u001f"}',"b","\u001c B \u001f"),
        ("truncate",'{"other":"Preservado"}',"new","á"*41),
        ("truncate-astral","{}","new","𐐀"*41),("delete","{\"a\":\"A\",\"b\":\"B\"}","a",None),
        ("delete-blank",'{"a":"A"}',"a","\u001c\u001f"),
        ("surrogate-existing",'{"a":"\\ud800"}',"b","B"),
        ("surrogate-value",'{"a":"A"}',"b","\ud800"),
        ("surrogate-id",'{"a":"A"}',"\ud800","B")]
inputs += [("surrogate-after-limit", "{}", "a", "A"*40 + "\ud800"), ("surrogate-at-limit", "{}", "a", "A"*39 + "\ud800"), ("nonfinite", '{"a":NaN,"b":Infinity,"c":"NaN"}', "d", "D")]
inputs.append(("truncate-leaves-space", "{}", "a", "A"*39 + " X"))
cases=[]
for name,before,id_,value in inputs:
    with tempfile.TemporaryDirectory() as directory:
        path=Path(directory)/"aliases.json"
        if before is not None:path.write_text(before)
        namespace=dict(json=json,os=os,_MAX=40,_lock=threading.Lock(),_caminho=lambda:path,
                       migracao_sidecars=SimpleNamespace(caminho_de_leitura=lambda p:p),
                       atomico=SimpleNamespace(substituir=os.replace),_log=logging.getLogger("fixture"))
        exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),"exec"),namespace)
        read=namespace["ler"]()
        try: result={"map":namespace["definir"](id_,value)}
        except (OSError,ValueError) as error:result={"error":type(error).__name__}
        cases.append(dict(name=name,before=before,id=id_,value=value,read=read,result=result,
                          after=path.read_text() if path.exists() else None, read_again=namespace["ler"]()))
Path(__file__).with_name("aliases.json").write_text(json.dumps(dict(
    source_commit=subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
    command="uv run python tests/fixtures/contract/engines/generate_aliases.py",cases=cases),ensure_ascii=True,indent=2)+"\n")
