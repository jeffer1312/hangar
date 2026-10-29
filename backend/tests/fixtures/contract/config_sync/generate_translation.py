"""Congela tradução, lotes, cache e erros executando o Python original sem provedor real."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from pytest import MonkeyPatch
from app import config_sync_translate as tr
from app.narrar import NarrarError


def main():
    first = "Reads the file when the hook runs."
    inputs = [
        ("cache", [first, "Lê o arquivo quando o hook roda.", "", first], "pt", ["prefix"], False),
        ("english", ["Lê o arquivo quando o hook roda.", first], "en", ["prefix"], False),
        ("batch", [f"Runs the tests for file {i}." for i in range(43)], "pt", ["prefix"] * 3, False),
        ("rate", [first], "pt", ["429", "prefix"], False),
        ("limited", [first], "pt", ["429"] * 3, False),
        ("wrong-size", [first], "pt", ["[]"], False),
        ("invalid", [first], "pt", ["inválido"], False),
        ("parts", [first], "pt", ['[null]'], False),
        ("empty", [first], "pt", ['["  "]'], False),
        ("fence", [first], "pt", ['```json\n["Texto traduzido"]\n```'], False),
        ("disk", [first], "pt", ["prefix"], True),
        ("failure", [first], "pt", ["erro"], False),
    ]
    cases = []
    for name, texts, lang, answers, disk_error in inputs:
        with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as patch:
            patch.setenv("HOME", folder)
            calls, waits = [], []
            pending = iter(answers)
            def chat(system, prompt, **kw):
                batch = json.loads(prompt)
                calls.append({"system": system, "prompt": prompt, **kw})
                answer = next(pending)
                if answer in ("429", "erro"):
                    raise NarrarError(502, "provedor 429: rate limit" if answer == "429" else "falha simulada")
                return json.dumps([f"{lang}:{text}" for text in batch], ensure_ascii=False) if answer == "prefix" else answer
            patch.setattr(tr, "chamar_chat", chat)
            patch.setattr(tr.time, "sleep", waits.append)
            if disk_error:
                def fail_save(cache):
                    raise OSError("disco cheio")
                patch.setattr(tr, "_save", fail_save)
            output, error = tr.translate(texts, lang)
            cached = None
            if name == "cache":
                cached = tr.translate(texts, lang)
            path = tr._cache_path()
            cases.append({"name": name, "texts": texts, "lang": lang, "answers": answers,
                          "disk_error": disk_error, "output": output, "error": error,
                          "calls": calls, "waits": waits, "cached": cached,
                          "cache": json.loads(path.read_text()) if path.exists() else None})
    result = {"source_commit": subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              "command": "cd backend && uv run python tests/fixtures/contract/config_sync/generate_translation.py",
              "cases": cases}
    Path(__file__).with_name("translation.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
