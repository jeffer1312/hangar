import os
import subprocess

import pytest

from app import filesearch, git_ops
from app.filesearch import SearchError


def _repo(tmp_path):
    d = str(tmp_path)
    git_ops._run(d, "init", "-q", ".")
    git_ops._run(d, "config", "user.email", "t@t")
    git_ops._run(d, "config", "user.name", "t")
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "alvo.py").write_text("def buscar():\n    return 1\n")
    (tmp_path / ".gitignore").write_text("escondido.txt\n")
    (tmp_path / "escondido.txt").write_text("buscar")
    git_ops._run(d, "add", ".")
    git_ops._run(d, "commit", "-q", "-m", "base")
    return d


def test_nomes_acha_por_trecho_do_meio(tmp_path):
    d = _repo(tmp_path)
    paths = [h["path"] for h in filesearch.search(d, "alv", "names")["hits"]]
    assert "src/alvo.py" in paths


def test_nomes_respeita_gitignore(tmp_path):
    d = _repo(tmp_path)
    paths = [h["path"] for h in filesearch.search(d, "escondido", "names")["hits"]]
    assert paths == []


def test_conteudo_devolve_linha_e_numero(tmp_path):
    d = _repo(tmp_path)
    hits = filesearch.search(d, "def buscar", "contents")["hits"]
    assert hits[0]["path"] == "src/alvo.py" and hits[0]["line"] == 1


def test_sem_resultado_e_lista_vazia_nao_erro(tmp_path):
    """git grep sai com codigo 1 quando nao acha nada. Isso nao e falha."""
    d = _repo(tmp_path)
    r = filesearch.search(d, "coisaquenaoexiste", "contents")
    assert r["hits"] == [] and r["truncated"] is False


def test_termo_com_traco_nao_vira_flag(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "flags.txt").write_text("usa --force aqui\n")
    hits = filesearch.search(d, "--force", "contents")["hits"]
    assert any(h["path"] == "flags.txt" for h in hits)


def test_fora_de_repo_git_explica(tmp_path):
    with pytest.raises(SearchError) as e:
        filesearch.search(str(tmp_path), "x", "names")
    assert e.value.status == 409 and e.value.code == "erro_arq_nao_e_repo_git"


def test_q_vazio_recusado(tmp_path):
    d = _repo(tmp_path)
    with pytest.raises(SearchError) as e:
        filesearch.search(d, "   ", "names")
    assert e.value.code == "erro_arq_busca_vazia"


def test_corta_em_200(tmp_path):
    d = _repo(tmp_path)
    for i in range(210):
        (tmp_path / f"m{i}.txt").write_text("agulha\n")
    r = filesearch.search(d, "agulha", "contents")
    assert len(r["hits"]) == 200 and r["truncated"] is True


def test_acento_no_nome_nao_volta_escapado(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "sessão-única.md").write_text("x")
    paths = [h["path"] for h in filesearch.search(d, "sess", "names")["hits"]]
    assert "sessão-única.md" in paths


def test_conteudo_nomes_com_dois_pontos_aspas_e_newline(tmp_path):
    d = _repo(tmp_path)
    for nome in ("a:b.txt", 'a"b.txt', "line\nname.txt"):
        (tmp_path / nome).write_text("needle\n")
    hits = filesearch.search(d, "needle", "contents")["hits"]
    achados = {(h["path"], h["line"], h["text"]) for h in hits}
    assert achados == {("a:b.txt", 1, "needle"), ('a"b.txt', 1, "needle"), ("line\nname.txt", 1, "needle")}


def test_teto_conta_hits_reais_com_nomes_estranhos(tmp_path):
    d = _repo(tmp_path)
    for i in range(201):
        (tmp_path / f"a:{i}.txt").write_text("needle\n")
    r = filesearch.search(d, "needle", "contents")
    assert len(r["hits"]) == 200 and r["truncated"] is True
    # Nenhum path cotado/escapado: todos os hits existem de verdade.
    assert all(os.path.exists(os.path.join(d, h["path"])) for h in r["hits"])


def test_nome_nao_lista_rastreado_apagado(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "gone.txt").write_text("x")
    git_ops._run(d, "add", ".")
    git_ops._run(d, "commit", "-q", "-m", "add gone")
    os.remove(os.path.join(d, "gone.txt"))
    r = filesearch.search(d, "gone", "names")
    assert r["hits"] == []


def test_nome_em_conflito_de_merge_aparece_uma_vez(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "c.txt").write_text("base\n")
    git_ops._run(d, "add", ".")
    git_ops._run(d, "commit", "-q", "-m", "base")
    git_ops._run(d, "checkout", "-q", "-b", "outro")
    (tmp_path / "c.txt").write_text("outro\n")
    git_ops._run(d, "add", ".")
    git_ops._run(d, "commit", "-q", "-m", "outro")
    git_ops._run(d, "checkout", "-q", "master")
    (tmp_path / "c.txt").write_text("master\n")
    git_ops._run(d, "add", ".")
    git_ops._run(d, "commit", "-q", "-m", "master")
    git_ops._run(d, "merge", "outro")
    paths = [h["path"] for h in filesearch.search(d, "c", "names")["hits"]]
    assert paths.count("c.txt") == 1


def test_teto_names_conta_unicos(tmp_path):
    d = _repo(tmp_path)
    for i in range(210):
        (tmp_path / f"m{i}.txt").write_text("x\n")
    r = filesearch.search(d, "m", "names")
    assert len(r["hits"]) == 200 and r["truncated"] is True
    assert len({h["path"] for h in r["hits"]}) == 200


def test_erro_do_git_vira_search_error_no_e_repo(tmp_path, monkeypatch):
    d = _repo(tmp_path)

    def quebra(cwd, *args, **kw):
        raise git_ops.GitError(500, "git nao encontrado")

    monkeypatch.setattr(git_ops, "_run", quebra)
    for modo in ("names", "contents"):
        with pytest.raises(SearchError) as e:
            filesearch.search(d, "x", modo)
        assert e.value.status == 500
        assert e.value.code == "erro_arq_busca_falhou"
        assert e.value.msg == "git nao encontrado"


def test_erro_do_git_vira_search_error_nas_buscas(tmp_path, monkeypatch):
    d = _repo(tmp_path)

    def quebra_nas_buscas(cwd, *args, **kw):
        if "rev-parse" in args:
            return subprocess.CompletedProcess(args, 0, stdout="true\n", stderr="")
        raise git_ops.GitError(500, "git nao encontrado")

    monkeypatch.setattr(git_ops, "_run", quebra_nas_buscas)
    for modo in ("names", "contents"):
        with pytest.raises(SearchError) as e:
            filesearch.search(d, "x", modo)
        assert e.value.status == 500
        assert e.value.code == "erro_arq_busca_falhou"


def test_quote_path_false_em_todas_as_chamadas_git(tmp_path, monkeypatch):
    d = _repo(tmp_path)
    chamadas = []

    def captura(cwd, *args, **kw):
        chamadas.append(args)
        out = "true\n" if "rev-parse" in args else ""
        return subprocess.CompletedProcess(args, 0, stdout=out, stderr="")

    monkeypatch.setattr(git_ops, "_run", captura)
    filesearch.search(d, "x", "names")
    filesearch.search(d, "x", "contents")
    assert len(chamadas) == 4  # rev-parse + ls-files no names; rev-parse + grep no contents
    for c in chamadas:
        assert "-c" in c and "core.quotePath=false" in c
