from pathlib import Path

import pytest

from app import uploads
from app.uploads import save_upload, UploadError

# 1x1 PNG valido (bytes minimos)
PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
    "0000000a49444154789c6360000002000154a24f5f0000000049454e44ae426082"
)

SESSAO = "sessao-teste"


@pytest.fixture(autouse=True)
def cofre(tmp_path, monkeypatch):
    """Desvia o cofre pra `tmp_path`. Sem isto a suite escreveria em `~/.hangar/uploads` DE VERDADE
    — os anexos deixaram de morar no cwd, entao `tmp_path` sozinho nao isola mais nada."""
    raiz = tmp_path / "cofre"
    monkeypatch.setattr(uploads, "_raiz", lambda: raiz)
    return raiz


def _pasta(cofre, tmp_path, sessao=SESSAO):
    return cofre / uploads._projeto(str(tmp_path)) / sessao


def test_save_upload_writes_into_cofre_por_projeto_e_sessao(tmp_path, cofre):
    path = save_upload(str(tmp_path), SESSAO, PNG, "foto.png")
    p = Path(path)
    assert p.exists()
    assert p.read_bytes() == PNG
    assert p.parent == _pasta(cofre, tmp_path)
    assert p.suffix == ".png"
    # o defeito que a mudanca corrige: nada e escrito dentro do projeto
    assert not (tmp_path / ".hangar-uploads").exists()


def test_sessoes_diferentes_nao_se_misturam(tmp_path, cofre):
    a = Path(save_upload(str(tmp_path), "sessao-a", PNG, "foto.png"))
    b = Path(save_upload(str(tmp_path), "sessao-b", PNG, "foto.png"))
    assert a.parent != b.parent
    assert a.parent.parent == b.parent.parent  # mesmo projeto


def test_nome_de_sessao_hostil_nao_escapa_do_cofre(tmp_path, cofre):
    # nome de sessao vem de fora (tmux); `..` subiria um nivel se fosse concatenado cru
    for hostil in ["..", "../..", "a/b", "..\\x"]:
        p = Path(save_upload(str(tmp_path), hostil, PNG, "x.png"))
        assert cofre.resolve() in p.resolve().parents


def test_acento_no_nome_do_projeto_vira_letra_base(tmp_path, cofre, monkeypatch):
    # "Área de trabalho" saía como "rea-de-trabalho": o filtro comia o Á e o strip levava o resto.
    projeto = tmp_path / "Área de trabalho"
    projeto.mkdir()
    p = Path(save_upload(str(projeto), SESSAO, PNG, "x.png"))
    assert p.parent.parent.name.startswith("Area-de-trabalho-")


def test_projetos_de_mesmo_nome_em_lugares_diferentes_nao_se_misturam(tmp_path, cofre):
    # Dois checkouts chamados "api": com o basename sozinho caiam no mesmo balde, a galeria de um
    # mostrava o anexo do outro e o prune apagava os dois.
    a, b = tmp_path / "work" / "api", tmp_path / "side" / "api"
    a.mkdir(parents=True)
    b.mkdir(parents=True)
    pa = Path(save_upload(str(a), SESSAO, PNG, "a.png"))
    pb = Path(save_upload(str(b), SESSAO, PNG, "b.png"))
    assert pa.parent != pb.parent
    assert len(list_uploads(str(a), SESSAO, 30)) == 1
    _envelhecer(str(pb), 40)
    assert prune_old(str(a), 30) == 0      # varrer um nao alcanca o outro
    assert pb.exists()


def test_a_pasta_e_chaveada_pelo_ID_entao_o_rename_nao_a_alcanca(tmp_path):
    # O identificador é o id da sessão (models.session_key), não o nome: renomear a sessão não muda
    # o id, então a galeria continua de pé sem ninguém precisar mover pasta.
    from app.models import session_key

    sid = session_key("/qualquer/projects/dir/0fa9dee8-3392-4bf1-a65a-95cd0007cf60.jsonl")
    p = Path(save_upload(str(tmp_path), sid, PNG, "x.png"))
    assert p.parent.name == "0fa9dee8-3392-4bf1-a65a-95cd0007cf60"
    assert [f["filename"] for f in list_uploads(str(tmp_path), sid, 30)] == [p.name]


def test_save_upload_accepts_any_type(tmp_path):
    # video, pdf, etc -> aceitos; ext derivada do filename do cliente.
    for fname, ext in [("clip.mp4", ".mp4"), ("doc.pdf", ".pdf"), ("a.tar.gz", ".gz")]:
        p = Path(save_upload(str(tmp_path), SESSAO, b"data", fname))
        assert p.suffix == ext


def test_save_upload_no_extension_falls_back_to_bin(tmp_path):
    p = Path(save_upload(str(tmp_path), SESSAO, b"data", "Makefile"))
    assert p.suffix == ".bin"


def test_save_upload_ext_is_sanitized(tmp_path, cofre):
    # filename hostil: a extensao e reduzida a [a-z0-9]; o nome continua gerado pelo servidor.
    p = Path(save_upload(str(tmp_path), SESSAO, b"data", "../../etc/passwd.p ng;rm"))
    assert p.parent == _pasta(cofre, tmp_path)
    assert p.suffix == ".pngrm"  # 'p ng;rm' -> 'pngrm'


def test_save_upload_rejects_empty(tmp_path):
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), SESSAO, b"", "x.png")
    assert e.value.status == 400


def test_save_upload_rejects_too_large(tmp_path):
    big = b"x" * (100 * 1024 * 1024 + 1)
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), SESSAO, big, "x.bin")
    assert e.value.status == 413


def test_save_upload_server_generated_name_not_client(tmp_path):
    a = save_upload(str(tmp_path), SESSAO, PNG, "foto.png")
    b = save_upload(str(tmp_path), SESSAO, PNG, "foto.jpg")
    assert a != b
    assert Path(a).suffix == ".png" and Path(b).suffix == ".jpg"


from app.uploads import resolve_upload  # noqa: E402


def test_resolve_upload_returns_path_for_existing_file(tmp_path):
    saved = save_upload(str(tmp_path), SESSAO, PNG, "foto.png")
    fname = Path(saved).name
    assert resolve_upload(str(tmp_path), SESSAO, fname) == saved


def test_resolve_upload_nao_ve_anexo_de_outra_sessao(tmp_path):
    saved = save_upload(str(tmp_path), "sessao-a", PNG, "foto.png")
    with pytest.raises(UploadError) as e:
        resolve_upload(str(tmp_path), "sessao-b", Path(saved).name)
    assert e.value.status == 404


def test_resolve_upload_rejects_traversal(tmp_path):
    for bad in ["../secret.png", "a/b.png", "..", "x\\y.png"]:
        with pytest.raises(UploadError) as e:
            resolve_upload(str(tmp_path), SESSAO, bad)
        assert e.value.status == 400


def test_resolve_upload_missing_file_is_404(tmp_path):
    with pytest.raises(UploadError) as e:
        resolve_upload(str(tmp_path), SESSAO, "1234-abcdef.png")
    assert e.value.status == 404


# ── list_uploads: galeria de anexos ────────────────────────────────────────────────────────────
import os  # noqa: E402
import time  # noqa: E402

from app.uploads import list_uploads, prune_old  # noqa: E402


def _envelhecer(path: str, dias: float) -> None:
    """Recua o mtime do arquivo em `dias` — a expiração é calculada a partir dele."""
    t = time.time() - dias * 86400
    os.utime(path, (t, t))


def test_list_uploads_missing_dir_is_empty(tmp_path):
    assert list_uploads(str(tmp_path), SESSAO, 30) == []


def test_list_uploads_newest_first(tmp_path):
    velho = save_upload(str(tmp_path), SESSAO, PNG, "velho.png")
    novo = save_upload(str(tmp_path), SESSAO, PNG, "novo.png")
    _envelhecer(velho, 5)
    nomes = [f["filename"] for f in list_uploads(str(tmp_path), SESSAO, 30)]
    assert nomes == [Path(novo).name, Path(velho).name]


def test_list_uploads_so_da_propria_sessao(tmp_path):
    save_upload(str(tmp_path), "sessao-a", PNG, "a.png")
    save_upload(str(tmp_path), "sessao-b", PNG, "b.png")
    assert len(list_uploads(str(tmp_path), "sessao-a", 30)) == 1


def test_list_uploads_expiry_counts_down_from_mtime(tmp_path):
    p = save_upload(str(tmp_path), SESSAO, PNG, "foto.png")
    _envelhecer(p, 10)
    (item,) = list_uploads(str(tmp_path), SESSAO, 30)
    assert item["size"] == len(PNG)
    assert item["expires_in_days"] == pytest.approx(20, abs=0.01)


def test_list_uploads_expiry_can_be_negative(tmp_path):
    # O prune só roda no upload -> anexo vencido continua listado; o número tem que dizer isso.
    _envelhecer(save_upload(str(tmp_path), SESSAO, PNG, "foto.png"), 40)
    (item,) = list_uploads(str(tmp_path), SESSAO, 30)
    assert item["expires_in_days"] < 0


def test_list_uploads_retention_zero_never_expires(tmp_path):
    save_upload(str(tmp_path), SESSAO, PNG, "foto.png")
    (item,) = list_uploads(str(tmp_path), SESSAO, 0)
    assert item["expires_in_days"] is None


def test_prune_desligado_e_pasta_inexistente(tmp_path):
    p = save_upload(str(tmp_path), SESSAO, PNG, "foto.png")
    _envelhecer(p, 40)
    assert prune_old(str(tmp_path), 0) == 0        # desligado -> não toca em nada
    assert Path(p).exists()
    assert prune_old(str(tmp_path / "nada"), 30) == 0   # projeto que nunca recebeu anexo


def test_prune_varre_o_projeto_inteiro_nao_so_a_sessao_ativa(tmp_path):
    # Higiene tem de alcancar sessao encerrada: se so limpasse a pasta de quem esta enviando,
    # as outras cresceriam pra sempre — que e o defeito que o prune existe pra evitar.
    velho = save_upload(str(tmp_path), "sessao-encerrada", PNG, "velho.png")
    _envelhecer(velho, 40)
    novo = save_upload(str(tmp_path), "sessao-ativa", PNG, "novo.png")
    assert prune_old(str(tmp_path), 30) == 1
    assert not Path(velho).exists()
    assert Path(novo).exists()
