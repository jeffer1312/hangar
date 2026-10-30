"""Captura aliases de skills com as três terminações de linha aceitas por read_text."""
import io,json,subprocess,sys,tarfile,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
cases=[]
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw)/'archive';archive.mkdir()
 data=subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT)
 with tarfile.open(fileobj=io.BytesIO(data)) as tar:tar.extractall(archive,filter='data')
 sys.path.insert(0,str(archive/'backend'))
 from app import codex_skills,skill_bridge
 from app.codex_msgs import serializar
 for source_ending in ['\n','\r\n','\r']:
  for native_ending in ['\n','\r\n','\r']:
   with tempfile.TemporaryDirectory() as directory:
    root=Path(directory);home=root/'home';account=home/'.codex';repo=root/'repo';skill_bridge._REPO=repo
    source=home/'.claude/plugins/cache/market/plugin/1/skills/source-name';native=account/'plugins/plugin/skills/native-name'
    source.mkdir(parents=True);native.mkdir(parents=True)
    source_bytes=source_ending.join(['---','name: shared-alias','---','source','']).encode()
    native_bytes=native_ending.join(['---','name: shared-alias','---','native','']).encode()
    (source/'SKILL.md').write_bytes(source_bytes);(native/'SKILL.md').write_bytes(native_bytes)
    manifest,warnings=codex_skills.reconciliar_skills(home,account,{'plugin@market':{'path':str(account/'plugins/plugin')}},{},home/'backups')
    result=dict(manifest=manifest,warnings=[serializar(w) for w in warnings],bridge_exists=(account/'skills/source-name').exists(),source_hex=(source/'SKILL.md').read_bytes().hex(),native_hex=(native/'SKILL.md').read_bytes().hex())
    cases.append(dict(source_hex=source_bytes.hex(),native_hex=native_bytes.hex(),result=json.loads(json.dumps(result).replace(str(root),'$ROOT'))))
Path(__file__).with_name('skill-newlines.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_skill_newlines.py',cases=cases),indent=2)+'\n')
print(len(cases))
