"""Captura seleção, importação e abertura de log sem abrir janela ou processo externo."""
import json,os,runpy,subprocess,sys,tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip();rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp);action=runpy.run_path(str(reference/'scripts/hangar-panel-action'));common=sys.modules['hangar_panel_common'];scope=action['main'].__globals__
 for label,term,projects,code,failure in [('missing','kitty',[],0,''),('no-terminal','', [{'name':'project','tmux':'runner'}],0,''),('dead','kitty',[{'name':'project','tmux':'runner'}],1,''),('tmux-failed','kitty',[{'name':'project','tmux':'runner'}],0,'run'),('spawn-failed','kitty',[{'name':'project','tmux':'runner'}],0,'spawn')]+[(term,term,[{'name':'project','tmux':'runner'}],0,'') for term in ['kitty','foot','alacritty']]:
  calls=[]
  def api(*args,**kwargs):calls.append(dict(kind='api',args=args,kwargs=kwargs));return projects
  def run(args,**kwargs):
   calls.append(dict(kind='run',args=args,kwargs=kwargs))
   if failure=='run':raise OSError('fixture failure')
   return SimpleNamespace(returncode=code)
  def spawn(args,**kwargs):
   calls.append(dict(kind='spawn',args=args,kwargs=kwargs))
   if failure=='spawn':raise OSError('fixture failure')
   return SimpleNamespace(pid=123)
  with patch.dict(scope,{'api':api}),patch.object(scope['shutil'],'which',side_effect=lambda value:value if value==term else None),patch.object(subprocess,'run',run),patch.object(subprocess,'Popen',spawn):
   try:result={'value':action['op_project']('project','open')}
   except common.PanelError as error:result={'error':str(error)}
  rows.append(dict(kind='open',name=label,term=term,projects=projects,code=code,failure=failure,calls=calls,result=result))
 for term,code,stdout in [('kdialog',0,' /chosen\n'),('zenity',0,' /chosen\n'),('',0,''),('kdialog',1,''),('kdialog',0,'  \n')]:
  calls=[]
  def run(args,**kwargs):calls.append(dict(args=args,kwargs=kwargs));return SimpleNamespace(returncode=code,stdout=stdout)
  with patch.object(scope['shutil'],'which',side_effect=lambda value:value if value==term else None),patch.object(subprocess,'run',run):result=action['op_pick_folder']('/start')
  rows.append(dict(kind='pick',term=term,code=code,stdout=stdout,calls=calls,result=result))
 first=Path(tmp)/'first';second=Path(tmp)/'second';first.mkdir();second.mkdir()
 for root in [first,second]:(root/'collision').mkdir()
 (first/'unique').mkdir()
 remote=[{'name':'one','cwd':'/remote/unique/','command':'npm run dev','port':123},{'name':'collision','cwd':'/remote/collision','command':'start'},{'name':'absent','cwd':'/remote/absent','command':'start'},{'name':'dead-command','cwd':'/remote/unique','command':'bash /missing-launcher'}]
 for value in [remote,{},[]]:
  with patch.dict(scope,{'api':lambda *a,**kw:value,'env':lambda key:f'{first},{second},{first}, /missing-root'}):result=action['op_import_candidates']('remote')
  rows.append(dict(kind='import',remote=value,result=result))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-panel-projects.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=True,indent=2).replace(tmp,'<ROOT>')+'\n');print(f'{len(rows)} casos de projetos capturados')
