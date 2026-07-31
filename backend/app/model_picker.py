"""Parsing puro do picker interativo do `/model` do Claude Code.

O `/model` (sem argumento) abre um picker UNIFICADO modelo+esforco. Diferente de
`/model <arg>` / `/effort <level>` (que salvam como DEFAULT pra novas sessoes), o picker
permite aplicar SO na sessao atual apertando `s` (Enter = salvar como default; Esc = cancela).

Layout real capturado (Opus ativo):

    Select model
    Switch between Claude models. Your pick becomes the default for new sessions. ...

      1. Default (recommended)  Opus 4.8 with 1M context · Best for everyday, complex tasks
    ❯ 2. Opus ✔                 Opus 4.8 with 1M context · Best for everyday, complex tasks
      3. Sonnet                 Sonnet 4.6 · Efficient for routine tasks
    ↓ 4. Haiku                  Haiku 4.5 · Fastest for quick answers

    ◉ xHigh effort ←/→ to adjust

    Enter to set as default · s to use this session only · Esc to cancel

Achados das medicoes ao vivo (tmux):
- `❯` = cursor; `✔` = modelo ativo. O picker abre com o cursor sobre o modelo atual.
- Teclas de NUMERO (1..n) = seleciona E confirma como DEFAULT (fecha) -> NUNCA usar.
- Navegar o modelo so com Up/Down (move o `❯` uma linha por toque).
- Esforco: Left/Right e CICLICO (da a volta) e o conjunto DEPENDE do modelo:
  Opus = [low, medium, high, xhigh, max, ultracode]; Sonnet = [low, medium, high, max];
  Haiku = nenhum ("Effort not supported"). Por isso o driver le o marcador a cada passo
  em vez de assumir um numero fixo de passos.
- A lista de modelos e DINAMICA (linhas/rotulos mudam conforme o modelo atual) -> casar por
  NOME (primeira palavra do rotulo), nunca por numero fixo.

Tudo aqui e puro (sem IO) pra ser testavel com fixtures de pane capturados de verdade.
"""

import re

# Fallback de ordem/numero de linha, NAO a verdade. A lista real e lida do picker
# (parse_model_rows): medido em 31/07/2026 o picker tem 5 linhas — Default, Opus (1M context),
# Fable, Sonnet, Haiku — ou seja, chumbar 4 aqui ja tinha ficado velho (Fable sumia da tela do app
# e empurrava Sonnet/Haiku uma linha pra baixo). Serve so pra quando a linha-alvo esta rolada pra
# fora da viewport, onde nao ha o que parsear.
MODEL_ORDER = ["default", "opus", "fable", "sonnet", "haiku"]
MODEL_NUMBERS = {"default": 1, "opus": 2, "fable": 3, "sonnet": 4, "haiku": 5}

# Niveis do /effort do Claude Code, Faster -> Smarter. Usado pra contagem de passos no
# caso canonico (Opus). Modelos menores expoem um subconjunto (lido ao vivo pelo driver).
EFFORT_ORDER = ["low", "medium", "high", "xhigh", "max", "ultracode"]

# Linha de modelo: espacos, marcador opcional (❯ cursor / ↑↓ rolagem), "N.", rotulo ate 2+ espacos,
# e o resto da linha = descricao ("Opus 5 with 1M context · Best for everyday, complex tasks").
_ROW_RE = re.compile(r"^\s*([❯↑↓]?)\s*(\d+)\.\s+(.+?)\s{2,}(.*)$")
# Marcador de esforco ajustavel: "<glifo> <Palavra> effort ... to adjust".
_EFFORT_RE = re.compile(r"^\S\s+(\w+)\s+effort\b")
# Follow-up CONDICIONAL que o /model dispara ao confirmar uma TROCA de effort que invalida o
# cache da conversa:
#   Change effort level?
#   ❯ 1. Yes, switch to <nivel>
#     2. No, go back
# Nao aparece sempre (so quando ha cache a re-ler). Por design, NAO confirmamos sozinhos: o app
# mostra esse menu como OptionButtons (via state.classify) pro usuario escolher Yes/No.
_EFFORT_CONFIRM_RE = re.compile(r"Yes,\s+switch\s+to\s+\w+", re.IGNORECASE)


class PickerError(Exception):
    """Falha ao dirigir o picker. `status` vira o HTTP correspondente na camada de API."""

    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def _picker_region(pane: str) -> list[str]:
    """Recorta so o bloco do picker (de 'Select model' ate 'Esc to cancel').

    Evita falso-positivo com listas numeradas que apareçam no historico do chat: o picker
    e um overlay (nao vai pro scrollback), entao quando fechado o recorte fica vazio.
    """
    lines = pane.splitlines()
    start = None
    for i, ln in enumerate(lines):
        if "Select model" in ln:
            start = i  # usa a ultima ocorrencia (overlay unico)
    if start is None:
        return []
    end = len(lines)
    for j in range(start, len(lines)):
        if "Esc to cancel" in lines[j]:
            end = j + 1
            break
    return lines[start:end]


def picker_open(pane: str) -> bool:
    """True se o pane mostra o picker do /model aberto."""
    return bool(_picker_region(pane))


def picker_desenhado(pane: str) -> bool:
    """True so quando o picker esta INTEIRO na tela (o rodape ja foi desenhado).

    `picker_open` casa assim que o titulo aparece, e nesse instante as linhas de modelo ainda estao
    sendo pintadas: medido numa sessao real, ler ali devolvia 4 modelos em vez de 5 — o Haiku, que e
    a ultima linha, simplesmente nao existia no pane ainda. O rodape ("Esc to cancel") vem DEPOIS
    das linhas, entao ele e a prova de que a lista esta completa.
    """
    return any("Esc to cancel" in ln for ln in _picker_region(pane))


def parse_model_rows(pane: str) -> list[dict]:
    """Linhas de modelo do picker: numero, rotulo, keyword (1a palavra), descricao, cursor, ativo.

    `name` e o rotulo sem o ✔ e sem o "(recommended)" — e o que a tela mostra. `keyword` continua
    sendo a 1a palavra em minusculas, que e o que o driver usa pra casar a linha-alvo.
    """
    rows: list[dict] = []
    for ln in _picker_region(pane):
        m = _ROW_RE.match(ln)
        if not m:
            continue
        marker, num, label, desc = m.group(1), int(m.group(2)), m.group(3).strip(), m.group(4).strip()
        parts = label.split()
        keyword = parts[0].lower() if parts else ""
        rows.append(
            {
                "number": num,
                "label": label,
                "name": label.replace("✔", "").replace("(recommended)", "").strip(),
                "desc": desc,
                "keyword": keyword,
                "cursor": marker == "❯",  # ❯
                "active": "✔" in label,  # ✔
            }
        )
    return rows


def cursor_row(rows: list[dict]) -> dict | None:
    """Linha sob o cursor (❯); cai pra linha ativa (✔) se o cursor nao foi capturado."""
    return next((r for r in rows if r["cursor"]), None) or next(
        (r for r in rows if r["active"]), None
    )


def model_nav_steps(rows: list[dict], target_keyword: str) -> int:
    """Passos (com sinal) do cursor ate o modelo-alvo. >0 = Down, <0 = Up.

    Usa o numero de linha real quando o alvo esta visivel; senao cai pro MODEL_NUMBERS
    (a linha-alvo pode estar rolada pra fora). Levanta ValueError se o cursor nao for achado
    ou o alvo for desconhecido.
    """
    base = cursor_row(rows)
    if base is None:
        raise ValueError("cursor row not found in picker")
    target = next((r for r in rows if r["keyword"] == target_keyword), None)
    if target is not None:
        tnum = target["number"]
    elif target_keyword in MODEL_NUMBERS:
        tnum = MODEL_NUMBERS[target_keyword]
    else:
        raise ValueError(f"target model {target_keyword!r} not in picker")
    return tnum - base["number"]


def parse_current_effort(pane: str) -> str | None:
    """Nivel de esforco atual (minusculo), ou None se o modelo nao suporta / nao ha marcador."""
    for ln in _picker_region(pane):
        s = ln.strip()
        if "not supported" in s.lower():
            return None  # ex: Haiku
        m = _EFFORT_RE.match(s)
        if m and "to adjust" in s:
            return m.group(1).lower()
    return None


def effort_confirm_open(pane: str) -> bool:
    """True se o follow-up condicional 'Change effort level?' (re-leitura do cache) esta aberto."""
    return bool(_EFFORT_CONFIRM_RE.search(pane))


def parse_result_line(pane: str) -> str | None:
    """Linha de resultado apos confirmar (ex: 'Set model to X for this session only ...')."""
    for ln in reversed(pane.splitlines()):
        s = ln.strip().lstrip("⎿").strip()  # tira o glifo ⎿ de resultado
        if s.startswith("Set model to") or s.startswith("Kept model"):
            return s
    return None


# Token logo apos "Set model to": e o id EXATO que o Claude Code aplicou.
_RESULT_MODEL_RE = re.compile(r"^Set model to\s+(\S+)")


def result_model(result_line: str | None) -> str | None:
    """Id do modelo citado na linha de resultado, ou None.

    Existe porque `alvo in result` NAO serve pra decidir se a confirmacao e desta troca: a linha da
    troca ANTERIOR continua na tela, e num catalogo real um id e prefixo do outro — trocar de
    `k3-256k` para `k3` casaria na linha velha ("Set model to k3-256k …" contem "k3") e o app
    reportaria sucesso antes de a troca acontecer. Comparar o TOKEN inteiro resolve nos dois
    sentidos.
    """
    if not result_line:
        return None
    m = _RESULT_MODEL_RE.match(result_line)
    return m.group(1) if m else None
