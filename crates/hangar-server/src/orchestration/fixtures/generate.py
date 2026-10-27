"""Congela contratos executando a referência Python; rode com o Python do backend na raiz.

Comando: python crates/hangar-server/src/orchestration/fixtures/generate.py
A referência deve existir: a ausência do Python original é erro, nunca um skip.
"""
from dataclasses import asdict
import importlib.util
import json
import re
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[5]
sys.path.insert(0, str(ROOT / "backend"))
from app import orq, orq_md, orq_papeis, orq_politica, orq_timeline, orq_start

DEST = Path(__file__).parent
COMMIT = subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip()


def save(name, cases):
    value = {"source_commit": COMMIT,
             "command": "python crates/hangar-server/src/orchestration/fixtures/generate.py",
             "cases": cases}
    (DEST / name).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


headers = ["papel", "sessão", "provider", "conta", "modelo", "esforço"]
values = dict(zip(headers, ["executor", "trabalho-t*", "codex", "padrão", "gpt", "high"]))
texts = ["", "# Prosa\n", "# Título\n\n## Quem é quem\n\n| Papel | Sessão | Provider | Conta | Modelo | Esforço |\n|---|---|---|---|---|---|\n| **Árbitro** | `a` | claude | p | opus | - |\n| executor | b | claude | p | sonnet | low |\n\n## Gates\n\n- Intacto\n", "```\n| papel | sessão | provider | conta | modelo | esforço |\n|---|---|---|---|---|---|\n| exemplo | x | x | x | x | x |\n```\n"]
save("markdown.json", [dict(text=s, headers=headers, key=["executor"], values=values,
    table=orq_md.ler_tabela(s, tuple(headers)),
    replaced=orq_md.trocar_linha(s, tuple(headers), "executor", values, "Quem é quem"),
    removed=orq_md.remover_linha(s, tuple(headers), "executor"),
    column=orq_md.inserir_coluna(s, tuple(headers), "vez", 1),
    section=orq_md.trocar_secao(s, "Gates", "- Novo")) for s in texts])

spec = importlib.util.spec_from_file_location("triage", ROOT / "skills/orquestrar/scripts/orq_triage.py")
triage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(triage)
(DEST / "triage-rules.json").write_text(json.dumps({"status": list(triage.STATUS.items()), "veto": triage.VETO}, ensure_ascii=False, indent=2) + "\n")
entries = []
with tempfile.TemporaryDirectory() as scratch:
    for name in ("2026-09-29-cad3e6fe", "synthetic"):
        directory = Path(scratch) / name
        directory.mkdir()
        source = ROOT / "backend/tests/fixtures/orq" / name
        for path in source.iterdir():
            if path.is_file():
                directory.joinpath(path.name).write_text(re.sub(r"/home/[^/]+", "/fixture/user", path.read_text()), encoding="utf-8")
        rows = [json.loads(line) for line in orq_timeline.timeline_path(directory).read_text().splitlines() if line.strip()]
        entries.append({"run": name, "shadow": (directory / "jev-shadow.jsonl").read_text() if (directory / "jev-shadow.jsonl").exists() else "",
                        "entries": [{"input": row, "output": orq_timeline.entry(row, orq_timeline.run_files(directory)), "id": orq_timeline.event_id(row)} for row in rows]})
save("timeline.json", entries)

save("roles.json", [{"input": vars(role), "text": orq_papeis.escrever_papel(texts[2], role), "output": [vars(p) for p in orq_papeis.ler(orq_papeis.escrever_papel(texts[2], role))]} for role in [
    orq_papeis.Papel("executor", "w-t*", "claude", "padrao", "opus", "high"),
    orq_papeis.Papel("executor", "w-t*", "codex", "openai-codex", "gpt", "high", "2", True, "Full Access", abertura_extra="--read-only", janela="60"),
    orq_papeis.Papel("executor", "w-t*", "omp", "api", "m", "max", perfil="trabalho")]])

events = [
    {"tipo": "execucao_inicio", "gid": "g1", "branch": "main", "plano": "p.md", "ts": "2026-09-29T10:00:00+00:00"},
    {"tipo": "task_inicio", "task": 1, "executor": "ex", "par": "rev", "titulo": "Primeira", "ts": "2026-09-29T10:01:00+00:00"},
    {"tipo": "entrega", "task": 1, "rodada": 1, "ts": "2026-09-29T10:02:00+00:00"},
    {"tipo": "veredito", "task": 1, "rodada": 1, "resultado": "aprova", "fase": "codigo", "ts": "2026-09-29T10:03:00+00:00"},
    {"tipo": "execucao_fim", "resultado": "ok", "ts": "2026-09-29T10:04:00+00:00"},
    {"tipo": "task_inicio", "task": 2, "executor": "ex2", "par": "subagente", "ts": "2026-09-29T10:05:00+00:00"}]
save("summaries.json", [{"events": events[:i], "summary": asdict(orq._monta("run", events[:i]))} for i in range(1, len(events)+1)])

pure = []
for rows in ([], events, events[:3], events + [{"tipo":"sessao_trocada","de":"ex2","para":"next","ts":"2026-09-29T10:06:00+00:00"}]):
    with tempfile.TemporaryDirectory() as scratch:
        directory = Path(scratch) / "run"
        directory.mkdir()
        cfg = {"arbiter":"arb", "auto":True}
        closed = [{"task":1,"hash":"abc","ts":"2026-09-29T10:03:30+00:00"}]
        for filename, value in (("orq.json",cfg),("eventos.jsonl",rows),("closed.jsonl",closed)):
            directory.joinpath(filename).write_text(json.dumps(value) if filename.endswith(".json") else "".join(json.dumps(v)+"\n" for v in value),encoding="utf-8")
        module = orq_start._orq()
        state = module.state(directory)
        state["waiting"] = sorted(state["waiting"])
        panel, aux = orq_timeline._build_panel(directory)
        pure.append({"config":cfg,"events":rows,"closed":closed,"state":state,"panel":panel,"aux":aux})
save("state-panel.json",pure)
save("rounding.json",[{"input":v,"digits":digits,"output":round(v,digits)} for v in
    [2.675,2.685,0.00015,0.00025,1.00005,-1.00005,1.01235,99999.99995] for digits in [1,2,4]])
