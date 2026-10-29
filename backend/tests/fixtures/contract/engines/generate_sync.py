"""Caracterização dos espelhos, executada em pastas temporárias isoladas."""
import json
import subprocess
import tempfile
from pathlib import Path
from app import agentes_sync

cases = []
for target, folder, config in [("pi", ".pi/agent", "models.json"), ("kimi", ".kimi-code", "config.toml"), ("codex", ".codex", "config.toml")]:
    for initial in [None, "", "inválido", "{}" if target == "pi" else 'model = "pessoal"\n', '{"providers":{"pessoal":{"models":[]}}}' if target == "pi" else '[pessoal]\nx = 1\n']:
        for base in ["https://provider.invalid", "https://opencode.ai/zen/go/v1"]:
            with tempfile.TemporaryDirectory() as root:
                home = Path(root)
                directory = home / folder
                directory.mkdir(parents=True)
                if initial is not None:
                    (directory / config).write_text(initial, encoding="utf-8")
                models = [{"id": "m", "context_length": 1234, "vision": True}, {"id": "outro"}]
                result = agentes_sync.sincronizar("motor", base, "fixture-key", models, (target,), homes={target:home})
                files = {str(p.relative_to(home)):p.read_text() for p in home.rglob("*") if p.is_file()}
                removed = agentes_sync.remover("motor", (target,), homes={target:home})
                after = {str(p.relative_to(home)):p.read_text() for p in home.rglob("*") if p.is_file()}
                case = {"target":target,"folder":folder,"config":config,"initial":initial,"base":base,"models":models,"expected":result,"files":files,"removed":removed,"after":after}
                cases.append(json.loads(json.dumps(case).replace(root,"$ROOT")))
root = Path(__file__).resolve().parents[5]
output = {"source_commit":subprocess.check_output(["git","rev-parse","HEAD"],cwd=root,text=True).strip(),"command":"PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_sync.py","cases":cases}
Path(__file__).with_name("sync.json").write_text(json.dumps(output,ensure_ascii=False,indent=2)+"\n",encoding="utf-8")
