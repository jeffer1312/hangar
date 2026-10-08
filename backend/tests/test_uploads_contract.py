"""Referência real de HTTP e disco para comparar a migração dos anexos."""
import base64
import json
import os
import socket
import sys
from dataclasses import asdict

import pytest

from tests.uploads_contract import FIXTURE_NAMES, FIXTURES, UploadReference


@pytest.fixture
def upload_reference(tmp_path):
    with UploadReference(tmp_path) as reference:
        yield reference


def test_reference_is_repeatable(upload_reference):
    first = upload_reference.run_fixture("binary-and-gallery")
    second = upload_reference.run_fixture("binary-and-gallery")
    assert first.normalized_response == second.normalized_response
    assert first.normalized_tree == second.normalized_tree


@pytest.mark.parametrize("name", FIXTURE_NAMES)
def test_complete_response_and_tree_match_frozen_reference(upload_reference, name):
    from tests.uploads_contract import select_upload_reference

    selection = select_upload_reference()
    expected = selection.read(name)
    assert asdict(upload_reference.run_fixture(name)) == expected
    assert selection.read(name) == expected


@pytest.fixture
def xml_mapping(monkeypatch):
    import mimetypes

    mimetypes.init()
    database = mimetypes.MimeTypes()
    monkeypatch.setattr(mimetypes, "_db", database)
    return lambda value: database.add_type(value, ".xml")


@pytest.mark.parametrize("mime", ["application/xml", "text/xml"] if sys.platform == "linux" else ["text/xml"])
def test_xml_mapping_selects_complete_independent_reference(upload_reference, xml_mapping, mime):
    from tests.uploads_contract import select_upload_reference

    xml_mapping(mime)
    directory = "linux-text-xml" if sys.platform == "linux" and mime == "text/xml" else sys.platform
    expected = json.loads((FIXTURES / directory / "active-content-range.json").read_text(encoding="utf-8"))
    selection = select_upload_reference()
    assert selection.read("active-content-range") == expected
    assert asdict(upload_reference.run_fixture("active-content-range")) == expected


def test_unknown_xml_mapping_refuses_comparison_before_request(xml_mapping):
    from tests.uploads_contract import select_upload_reference

    xml_mapping("application/x-unsupported-xml")
    with pytest.raises(ValueError, match="perfil MIME desconhecido"):
        select_upload_reference().read("active-content-range")


def test_reference_selection_rejects_environment_change(xml_mapping):
    from tests.uploads_contract import select_upload_reference

    xml_mapping("text/xml")
    selection = select_upload_reference()
    selection.read("active-content-range")
    xml_mapping("application/x-changed-xml")
    with pytest.raises(ValueError, match="ambiente MIME mudou"):
        selection.read("active-content-range")


def test_reference_selection_rejects_corrupted_golden_before_request(tmp_path, xml_mapping):
    import shutil

    from tests.uploads_contract import select_upload_reference

    xml_mapping("text/xml")
    fixtures = tmp_path / "fixtures"
    shutil.copytree(FIXTURES, fixtures)
    path = fixtures / ("linux-text-xml" if sys.platform == "linux" else "win32") / "active-content-range.json"
    path.write_text("{}\n", encoding="utf-8")
    with pytest.raises(ValueError, match="hash da referência divergiu"):
        select_upload_reference(fixtures=fixtures).read("active-content-range")


def test_retention_disabled_keeps_files_and_null_expiry(upload_reference):
    capture = upload_reference.run_fixture("binary-and-gallery")
    files = capture.normalized_response[3]["json"]["files"]
    assert len(files) == 2
    assert all(item["expires_in_days"] is None for item in files)
    assert [item["size"] for item in files] == [len("referência-binária".encode()) + 2,
                                              len("texto com acentos: ação".encode())]


def test_active_html_is_isolated_and_binary_range_is_exact(upload_reference):
    capture = upload_reference.run_fixture("active-content-range")
    records = capture.normalized_response
    html = records[1]
    document = base64.b64decode(html["body_base64"])
    assert b'sandbox="allow-scripts allow-popups"' in document
    assert b"src=\"data:text/html;charset=utf-8;base64," in document
    assert b"cp_token" not in document
    binary_range = records[-2]
    assert binary_range["status"] == 206
    assert binary_range["headers"]["content-range"] == "bytes 2-5/32"
    assert base64.b64decode(binary_range["body_base64"]) == bytes([2, 3, 4, 5])
    assert records[-1]["status"] == 416


def test_project_prune_preserves_other_project_and_audio_identity(upload_reference):
    capture = upload_reference.run_fixture("retention-and-audio")
    audio = {r["case"]: r["result"] for r in capture.normalized_response
             if r["kind"] == "audio-resolver"}
    assert audio["own-name"]["status"] == 200
    assert audio["previous-transcript"]["status"] == 200
    assert audio["guest-absolute"]["status"] == 403
    assert audio["other-project"]["status"] == 400
    assert audio["network"]["status"] == 400
    assert audio["nul"]["status"] == 400
    galleries = [r["json"]["files"] for r in capture.normalized_response
                 if r["kind"] == "http" and r["method"] == "GET"]
    assert galleries[0][0]["expires_in_days"] == -1
    assert galleries[1] == []
    assert len(galleries[2]) == 1
    assert galleries[2][0]["expires_in_days"] == -1


def test_video_without_ffmpeg_keeps_upload_and_has_no_derived_artifacts(upload_reference):
    capture = upload_reference.run_fixture("video-without-ffmpeg")
    assert capture.normalized_response[0]["json"]["frames"] == []
    assert capture.normalized_response[0]["json"]["transcript"] == ""
    files = [p for p in capture.normalized_tree if p["kind"] == "file"]
    assert len(files) == 1
    assert base64.b64decode(files[0]["content_base64"]) == b"synthetic-video-no-decoder"


def test_network_is_blocked_before_connecting(upload_reference, tmp_path):
    with upload_reference.environment(tmp_path / "network"):
        with socket.socket() as sock:
            with pytest.raises(OSError, match="rede externa bloqueada"):
                sock.connect(("192.0.2.1", 80))
            with pytest.raises(OSError, match="rede externa bloqueada"):
                sock.connect_ex(("192.0.2.1", 80))
        with pytest.raises(OSError, match="rede externa bloqueada"):
            socket.getaddrinfo("outside.invalid", 443)


def test_golden_update_requires_explicit_approval(upload_reference):
    with pytest.raises(ValueError, match="aprovação explícita"):
        upload_reference.write_golden("binary-and-gallery")


def test_hostile_platform_error_is_captured_without_changing_existing_expectation(upload_reference):
    records = upload_reference.run_fixture("errors-and-isolation").normalized_response
    hostile = [r for r in records if r["kind"] == "platform-save"]
    assert len(hostile) == 4
    if os.name == "nt":
        assert any(r["result"] == {"status": 400, "detail": "caminho invalido"} for r in hostile)
    else:
        assert all(r["result"]["status"] == 200 for r in hostile)


def test_native_detector_rejects_actual_baseline_proxy_observation():
    from tests.uploads_contract import assert_native_upload

    observation = json.loads((FIXTURES / "proxy-baseline.json").read_text(encoding="utf-8"))
    assert observation["status"] == 503
    assert observation["upstream_body_bytes"] == observation["request_body_bytes"] == 23
    assert base64.b64decode(observation["body_base64"]) == json.dumps(
        observation["json"], ensure_ascii=False, separators=(",", ":")).encode()
    with pytest.raises(AssertionError, match="status 503, 23 bytes no upstream"):
        assert_native_upload(observation)


@pytest.mark.parametrize("status, upstream_bytes", [(200, 23), (503, 0)])
def test_native_detector_requires_success_and_zero_upstream_bytes(status, upstream_bytes):
    from tests.uploads_contract import assert_native_upload

    observation = json.loads((FIXTURES / "proxy-baseline.json").read_text(encoding="utf-8"))
    observation.update(status=status, upstream_body_bytes=upstream_bytes)
    with pytest.raises(AssertionError, match="autoria recusada"):
        assert_native_upload(observation)


@pytest.mark.parametrize("wire", [
    b"{\n  \"files\": []\n}",
    br'{"\u0066iles":[]}',
    b"{\"files\":[],\"files\":[]}",
])
def test_json_wire_preserves_nonvolatile_representation(upload_reference, tmp_path, wire):
    from contextlib import closing
    from unittest.mock import patch

    from fastapi.testclient import TestClient
    from starlette.responses import JSONResponse

    from app import api

    with upload_reference.environment(tmp_path / "wire"), closing(TestClient(api.app, headers={"accept-encoding": "identity"})) as client:
        upload_reference.request(client, "GET", "/api/sessions/fixture/uploads")
        original = upload_reference.records[-1]
        if wire == base64.b64decode(original["body_base64"]):
            wire += b"\n"
        with patch.object(JSONResponse, "render", return_value=wire):
            response = upload_reference.request(client, "GET", "/api/sessions/fixture/uploads")
        captured = upload_reference.records[-1]
        assert response.json() == original["json"] == captured["json"] == {"files": []}
        assert base64.b64decode(captured["body_base64"]) == wire
        assert captured["headers"]["content-length"] == str(len(wire))
        assert captured["body_base64"] != original["body_base64"]


def test_json_wire_preserves_nonvolatile_escapes_inside_normalized_path(upload_reference, tmp_path):
    from contextlib import closing
    from unittest.mock import patch

    from fastapi.testclient import TestClient
    from starlette.responses import JSONResponse

    from app import api

    def render_with_escapes(response, content):
        return json.dumps(content, ensure_ascii=True, separators=(",", ":")).replace(
            "vault", r"\u0076ault").replace(".bin", r".b\u0069n").encode()

    with upload_reference.environment(tmp_path / "ação"), closing(TestClient(api.app, headers={"accept-encoding": "identity"})) as client:
        with patch.object(JSONResponse, "render", render_with_escapes):
            upload_reference._upload(client, b"wire-fixture", "data.bin")
        captured = upload_reference.records[-1]
        wire = base64.b64decode(captured["body_base64"])
        assert br"\u0076ault" in wire
        assert br".b\u0069n" in wire
        assert captured["json"]["path"].startswith("<root>/vault/<project>/")
        assert json.loads(wire) == captured["json"]
        assert captured["headers"]["content-length"] == str(len(wire))
