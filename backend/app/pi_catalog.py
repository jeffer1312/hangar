"""Catálogo de modelos do Pi: fonte única da LISTA.

`pi --list-models` roda solto no terminal, sem sessão viva — é o que a tela de abertura precisa. E
medido em 10/08/2026, ele traz exatamente os mesmos 384 modelos que o sidecar da extensão, com dois
campos a mais (contexto e imagem). Por isso a folha da sessão viva também usa esta fonte: uma lista
só, com etiqueta, nos dois lugares.

O sidecar continua indispensável pro que só ele sabe: modelo atual, nível atual e `levels` — quais
níveis AQUELE modelo aceita, que variam por modelo e não aparecem aqui.

Cache porque é subprocess Node: pagar isso a cada abertura de tela é caro pra uma lista que muda de
mês em mês.
"""
import logging
import shutil
import subprocess
import threading
import time

_log = logging.getLogger(__name__)
_TTL = 600.0
_cache: tuple[float, list[dict]] | None = None
# Cache VENCIDO serve na hora e renova por trás (uma renovação por vez): o `pi --list-models` leva
# ~7s (medido 26/08/2026) e a tela de orquestração pagava isso na primeira abertura de cada 10 min.
_renovando = threading.Lock()


def _renovar_em_fundo() -> None:
    if not _renovando.acquire(blocking=False):
        return
    def _run():
        try:
            listar(fresco=True)
        except Exception as e:  # noqa: BLE001 — falha em fundo só loga; o cache velho segue servindo
            _log.warning("pi --list-models em fundo falhou: %s", e)
        finally:
            _renovando.release()
    threading.Thread(target=_run, name="pi-catalog-refresh", daemon=True).start()


class PiAusente(RuntimeError):
    """`pi` não está no PATH deste backend — não é falha do comando, é ausência do binário."""


def _binario() -> str:
    """Caminho do `pi`, resolvido — nunca o nome cru no argv.

    No Windows o `pi` do npm global é um **`pi.CMD`**, e o `CreateProcess` só completa `.exe`: um
    `subprocess.run(["pi", …])` levanta `FileNotFoundError [WinError 2]` mesmo com o `pi` na frente
    de quem digita no terminal (medido 22/08/2026 — a tela de abertura respondia 502 aqui enquanto
    o `cli_probe`, que resolve por `shutil.which`, dizia que o pi existia). O `which` aplica o
    PATHEXT e devolve o `.CMD`, que o `CreateProcess` executa.

    `None` vira erro PRÓPRIO: "não achei o pi" é outra conversa que "o pi falhou", e o WinError 2
    cru mandava a pessoa procurar defeito no comando.
    """
    exe = shutil.which("pi")
    if exe is None:
        raise PiAusente("nao achei o executavel `pi` no PATH deste servidor — instale o Pi "
                        "(npm i -g @earendil-works/pi-coding-agent) ou ajuste o PATH do backend")
    return exe


def parse(saida: str) -> list[dict]:
    out: list[dict] = []
    for linha in saida.splitlines():
        campos = linha.split()
        # 6 colunas exatas; o cabeçalho cai pelo próprio nome, e linha torta é pulada em vez de
        # derrubar a lista — um provedor novo com coluna a mais não pode cegar o seletor.
        if len(campos) != 6 or campos[0] == "provider":
            continue
        provider, ident, context, max_out, thinking, images = campos
        out.append({"provider": provider, "id": ident, "context": context,
                    "max_out": max_out, "thinking": thinking == "yes", "images": images == "yes"})
    return out


def listar(fresco: bool = False) -> list[dict]:
    global _cache
    if _cache and not fresco:
        if time.monotonic() - _cache[0] >= _TTL:
            _renovar_em_fundo()
        return _cache[1]
    # `encoding` explicito pelo mesmo motivo dos outros: `text=True` sozinho decodifica pelo
    # locale, cp1252 no Windows, e a tabela do `pi` traz rotulo de modelo que nao e so ASCII.
    r = subprocess.run([_binario(), "--list-models"], capture_output=True, text=True,
                       encoding="utf-8", errors="replace", timeout=30)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip() or "pi --list-models falhou")
    modelos = parse(r.stdout)
    if not modelos:
        # rc=0 com tabela que o parse não reconhece (coluna a mais, saída truncada, saída
        # vazia) é falha do provedor, não catálogo vazio. Levanta pra virar o 502 que a rota
        # já sabe dar, e NÃO cacheia: senão o erro dura 10 min depois de o pi voltar.
        primeira = (r.stdout.strip().splitlines() or [""])[0][:120]
        raise RuntimeError(f"pi --list-models nao devolveu modelo nenhum (1a linha: {primeira!r})")
    # O `errors="replace"` acima e rede contra byte que nao decodifica (sem ele, medido no
    # Windows, o erro morre DENTRO da thread leitora do subprocess, o `run()` nao levanta nada e o
    # `stdout` volta None — o TypeError cai longe da causa). O preco e o byte ruim virar `�` e
    # seguir como texto. Aqui isso importa mais que num rotulo: o `id` e DIGITADO na TUI depois
    # (`/cp-model <provider> <id>`), entao um `�` no meio dele vira uma troca de modelo que falha
    # sem ninguem entender por que. Mesma doutrina do bloco acima — tabela em que nao da pra
    # confiar e falha do provedor: 502 e NAO cacheia. Rotulo ilegivel em coluna que so se le
    # (contexto, max_out) passa: ali `�` e feio, nao errado.
    ruins = [m["id"] for m in modelos if "�" in m["id"] or "�" in m["provider"]]
    if ruins:
        raise RuntimeError(
            f"pi --list-models devolveu {len(ruins)} id/provider com byte ilegivel (ex: {ruins[:3]})")
    _cache = (time.monotonic(), modelos)
    return modelos
