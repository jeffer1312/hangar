"""Caracteriza o scanner original sob o contexto do convidado, sem ponte Rust."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    base=Path(scratch)
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch)
    from app import fs,guest_users
    root=base/'allowed';root.mkdir()
    (base/'outside').mkdir();(root/'project').mkdir();(root/'file').write_text('fixture')
    (root/'escape').symlink_to(base/'outside',target_is_directory=True)
    os.utime(root/'project',(2000000000,2000000000))
    guest_users.current.set(guest_users.Guest('fixture','Ana','hash',str(root)))
    rows=[]
    cases=[('roots',None,None),('scan',str(root),None),('other_root',str(base/'outside'),None),
           ('escape',str(root),str(base/'outside')),('symlink',str(root),str(root/'escape')),
           ('missing',str(root),str(root/'missing')),('file',str(root),str(root/'file'))]
    for name,requested,path in cases:
        try:
            value=fs.list_roots.__wrapped__() if name=='roots' else fs.scan_dir.__wrapped__(requested,path)
            body=[v.model_dump() for v in value] if isinstance(value,list) else value.model_dump()
            status=200
        except fs.FsError as error:
            status=error.status;body={'detail':error.detail}
        rows.append({'name':name,'root':requested,'path':path,'status':status,'response':body})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_fs.py','rows':rows}
    (HERE/'fs.json').write_text(json.dumps(result,ensure_ascii=False,indent=2).replace(scratch,'⟦ROOT⟧')+'\n',encoding='utf-8')
    print(f'{len(rows)} respostas do scanner Python capturadas')
