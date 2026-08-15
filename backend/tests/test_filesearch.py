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
