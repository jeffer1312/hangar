import json
from app.transcript import citation_cwds


def test_citation_uses_cwd_from_the_line_that_mentions_it(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"cwd": "/project", "message": {"content": "src/a.ts"}}) + "\n"
        + json.dumps({"cwd": "/other", "message": {"content": "outro arquivo"}}) + "\n"
        + json.dumps({"snapshot": {"src/a.ts": {}}}) + "\n",
        encoding="utf-8",
    )
    assert citation_cwds(transcript, ["src/a.ts", "ausente.ts"]) == {"src/a.ts": ["/project"]}


def test_windows_citation_is_found_with_escaped_backslashes(tmp_path):
    # O JSON da conversa dobra a contrabarra; quem pede o arquivo manda o caminho com uma só.
    path = r"C:\Users\Admin\Área de trabalho\shot 1.png"
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(json.dumps({"cwd": r"C:\web", "message": {"content": f"Print: {path}"}},
                                     ensure_ascii=False) + "\n", encoding="utf-8")
    assert citation_cwds(transcript, [path]) == {path: [r"C:\web"]}


def test_longer_citation_does_not_cite_its_prefix(tmp_path):
    # `config.json.bak` citado sozinho não cita `config.json`: o mais longo vence na mesma posição.
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(json.dumps({"cwd": "/repo", "message": {"content": "/home/x/config.json.bak"}}) + "\n",
                          encoding="utf-8")
    got = citation_cwds(transcript, ["/home/x/config.json", "/home/x/config.json.bak"])
    assert got == {"/home/x/config.json.bak": ["/repo"]}
