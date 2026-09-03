"""Catálogo de modelos do Pi e do omp: fonte única da LISTA.

`pi --list-models` roda solto no terminal, sem sessão viva — é o que a tela de abertura precisa. E
medido em 10/08/2026, ele traz exatamente os mesmos 384 modelos que o sidecar da extensão, com dois
campos a mais (contexto e imagem). Por isso a folha da sessão viva também usa esta fonte: uma lista
só, com etiqueta, nos dois lugares.

`omp` (fork do Pi) não tem `--list-models` — tem `models --json`. `parse_omp` devolve o MESMO shape
de `parse`: quem consome (tela de abertura, popover) não precisa saber de qual dos dois veio.

O sidecar continua indispensável pro que só ele sabe: modelo atual, nível atual e `levels` — quais
níveis AQUELE modelo aceita, que variam por modelo e não aparecem aqui.

Cache por PROVIDER (pi e omp têm catálogos independentes, e cada subprocess é caro) porque pagar
isso a cada abertura de tela é caro pra uma lista que muda de mês em mês.
"""
import json
import logging
import shutil
import subprocess
import threading
import time

_log = logging.getLogger(__name__)
_TTL = 600.0
_cache: dict[str, tuple[float, list[dict]]] = {}
_BIN = {"pi": "pi", "omp": "omp"}
# Cache VENCIDO serve na hora e renova por trás (uma renovação por vez, POR PROVIDER): o
# `pi --list-models` leva ~7s (medido 26/08/2026) e a tela de orquestração pagava isso na primeira
# abertura de cada 10 min.
_renovando: dict[str, threading.Lock] = {"pi": threading.Lock(), "omp": threading.Lock()}


def _renovar_em_fundo(provider: str) -> None:
    lock = _renovando[provider]
    if not lock.acquire(blocking=False):
        return
    def _run():
        try:
            listar(provider, fresco=True)
        except Exception as e:  # noqa: BLE001 — falha em fundo só loga; o cache velho segue servindo
            _log.warning("catalogo do %s em fundo falhou: %s", provider, e)
        finally:
            lock.release()
    threading.Thread(target=_run, name=f"{provider}-catalog-refresh", daemon=True).start()


class PiAusente(RuntimeError):
    """`pi`/`omp` não está no PATH deste backend — não é falha do comando, é ausência do binário."""


def _binario(provider: str = "pi") -> str:
    """Caminho do binário, resolvido — nunca o nome cru no argv.

    No Windows o `pi` do npm global é um **`pi.CMD`**, e o `CreateProcess` só completa `.exe`: um
    `subprocess.run(["pi", …])` levanta `FileNotFoundError [WinError 2]` mesmo com o `pi` na frente
    de quem digita no terminal (medido 22/08/2026 — a tela de abertura respondia 502 aqui enquanto
    o `cli_probe`, que resolve por `shutil.which`, dizia que o pi existia). O `which` aplica o
    PATHEXT e devolve o `.CMD`, que o `CreateProcess` executa.

    `None` vira erro PRÓPRIO: "não achei o binário" é outra conversa que "o binário falhou".
    """
    nome = _BIN[provider]
    exe = shutil.which(nome)
    if exe is None:
        # A dica é POR PROVIDER: o Pi vem do npm, o omp é binário nativo — mandar `npm i -g` numa
        # máquina sem omp manda a pessoa instalar o agente errado.
        raise PiAusente(f"nao achei o executavel `{nome}` no PATH deste servidor — " + (
            "instale o oh-my-pi ou ajuste o PATH do backend" if provider == "omp"
            else "instale o Pi (npm i -g @earendil-works/pi-coding-agent) ou ajuste o PATH do backend"))
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


def _k(n: int) -> str:
    """`1000000` -> `"1M"`, `384000` -> `"384K"` — a mesma etiqueta curta que `parse` já usa."""
    if n >= 1_000_000 and n % 1_000_000 == 0:
        return f"{n // 1_000_000}M"
    return f"{n // 1000}K" if n >= 1000 else str(n)


def parse_omp(saida: str) -> list[dict]:
    """`omp models --json` -> o shape de `parse`, pra tela de abertura e o popover nao saberem de
    qual dos dois veio."""
    try:
        dados = json.loads(saida)
    except ValueError:
        return []
    out: list[dict] = []
    for m in (dados.get("models") if isinstance(dados, dict) else None) or []:
        if not isinstance(m, dict) or not m.get("provider") or not m.get("id"):
            continue
        out.append({"provider": m["provider"], "id": m["id"],
                    "context": _k(int(m.get("contextWindow") or 0)),
                    "max_out": _k(int(m.get("maxTokens") or 0)),
                    "thinking": bool(m.get("thinking")),
                    "images": "image" in (m.get("input") or [])})
    return out


def listar(provider: str = "pi", fresco: bool = False) -> list[dict]:
    hit = _cache.get(provider)
    if hit and not fresco:
        if time.monotonic() - hit[0] >= _TTL:
            _renovar_em_fundo(provider)
        return hit[1]
    args = [_binario(provider)] + (["models", "--json"] if provider == "omp" else ["--list-models"])
    # `encoding` explicito pelo mesmo motivo dos outros: `text=True` sozinho decodifica pelo
    # locale, cp1252 no Windows, e a tabela/JSON traz rotulo de modelo que nao e so ASCII.
    r = subprocess.run(args, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=30)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip() or f"{' '.join(args[1:])} falhou")
    modelos = parse_omp(r.stdout) if provider == "omp" else parse(r.stdout)
    if not modelos:
        # rc=0 com saida que o parse nao reconhece (coluna a mais, JSON truncado, saida vazia) e
        # falha do provedor, nao catalogo vazio. Levanta pra virar o 502 que a rota ja sabe dar, e
        # NAO cacheia: senao o erro dura 10 min depois de o binario voltar.
        primeira = (r.stdout.strip().splitlines() or [""])[0][:120]
        raise RuntimeError(f"{' '.join(args[1:])} nao devolveu modelo nenhum (1a linha: {primeira!r})")
    # O `errors="replace"` acima e rede contra byte que nao decodifica (sem ele, medido no Windows,
    # o erro morre DENTRO da thread leitora do subprocess, o `run()` nao levanta nada e o `stdout`
    # volta None). O preco e o byte ruim virar `�` e seguir como texto. Aqui isso importa mais que
    # num rotulo: o `id` e DIGITADO na TUI depois (`/cp-model <provider> <id>`), entao um `�` no
    # meio dele vira uma troca de modelo que falha sem ninguem entender por que. Rotulo ilegivel em
    # coluna que so se le (contexto, max_out) passa: ali `�` e feio, nao errado.
    ruins = [m["id"] for m in modelos if "�" in m["id"] or "�" in m["provider"]]
    if ruins:
        raise RuntimeError(
            f"{provider} devolveu {len(ruins)} id/provider com byte ilegivel (ex: {ruins[:3]})")
    _cache[provider] = (time.monotonic(), modelos)
    return modelos
