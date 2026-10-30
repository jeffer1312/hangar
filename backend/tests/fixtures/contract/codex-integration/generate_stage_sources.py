"""Captura agentes de plugins e cópia seletiva de memória usados pelo stage nativo."""
import io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
cases=[]
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw)/'archive';archive.mkdir();data=subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT)
 with tarfile.open(fileobj=io.BytesIO(data)) as tar:tar.extractall(archive,filter='data')
 os.environ['HOME']=str(Path(raw)/'import-home');Path(os.environ['HOME']).mkdir();sys.path.insert(0,str(archive/'backend'))
 from app import codex_integracao as original
 texts=['---\nname: example\n---\nbody\n','---\r\nname: example\r\n---\r\nbody\r\n','---\rname: example\r---\rbody\r','\ufeff---\nname: "example"\n---\n','---\nname: \'ação\'\n---\n','---\nname: a.b-c x\n---\n','---\nname: a\t\n---\n','---\nname: \n---\n','# note\n','---\nname: a\n','---\nname: a\nname: b\n---\n','---\nname: a\x1c\n---\n','---\nname:\u00a0a\n---\n']
 for index,text in enumerate(texts):
  for enabled in [False,True]:
   with tempfile.TemporaryDirectory() as folder:
    home=Path(folder);root=home/'plugin';(root/'agents').mkdir(parents=True);(root/'agents'/'agent.md').write_bytes(text.encode());plugins=home/'.claude/plugins';plugins.mkdir(parents=True)
    (plugins/'installed_plugins.json').write_text(json.dumps({'plugins':{'demo-name@market':[{'scope':'project','installPath':str(home/'other')},{'scope':'user','installPath':str(root)}]}}))
    settings={'enabledPlugins':{'demo-name@market':enabled}}
    agents,unreadable=original._agentes_de_plugins(home,settings)
    cases.append(dict(kind='agents',name=f'text-{index}-{enabled}',files={'plugin/agents/agent.md':text.encode().hex()},settings=settings,result=dict(agents=agents,unreadable=[str(p.relative_to(home)) for p in unreadable])))
 for name,files,directories in [
  ('invalid-utf8',{'plugin/agents/bad.md':b'---\nname: a\xff\n---\n'},[]),
  ('directory-markdown',{},['plugin/agents/dir.md']),
  ('duplicate',{'plugin/agents/a.md':b'---\nname: same\n---\nfirst','plugin/agents/b.md':b'---\nname: same\n---\nlast'},[]),
 ]:
  with tempfile.TemporaryDirectory() as folder:
   home=Path(folder);root=home/'plugin';(root/'agents').mkdir(parents=True)
   for path,data in files.items():p=home/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(data)
   for path in directories:(home/path).mkdir(parents=True,exist_ok=True)
   plugins=home/'.claude/plugins';plugins.mkdir(parents=True);(plugins/'installed_plugins.json').write_text(json.dumps({'plugins':{'demo-name@market':[{'scope':'user','installPath':str(root)}]}}));settings={'enabledPlugins':{'demo-name@market':True}}
   agents,unreadable=original._agentes_de_plugins(home,settings)
   cases.append(dict(kind='agents',name=name,files={k:v.hex() for k,v in files.items()},directories=directories,settings=settings,result=dict(agents=agents,unreadable=[str(p.relative_to(home)) for p in unreadable])))
 for name,files in [
  ('absent',{}),('no-transcript',{'a/memory/a.md':b'memory'}),('metadata',{'a/memory/a.md':b'memory','a/a.jsonl':b'{"type":"system"}\n'}),
  ('compact',{'a/memory/a.md':b'memory','a/a.jsonl':b'{"type":"user"}\n'}),('spaced',{'a/memory/a.md':b'memory','a/a.jsonl':b'{"type": "user"}\n'}),
  ('different-space',{'a/memory/a.md':b'memory','a/a.jsonl':b'{"type" : "user"}\n'}),('lossy',{'a/memory/a.md':b'memory','a/a.jsonl':b'\xff{"type":"user"}\n'}),
  ('smallest-valid',{'a/memory/a.md':b'memory','a/small.jsonl':b'{}','a/medium.jsonl':b'{"type":"user"}','a/big.jsonl':b'{"type":"user"}'+b' '*100}),
  ('no-markdown',{'a/memory/a.txt':b'memory','a/a.jsonl':b'{"type":"user"}'}),('nested-only',{'a/memory/nested/a.md':b'memory','a/a.jsonl':b'{"type":"user"}'}),
  ('ignored',{'a/memory/a.md':b'memory','a/a.jsonl':b'{"type":"user"}','a/memory/.git/no':b'x','a/memory/__pycache__/no':b'x','a/memory/x.pyc':b'x','a/memory/nested/yes':b'yes'}),
  ('multiple',{'a/memory/a.md':b'a','a/a.jsonl':b'{"type":"user"}','b/memory/b.md':b'b','b/b.jsonl':b'{"type": "user"}'}),
 ]:
  with tempfile.TemporaryDirectory() as folder:
   base=Path(folder);source=base/'source';target=base/'target'
   for path,data in files.items():p=source/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(data)
   errors=original.copiar_memorias(source,target)
   result={str(p.relative_to(target)):p.read_bytes().hex() for p in sorted(target.rglob('*')) if p.is_file()}
   cases.append(dict(kind='memory',name=name,files={k:v.hex() for k,v in files.items()},result=dict(files=result,errors=errors)))
Path(__file__).with_name('stage-sources.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_stage_sources.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
