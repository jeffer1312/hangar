"""Contas Claude e aparência do app nativo na configuração compartilhada: a conta chega sem login
e o login que já existe no destino fica intocado."""
import io
import json
import os
import tarfile
from pathlib import Path

import pytest

from app import apelidos, config_sync, contas
from tests.config_sync_machines import make_machine, use_machine

# A conta nasce pelo `contas.criar`, que chama o `hangar-server conta` desta cópia.
pytestmark = pytest.mark.usefixtures("hangar_server_bin")

ITEMS = ["claude_accounts", "hangar_appearance"]
SECRETS = (b"rt-ana-secreto", b"conta-ana@x", b"env-secreto", b"helper-secreto",
           b"otel-secreto", b"bypassPermissions", b"nao-pode-sair")


async def _no_after(ctx, items):
    return None


def _send(monkeypatch, origin, target, items):
    use_machine(monkeypatch, origin)
    bundle = config_sync.unpack(config_sync.pack(config_sync.export_bundle(origin, items)))
    use_machine(monkeypatch, target)
    return bundle


async def _apply(bundle, items, roots):
    return await config_sync.apply_bundle(bundle, items, roots, after=_no_after)


def _account(monkeypatch, roots, name, alias, settings, token, email):
    """Conta criada pelo Hangar, logada e com apelido, nesta máquina."""
    use_machine(monkeypatch, roots)
    path = contas.criar(name)
    (path / ".credentials.json").write_text(json.dumps(
        {"claudeAiOauth": {"accessToken": token, "refreshToken": token}}))
    login = json.loads((path / ".claude.json").read_text())
    (path / ".claude.json").write_text(json.dumps({**login, "oauthAccount": {"emailAddress": email}}))
    (path / "settings.json").write_text(json.dumps(settings))
    apelidos.definir(f"claude:{path.resolve()}", alias)
    return path


def _appearance(monkeypatch, roots, values: dict, image: bytes | None = None) -> Path:
    use_machine(monkeypatch, roots)
    folder = config_sync._native_dir(roots)
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "appearance.json").write_text(json.dumps(values))
    if image is not None:
        (folder / "background-image").write_bytes(image)
        (folder / "background-name").write_text("praia.jpg")
    return folder


@pytest.fixture
def pair(tmp_path, monkeypatch):
    ana, bia = make_machine(tmp_path, "ana"), make_machine(tmp_path, "bia", full=False)
    _account(monkeypatch, ana, "claude-2", "Claude 2",
             {"model": "opus", "outputStyle": "Concise", "env": {"T": "env-secreto"},
              "apiKeyHelper": "echo helper-secreto", "otelHeadersHelper": "echo otel-secreto",
              "permissions": {"defaultMode": "bypassPermissions"}},
             "rt-ana-secreto", "conta-ana@x")
    (Path(ana.home) / ".claude-solta").mkdir()   # sem marcador: não é conta do Hangar
    _appearance(monkeypatch, ana, {"theme": "light", "font": "mono", "chrome_autofill": True,
                      "side_width": 512.0, "language": "en"}, b"\x89PNG-ana")
    return ana, bia


def test_bundle_carries_name_alias_and_own_keys_but_no_login_or_secret(pair, monkeypatch):
    ana, _ = pair
    use_machine(monkeypatch, ana)
    bundle = config_sync.export_bundle(ana, ITEMS)
    assert bundle.items["claude_accounts"]["accounts"] == {
        "claude-2": {"alias": "Claude 2", "settings": {"outputStyle": "Concise"}}}
    assert bundle.items["hangar_appearance"]["native"] == {"theme": "light", "font": "mono"}
    raw = config_sync.pack(bundle)
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as tar:
        names = [m.name for m in tar.getmembers()]
        blob = b"".join(tar.extractfile(m).read() for m in tar.getmembers())
    assert not any(n.endswith((".credentials.json", "auth.json", ".claude.json")) for n in names)
    for secret in SECRETS:
        assert secret not in blob, secret


async def test_missing_account_is_created_without_login(pair, monkeypatch):
    ana, bia = pair
    report = await _apply(_send(monkeypatch, ana, bia, ITEMS), ITEMS, bia)
    path = Path(bia.home) / ".claude-claude-2"
    assert contas.e_conta(path)
    assert not (path / ".credentials.json").exists()
    assert "oauthAccount" not in json.loads((path / ".claude.json").read_text())
    assert json.loads((path / "settings.json").read_text())["outputStyle"] == "Concise"
    assert apelidos.ler()[f"claude:{path.resolve()}"] == "Claude 2"
    assert not (Path(bia.home) / ".claude-solta").exists()
    result = report["items"]["claude_accounts"]
    assert result["changed"] == ["claude-2"]
    assert {"code": "config_sync_account_needs_login",
            "params": {"account": "claude-2"}} in result["warnings"]


async def test_existing_login_on_target_is_left_alone(pair, monkeypatch):
    ana, bia = pair
    path = _account(monkeypatch, bia, "claude-2", "Velho", {"outputStyle": "Explanatory"},
                    "token-da-bia", "bia@x")
    before = {f: (path / f).read_bytes() for f in (".credentials.json", ".claude.json")}
    report = await _apply(_send(monkeypatch, ana, bia, ITEMS), ITEMS, bia)
    assert {f: (path / f).read_bytes() for f in before} == before
    assert apelidos.ler()[f"claude:{path.resolve()}"] == "Claude 2"
    assert json.loads((path / "settings.json").read_text())["outputStyle"] == "Concise"
    codes = [w["code"] for w in report["items"]["claude_accounts"]["warnings"]]
    assert "config_sync_account_needs_login" not in codes


async def test_forged_accounts_and_secret_keys_are_refused(pair, monkeypatch):
    ana, bia = pair
    bundle = _send(monkeypatch, ana, bia, ["claude_accounts"])
    (Path(bia.home) / ".claude-solta").mkdir()
    (Path(bia.home) / ".claude-solta" / "nota.txt").write_text("minha")
    bundle.items["claude_accounts"]["accounts"].update({
        "../fora": {"settings": {}}, "Maiuscula": {"settings": {}}, "solta": {"settings": {}}})
    bundle.items["claude_accounts"]["accounts"]["claude-2"]["settings"].update(
        {"env": {"T": "forjado"}, "apiKeyHelper": "echo forjado",
         "permissions": {"defaultMode": "bypassPermissions"}})
    report = await _apply(bundle, ["claude_accounts"], bia)
    warnings = report["items"]["claude_accounts"]["warnings"]
    assert {(w["code"], w["params"].get("entry") or w["params"].get("account"))
            for w in warnings} >= {("config_sync_invalid_entry", "../fora"),
                                   ("config_sync_invalid_entry", "Maiuscula"),
                                   ("config_sync_account_not_hangar", "solta")}
    assert not (Path(bia.home).parent / "fora").exists()
    assert sorted(p.name for p in (Path(bia.home) / ".claude-solta").iterdir()) == ["nota.txt"]
    settings = json.loads((Path(bia.home) / ".claude-claude-2" / "settings.json").read_text())
    assert not {"env", "apiKeyHelper", "permissions"} & set(settings)


async def test_fixed_account_list_refuses_the_creation_with_the_rust_message(pair, monkeypatch):
    """K22/D12: com `CP_CLAUDE_CONFIG_DIRS` a lista de contas é fixa; quem recusa a criação é o Rust, e o
    aviso leva a frase dele."""
    ana, bia = pair
    bundle = _send(monkeypatch, ana, bia, ["claude_accounts"])
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS", str(Path(bia.home) / ".claude"))
    report = await _apply(bundle, ["claude_accounts"], bia)
    failed = [w for w in report["items"]["claude_accounts"]["warnings"] if w["code"] == "config_sync_account_failed"]
    assert [w["params"]["account"] for w in failed] == ["claude-2"], report
    assert failed[0]["params"]["error"].startswith("CP_CLAUDE_CONFIG_DIRS está setado: a lista de contas é fixa"), failed
    assert not (Path(bia.home) / ".claude-claude-2").exists()


async def test_appearance_keeps_machine_choices_and_carries_the_image(pair, monkeypatch):
    ana, bia = pair
    folder = _appearance(monkeypatch, bia, {"theme": "dark", "chrome_autofill": False, "side_width": 300.0,
                               "language": "pt", "accent": {"preset": 2}})
    report = await _apply(_send(monkeypatch, ana, bia, ITEMS), ITEMS, bia)
    assert json.loads((folder / "appearance.json").read_text()) == {
        "theme": "light", "font": "mono", "chrome_autofill": False, "side_width": 300.0,
        "language": "pt", "accent": {"preset": 2}}
    assert (folder / "background-image").read_bytes() == b"\x89PNG-ana"
    assert (folder / "background-name").read_text() == "praia.jpg"
    assert report["items"]["hangar_appearance"]["changed"] == ["background-image",
                                                               "appearance.json"]
    again = await _apply(_send(monkeypatch, ana, bia, ITEMS), ITEMS, bia)
    assert again["items"]["hangar_appearance"]["status"] == "same"


async def test_new_image_alone_touches_appearance_for_the_open_app(pair, monkeypatch):
    ana, bia = pair
    folder = _appearance(monkeypatch, bia, {"theme": "light", "font": "mono"}, b"velha")
    old = (folder / "appearance.json").stat().st_mtime_ns
    os.utime(folder / "appearance.json", ns=(old - 10**9, old - 10**9))
    await _apply(_send(monkeypatch, ana, bia, ["hangar_appearance"]), ["hangar_appearance"], bia)
    assert (folder / "background-image").read_bytes() == b"\x89PNG-ana"
    assert (folder / "appearance.json").stat().st_mtime_ns > old - 10**9
