"""Todo `subprocess` que le saida de CLI declara `encoding` — senao o Windows decodifica em cp1252.

`text=True` sozinho decodifica pelo LOCALE. No Linux o locale e UTF-8 e nada acontece; no Windows
e cp1252 (medido nesta VM em 22/08/2026: `sys.flags.utf8_mode` = 0, `locale.getencoding()` =
'cp1252'), e ai a saida de qualquer CLI que fale UTF-8 chega corrompida — 23 caracteres acentuados
voltaram como 28. Pior que embaralhar: cp1252 tem bytes INDEFINIDOS, entao a sequencia errada
levanta `UnicodeDecodeError` em vez de so ficar feia, e quem parseia JSON (`tailscale status
--json`, `claude auth status --json`) perde a resposta inteira.

Os quatro que faltavam eram alcance, cli_probe, conta_estado e pi_catalog — o resto do backend
(git_ops, search, loop, deploy, tmux) ja passava `encoding="utf-8"`, entao isto era esquecimento,
nao decisao.

O caso NAO depende do locale de quem roda: ele forca a decodificacao errada explicitamente, entao
falha no codigo velho tambem no Linux.
"""
import ast
import subprocess
import sys
from pathlib import Path

import pytest

APP = Path(__file__).resolve().parent.parent / "app"

# Modulos que rodam CLI e leem a saida. Nao e a lista de TODOS os subprocess do backend: quem so
# olha returncode (renova_token) ou manda bytes (tts) nao decodifica nada.
MODULOS = ["alcance.py", "cli_probe.py", "conta_estado.py", "pi_catalog.py",
           "git_ops.py", "loop.py", "deploy.py", "search.py", "tmux.py"]


def _chamadas_sem_encoding(caminho: Path) -> list[int]:
    """Linhas com `subprocess.run/Popen(... text=True ...)` e SEM `encoding=`."""
    arvore = ast.parse(caminho.read_text(encoding="utf-8"))
    faltando = []
    for no in ast.walk(arvore):
        if not isinstance(no, ast.Call):
            continue
        f = no.func
        alvo = f.attr if isinstance(f, ast.Attribute) else getattr(f, "id", "")
        if alvo not in ("run", "Popen"):
            continue
        nomes = {k.arg for k in no.keywords if k.arg}
        # `text=True` (ou universal_newlines) e o que dispara a decodificacao pelo locale.
        if ("text" in nomes or "universal_newlines" in nomes) and "encoding" not in nomes:
            faltando.append(no.lineno)
    return faltando


@pytest.mark.parametrize("modulo", MODULOS)
def test_subprocess_que_le_saida_declara_encoding(modulo):
    caminho = APP / modulo
    linhas = _chamadas_sem_encoding(caminho)
    assert not linhas, (
        f"app/{modulo}: subprocess com text=True e sem encoding= nas linhas {linhas}. "
        "No Windows isso decodifica em cp1252 e corrompe (ou derruba) a saida do CLI."
    )


def test_o_defeito_e_real_e_nao_teorico():
    """Prova que a ausencia de `encoding` corrompe — sem depender do locale de quem roda o teste.

    Decodificar explicitamente em cp1252 e o que o Windows faz por padrao. Se um dia o Python
    ligar o modo UTF-8 por padrao (PEP 686) este caso continua valendo como registro do porque a
    regra existe, e o de cima e que segura a regra.
    """
    original = "plano Máximo — açaí çéí"
    prog = f"import sys;sys.stdout.buffer.write({original!r}.encode('utf-8'))"

    certo = subprocess.run([sys.executable, "-c", prog], capture_output=True,
                           text=True, encoding="utf-8").stdout
    assert certo == original

    errado = subprocess.run([sys.executable, "-c", prog], capture_output=True,
                            text=True, encoding="cp1252", errors="replace").stdout
    assert errado != original
    # Nao e so "feio": o texto muda de TAMANHO, entao qualquer comparacao ou parse quebra.
    assert len(errado) > len(original)
