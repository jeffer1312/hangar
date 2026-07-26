from pathlib import Path

import pytest

from app.uploads import save_upload, UploadError

# 1x1 PNG valido (bytes minimos)
PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
    "0000000a49444154789c6360000002000154a24f5f0000000049454e44ae426082"
)


def test_save_upload_writes_into_cwd_subdir(tmp_path):
    cwd = str(tmp_path)
    path = save_upload(cwd, PNG, "foto.png")
    p = Path(path)
    assert p.exists()
    assert p.read_bytes() == PNG
    assert p.parent == tmp_path / ".claude-pocket-uploads"
    assert p.suffix == ".png"


def test_save_upload_accepts_any_type(tmp_path):
    # video, pdf, etc -> aceitos; ext derivada do filename do cliente.
    for fname, ext in [("clip.mp4", ".mp4"), ("doc.pdf", ".pdf"), ("a.tar.gz", ".gz")]:
        p = Path(save_upload(str(tmp_path), b"data", fname))
        assert p.suffix == ext


def test_save_upload_no_extension_falls_back_to_bin(tmp_path):
    p = Path(save_upload(str(tmp_path), b"data", "Makefile"))
    assert p.suffix == ".bin"


def test_save_upload_ext_is_sanitized(tmp_path):
    # filename hostil: a extensao e reduzida a [a-z0-9]; o nome continua gerado pelo servidor.
    p = Path(save_upload(str(tmp_path), b"data", "../../etc/passwd.p ng;rm"))
    assert p.parent == tmp_path / ".claude-pocket-uploads"
    assert p.suffix == ".pngrm"  # 'p ng;rm' -> 'pngrm'


def test_save_upload_rejects_empty(tmp_path):
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), b"", "x.png")
    assert e.value.status == 400


def test_save_upload_rejects_too_large(tmp_path):
    big = b"x" * (100 * 1024 * 1024 + 1)
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), big, "x.bin")
    assert e.value.status == 413


def test_save_upload_server_generated_name_not_client(tmp_path):
    a = save_upload(str(tmp_path), PNG, "foto.png")
    b = save_upload(str(tmp_path), PNG, "foto.jpg")
    assert a != b
    assert Path(a).suffix == ".png" and Path(b).suffix == ".jpg"


from app.uploads import resolve_upload  # noqa: E402


def test_resolve_upload_returns_path_for_existing_file(tmp_path):
    saved = save_upload(str(tmp_path), PNG, "foto.png")
    fname = Path(saved).name
    assert resolve_upload(str(tmp_path), fname) == saved


def test_resolve_upload_rejects_traversal(tmp_path):
    for bad in ["../secret.png", "a/b.png", "..", "x\\y.png"]:
        with pytest.raises(UploadError) as e:
            resolve_upload(str(tmp_path), bad)
        assert e.value.status == 400


def test_resolve_upload_missing_file_is_404(tmp_path):
    with pytest.raises(UploadError) as e:
        resolve_upload(str(tmp_path), "1234-abcdef.png")
    assert e.value.status == 404


# ── list_uploads: galeria de anexos ────────────────────────────────────────────────────────────
import os  # noqa: E402
import time  # noqa: E402

from app.uploads import list_uploads  # noqa: E402


def _envelhecer(path: str, dias: float) -> None:
    """Recua o mtime do arquivo em `dias` — a expiração é calculada a partir dele."""
    t = time.time() - dias * 86400
    os.utime(path, (t, t))


def test_list_uploads_missing_dir_is_empty(tmp_path):
    assert list_uploads(str(tmp_path), 30) == []


def test_list_uploads_newest_first(tmp_path):
    velho = save_upload(str(tmp_path), PNG, "velho.png")
    novo = save_upload(str(tmp_path), PNG, "novo.png")
    _envelhecer(velho, 5)
    nomes = [f["filename"] for f in list_uploads(str(tmp_path), 30)]
    assert nomes == [Path(novo).name, Path(velho).name]


def test_list_uploads_expiry_counts_down_from_mtime(tmp_path):
    p = save_upload(str(tmp_path), PNG, "foto.png")
    _envelhecer(p, 10)
    (item,) = list_uploads(str(tmp_path), 30)
    assert item["size"] == len(PNG)
    assert item["expires_in_days"] == pytest.approx(20, abs=0.01)


def test_list_uploads_expiry_can_be_negative(tmp_path):
    # O prune só roda no upload -> anexo vencido continua listado; o número tem que dizer isso.
    _envelhecer(save_upload(str(tmp_path), PNG, "foto.png"), 40)
    (item,) = list_uploads(str(tmp_path), 30)
    assert item["expires_in_days"] < 0


def test_list_uploads_retention_zero_never_expires(tmp_path):
    save_upload(str(tmp_path), PNG, "foto.png")
    (item,) = list_uploads(str(tmp_path), 0)
    assert item["expires_in_days"] is None
