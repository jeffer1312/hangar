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
    path = save_upload(cwd, PNG, "image/png")
    p = Path(path)
    assert p.exists()
    assert p.read_bytes() == PNG
    assert p.parent == tmp_path / ".claude-pocket-uploads"
    assert p.suffix == ".png"


def test_save_upload_rejects_bad_content_type(tmp_path):
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), PNG, "application/pdf")
    assert e.value.status == 415


def test_save_upload_rejects_empty(tmp_path):
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), b"", "image/png")
    assert e.value.status == 400


def test_save_upload_rejects_too_large(tmp_path):
    big = b"x" * (10 * 1024 * 1024 + 1)
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), big, "image/png")
    assert e.value.status == 413


def test_save_upload_server_generated_name_not_client(tmp_path):
    a = save_upload(str(tmp_path), PNG, "image/png")
    b = save_upload(str(tmp_path), PNG, "image/jpeg")
    assert a != b
    assert Path(a).suffix == ".png" and Path(b).suffix == ".jpg"
