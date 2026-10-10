"""Gera o par VAPID do web push quando o backend/.env não tem um: `python -m app.vapid_setup`.

Sem as chaves o push para o celular é descartado calado e o celular nem consegue se inscrever.
Chave que já existe nunca é trocada: as inscrições dos aparelhos são assinadas por ela.
"""
import base64
import os
import sys
import tempfile
from pathlib import Path

from app import atomico

ENV = Path(__file__).resolve().parent.parent / ".env"
KEYS = ("CP_VAPID_PUBLIC", "CP_VAPID_PRIVATE")


def _b64(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


def new_pair() -> tuple[str, str]:
    """(pública, privada) em base64url: a pública no ponto não comprimido que o navegador pede, a privada crua."""
    from cryptography.hazmat.primitives import serialization
    from py_vapid import Vapid02
    vapid = Vapid02()
    vapid.generate_keys()
    private = vapid.private_key.private_numbers().private_value.to_bytes(32, "big")
    public = vapid.public_key.public_bytes(serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)
    return _b64(public), _b64(private)


def with_keys(text: str) -> str | None:
    """O `.env` com as duas chaves no fim; None quando já tem as duas. Só uma delas é erro: não adivinha a outra."""
    present = {line.split("=", 1)[0].strip() for line in text.splitlines()
               if "=" in line and not line.lstrip().startswith("#") and line.split("=", 1)[1].strip()}
    found = [k for k in KEYS if k in present]
    if len(found) == len(KEYS):
        return None
    if found:
        raise ValueError(f"backend/.env tem só {found[0]}; apague a linha ou complete o par à mão")
    public, private = new_pair()
    sep = "" if not text or text.endswith("\n") else "\n"
    return f"{text}{sep}CP_VAPID_PUBLIC={public}\nCP_VAPID_PRIVATE={private}\n"


def main() -> int:
    try:
        text = ENV.read_text(encoding="utf-8") if ENV.exists() else ""
        new = with_keys(text)
    except (OSError, ValueError) as e:
        print(f"erro: {e}", file=sys.stderr)
        return 1
    if new is None:
        print("ok: backend/.env já tem as chaves VAPID (mantidas)")
        return 0
    fd, tmp = tempfile.mkstemp(dir=str(ENV.parent), suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="") as fh:
            fh.write(new)
        if ENV.exists():
            os.chmod(tmp, ENV.stat().st_mode & 0o777)
        atomico.substituir(tmp, ENV)
    except OSError as e:
        Path(tmp).unlink(missing_ok=True)
        print(f"erro: não consegui gravar backend/.env: {e}", file=sys.stderr)
        return 1
    print("ok: chaves VAPID geradas em backend/.env (reinicie o backend para valer)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
