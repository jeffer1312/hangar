"""Busca de conteudo cross-session (rg em todos os transcripts): campos do hit, snippet legivel,
teto de resultados, join live/dead e a garantia de seguranca (q passa como ARGUMENTO, nunca shell)."""
import io
import json

import pytest

from app import search


SID = "11111111-1111-1111-1111-111111111111"


@pytest.fixture(autouse=True)
def _tmp_projects(tmp_path, monkeypatch):
    # search() e archive._head_info leem settings.projects_dir -> aponta os dois pro tmp.
    monkeypatch.setattr(search.settings, "projects_dir", tmp_path)
    monkeypatch.setattr("app.archive.settings.projects_dir", tmp_path)
    return tmp_path


def _write(projdir, sid=SID, text="hello needle world", cwd="/home/u/proj"):
    projdir.mkdir(parents=True, exist_ok=True)
    j = projdir / f"{sid}.jsonl"
    j.write_text(json.dumps({
        "type": "user", "uuid": "u1", "cwd": cwd,
        "message": {"role": "user", "content": text},
    }) + "\n", encoding="utf-8")
    return j


def test_finds_match_and_returns_fields(tmp_path):
    _write(tmp_path / "-home-u-proj")
    hits = search.search("needle", {})
    assert len(hits) == 1
    h = hits[0]
    assert h.project == "-home-u-proj"
    assert h.session_id == SID
    assert h.cwd == "/home/u/proj"
    assert h.live is False and h.session_name is None
    # snippet legivel = texto da msg, nao o JSON cru
    assert "needle" in h.line
    assert not h.line.startswith("{")


def test_blank_query_returns_empty(tmp_path):
    _write(tmp_path / "-home-u-proj")
    assert search.search("", {}) == []
    assert search.search("   ", {}) == []


def test_result_cap_enforced(tmp_path):
    # 3 arquivos, cada um com 1 match; limite 2 -> no maximo 2 hits.
    for i in range(3):
        sid = f"{i}1111111-1111-1111-1111-111111111111"
        _write(tmp_path / "-home-u-proj", sid=sid)
    hits = search.search("needle", {}, limit=2)
    assert len(hits) == 2


def test_live_vs_dead_flag(tmp_path):
    import os
    j_live = _write(tmp_path / "-home-u-live", cwd="/home/u/live")
    _write(tmp_path / "-home-u-dead", sid="22222222-2222-2222-2222-222222222222", cwd="/home/u/dead")
    live_names = {os.path.realpath(str(j_live)): "sessao-viva"}
    hits = search.search("needle", live_names)
    by_project = {h.project: h for h in hits}
    assert by_project["-home-u-live"].live is True
    assert by_project["-home-u-live"].session_name == "sessao-viva"
    assert by_project["-home-u-dead"].live is False
    assert by_project["-home-u-dead"].session_name is None


def test_query_passed_as_argv_not_shell(tmp_path, monkeypatch):
    # SEGURANCA: o rg tem que ser chamado com LISTA DE ARGUMENTOS (sem shell) e a query como VALOR de -e.
    recorded = {}
    match_line = json.dumps({
        "type": "match",
        "data": {"path": {"text": str(tmp_path / "-p" / f"{SID}.jsonl")},
                 "lines": {"text": json.dumps({"type": "user", "uuid": "u1", "cwd": "/c",
                                               "message": {"role": "user", "content": "x; rm -rf /"}}) + "\n"}},
    })

    class _FakePopen:
        def __init__(self, argv, **kwargs):
            recorded["argv"] = argv
            recorded["kwargs"] = kwargs
            self.stdout = io.StringIO(match_line + "\n")

        def terminate(self):
            pass

        def wait(self):
            pass

    monkeypatch.setattr(search.subprocess, "Popen", _FakePopen)
    hits = search.search("; rm -rf /", {})
    argv = recorded["argv"]
    assert isinstance(argv, list)                       # lista de args, nao string
    assert recorded["kwargs"].get("shell") in (None, False)  # nunca shell=True
    assert "-F" in argv                                 # fixed-string (q literal, nao regex)
    # a query vai como VALOR de -e (proximo item), literal — nunca concatenada num comando
    assert argv[argv.index("-e") + 1] == "; rm -rf /"
    assert len(hits) == 1


# --- ask-history helpers (RAG lexical) ---------------------------------------
from app.search import extract_terms, search_terms, build_ask_prompt, SearchHit


def test_extract_terms_strips_stopwords_short_and_dups():
    terms = extract_terms("Onde eu falei sobre o DEPLOY do backend e o deploy?")
    assert "deploy" in terms and "backend" in terms
    # stopwords/curtas fora, minusculo, sem duplicata
    for junk in ("onde", "eu", "sobre", "do", "o", "e"):
        assert junk not in terms
    assert terms == [t.lower() for t in terms]
    assert len(terms) == len(set(terms))


def test_extract_terms_caps_at_max():
    terms = extract_terms("alpha bravo charlie delta echo foxtrot golf hotel", max_terms=6)
    assert len(terms) == 6


def test_search_terms_dedups_by_session_and_line(monkeypatch):
    from app import search as search_mod
    h = SearchHit(project="p", session_id="s1", session_name="viva", cwd="/c",
                  line="trecho igual", mtime=1.0, live=True)
    # mesmo hit devolvido por 2 termos -> aparece 1x
    monkeypatch.setattr(search_mod, "search", lambda q, live, limit=30: [h])
    out = search_terms(["a", "b"], {}, cap=30)
    assert len(out) == 1


def test_build_ask_prompt_labels_live_and_archived():
    hits = [
        SearchHit(project="p", session_id="s1", session_name="minha-sessao", cwd="/c",
                  line="falei de deploy", mtime=2.0, live=True),
        SearchHit(project="proj-morto", session_id="deadbeef1234", session_name=None, cwd="/c",
                  line="deploy antigo", mtime=1.0, live=False),
    ]
    p = build_ask_prompt("onde falei de deploy?", hits)
    assert "[sessão minha-sessao — viva]: falei de deploy" in p
    assert "— arquivada]:" in p and "proj-morto" in p
    assert "onde falei de deploy?" in p
