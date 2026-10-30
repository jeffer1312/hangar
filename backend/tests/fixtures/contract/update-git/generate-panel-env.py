"""Leitor literal do painel, sem rede e com seletor substituto."""
import contextlib,io,json,os,runpy,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip();rows=[]
with tempfile.TemporaryDirectory() as tmp:
 root=Path(tmp);config=root/'.hangar/config';config.mkdir(parents=True);(root/'chosen').mkdir();(root/'other').mkdir();os.environ.update(HOME=tmp,USERPROFILE=tmp,PATH=tmp);os.chdir(tmp)
 action=runpy.run_path(str(reference/'scripts/hangar-panel-action'))
 dialog=root/'kdialog';dialog.write_text(f'#!{sys.executable}\nimport sys\nprint(sys.argv[2])\n');dialog.chmod(0o700)
 cases=[('missing',None,['fixture','pick-folder']),('invalid-read',b'\xff',['fixture','pick-folder']),('invalid-independent',b'\xff',['peer','peer','on']),('invalid-empty-rename',b'\xff',['fixture','rename'])]
 cases += [('separator-'+str(ord(separator)),('ignored'+separator+'CP_SCAN_ROOTS=<ROOT>/chosen\nCP_SCAN_ROOTS=<ROOT>/other\n').encode(),['fixture','pick-folder']) for separator in ['\n','\r','\r\n','\v','\f','\x1c','\x1d','\x1e','\x85','\u2028','\u2029'] if len(separator)==1]
 for name,raw,args in cases:
  path=config/'.env'
  if raw is None:path.unlink(missing_ok=True)
  else:path.write_bytes(raw.replace(b'<ROOT>',tmp.encode()))
  (config/'peers.json').write_text('{"peer":{}}');stdout=io.StringIO();result={}
  try:
   with patch.object(sys,'argv',['hangar-panel-action',*args]),contextlib.redirect_stdout(stdout):rc=action['main']()
   result['rc']=rc
  except Exception as error:result.update(rc=1,exception=type(error).__name__,message=str(error).splitlines()[0])
  result['stdout']=stdout.getvalue().replace(tmp,'<ROOT>');result['book']=(config/'peers.json').read_text()
  rows.append(dict(name=name,input_hex=None if raw is None else raw.hex(),args=args,result=result))
output.write_text(json.dumps(dict(source_commit=commit,cases=rows),ensure_ascii=True,indent=2)+'\n');print(len(rows))
