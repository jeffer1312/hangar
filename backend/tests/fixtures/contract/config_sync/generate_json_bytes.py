"""Captura a leitura de JSON em bytes antes da validação HTTP."""
import json
import subprocess
from pathlib import Path


def main():
    cases = []
    texts = [
        '{"nome":"ação 😀"}',
        '{"nome":"ação",}',
        '[1,]',
        '{"x":}',
        '{"x":"\ud800"}',
        '{"x":"\udfff"}',
        '{"x":"\\ud800"}',
        '{"x":"\ud800",}',
        '{"x":"\ud800"} trailing',
        '\ud800',
        '{"x":\ud800}',
        '{"x":"\\\ud800"}',
    ]
    for index, text in enumerate(texts):
        for encoding in ["utf-8", "utf-8-sig", "utf-16", "utf-16-le", "utf-16-be", "utf-32", "utf-32-le", "utf-32-be"]:
            raw = text.encode(encoding, errors="surrogatepass")
            cases.append((f"text-{index}-{encoding}", raw))
    cases.extend([
        ("invalid-utf8", b'{"x":"\xff"}'),
        ("truncated-utf16", b'\xff\xfe{'),
        ("truncated-utf32", b'\xff\xfe\0\0{\0\0'),
        ("out-of-range-utf32", b'\xff\xfe\0\0\0\0\x11\0'),
        ("utf8-short", b'1'),
        ("utf16-short-le", b'1\0'),
        ("utf16-short-be", b'\0' + b'1'),
    ])
    captured = []
    for name, raw in cases:
        row = {"name": name, "hex": raw.hex()}
        try:
            row["value_json"] = json.dumps(json.loads(raw), ensure_ascii=True)
        except json.JSONDecodeError as error:
            row.update(status=422, message=error.msg, position=error.pos)
        except UnicodeDecodeError:
            row.update(status=400)
        captured.append(row)
    target = Path(__file__).with_name("json-bytes.json")
    target.write_text(json.dumps({
        "source": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "command": "uv run python tests/fixtures/contract/config_sync/generate_json_bytes.py",
        "cases": captured,
    }, ensure_ascii=True, indent=2) + "\n")


if __name__ == "__main__":
    main()
