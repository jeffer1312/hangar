"""Captura o parser datetime.time usado pelas preferências de silêncio."""
from datetime import time
import json
from pathlib import Path
import subprocess
bases=['00','0000','000000','12:30','12:30:59','24','2400','24:00','24:00:00','24:00:00.000000','24:00:00.000001','23:60','23:59:60','T12:30','t12:30','12.5','12:30.5','12:30:59.','12:30:59,','１２:３０','12:30:59.123','12:30:59.123456','12:30:59.1234567','12:30:59,123456']
rows=[]
for base in bases:
    for suffix in ['', '\x00','\x00resto','Z','+00:00','-00:00:30','+23:59:59.999999','+24:00','.1234567','resto']:
        value=base+suffix
        try:
            parsed=time.fromisoformat(value)
            result={'iso':parsed.isoformat(),'hour':parsed.hour,'minute':parsed.minute,'second':parsed.second,'microsecond':parsed.microsecond,'offset':None if parsed.utcoffset() is None else int(parsed.utcoffset().total_seconds()*1_000_000)}
        except Exception as error:result={'error':type(error).__name__}
        rows.append({'text':value,**result})
out={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_push_time.py','isolation':'Parser stdlib exato usado por push.set_quiet_hours/_in_quiet_hours; sem arquivo, relógio ou envio.','rows':rows}
Path(__file__).with_name('push-time.json').write_text(json.dumps(out,ensure_ascii=True,indent=2)+'\n')
print(len(rows))
