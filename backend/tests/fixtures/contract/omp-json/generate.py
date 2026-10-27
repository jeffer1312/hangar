"""Congela a leitura JSON do sincronizador na referência Python indicada."""
import argparse
import json
import sys
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--backend", type=Path, required=True)
args = parser.parse_args()
sys.path.insert(0, str(args.backend))
from app.omp_plugin_sync import _read

cases = [
    '{"x":0.5}', '{"x":1e3}', '{"x":-2.5e-2}',
    '{"x":18446744073709551616}', '{"x":-18446744073709551616}',
    '{"x":[0.5,{"y":1e3}]}',
    '{"$serde_json::private::Number":"0.5","other":true}',
    '{"nested":{"$serde_json::private::Number":"invalid"}}',
    '0.5', '1e3', '{"x":0.5,"x":1}', '{"x":{"y":0.5,"y":1}}',
]
out = []
with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / "record.json"
    for raw in cases:
        path.write_text(raw, encoding="utf-8")
        try:
            expected = {"text": json.dumps(_read(path), ensure_ascii=False, separators=(",", ":"))}
        except ValueError as error:
            expected = {"error": str(error)}
        out.append({"input": raw, "expected": expected})
Path(__file__).with_name("numbers.json").write_text(json.dumps({
    "source_commit": "d2a42410f9fe6df4ea491b296c7fea7b98d8b844",
    "generated_by": "generate.py --backend <referência>/backend",
    "cases": out,
}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
