"""filesearch.resolver: o que a visão "citados" pode listar, e onde cada caminho está."""
import subprocess

from app import filesearch


def _repo(tmp_path):
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / "backend" / "tests").mkdir(parents=True)
    (tmp_path / "backend" / "tests" / "test_x.py").write_text("1", encoding="utf-8")
    (tmp_path / "backend" / "app").mkdir()
    (tmp_path / "backend" / "app" / "x.py").write_text("1", encoding="utf-8")
    (tmp_path / "x.py").write_text("raiz", encoding="utf-8")
    return str(tmp_path)


def test_existe_no_cwd_e_relativo_a_outra_pasta(tmp_path):
    cwd = _repo(tmp_path)
    r = filesearch.resolver(cwd, ["backend/app/x.py", "tests/test_x.py", "./x.py", "nao/existe.py"])
    assert r["ok"]["backend/app/x.py"]["relativo"] == "backend/app/x.py"
    assert r["ok"]["tests/test_x.py"]["relativo"] == "backend/tests/test_x.py"
    assert r["ok"]["./x.py"]["relativo"] == "x.py"
    assert r["ok"]["./x.py"]["real"] == str((tmp_path / "x.py").resolve())
    assert r["faltam"] == ["nao/existe.py"]


def test_symlink_e_til_apontam_pro_mesmo_real(tmp_path, monkeypatch):
    cwd = _repo(tmp_path)
    alvo = tmp_path.parent / "fora.md"
    alvo.write_text("x", encoding="utf-8")
    link = tmp_path.parent / "link-fora.md"
    if link.exists() or link.is_symlink():
        link.unlink()
    link.symlink_to(alvo)
    r = filesearch.resolver(cwd, [str(alvo), str(link)])
    assert r["ok"][str(alvo)]["real"] == r["ok"][str(link)]["real"]


def test_sufixo_mais_longo_vence_o_homonimo(tmp_path):
    cwd = _repo(tmp_path)
    r = filesearch.resolver(cwd, ["app/x.py"])
    assert r["ok"]["app/x.py"]["relativo"] == "backend/app/x.py"


def test_absoluto_dentro_e_fora_do_cwd(tmp_path):
    cwd = _repo(tmp_path)
    fora = tmp_path.parent / "fora.md"
    fora.write_text("x", encoding="utf-8")
    r = filesearch.resolver(cwd, [str(tmp_path / "x.py"), str(fora), str(tmp_path / "sumiu.md")])
    assert r["ok"][str(tmp_path / "x.py")]["relativo"] == "x.py"
    assert r["ok"][str(fora)]["relativo"] is None
    assert r["faltam"] == [str(tmp_path / "sumiu.md")]


def test_fora_de_repo_so_confere_existencia(tmp_path):
    (tmp_path / "a.txt").write_text("1", encoding="utf-8")
    r = filesearch.resolver(str(tmp_path), ["a.txt", "b.txt", "sub/a.txt"])
    assert list(r["ok"]) == ["a.txt"] and r["ok"]["a.txt"]["relativo"] == "a.txt"
    assert r["faltam"] == ["b.txt", "sub/a.txt"]
