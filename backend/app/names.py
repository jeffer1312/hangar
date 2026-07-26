"""Nome de sessão: fonte ÚNICA da regra de sanitização.

Mora sozinho, sem importar nada do app, porque os dois lados precisam dele e importam em ordens
opostas: `registry` importa `adapters.codex.sessions`, então o sidecar não pode importar de volta
o `registry` (ciclo). Antes desta separação a regra existia duas vezes — e a cópia do sidecar não
recebeu a correção de acentuação, ficando divergente na primeira vez que alguém a usasse com um
nome cru.
"""
import re
import unicodedata


def sanitize_session_name(name: str) -> str:
    """Nome seguro pra alvo do tmux E pra basename de arquivo (sidecar Codex, fila, pareamento).

    O NFKD + descarte dos acentos vem ANTES do filtro. Sem isso, letra acentuada virava "-" e o
    `.strip("-")` do fim comia o traço junto com ela: "Área de trabalho" saía como
    "rea-de-trabalho" — a primeira letra sumia. Agora o acento é rebaixado ao ASCII equivalente
    ("A"), então o nome preserva o sentido: "Area-de-trabalho", "São Paulo" -> "Sao-Paulo".

    IDEMPOTENTE e no-op pra qualquer nome já ASCII — condição obrigatória, não conveniência:
    sessões, sidecars, filas e pareamentos existentes são keyed por este nome, e reescrever
    qualquer um deles deixaria o estado antigo órfão. (NFKD é identidade sobre ASCII puro.)

    Nome inteiro fora do ASCII (ex: só ideogramas) resulta em "": os chamadores já tratam isso
    como nome inválido, melhor que inventar um nome que o usuário não escreveu.

    Colisão possível ("Café" e "Cafe" viram o mesmo): não é silenciosa — quem cria/renomeia checa
    unicidade antes e devolve "já existe uma sessão com esse nome".
    """
    ascii_name = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
    return re.sub(r"[^A-Za-z0-9_-]", "-", ascii_name.strip()).strip("-")
