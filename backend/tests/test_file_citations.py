import json
from types import SimpleNamespace

from fastapi.testclient import TestClient

from app import api
from app.config import settings
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


def test_relative_citation_uses_transcript_cwd_and_opens_in_file_endpoint(tmp_path, monkeypatch):
    born = tmp_path / "born"
    current = tmp_path / "project"
    born.mkdir()
    (current / "frontend").mkdir(parents=True)
    target = current / "frontend" / "vitest.config.ts"
    target.write_text("export default { test: { maxWorkers: 2 } }", encoding="utf-8")
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(json.dumps({
        "cwd": str(current),
        "message": {"content": "frontend/vitest.config.ts"},
    }) + "\n", encoding="utf-8")
    monkeypatch.setattr(api, "_cached_info_sync", lambda name: SimpleNamespace(
        name=name, cwd=str(born), jsonl=str(transcript),
    ))
    settings.auth_token = "secret"
    client = TestClient(api.app)
    headers = {"Authorization": "Bearer secret"}

    resolved = client.post("/api/sessions/s/files/resolver", headers=headers,
                           json={"caminhos": ["frontend/vitest.config.ts"]})
    assert resolved.status_code == 200
    assert resolved.json()["ok"]["frontend/vitest.config.ts"] == {
        "relativo": None, "real": str(target),
    }
    opened = client.get("/api/sessions/s/file", headers=headers,
                        params={"path": "frontend/vitest.config.ts"})
    assert opened.status_code == 200
    assert "maxWorkers: 2" in opened.text
