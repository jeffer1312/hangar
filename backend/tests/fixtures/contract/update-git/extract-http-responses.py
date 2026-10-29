"""Reutiliza respostas capturadas do FastAPI para o construtor HTTP compartilhado."""
import hashlib,json,sys
from pathlib import Path
source=Path(sys.argv[1]);output=Path(sys.argv[2]);raw=source.read_bytes();fixture=json.loads(raw)
rows=[row for row in fixture['rows'] if row['token']=='owner' and row['status'] in [404,405,307]]
output.write_text(json.dumps(dict(source_commit=fixture['source_commit'],source_command=fixture['command'],source_fixture_sha256=hashlib.sha256(raw).hexdigest(),command='python extract-http-responses.py <methods.json-capturado> <fixture>',rows=rows),ensure_ascii=False,indent=2)+'\n')
print(f'{len(rows)} respostas originais extraídas')
