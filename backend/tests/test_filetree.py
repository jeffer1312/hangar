"""Cobertura do filetree: listar um nivel do repo da sessao e ler arquivo."""
import os
import subprocess

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
    for ruim in ("..", "../.."):
        with pytest.raises(FileError) as e:
            filetree.list_dir(d, ruim)
        assert e.value.code == "erro_arq_fora_da_raiz"


def test_recusa_caminho_absoluto(tmp_path):
    """So caminho RELATIVO: absoluto e recusado antes do join, mesmo dentro do cwd.
    (O "/etc" do teste acima caiu nesta regua: e recusado como invalido, nao como fora.)"""
    d = _repo(tmp_path)
    for fn in (filetree.list_dir, filetree.read_file):
        with pytest.raises(FileError) as e:
            fn(d, str(tmp_path / "base.txt"))
        assert e.value.code == "erro_arq_caminho_invalido"


def test_repo_sob_pasta_ancestral_git_lista_e_le(tmp_path):
    """Pasta ANCESTRAL chamada .git nao e area interna: um repo legitimo em
    /tmp/.git/projeto tem o git-dir em /tmp/.git/projeto/.git, e a raiz fica FORA dele.
    (O guard por `raiz.parts` recusava tudo — medido no parecer 2ac646c.)"""
    (tmp_path / ".git" / "projeto").mkdir(parents=True)
    d = _repo(tmp_path / ".git" / "projeto")
    r = filetree.list_dir(d, so_modificados=False)
    assert any(e["name"] == "base.txt" for e in r["entries"])
    assert filetree.read_file(d, "base.txt")["text"] == "base\n"
    # O git-dir REAL continua recusado, via symlink ou por caminho.
    for ruim in (".git/config", "atalho/config"):
        if ruim == "atalho/config":
            os.symlink(".git", tmp_path / ".git" / "projeto" / "atalho")
        with pytest.raises(FileError) as e:
            filetree.read_file(d, ruim)
        assert e.value.code == "erro_arq_area_do_git", ruim


def test_repo_sem_commit_lista_sem_numstat(tmp_path):
    """Sem HEAD, `diff --numstat HEAD` sai 128 — e o caso legitimo de vazio, nao falha.
    O `_numstat` devolve {} e a lista sai sem somas (nem marca, nem erro)."""
    d = str(tmp_path)
    git_ops._run(d, "init", "-q", ".")
    (tmp_path / "a.txt").write_text("x")
    r = filetree.list_dir(d, so_modificados=False)
    assert any(e["name"] == "a.txt" for e in r["entries"])


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


# ===== Round de correcao: bloqueadores do parecer 35a69cd =====


def test_rota_path_diff_nao_atravessa_cwd(monkeypatch, tmp_path, cliente):
    """Sessao aberta em SUBPASTA do repo: ../segredo.txt esta DENTRO do repo git mas FORA
    do cwd da sessao — o diff tem que recusar, nao ler o arquivo do vizinho."""
    from app import api
    d = _repo(tmp_path)
    (tmp_path / "sub").mkdir()
    (tmp_path / "segredo.txt").write_text("SENHA\n")
    sub = str(tmp_path / "sub")
    monkeypatch.setattr(api, "_session_cwd", lambda name: sub)
    r = cliente.post("/api/sessions/s/git/path-diff",
                     json={"path": "../segredo.txt", "escopo": "nao_commitado"},
                     headers={"Authorization": "Bearer secret"})
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "erro_git_diff"


def test_rota_recusa_caminho_absoluto(monkeypatch, tmp_path, cliente):
    from app import api
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    h = {"Authorization": "Bearer secret"}
    absoluto = str(tmp_path / "base.txt")
    r = cliente.get("/api/sessions/s/files/list", params={"path": absoluto}, headers=h)
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_arq_caminho_invalido"
    r = cliente.get("/api/sessions/s/files/read", params={"path": absoluto}, headers=h)
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_arq_caminho_invalido"
    r = cliente.post("/api/sessions/s/git/path-diff", json={"path": absoluto}, headers=h)
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_git_diff"


def test_rota_symlink_que_atravessa_cwd_recusado(monkeypatch, tmp_path, cliente):
    import os
    from app import api
    d = _repo(tmp_path)
    (tmp_path / "sub").mkdir()
    (tmp_path / "segredo.txt").write_text("SENHA\n")
    os.symlink("../segredo.txt", tmp_path / "sub" / "elo")
    sub = str(tmp_path / "sub")
    monkeypatch.setattr(api, "_session_cwd", lambda name: sub)
    h = {"Authorization": "Bearer secret"}
    r = cliente.get("/api/sessions/s/files/read", params={"path": "elo"}, headers=h)
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_arq_fora_da_raiz"
    r = cliente.post("/api/sessions/s/git/path-diff",
                     json={"path": "elo", "escopo": "nao_commitado"}, headers=h)
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_git_diff"


def test_rota_path_diff_trata_asterisco_como_literal(monkeypatch, tmp_path, cliente):
    """Arquivo real chamado `*`: sem --literal-pathspecs o diff devolvia a soma de TODOS
    os arquivos modificados (medido no parecer). O diff tem que ser so do arquivo `*`."""
    from app import api, git_ops
    d = _repo(tmp_path)
    (tmp_path / "*").write_text("estrela\n")
    (tmp_path / "a.txt").write_text("um\n")
    (tmp_path / "b.txt").write_text("dois\n")
    git_ops._run(d, "add", ".")
    git_ops._run(d, "commit", "-q", "-m", "base")
    (tmp_path / "*").write_text("estrela\nestrela2\n")
    (tmp_path / "a.txt").write_text("um\nAAA\n")
    (tmp_path / "b.txt").write_text("dois\nBBB\n")
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.post("/api/sessions/s/git/path-diff",
                     json={"path": "*", "escopo": "nao_commitado"},
                     headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    diff = r.json()["diff"]
    assert "+AAA" not in diff and "+BBB" not in diff
    assert "estrela2" in diff


def test_rota_cwd_dentro_do_git_recusado(monkeypatch, tmp_path, cliente):
    """Cwd da sessao em /repo/.git: `config` e um caminho RELATIVO limpo, e o guard do
    alvo nunca ve o .git — a RAIZ tem que ser recusada tambem. Symlink pro .git idem
    (o realpath resolve antes de comparar)."""
    import os
    from app import api, git_ops
    d = _repo(tmp_path)
    git_ops._run(d, "remote", "add", "origin", "https://u:TOKEN@github.com/x/y.git")
    gitdir = str(tmp_path / ".git")
    h = {"Authorization": "Bearer secret"}
    monkeypatch.setattr(api, "_session_cwd", lambda name: gitdir)
    r = cliente.get("/api/sessions/s/files/read", params={"path": "config"}, headers=h)
    assert r.status_code == 403 and r.json()["detail"]["code"] == "erro_arq_area_do_git"
    r = cliente.get("/api/sessions/s/files/list", headers=h)
    assert r.status_code == 403 and r.json()["detail"]["code"] == "erro_arq_area_do_git"
    os.symlink(".git", tmp_path / "atalho")
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path / "atalho"))
    r = cliente.get("/api/sessions/s/files/read", params={"path": "config"}, headers=h)
    assert r.status_code == 403 and r.json()["detail"]["code"] == "erro_arq_area_do_git"


def test_rota_pasta_comum_lista_e_le(monkeypatch, tmp_path, cliente):
    """Regra do usuario: a arvore FUNCIONA fora de repo git. Pasta comum lista tudo sem
    marca e sem soma, e le arquivos normalmente; um arquivo comum chamado `config` nao
    confunde. O registro em registrar_estado_git simula a classificacao da criacao da
    sessao (o api registra antes da sessao nascer)."""
    from app import api, filetree
    (tmp_path / "leia.txt").write_text("texto fora de git\n")
    (tmp_path / "config").write_text("config comum\n")
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "x.txt").write_text("x\n")
    filetree.registrar_estado_git(str(tmp_path))
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path))
    h = {"Authorization": "Bearer secret"}
    r = cliente.get("/api/sessions/s/files/list", params={"so_modificados": "false"}, headers=h)
    assert r.status_code == 200
    ent = {e["name"]: e for e in r.json()["entries"]}
    assert "leia.txt" in ent and "config" in ent and "sub" in ent
    assert all(e["changed"] is None and e["add"] == 0 and e["del"] == 0 for e in ent.values())
    # so_modificados padrao (true) tambem lista: fora de git nao ha marcas pra filtrar.
    r = cliente.get("/api/sessions/s/files/list", headers=h)
    assert r.status_code == 200
    assert any(e["name"] == "leia.txt" for e in r.json()["entries"])
    r = cliente.get("/api/sessions/s/files/read", params={"path": "leia.txt"}, headers=h)
    assert r.status_code == 200
    assert r.json()["text"] == "texto fora de git\n"


def test_rota_pasta_comum_plausivel_3_marcadores(monkeypatch, tmp_path, cliente):
    """Pasta comum PLAUSIVEL com config+objects+refs (cache de app, nao git): registrada
    como pasta comum na criacao, list e read funcionam — a assinatura por nomes NAO pode
    bloquear (parecer 47612d58, B2)."""
    from app import api, filetree
    (tmp_path / "config").write_text("config comum\n")
    (tmp_path / "objects").mkdir()
    (tmp_path / "refs").mkdir()
    (tmp_path / "README.txt").write_text("comum\n")
    filetree.registrar_estado_git(str(tmp_path))
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path))
    h = {"Authorization": "Bearer secret"}
    r = cliente.get("/api/sessions/s/files/read", params={"path": "README.txt"}, headers=h)
    assert r.status_code == 200
    assert r.json()["text"] == "comum\n"
    r = cliente.get("/api/sessions/s/files/list", params={"so_modificados": "false"}, headers=h)
    assert r.status_code == 200
    assert any(e["name"] == "README.txt" for e in r.json()["entries"])


def test_rota_git_dir_ambiguo_nao_libera(monkeypatch, tmp_path, cliente):
    """Diretorio arbitrario JA corrompido antes da primeira classificacao (bare sem
    sufixo, so HEAD+config com token): sem resposta do git e SEM estado de sessao, e
    AMBIGUO — erro estruturado, nunca bytes (parecer 47612d58, B1: n>=3 deixava passar
    com 2 marcadores)."""
    import shutil
    from app import api, git_ops
    bare = tmp_path / "arbitrary-bare"
    bare.mkdir()
    d = str(bare)
    git_ops._run(d, "init", "-q", "--bare")
    git_ops._run(d, "remote", "add", "origin", "https://u:TWOMARKERTOKEN@host/x.git")
    (bare / "HEAD").write_text("not-a-ref\n")
    shutil.rmtree(bare / "objects")
    shutil.rmtree(bare / "refs")
    h = {"Authorization": "Bearer secret"}
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.get("/api/sessions/s/files/read", params={"path": "config"}, headers=h)
    assert r.status_code == 500
    assert r.json()["detail"]["code"] == "erro_arq_lista_falhou"
    assert "TWOMARKERTOKEN" not in r.text
    r = cliente.get("/api/sessions/s/files/list", headers=h)
    assert r.status_code == 500
    assert r.json()["detail"]["code"] == "erro_arq_lista_falhou"


def test_rota_busca_fora_de_repo_continua_409(monkeypatch, tmp_path, cliente):
    """A busca NAO muda com a regra da pasta comum: fora de repo ela continua exigindo
    git e explicando com 409 erro_arq_nao_e_repo_git."""
    from app import api
    (tmp_path / "x.txt").write_text("agulha\n")
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path))
    r = cliente.get("/api/sessions/s/files/search", params={"q": "agulha"},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_arq_nao_e_repo_git"


def test_rota_list_falha_do_git_vira_envelope(monkeypatch, tmp_path, cliente):
    from app import api, git_ops
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)

    def quebra(cwd):
        raise git_ops.GitError(500, "git quebrou")

    monkeypatch.setattr(git_ops, "changed_files", quebra)
    r = cliente.get("/api/sessions/s/files/list", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 500
    assert r.json()["detail"]["code"] == "erro_arq_lista_falhou"


def test_rota_list_pasta_sem_permissao_vira_envelope(monkeypatch, tmp_path, cliente):
    from app import api
    d = _repo(tmp_path)
    (tmp_path / "privada").mkdir()
    (tmp_path / "privada").chmod(0o000)
    try:
        monkeypatch.setattr(api, "_session_cwd", lambda name: d)
        r = cliente.get("/api/sessions/s/files/list",
                        params={"path": "privada", "so_modificados": "false"},
                        headers={"Authorization": "Bearer secret"})
        assert r.status_code == 403
        assert r.json()["detail"]["code"] == "erro_arq_sem_permissao"
    finally:
        (tmp_path / "privada").chmod(0o755)


def test_rota_nul_recusado_em_todas_as_rotas(monkeypatch, tmp_path, cliente):
    from app import api
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    h = {"Authorization": "Bearer secret"}
    r = cliente.get("/api/sessions/s/files/list", params={"path": "\x00"}, headers=h)
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_arq_caminho_invalido"
    r = cliente.get("/api/sessions/s/files/read", params={"path": "\x00"}, headers=h)
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_arq_caminho_invalido"
    r = cliente.get("/api/sessions/s/files/search", params={"q": "\x00", "mode": "contents"}, headers=h)
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_arq_busca_falhou"
    r = cliente.post("/api/sessions/s/git/path-diff",
                     json={"path": "\x00", "escopo": "nao_commitado"}, headers=h)
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_git_diff"


def test_rota_modo_invalido_devolve_envelope(monkeypatch, tmp_path, cliente):
    from app import api
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.get("/api/sessions/s/files/search", params={"q": "x", "mode": "evil"},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "erro_arq_modo_invalido"


def test_rota_escopo_invalido_devolve_envelope(monkeypatch, tmp_path, cliente):
    from app import api
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.post("/api/sessions/s/git/path-diff",
                     json={"path": "base.txt", "escopo": "evil"},
                     headers={"Authorization": "Bearer secret"})
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "erro_git_diff"


def test_rota_nao_vaza_detalhe_interno_da_busca(monkeypatch, tmp_path, cliente):
    """O detail de um SearchError pode carregar stderr do git com caminho absoluto —
    vai pro log, nunca pro corpo da resposta."""
    from app import api
    from app.filesearch import SearchError
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)

    def quebra(cwd, q, mode):
        raise SearchError(409, "erro_arq_busca_falhou", "/home/privado/arquivo.txt: valor-secreto")

    monkeypatch.setattr(api.filesearch, "search", quebra)
    r = cliente.get("/api/sessions/s/files/search", params={"q": "x"},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 409
    corpo = r.text
    assert "/home/privado" not in corpo and "valor-secreto" not in corpo
    assert r.json()["detail"]["code"] == "erro_arq_busca_falhou"


def test_rota_nao_vaza_detalhe_interno_do_diff(monkeypatch, tmp_path, cliente):
    from app import api
    from app.git_ops import GitError
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)

    def quebra(cwd, path, escopo):
        raise GitError(409, "/home/privado/arquivo.txt: valor-secreto")

    monkeypatch.setattr(api.git_ops, "path_diff", quebra)
    r = cliente.post("/api/sessions/s/git/path-diff", json={"path": "base.txt"},
                     headers={"Authorization": "Bearer secret"})
    assert r.status_code == 409
    corpo = r.text
    assert "/home/privado" not in corpo and "valor-secreto" not in corpo
    assert r.json()["detail"]["code"] == "erro_git_diff"


# ===== Round 2 de correcao: bloqueadores do parecer 2ac646c =====


def test_rota_repo_sob_pasta_ancestral_git_lista_e_le(monkeypatch, tmp_path, cliente):
    from app import api
    (tmp_path / ".git" / "projeto").mkdir(parents=True)
    d = _repo(tmp_path / ".git" / "projeto")
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    h = {"Authorization": "Bearer secret"}
    r = cliente.get("/api/sessions/s/files/list", params={"so_modificados": "false"}, headers=h)
    assert r.status_code == 200
    r = cliente.get("/api/sessions/s/files/read", params={"path": "base.txt"}, headers=h)
    assert r.status_code == 200


def test_rota_numstat_falha_vira_envelope(monkeypatch, tmp_path, cliente):
    """Falha do `git diff --numstat HEAD` NAO pode virar 200 com add=0/del=0 (resposta
    falsa, medida no parecer): o cliente recebe o envelope erro_arq_lista_falhou."""
    from app import api, git_ops
    d = _repo(tmp_path)
    (tmp_path / "base.txt").write_text("base\nmexido\n")
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    orig = git_ops._run

    def quebra_numstat(cwd, *args, **kw):
        if "--numstat" in args:
            return subprocess.CompletedProcess(args, 128, stdout="", stderr="fatal: quebrado")
        return orig(cwd, *args, **kw)

    monkeypatch.setattr(git_ops, "_run", quebra_numstat)
    r = cliente.get("/api/sessions/s/files/list", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 500
    assert r.json()["detail"]["code"] == "erro_arq_lista_falhou"


def test_rota_probe_git_falhou_vira_envelope(monkeypatch, tmp_path, cliente):
    """Falha de subprocesso (git ausente/timeout) no probe NAO e "fora de repo": antes
    virava 500 sem JSON (GitError cru escapando do _e_repo — traceback no parecer)."""
    from app import api, git_ops
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)

    def quebra(cwd, *args, **kw):
        raise git_ops.GitError(500, "git nao encontrado")

    monkeypatch.setattr(git_ops, "_run", quebra)
    r = cliente.get("/api/sessions/s/files/list", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 500
    assert r.json()["detail"]["code"] == "erro_arq_lista_falhou"


def test_rota_changed_files_504_preserva_status(monkeypatch, tmp_path, cliente):
    """504 de timeout do git chega como 504 (o cliente decide retry), nunca 500 fixo."""
    from app import api, git_ops
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)

    def quebra(cwd):
        raise git_ops.GitError(504, "timeout")

    monkeypatch.setattr(git_ops, "changed_files", quebra)
    r = cliente.get("/api/sessions/s/files/list", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 504
    assert r.json()["detail"]["code"] == "erro_arq_lista_falhou"


# ===== Round 3 de correcao: bloqueadores do parecer 72e866e3 =====


def test_rota_config_malformada_nao_libera_git_dir(monkeypatch, tmp_path, cliente):
    """Sessao GIT cuja config quebra DEPOIS da criacao: o gitdir guardado no estado da
    sessao continua recusando raiz/alvo dentro dele (parecer 47612d58, passo 4)."""
    from app import api, filetree, git_ops
    d = _repo(tmp_path)
    git_ops._run(d, "remote", "add", "origin", "https://u:TOKEN@github.com/x/y.git")
    filetree.registrar_estado_git(str(tmp_path / ".git"))
    (tmp_path / ".git" / "config").write_text(
        "[malformado\n[remote \"origin\"]\n\turl = https://u:TOKEN@github.com/x/y.git\n")
    h = {"Authorization": "Bearer secret"}
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path / ".git"))
    r = cliente.get("/api/sessions/s/files/read", params={"path": "config"}, headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"
    assert "TOKEN" not in r.text
    r = cliente.get("/api/sessions/s/files/list", headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"


def test_rota_alias_cwd_git_dir_config_malformada(monkeypatch, tmp_path, cliente):
    """Symlink DIRETO para o git-dir como cwd: a sessao criada com o cwd ALIAS classifica
    o alias como git-dir (realpath); config malformada depois nao muda a classificacao."""
    import os
    from app import api, filetree, git_ops
    repo_dir = tmp_path / "repo"
    repo_dir.mkdir()
    d = _repo(repo_dir)
    git_ops._run(d, "remote", "add", "origin", "https://u:TOKEN@github.com/x/y.git")
    os.symlink(repo_dir / ".git", tmp_path / "git-area-alias")
    filetree.registrar_estado_git(str(tmp_path / "git-area-alias"))
    (repo_dir / ".git" / "config").write_text(
        "[malformado\n[remote \"origin\"]\n\turl = https://u:TOKEN@github.com/x/y.git\n")
    h = {"Authorization": "Bearer secret"}
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path / "git-area-alias"))
    r = cliente.get("/api/sessions/s/files/read", params={"path": "config"}, headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"
    assert "TOKEN" not in r.text
    r = cliente.get("/api/sessions/s/files/list", headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"


def test_rota_git_dir_incompleto_config_malformada(monkeypatch, tmp_path, cliente):
    """Git-dir INCOMPLETO (refs removido) com config malformada, numa sessao classificada
    como Git na criacao: o gitdir guardado continua recusando (parecer 71d7b190)."""
    import shutil
    from app import api, filetree, git_ops
    d = _repo(tmp_path)
    git_ops._run(d, "remote", "add", "origin", "https://u:PARTIALTOKEN@github.com/x/y.git")
    filetree.registrar_estado_git(str(tmp_path / ".git"))
    (tmp_path / ".git" / "config").write_text(
        "[malformado\n[remote \"origin\"]\n\turl = https://u:PARTIALTOKEN@github.com/x/y.git\n")
    shutil.rmtree(tmp_path / ".git" / "refs")
    h = {"Authorization": "Bearer secret"}
    monkeypatch.setattr(api, "_session_cwd", lambda name: str(tmp_path / ".git"))
    r = cliente.get("/api/sessions/s/files/read", params={"path": "config"}, headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"
    assert "PARTIALTOKEN" not in r.text
    r = cliente.get("/api/sessions/s/files/list", headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"


def test_rota_bare_git_dir_config_malformada(monkeypatch, tmp_path, cliente):
    """Bare git-dir (nome termina em .git) com config malformada, numa sessao
    classificada como Git na criacao: o gitdir guardado continua recusando."""
    from app import api, filetree, git_ops
    bare = tmp_path / "bare-repository.git"
    bare.mkdir()
    d = str(bare)
    git_ops._run(d, "init", "-q", "--bare")
    git_ops._run(d, "remote", "add", "origin", "https://u:PARTIALTOKEN@github.com/x/y.git")
    filetree.registrar_estado_git(d)
    (bare / "config").write_text(
        "[malformado\n[remote \"origin\"]\n\turl = https://u:PARTIALTOKEN@github.com/x/y.git\n")
    h = {"Authorization": "Bearer secret"}
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.get("/api/sessions/s/files/read", params={"path": "config"}, headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"
    assert "PARTIALTOKEN" not in r.text
    r = cliente.get("/api/sessions/s/files/list", headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"


def test_rota_bare_sem_sufixo_head_quebrado(monkeypatch, tmp_path, cliente):
    """Bare git-dir SEM sufixo com HEAD corrompido, numa sessao classificada como Git na
    criacao: o gitdir guardado recusa — o stderr nunca participou da decisao (parecer
    99916b58)."""
    from app import api, filetree, git_ops
    bare = tmp_path / "bare-repository"
    bare.mkdir()
    d = str(bare)
    git_ops._run(d, "init", "-q", "--bare")
    git_ops._run(d, "remote", "add", "origin", "https://u:BADHEADTOKEN@github.com/x/y.git")
    filetree.registrar_estado_git(d)
    (bare / "HEAD").write_text("not-a-ref\n")
    h = {"Authorization": "Bearer secret"}
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.get("/api/sessions/s/files/read", params={"path": "config"}, headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"
    assert "BADHEADTOKEN" not in r.text
    r = cliente.get("/api/sessions/s/files/list", headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"


def test_rota_bare_sem_sufixo_head_removido(monkeypatch, tmp_path, cliente):
    """Mesma classe com HEAD REMOVIDO: a protecao vem do estado da sessao, nao do
    arquivo HEAD existir nem de nomes (parecer 219a9e09)."""
    from app import api, filetree, git_ops
    bare = tmp_path / "bare-repository"
    bare.mkdir()
    d = str(bare)
    git_ops._run(d, "init", "-q", "--bare")
    git_ops._run(d, "remote", "add", "origin", "https://u:NOHEADTOKEN@github.com/x/y.git")
    filetree.registrar_estado_git(d)
    (bare / "HEAD").unlink()
    h = {"Authorization": "Bearer secret"}
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.get("/api/sessions/s/files/read", params={"path": "config"}, headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"
    assert "NOHEADTOKEN" not in r.text
    r = cliente.get("/api/sessions/s/files/list", headers=h)
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_arq_area_do_git"


def test_rota_head_probe_timeout_vira_envelope(monkeypatch, tmp_path, cliente):
    """A probe de HEAD levantando GitError NAO pode escapar cru (500 text/plain, medido
    no parecer): vira 504 com envelope, status preservado."""
    from app import api, git_ops
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    orig = git_ops._run

    def quebra(cwd, *args, **kw):
        if "--verify" in args:
            raise git_ops.GitError(504, "HEAD probe timeout")
        return orig(cwd, *args, **kw)

    monkeypatch.setattr(git_ops, "_run", quebra)
    r = cliente.get("/api/sessions/s/files/list", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 504
    assert r.json()["detail"]["code"] == "erro_arq_lista_falhou"


def test_rota_head_quebrado_nao_vira_lista_vazia(monkeypatch, tmp_path, cliente):
    """Probe de HEAD com rc=128 e stderr e FALHA, nao "repo sem commit": lista vazia
    calada esconderia o HEAD quebrado (medido no parecer)."""
    import subprocess
    from app import api, git_ops
    d = _repo(tmp_path)
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    orig = git_ops._run

    def quebra(cwd, *args, **kw):
        if "--verify" in args:
            return subprocess.CompletedProcess(args, 128, stdout="", stderr="fatal: broken HEAD")
        return orig(cwd, *args, **kw)

    monkeypatch.setattr(git_ops, "_run", quebra)
    r = cliente.get("/api/sessions/s/files/list", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 500
    assert r.json()["detail"]["code"] == "erro_arq_lista_falhou"


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
