"""Cobertura do filetree: listar um nivel do repo da sessao e ler arquivo."""
import pytest
from fastapi.testclient import TestClient

from app import filetree, git_ops
from app.filetree import FileError
from app.config import settings
from app.api import app


@pytest.fixture
def cliente():
    """Mesmo arranjo de test_api.py: sem armar o token, toda rota devolve 401."""
    anterior = settings.auth_token
    settings.auth_token = "secret"
    yield TestClient(app)
    settings.auth_token = anterior


def _repo(tmp_path):
    """Repo git com um commit, pra changed_files funcionar."""
    d = str(tmp_path)
    git_ops._run(d, "init", "-q", ".")
    git_ops._run(d, "config", "user.email", "t@t")
    git_ops._run(d, "config", "user.name", "t")
    (tmp_path / "base.txt").write_text("base\n")
    git_ops._run(d, "add", "base.txt")
    git_ops._run(d, "commit", "-q", "-m", "base")
    return d


def test_lista_pastas_antes_de_arquivos(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "zeta").mkdir()
    (tmp_path / "alfa.txt").write_text("a")
    nomes = [e["name"] for e in filetree.list_dir(d, so_modificados=False)["entries"]]
    assert nomes.index("zeta") < nomes.index("alfa.txt")


def test_esconde_git_mas_mostra_dotfile(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / ".env.example").write_text("X=1")
    nomes = [e["name"] for e in filetree.list_dir(d, so_modificados=False)["entries"]]
    assert ".git" not in nomes
    assert ".env.example" in nomes


def test_recusa_escapar_da_raiz(tmp_path):
    d = _repo(tmp_path)
    for ruim in ("..", "../..", "/etc"):
        with pytest.raises(FileError) as e:
            filetree.list_dir(d, ruim)
        assert e.value.code == "erro_arq_fora_da_raiz"


def test_pasta_herda_marca_e_soma_do_neto(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "src" / "lib").mkdir(parents=True)
    alvo = tmp_path / "src" / "lib" / "x.txt"
    alvo.write_text("a\nb\n")
    git_ops._run(d, "add", "src/lib/x.txt")
    git_ops._run(d, "commit", "-q", "-m", "x")
    alvo.write_text("a\nb\nc\nd\n")
    src = [e for e in filetree.list_dir(d)["entries"] if e["name"] == "src"][0]
    assert src["changed"] == "M"
    assert src["add"] == 2 and src["del"] == 0


def test_nome_com_acento_nao_volta_escapado(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "sessão-única.md").write_text("x")
    paths = [e["path"] for e in filetree.list_dir(d)["entries"]]
    assert "sessão-única.md" in paths
    assert not any("\\303" in p for p in paths)


def test_so_modificados_esconde_intocado_mas_mantem_o_caminho(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "novo.txt").write_text("n")
    (tmp_path / "intocado.txt").write_text("i")
    git_ops._run(d, "add", "intocado.txt")
    git_ops._run(d, "commit", "-q", "-m", "i")
    nomes = [e["name"] for e in filetree.list_dir(d)["entries"]]
    assert "src" in nomes
    assert "intocado.txt" not in nomes


def test_sessao_aberta_numa_SUBPASTA_do_repo(tmp_path):
    """O git devolve caminho relativo ao TOPO do repo; a arvore lista relativo ao cwd DA
    SESSAO. Sem o prefixo, a arvore volta vazia no modo padrao. Nenhum outro teste pega
    isto, porque todos abrem no topo."""
    d = _repo(tmp_path)
    (tmp_path / "backend" / "app").mkdir(parents=True)
    alvo = tmp_path / "backend" / "app" / "x.py"
    alvo.write_text("a\n")
    git_ops._run(d, "add", "backend/app/x.py")
    git_ops._run(d, "commit", "-q", "-m", "x")
    alvo.write_text("a\nb\n")

    sub = str(tmp_path / "backend")          # a sessao vive AQUI, nao no topo
    ent = {e["name"]: e for e in filetree.list_dir(sub)["entries"]}
    assert "app" in ent, "arvore vazia: o prefixo do repo nao foi descontado"
    assert ent["app"]["changed"] == "M"
    assert ent["app"]["add"] == 1


def test_recusa_path_comecando_com_traco(tmp_path):
    """Global Constraint: nenhum path do cliente pode virar flag de git."""
    d = _repo(tmp_path)
    for ruim in ("-rf", "--output=/tmp/x"):
        with pytest.raises(FileError) as e:
            filetree.list_dir(d, ruim)
        assert e.value.code == "erro_arq_caminho_invalido"
        with pytest.raises(FileError):
            filetree.read_file(d, ruim)


def test_le_texto_inteiro(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "a.txt").write_text("linha\n")
    r = filetree.read_file(d, "a.txt")
    assert r["text"] == "linha\n" and r["truncated"] is False


def test_corta_arquivo_grande(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "g.txt").write_text("x" * (filetree.MAX_BYTES + 5000))
    r = filetree.read_file(d, "g.txt")
    assert r["truncated"] is True
    assert len(r["text"].encode()) <= filetree.MAX_BYTES


def test_recusa_binario(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "i.png").write_bytes(b"\x89PNG\r\n\x1a\n\x00\x00binario")
    with pytest.raises(FileError) as e:
        filetree.read_file(d, "i.png")
    assert e.value.status == 415 and e.value.code == "erro_arq_binario"


def test_nome_com_espaco_aparece_na_arvore(tmp_path):
    """`core.quotePath=false` cobre o acento e NAO o espaco — o porcelain cita os dois."""
    d = _repo(tmp_path)
    (tmp_path / "com espaco.txt").write_text("um\n")
    (tmp_path / "pasta com espaco").mkdir()
    (tmp_path / "pasta com espaco" / "x.txt").write_text("novo\n")
    git_ops._run(d, "add", "com espaco.txt")
    git_ops._run(d, "commit", "-q", "-m", "e")
    (tmp_path / "com espaco.txt").write_text("um\ndois\n")
    ent = {e["name"]: e for e in filetree.list_dir(d)["entries"]}
    assert "com espaco.txt" in ent, "arquivo com espaco sumiu da arvore"
    assert ent["com espaco.txt"]["changed"] == "M"
    assert ent["com espaco.txt"]["add"] == 1
    assert "pasta com espaco" in ent


def test_git_interno_fora_do_alcance(tmp_path):
    """`.git` esconder-se da LISTA nao basta: o cliente manda o caminho que quiser, e o
    `.git/config` carrega o token do remote (o `_scrub` do git_ops existe por isso).
    O symlink `atalho -> .git` cobre a regua "vale pro alvo real depois de resolver
    symlink": quem escapa de um guard que olha a STRING pedida e justamente o caminho
    que nao tem `.git` escrito nele."""
    import os
    d = _repo(tmp_path)
    git_ops._run(d, "remote", "add", "origin", "https://u:TOKEN@github.com/x/y.git")
    os.symlink(".git", tmp_path / "atalho")
    for alvo in (".git", ".git/config", ".git/logs/HEAD", "./.git/config", "atalho", "atalho/config"):
        for fn in (filetree.list_dir, filetree.read_file):
            with pytest.raises(FileError) as e:
                fn(d, alvo)
            assert e.value.code == "erro_arq_area_do_git", (fn.__name__, alvo)
    (tmp_path / ".gitignore").write_text("node_modules\n")
    assert filetree.read_file(d, ".gitignore")["text"] == "node_modules\n"


def test_nao_trava_em_fifo_nem_estoura_em_socket(tmp_path):
    """FIFO sem escritor bloqueia no open() e come uma thread do pool pra sempre."""
    import os, socket
    d = _repo(tmp_path)
    os.mkfifo(str(tmp_path / "cano"))
    with pytest.raises(FileError) as e:
        filetree.read_file(d, "cano")
    assert e.value.code == "erro_arq_nao_e_arquivo"
    s = socket.socket(socket.AF_UNIX)
    try:
        s.bind(str(tmp_path / "soq"))
        with pytest.raises(FileError):
            filetree.read_file(d, "soq")
    finally:
        s.close()


def test_recusa_binario_com_nul_depois_dos_8192(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "tardio.bin").write_bytes(b"A" * 8192 + b"\x00" * 16 + b"B" * 100)
    with pytest.raises(FileError) as e:
        filetree.read_file(d, "tardio.bin")
    assert e.value.status == 415 and e.value.code == "erro_arq_binario"


def test_rota_list_exige_auth(cliente):
    assert cliente.get("/api/sessions/x/files/list").status_code == 401


def test_rota_binario_devolve_envelope(monkeypatch, tmp_path, cliente):
    from app import api
    d = _repo(tmp_path)
    (tmp_path / "i.png").write_bytes(b"\x89PNG\x00bin")
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.get("/api/sessions/s/files/read", params={"path": "i.png"},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 415
    assert r.json()["detail"]["code"] == "erro_arq_binario"     # envelope, nao texto solto


def test_rota_busca_vazia_devolve_envelope(monkeypatch, tmp_path, cliente):
    from app import api
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.get("/api/sessions/s/files/search", params={"q": "   "},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "erro_arq_busca_vazia"


def test_rota_path_diff_fora_de_repo_devolve_envelope(monkeypatch, tmp_path, cliente):
    from app import api
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path))
    r = cliente.post("/api/sessions/s/git/path-diff", json={"path": "x.txt"},
                     headers={"Authorization": "Bearer secret"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_git_diff"


def test_symlink_carrega_o_proprio_nome_e_a_propria_marca(tmp_path):
    """O `path` tem que ser o do LINK, nao o do alvo: com o do alvo, o link novo some do modo
    padrao (a marca do git nunca casa) e um link intocado herda o 'M' do vizinho."""
    import os
    d = _repo(tmp_path)
    (tmp_path / "alvo.txt").write_text("um\n")
    git_ops._run(d, "add", "alvo.txt")
    git_ops._run(d, "commit", "-q", "-m", "alvo")
    os.symlink("alvo.txt", tmp_path / "link-novo.txt")

    ent = {e["name"]: e for e in filetree.list_dir(d)["entries"]}
    assert "link-novo.txt" in ent, "symlink novo sumiu do modo padrao"
    assert ent["link-novo.txt"]["path"] == "link-novo.txt"

    git_ops._run(d, "add", "link-novo.txt")
    git_ops._run(d, "commit", "-q", "-m", "link")
    (tmp_path / "alvo.txt").write_text("um\ndois\n")          # so o ALVO muda
    ent = {e["name"]: e for e in filetree.list_dir(d)["entries"]}
    assert "link-novo.txt" not in ent, "link intocado herdou a marca do alvo"
    assert ent["alvo.txt"]["changed"] == "M"
