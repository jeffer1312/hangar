"""Bloco do MCP `hangar` no config.toml do Codex: um só, mesmo depois de o app reescrever o arquivo."""
import importlib.machinery
import importlib.util
import tomllib
from pathlib import Path

# registrar-mcp.py tem hífen: spec_from_file_location não acha loader por sufixo sem o SourceFileLoader.
_path = Path(__file__).with_name("registrar-mcp.py")
spec = importlib.util.spec_from_file_location(
    "registrar_mcp", _path, loader=importlib.machinery.SourceFileLoader("registrar_mcp", str(_path)))
rm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rm)

URL = "http://127.0.0.1:8765/mcp/"
TOKEN = "tok123"
BLOCO = "\n".join([
    rm.INICIO,
    "[mcp_servers.hangar]",
    f'url = "{URL}"',
    f'http_headers = {{ "Authorization" = "Bearer {TOKEN}" }}',
    'env_http_headers = { "X-Hangar-Key" = "CP_SESSION_KEY", "X-Hangar-Pane" = "TMUX_PANE", '
    '"X-Hangar-Session" = "CP_SESSION_NAME" }',
    rm.FIM, ""])

# Como o app desktop do Codex grava o servidor: sem comentários, headers em subtabelas.
APP_FORMAT = f"""model = "gpt-5"

[mcp_servers.pencil]
command = "pencil.exe"

[mcp_servers.hangar]
url = "{URL}"

[mcp_servers.hangar.http_headers]
Authorization = "Bearer {TOKEN}"

[mcp_servers.hangar.env_http_headers]
X-Hangar-Key = "CP_SESSION_KEY"
X-Hangar-Pane = "TMUX_PANE"
X-Hangar-Session = "CP_SESSION_NAME"

[mcp_servers.node_repl]
args = []
"""


def _hangar(texto: str) -> dict:
    dados = tomllib.loads(texto)
    return dados["mcp_servers"]["hangar"]


def test_liberacao_do_hangar_send_entra_uma_vez_e_preserva_o_resto():
    dados = {"permissions": {"allow": ["Bash(git:*)"], "defaultMode": "default"}, "model": "opus"}
    assert rm.com_permissoes(dados) is True
    assert dados["permissions"] == {"allow": ["Bash(git:*)", *rm.PERMISSOES], "defaultMode": "default"}
    assert dados["model"] == "opus"
    assert rm.com_permissoes(dados) is False
    assert rm.com_permissoes({"permissions": []}) is None


def test_arquivo_vazio_recebe_o_bloco():
    novo = rm.codex_config("", BLOCO)
    assert novo == BLOCO
    assert _hangar(novo)["url"] == URL


def test_bloco_marcado_ja_correto_nao_muda():
    texto = 'model = "gpt-5"\n\n' + BLOCO
    assert rm.codex_config(texto, BLOCO) == texto


def test_formato_do_app_com_mesmo_token_nao_muda():
    assert rm.codex_config(APP_FORMAT, BLOCO) == APP_FORMAT


def test_formato_do_app_com_token_antigo_e_substituido():
    velho = APP_FORMAT.replace(TOKEN, "antigo")
    novo = rm.codex_config(velho, BLOCO)
    assert novo.count("[mcp_servers.hangar]") == 1
    assert "antigo" not in novo
    assert _hangar(novo)["http_headers"]["Authorization"] == f"Bearer {TOKEN}"
    assert tomllib.loads(novo)["mcp_servers"]["node_repl"] == {"args": []}


def test_formato_do_app_sem_headers_de_identidade_e_substituido():
    sem_pane = APP_FORMAT.replace('X-Hangar-Pane = "TMUX_PANE"\n', "")
    novo = rm.codex_config(sem_pane, BLOCO)
    assert novo.count("[mcp_servers.hangar]") == 1
    assert _hangar(novo)["env_http_headers"]["X-Hangar-Pane"] == "TMUX_PANE"


def test_duplicata_app_mais_marcado_e_reparada():
    quebrado = APP_FORMAT + "\n" + BLOCO
    novo = rm.codex_config(quebrado, BLOCO)
    assert novo.count("[mcp_servers.hangar]") == 1
    assert _hangar(novo)["url"] == URL
    assert rm.codex_config(novo, BLOCO) == novo


def test_crlf_do_app_nao_atrapalha():
    texto = APP_FORMAT.replace("\n", "\r\n")
    assert rm.codex_config(texto, BLOCO) == texto
    reparado = rm.codex_config(texto.replace(TOKEN, "antigo"), BLOCO)
    assert reparado.count("[mcp_servers.hangar]") == 1
    assert _hangar(reparado)["url"] == URL
