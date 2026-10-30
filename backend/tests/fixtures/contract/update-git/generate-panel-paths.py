"""Captura caminhos do snapshot com ligações existentes e destinos ausentes."""
import json,os,runpy,subprocess,sys,tempfile
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip();rows=[]
with tempfile.TemporaryDirectory() as tmp:
 root=Path(tmp);(root/'real/nested').mkdir(parents=True);(root/'link').symlink_to('real/nested');(root/'dangling').symlink_to('absent/nested');(root/'cycle-a').symlink_to('cycle-b');(root/'cycle-b').symlink_to('cycle-a');os.chdir(tmp)
 data=runpy.run_path(str(reference/'scripts/hangar-panel-data'))
 for value in ['', '.', 'real/../real/nested', 'link','link/missing','link/../missing','dangling/leaf','cycle-a/leaf']:
  result=data['with_cwd_real']({'cwd':value})['cwd_real'];rows.append(dict(cwd=value,result=result.replace(tmp,'<ROOT>')))
output.write_text(json.dumps(dict(source_commit=commit,cases=rows),ensure_ascii=True,indent=2)+'\n');print(rows)
