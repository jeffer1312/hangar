"""Executa testes originais e registra entradas e efeitos da reconciliação de skills."""
import io, json, os, subprocess, sys, tarfile, tempfile
from pathlib import Path
import pytest
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
CASES=[];LINKS=[]
def normalize(value,root):
    return json.loads(json.dumps(value).replace(str(root),'$ROOT'))
def snapshot(root,backups):
    entries={};saved=[]
    for parent,dirs,files in os.walk(root,followlinks=False):
        for name in dirs+files:
            path=Path(parent)/name
            if path==backups or backups in path.parents:continue
            key=path.relative_to(root).as_posix()
            if path.is_symlink():entries[key]={'link':os.readlink(path)}
            elif path.is_dir():entries[key]={'directory':True}
            else:entries[key]={'hex':path.read_bytes().replace(str(root).encode(),b'$ROOT').hex()}
    if backups.is_dir():
        for path in backups.iterdir():
            data=json.loads(path.read_bytes())
            if isinstance(data.get('conteudo_hex'),str):data['conteudo_hex']=bytes.fromhex(data['conteudo_hex']).replace(str(root).encode(),b'$ROOT').hex()
            saved.append(data)
    saved.sort(key=lambda item:item['path'])
    return normalize(dict(entries=entries,backups=saved),root)
class Capture:
    def pytest_collection_modifyitems(self,items):
        from app import codex_skills
        from app.codex_msgs import serializar
        original=codex_skills.reconciliar_skills
        original_link=codex_skills._link_gerenciado
        def link(entry,roots,**kwargs):
            result=original_link(entry,roots,**kwargs)
            if kwargs.get('windows') is not None:
                LINKS.append(dict(entry=str(entry),target=os.readlink(entry),roots=roots,windows=kwargs['windows'],expected=result))
            return result
        codex_skills._link_gerenciado=link
        for item in items:
            if not hasattr(item.module,'reconciliar_skills'):continue
            def capture(home,codex,plugins,registry,backups,**kwargs):
                from app import skill_bridge
                root=home.parent;index=sum(case['test']==self.current for case in CASES)
                initial=snapshot(root,backups)
                inputs=normalize(dict(plugins=plugins,registry=registry,windows=kwargs.get('windows',False),home=str(home),codex=str(codex),repo=str(skill_bridge._REPO),backups=str(backups)),root)
                try:
                    manifest,warnings=original(home,codex,plugins,registry,backups,**kwargs)
                    result=dict(manifest=manifest,warnings=[serializar(warning) for warning in warnings])
                except Exception as error:
                    CASES.append(dict(test=self.current,index=index,initial=initial,inputs=inputs,error=type(error).__name__,message=str(error),final=snapshot(root,backups)))
                    raise
                CASES.append(dict(test=self.current,index=index,initial=initial,inputs=inputs,result=normalize(result,root),final=snapshot(root,backups)))
                return manifest,warnings
            item.module.reconciliar_skills=capture
    def pytest_runtest_setup(self,item):self.current=item.name
with tempfile.TemporaryDirectory() as raw:
    archive=Path(raw)
    data=subprocess.check_output(['git','archive',COMMIT,'backend/app','backend/tests/test_codex_skills.py','scripts'],cwd=ROOT)
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:tar.extractall(archive,filter='data')
    sys.path.insert(0,str(archive/'backend'))
    code=pytest.main(['-q',str(archive/'backend/tests/test_codex_skills.py')],plugins=[Capture()])
    if code:raise SystemExit(code)
Path(__file__).with_name('skills.json').write_text(json.dumps(dict(source_commit=COMMIT,
    command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_skills.py',cases=CASES,
    links=[dict(item,entry='$ROOT/'+Path(item['entry']).name) for item in LINKS]),ensure_ascii=True,indent=2)+'\n')
print(f'{len(CASES)} reconciliações; {len(LINKS)} comparações Windows')
