"""Catálogo local de contas Codex e origem dos rollouts."""

from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import shutil


_NOME = r"[a-z0-9][a-z0-9_-]{0,31}"
_MARKER_VERSION = 1
MARKER = ".hangar-codex-conta"
MARCADOR = MARKER

_AUTH_ENV = re.compile(
    r"^(?:OPENAI|CODEX|CHATGPT)_.+(?:(?:KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL)(?:_FILE)?|ACCOUNT_ID|ORG_ID|PROJECT_ID)$",
    re.IGNORECASE,
)
_PROVIDER_ENV = frozenset({
    "OPENAI_BASE_URL", "OPENAI_API_BASE", "OPENAI_ORGANIZATION", "OPENAI_ORG_ID",
    "OPENAI_PROJECT", "OPENAI_PROJECT_ID", "OPENAI_ENDPOINT",
})
_RUNTIME_ENV = frozenset({
    "CODEX_HOME", "CODEX_CONFIG_HOME", "CODEX_SQLITE_HOME", "HOME", "USERPROFILE",
    "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME",
})


class AccountError(Exception):
    def __init__(self, status: int, code: str, params: dict | None = None):
        self.status = status
        self.code = code
        self.params = dict(params or {})
        super().__init__(code)


@dataclass(frozen=True)
class Account:
    id: str
    home: Path
    is_default: bool


def environment(account: Account, *, home: Path | None = None,
                base: dict[str, str] | None = None) -> dict[str, str]:
    """Monta o ambiente de um processo Codex sem herdar identidade externa na secundária."""
    ambiente = dict(os.environ if base is None else base)
    if not account.is_default:
        for name in list(ambiente):
            upper = name.upper()
            if (_AUTH_ENV.fullmatch(upper) or upper in _PROVIDER_ENV
                    or upper in _RUNTIME_ENV):
                ambiente.pop(name, None)
    raiz_home = (home or Path.home()).absolute()
    ambiente.update({
        "HOME": str(raiz_home),
        "USERPROFILE": str(raiz_home),
        "CODEX_HOME": str(account.home.absolute()),
    })
    if raiz_home != Path.home().absolute():
        ambiente.update({
            "XDG_CONFIG_HOME": str(raiz_home / ".config"),
            "XDG_DATA_HOME": str(raiz_home / ".local" / "share"),
            "XDG_STATE_HOME": str(raiz_home / ".local" / "state"),
            "XDG_CACHE_HOME": str(raiz_home / ".cache"),
        })
    return ambiente


def _home_from_environment() -> Path:
    return Path(os.environ.get("CODEX_HOME") or "~/.codex").expanduser().absolute()


_DEFAULT_HOME = _home_from_environment()


def default_home() -> Path:
    return _DEFAULT_HOME


def _valid_name(name: object, *, allow_default: bool = False) -> bool:
    return (
        isinstance(name, str)
        and re.fullmatch(_NOME, name) is not None
        and (allow_default or name != "default")
    )


def _validate_name(name: object, *, allow_default: bool = False) -> str:
    if not _valid_name(name, allow_default=allow_default):
        raise AccountError(400, "codex_account_invalid_name", {})
    return name


def _account_home(name: str) -> Path:
    return Path.home() / f".codex-{name}"


def _canonical(path: Path) -> Path:
    return path.expanduser().resolve(strict=False)


def _marker_data(path: Path) -> dict | None:
    marker = path / MARKER
    if marker.is_symlink() or not marker.is_file():
        return None
    try:
        data = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def _managed(path: Path, name: str) -> bool:
    if path.is_symlink() or not path.is_dir():
        return False
    data = _marker_data(path)
    return bool(
        data
        and type(data.get("version")) is int
        and data.get("version") == _MARKER_VERSION
        and data.get("id") == name
    )


def _account(name: str, home: Path | None = None) -> Account:
    return Account(name, home if home is not None else _account_home(name), False)


def list_accounts() -> list[Account]:
    result = [Account("default", default_home(), True)]
    try:
        candidates = Path.home().glob(".codex-*")
    except OSError:
        candidates = ()
    managed = []
    for path in candidates:
        name = path.name.removeprefix(".codex-")
        if name != "default" and _valid_name(name) and _managed(path, name):
            managed.append(_account(name, path))
    return result + sorted(managed, key=lambda account: account.id)


def resolve_account(account_id: str = "default") -> Account:
    account_id = _validate_name(account_id, allow_default=True)
    if account_id == "default":
        return Account("default", default_home(), True)

    target = _account_home(account_id)
    if target.is_symlink() or not target.exists() or not target.is_dir():
        raise AccountError(404, "codex_account_not_found", {"account_id": account_id})
    if not _managed(target, account_id):
        raise AccountError(409, "codex_account_invalid_marker", {"account_id": account_id})
    return _account(account_id, target)


def _cleanup_new_directory(target: Path) -> None:
    marker = target / MARKER
    try:
        if marker.is_file() and not marker.is_symlink():
            marker.unlink()
        target.rmdir()
    except OSError:
        pass


def create_account(name: str) -> Account:
    name = _validate_name(name)
    target = _account_home(name)
    if _canonical(target) == _canonical(default_home()):
        raise AccountError(409, "codex_account_conflict", {"account_id": name})
    try:
        target.mkdir(mode=0o700, exist_ok=False)
    except FileExistsError:
        raise AccountError(409, "codex_account_exists", {"account_id": name}) from None
    except OSError as error:
        raise AccountError(500, "codex_account_create_failed", {"account_id": name}) from error

    try:
        (target / MARKER).write_text(
            json.dumps({"version": _MARKER_VERSION, "id": name}) + "\n",
            encoding="utf-8",
        )
        # Sem isto o login gravaria a credencial no keyring do sistema, fora da pasta da conta.
        (target / "config.toml").write_text('cli_auth_credentials_store = "file"\n', encoding="utf-8")
    except Exception as error:
        _cleanup_new_directory(target)
        raise AccountError(500, "codex_account_marker_failed", {"account_id": name}) from error
    return _account(name, target)


def delete_account(account: Account) -> None:
    """Apaga a pasta de uma conta ADICIONAL gerenciada. A padrao (~/.codex) nunca e apagada."""
    if account.is_default or _canonical(account.home) == _canonical(default_home()):
        raise AccountError(409, "codex_account_default_protected", {"account_id": account.id})
    if not _managed(account.home, account.id):
        raise AccountError(409, "codex_account_invalid_marker", {"account_id": account.id})
    try:
        shutil.rmtree(account.home)
    except OSError as error:
        raise AccountError(500, "codex_account_delete_failed", {"account_id": account.id}) from error


def _under(path: Path, root: Path) -> bool:
    return path == root or root in path.parents


def account_for_rollout(path: Path) -> Account | None:
    rollout = _canonical(Path(path))
    matches = []
    for account in list_accounts():
        home = _canonical(account.home)
        if _under(rollout, home / "sessions") or _under(rollout, home / "archived_sessions"):
            matches.append(account)
    if len(matches) > 1:
        raise AccountError(
            409,
            "codex_account_ambiguous_rollout",
            {"path": str(path), "accounts": [account.id for account in matches]},
        )
    return matches[0] if matches else None
