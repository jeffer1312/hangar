"""Captura preparo global original com trava, aliases, importações e backups reais."""
import ast, hashlib, json, os, subprocess, tempfile, tomllib
from contextlib import contextmanager
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
ns=dict(Path=Path,hashlib=hashlib,json=json,os=os,tempfile=tempfile,tomllib=tomllib,subprocess=subprocess,
 contextmanager=contextmanager,atomico=type('Atomic',(),{'substituir':staticmethod(os.replace)}),LIMITE_INSTRUCOES=1024*1024)
for path in ['backend/app/codex_arquivos.py','backend/app/codex_instrucoes.py']:
 source=subprocess.check_output(['git','show',f'{COMMIT}:{path}'],cwd=ROOT,text=True)
 nodes=[n for n in ast.parse(source).body if isinstance(n,(ast.FunctionDef,ast.ClassDef)) and (path.endswith("codex_instrucoes.py") or n.name in {"ler","json_obj","hash_bytes","json_bytes","gravar","backup","AlteradoExternamente"})]
 exec(compile(ast.Module(body=nodes,type_ignores=[]),path,'exec'),ns)
cases=[]
for name in ['missing','simple','uppercase','blank','personal','personal-link','dangling','import','nested','cycle','unresolved','space-import','crlf','unicode-lines','remove-source','change-source','modified-target','modified-link','repeat','copy-fallback']:
 with tempfile.TemporaryDirectory() as raw:
  home=Path(raw);claude=home/'.claude';codex=home/'.codex';claude.mkdir();codex.mkdir();target=codex/'AGENTS.override.md';source=claude/'CLAUDE.md'
  if name!='missing':source.write_text('Orientações\n')
  if name=='uppercase':source.rename(claude/'CLAUDE.MD')
  if name=='blank':source.write_text(' \n\t')
  if name=='personal':target.write_text('Pessoal\n')
  if name=='personal-link':target.symlink_to('../.claude/CLAUDE.md')
  if name=='dangling':target.symlink_to('../ausente')
  if name in ['import','nested','cycle']:
   source.write_text('Antes\n@extra.md\nDepois\n');(claude/'extra.md').write_text('Extra')
   if name=='nested':(claude/'extra.md').write_text('@deep.md');(claude/'deep.md').write_text('Profundo\n')
   if name=='cycle':(claude/'extra.md').write_text('@CLAUDE.md\n')
  if name=='unresolved':source.write_text('@ausente.md\n')
  if name=='space-import':source.write_text('@ extra.md\n');(claude/'extra.md').write_text('Não importar')
  if name=='crlf':source.write_bytes(b'Antes\r\n@extra.md\r\n');(claude/'extra.md').write_bytes(b'Extra\r\n')
  if name=='unicode-lines':source.write_text('Antes\u2028@extra.md\u2029Depois');(claude/'extra.md').write_text('Extra')
  before={str(p.relative_to(home)):{'link':os.readlink(p)} if p.is_symlink() else {'hex':p.read_bytes().hex()} for p in home.rglob('*') if p.is_file() or p.is_symlink()}
  results=[]
  original_symlink=Path.symlink_to
  def unavailable(*args,**kwargs):raise OSError('fixture symlink')
  if name=='copy-fallback':Path.symlink_to=unavailable
  try:
   for round in range(2 if name in ['remove-source','change-source','modified-target','modified-link','repeat'] else 1):
    if round:
     if name=='remove-source':source.unlink()
     elif name=='change-source':source.write_text('Mudança\n')
     elif name=='modified-target':target.unlink();target.write_text('Pessoal alterado\n');source.unlink()
     elif name=='modified-link':target.unlink();target.symlink_to('../other');source.unlink()
    try:ns['preparar_instrucoes'](home,codex,force=True);results.append({'ok':True})
    except Exception as error:results.append({'error':type(error).__name__,'message':str(error)})
  finally:Path.symlink_to=original_symlink
  files={};records=[];backups=[]
  for p in home.rglob('*'):
   if p.name=='.hangar-instrucoes.lock':continue
   if p.is_file() or p.is_symlink():
    relative=str(p.relative_to(home))
    if '.hangar-instrucoes/' in relative:
     data=json.loads(p.read_text());
     if '/backups/' in relative:backups.append(data)
     else:records.append(data)
    else:files[relative]={'link':os.readlink(p)} if p.is_symlink() else {'hex':p.read_bytes().hex()}
  case=dict(name=name,before=before,results=results,files=files,records=records,backups=backups)
  cases.append(json.loads(json.dumps(case).replace(raw,'$ROOT')))
Path(__file__).with_name('instructions.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_instructions.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
