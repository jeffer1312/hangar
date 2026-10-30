"""Captura bytes alimentados ao hash das fases e os metadados das mesmas fontes."""
import io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
cases=[]
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw)/'archive';archive.mkdir()
 data=subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT)
 with tarfile.open(fileobj=io.BytesIO(data)) as tar:tar.extractall(archive,filter='data')
 os.environ['HOME']=str(Path(raw)/'import-home');Path(os.environ['HOME']).mkdir()
 sys.path.insert(0,str(archive/'backend'))
 from app import codex_integracao,skill_bridge
 original_hash=codex_integracao.hashlib.sha256
 for variant in ['absent','sources','targets']:
  for phase in ['instructions','plugins','fragments','skills']:
   for sources_only in [False,True]:
    with tempfile.TemporaryDirectory() as directory:
     root=Path(directory);home=root/'home';account=home/'.codex';repo=root/'repo';home.mkdir();account.mkdir();repo.mkdir()
     codex_integracao._REPO=repo;skill_bridge._REPO=repo;codex_integracao.memoria_ligada=lambda:False
     content={}
     if variant!='absent':content={
      'home/.claude/settings.json':'{"hooks":{},"env":{"EXAMPLE":"value"},"enabledPlugins":{}}',
      'home/.claude/CLAUDE.md':'global\n@./import.md\n','home/.claude/import.md':'imported\n',
      'home/.claude/agents/agent.md':'agent\n','home/.claude/commands/command.md':'command\n',
      'home/.claude/hooks/hook.sh':'hook\n','home/.claude/skills/source/SKILL.md':'source\n',
      'home/.claude/agents/.git/ignored':'ignored','home/.claude/agents/__pycache__/ignored':'ignored',
      'home/.claude/agents/ignored.pyc':'ignored','repo/skills/repo/SKILL.md':'repo\n',
      'home/.claude.json':'{"mcpServers":{"example":{"command":"tool"}}}',
      'home/.codex/config.toml':'project_doc_max_bytes=65536\nproject_doc_fallback_filenames=["CLAUDE.md"]\n[projects.fixture]\ntrust_level="trusted"\n'}
     if variant=='targets':content.update({'home/.codex/AGENTS.md':'target\n','home/.codex/hooks.json':'{"hooks":{}}','repo/scripts/codex-hook-json.py':'fixture\n','home/.codex/.hangar-hooks/codex-hook-json.py':'fixture\n','home/.codex/plugins/installed_plugins.json':'{}','home/.claude/plugins/installed_plugins.json':'{}','home/.claude/plugins/known_marketplaces.json':'{}','home/.claude/ecc-slim-keep.txt':'source\n'})
     for path,text in content.items():path=root/path;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(text.encode())
     service=codex_integracao.IntegracaoCodex(home=home,codex_home=account)
     entries={}
     for path in sorted(root.rglob('*'),reverse=True):
      os.utime(path,ns=(1234000000000,1234000000000))
      entries[path.relative_to(root).as_posix()]={'directory':True,'mtime_ns':path.stat().st_mtime_ns} if path.is_dir() else {'hex':path.read_bytes().hex(),'mtime_ns':path.stat().st_mtime_ns}
     chunks=[]
     class Spy:
      def __init__(self,initial=b''):self.hasher=original_hash(initial);self.parts=[initial]
      def update(self,data):self.hasher.update(data);self.parts.append(data)
      def hexdigest(self):chunks.extend(part.decode().replace(str(root),'$ROOT') for part in self.parts);return self.hasher.hexdigest()
     codex_integracao.hashlib.sha256=Spy
     try:result=service._phase_fingerprints(fontes=sources_only,registry={},only=phase)
     finally:codex_integracao.hashlib.sha256=original_hash
     cases.append(dict(variant=variant,phase=phase,sources=sources_only,entries=entries,chunks=chunks))
Path(__file__).with_name('fingerprints.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_fingerprints.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
