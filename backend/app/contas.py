"""Contas Claude: um config dir por conta, com o ambiente compartilhado por atalho.

Por que uma pasta por conta: o Claude Code guarda a sessão logada em DOIS arquivos dentro do config
dir — `.credentials.json` (token OAuth, renovado sozinho a cada ~8h) e `.claude.json` (o bloco
`oauthAccount`). Com um arquivo só, duas sessões vivas em contas diferentes se atropelam: quem
renovar por último sobrescreve a outra, e a primeira passa a mandar o token da conta errada.

A reconciliação e a criação são do Rust (`crates/hangar-server/src/accounts/claude_config.rs`),
chamadas aqui pelo `hangar-server conta`.

Por que `~/.claude-<nome>`: `config.list_config_dirs()` já varre `~/.claude*` procurando config
dir. Nomear assim faz a conta nova aparecer na tela de criação sem inventar registro novo.
"""
import json
import re
import subprocess
from pathlib import Path

from app import diag, rust_bins


class ContaError(Exception):
    """status HTTP junto porque a API é o principal chamador; o CLI só imprime o detail."""

    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


_NOME_OK = re.compile(r"^[a-z0-9][a-z0-9_-]{0,31}$")

# Carimbo de "esta pasta foi criada aqui". reconciliar()/apagar() só mexem em pasta carimbada —
# sem isto, um `~/.claude-backup` do usuário entraria na poda de atalhos e no apagar.
MARCADOR = ".hangar-conta"


def compartilhado() -> Path:
    """SEMPRE o ~/.claude real, nunca CLAUDE_CONFIG_DIR.

    Se o backend já estiver rodando dentro de uma conta, derivar do env faria a conta nova apontar
    pra outra conta e os atalhos virariam corrente — dois saltos até o arquivo de verdade, e a
    remoção do elo do meio quebrando tudo em silêncio.
    """
    return Path.home() / ".claude"


def caminho(nome: str) -> Path:
    # fullmatch e não match+$: o $ casa antes de uma quebra de linha FINAL, então
    # 'conta2\n' passava e a pasta nascia com controle de linha no nome.
    if not _NOME_OK.fullmatch(nome or ""):
        raise ContaError(400, "nome: use minúsculas, números, '-' ou '_' (até 32 caracteres)")
    return Path.home() / f".claude-{nome}"


def e_conta(p: Path) -> bool:
    """Pasta de conta de verdade: diretório REAL (não symlink) com marcador real.

    Sem as duas guardas, um `~/.claude-evil -> /tmp/fora` com um `.hangar-conta` do lado de lá
    seria listado como conta, e a reconciliação remexeria — e o apagar destruiria — o diretório
    externo.
    """
    if p.is_symlink() or not p.is_dir() or (p / ".hangar-account-pending").exists():
        return False
    marcador = p / MARCADOR
    return marcador.is_file() and not marcador.is_symlink()


def listar() -> list[str]:
    return sorted(p.name.removeprefix(".claude-") for p in Path.home().glob(".claude-*")
                  if e_conta(p))


def _conferir_plugins(dir_conta: Path) -> list[str]:
    """Plugin ligado no settings.json mas sem instalação no compartilhado: avisa, não conserta.

    `enabledPlugins` (settings.json, cópia por conta) e `installed_plugins.json` (plugins/,
    compartilhado) são dois registros que o CLI mantém separados, e eles desalinham em silêncio:
    o clobber de 2026-08-19 apagou a instalação e o `enabledPlugins` continuou dizendo true; a
    limpeza de cache do CLI (`.last_inuse_sweep`) apaga a pasta de um plugin que saiu do registro.
    Nos dois casos a sessão abre sem o plugin e nada avisa — a skill "some" e a sessão improvisa
    lendo o que achar no disco. Só avisa porque instalar é do CLI (`claude plugin install`), e
    fazer isso daqui escreveria no compartilhado a partir de uma conta.
    """
    try:
        ligados = json.loads((dir_conta / "settings.json").read_text(encoding="utf-8")).get(
            "enabledPlugins") or {}
    except FileNotFoundError:
        return []   # máquina nova: o principal não tinha settings.json pra semear
    except (OSError, ValueError) as e:
        # Cópia recém-semeada de um principal corrompido passa direto pelo copyfile sem parse;
        # engolir aqui deixaria a conta abrir com settings quebrado e sem sinal nenhum.
        raise ContaError(500, f"não consegui ler o settings.json da conta: {e}") from e
    if not isinstance(ligados, dict):
        return []
    registro = compartilhado() / "plugins" / "installed_plugins.json"
    try:
        instalados = json.loads(registro.read_text(encoding="utf-8")).get("plugins") or {}
    except FileNotFoundError:
        instalados = {}
    except (OSError, ValueError) as e:
        return [f"plugins: não consegui ler {registro}: {e}"]
    avisos: list[str] = []
    for nome, ligado in sorted(ligados.items()):
        if not ligado:
            continue
        entradas = instalados.get(nome) or []
        if not entradas:
            avisos.append(f"plugin {nome} está ligado no settings.json mas não consta em "
                          f"installed_plugins.json — reinstalar: claude plugin install {nome}")
            continue
        sem_pasta = [e.get("installPath") for e in entradas
                     if not e.get("installPath") or not Path(e["installPath"]).is_dir()]
        if sem_pasta:
            avisos.append(f"plugin {nome} consta instalado mas a pasta sumiu "
                          f"({', '.join(map(str, sem_pasta))}) — reinstalar: "
                          f"claude plugin install {nome}")
    return avisos


def _conta_rust(*args: str) -> list[str]:
    """`hangar-server conta <args>`: os avisos `hangar-conta: ` do stderr; saída diferente de 0 é ContaError."""
    exe = rust_bins.find_bin("hangar-server", "CP_RUST_SERVER_BIN")
    if exe is None:
        raise ContaError(503, "hangar-server não encontrado: as contas Claude são reconciliadas e criadas pelo Rust")
    proc = subprocess.run([str(exe), "conta", *args], stdin=subprocess.DEVNULL, capture_output=True,
                          text=True, encoding="utf-8", errors="replace")
    avisos = [linha.removeprefix("hangar-conta: ") for linha in proc.stderr.splitlines() if linha.startswith("hangar-conta: ")]
    if proc.returncode != 0:
        raise ContaError(500, avisos[-1] if avisos else proc.stderr.strip()[:200])
    return avisos


def reconciliar(nome: str, projeto: str | None = None) -> list[str]:
    return _conta_rust("--prep", nome, *([] if projeto is None else [projeto]))


@diag.rastrear("conta.criar", provider="claude")
def criar(nome: str) -> Path:
    """Cria a pasta pronta pro `/login`. NÃO loga — o OAuth abre navegador e é interativo."""
    _conta_rust("--new", nome)
    return caminho(nome)
