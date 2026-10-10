#!/usr/bin/env python3
"""Registra o MCP `hangar` nos clientes desta máquina (chamado pelo install-hangar-send.sh).

Também libera `hangar-send` e as ferramentas `mcp__hangar__*` no `permissions.allow` do
~/.claude/settings.json principal, para recado entre sessões não pedir aprovação.

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


# Recado entre sessões não pede aprovação em nenhum modo; o resto do Claude continua pedindo.
PERMISSOES = ("Bash(hangar-send:*)", "mcp__hangar__*")


def com_permissoes(dados: object) -> bool | None:
    """Acrescenta as liberações que faltam em `permissions.allow`. True = mudou; None = formato estranho."""
    if not isinstance(dados, dict):
        return None
    permissoes = dados.setdefault("permissions", {})
    if not isinstance(permissoes, dict):
        return None
    allow = permissoes.setdefault("allow", [])
    if not isinstance(allow, list):
        return None
    faltam = [p for p in PERMISSOES if p not in allow]
    allow.extend(faltam)
    return bool(faltam)


def claude_permissoes(arq: Path) -> None:
    """Só o settings.json principal: o espelho do hangar-conta leva às contas, e a cópia delas é refeita."""
    try:
        dados = json.loads(arq.read_text(encoding="utf-8")) if arq.exists() else {}
    except (OSError, ValueError):
        print(f"aviso: {arq} ilegível — liberação do hangar-send não gravada", file=sys.stderr)
        return
    mudou = com_permissoes(dados)
    if mudou is None:
        print(f"aviso: {arq} tem permissions num formato inesperado — liberação não gravada", file=sys.stderr)
    if not mudou:
        return
    arq.parent.mkdir(parents=True, exist_ok=True)
    tmp = arq.with_name(arq.name + ".hangar-novo")
    tmp.write_text(json.dumps(dados, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(tmp, arq)
    print(f"ok: hangar-send e MCP hangar liberados em {arq}")


MARKED_BLOCK = re.compile(re.escape(INICIO) + r".*?" + re.escape(FIM) + r"\n?", re.S)
# Seções `[mcp_servers.hangar]` e `[mcp_servers.hangar.*]` fora dos marcadores, cada uma até a
# próxima seção. O app desktop do Codex reescreve o config.toml inteiro sem comentários e com os
# headers em subtabelas: os marcadores somem, mas o servidor continua lá.
UNMARKED_HANGAR = re.compile(r"^\[mcp_servers\.hangar(?:\.[^\]]*)?\][^\n]*\n(?:(?!\[)[^\n]*\n?)*", re.M)


def codex_has_hangar(texto: str, bloco: str) -> bool:
    """O config já registra o MCP `hangar` com tudo o que o bloco declara, em qualquer formato TOML.

    Conferir só URL e token deixaria passar um servidor sem os headers de identidade.
    """
    try:
        import tomllib
    except ModuleNotFoundError:
        return False
    try:
        servidor = tomllib.loads(texto).get("mcp_servers", {}).get("hangar")
    except tomllib.TOMLDecodeError:
        return False
    esperado = tomllib.loads(bloco)["mcp_servers"]["hangar"]
    return isinstance(servidor, dict) and all(servidor.get(k) == v for k, v in esperado.items())


def codex_config(texto: str, bloco: str) -> str:
    """Config.toml com um único `[mcp_servers.hangar]`: o do app, se já bater, senão o bloco marcado."""
    sem_marcado = MARKED_BLOCK.sub("", texto)
    if sem_marcado == texto and codex_has_hangar(texto, bloco):
        return texto
    base = UNMARKED_HANGAR.sub("", sem_marcado).rstrip("\n")
    return (base + "\n\n" if base else "") + bloco


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
        novo = codex_config(texto, bloco)
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
    claude_permissoes(HOME / ".claude" / "settings.json")
    codex(url, env["CP_AUTH_TOKEN"])
