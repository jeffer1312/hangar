#!/usr/bin/env python3
"""Registra o MCP `hangar` nos clientes desta máquina (chamado pelo install-hangar-send.sh).

Claude Code: `mcpServers.hangar` no ~/.claude.json e em cada ~/.claude-<conta>/.claude.json, com
`headersHelper` (o token nunca entra no ambiente da sessão). Codex: bloco marcado no config.toml de
cada CODEX_HOME (~/.codex, ~/.codex-*), com `http_headers` (o arquivo já é 0600 e guarda
credencial) e `env_http_headers` pra identidade. Idempotente. Stdlib só.
"""

import json
import os
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
HOME = Path.home()
INICIO, FIM = "# >>> hangar: mcp", "# <<< hangar: mcp"


def env_backend() -> dict[str, str]:
    out: dict[str, str] = {}
    for linha in (REPO / "backend" / ".env").read_text(encoding="utf-8").splitlines():
        if "=" in linha and not linha.startswith("#"):
            k, v = linha.split("=", 1)
            out[k.strip()] = v.strip()
    return out


def claude(url: str, helper: str) -> None:
    # Conta é pasta que já tem `.claude.json`; pasta `.claude-*` sem ele não é conta (uploads etc).
    alvos = [HOME / ".claude.json"] + sorted(p / ".claude.json" for p in HOME.glob(".claude-*")
                                             if (p / ".claude.json").is_file() and not p.name.endswith(".lock"))
    for arq in alvos:
        try:
            dados = json.loads(arq.read_text(encoding="utf-8")) if arq.exists() else {}
        except ValueError:
            print(f"aviso: {arq} não é JSON válido — pulado", file=sys.stderr)
            continue
        servidores = dados.setdefault("mcpServers", {})
        novo = {"type": "http", "url": url, "headersHelper": helper}
        if servidores.get("hangar") == novo:
            continue
        servidores["hangar"] = novo
        tmp = arq.with_name(arq.name + ".hangar-novo")
        tmp.write_text(json.dumps(dados, indent=2), encoding="utf-8")
        os.replace(tmp, arq)
        print(f"ok: MCP hangar em {arq}")


def codex(url: str, token: str) -> None:
    bloco = "\n".join([
        INICIO,
        "[mcp_servers.hangar]",
        f'url = "{url}"',
        f'http_headers = {{ "Authorization" = "Bearer {token}" }}',
        'env_http_headers = { "X-Hangar-Key" = "CP_SESSION_KEY", "X-Hangar-Pane" = "TMUX_PANE", '
        '"X-Hangar-Session" = "CP_SESSION_NAME" }',
        FIM, ""])
    for home in [HOME / ".codex"] + sorted(HOME.glob(".codex-*")):
        if not home.is_dir():
            continue
        arq = home / "config.toml"
        texto = arq.read_text(encoding="utf-8") if arq.exists() else ""
        padrao = re.compile(re.escape(INICIO) + r".*?" + re.escape(FIM) + r"\n?", re.S)
        novo = padrao.sub(lambda _: bloco, texto) if padrao.search(texto) else texto.rstrip("\n") + "\n\n" + bloco
        if novo == texto:
            continue
        tmp = arq.with_name(arq.name + ".hangar-novo")
        tmp.write_text(novo, encoding="utf-8")
        os.chmod(tmp, 0o600)
        os.replace(tmp, arq)
        print(f"ok: MCP hangar em {arq}")


if __name__ == "__main__":
    env = env_backend()
    if not env.get("CP_AUTH_TOKEN"):
        sys.exit("erro: CP_AUTH_TOKEN ausente no backend/.env")
    url = f"http://127.0.0.1:{env.get('CP_PORT') or 8765}/mcp/"
    # Windows não executa o shim sem extensão; o .cmd vem do install.ps1.
    helper = "hangar-mcp-headers.cmd" if os.name == "nt" else "hangar-mcp-headers"
    claude(url, str(HOME / ".local" / "bin" / helper))
    codex(url, env["CP_AUTH_TOKEN"])
