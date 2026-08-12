"""Modelo e nível de esforço: validar antes de virar comando, montar por provider.

Por que a validação mora aqui e não no front: o comando que sobe a sessão vira uma STRING única e é
executado como `exec {command}` por um `$SHELL -c` (tmux.py:391) — quem monta usa `shlex.join`, mas
a regex continua sendo a barreira, porque ela é o que garante que o valor não é uma flag nem carrega
metacaractere caso alguém remonte o comando sem citar. O id vem de JSON de provedor (292 ids na
omniroute) ou de parse de tabela — nenhum dos dois é fonte confiável, e o front é só um cliente
entre outros. O repo já trata assim todo caminho comparável: uuid validado em registry.py:1404
("vai DIRETO pro comando do shell"), _PROIBIDO_NO_VALOR no engines.py, _clean no pi_models.py.

Por que flags diferentes por provider: o binário do Pi não conhece `--effort` e o do Claude não
conhece `--thinking`. A errada mata o processo no arranque com o pane já criado — o app reportaria
uma sessão aberta que não existe.
"""
import re

# Cobre tudo que os provedores medidos usam: `k3-256k`, `cx/gpt-5.6-sol-high`,
# `clinepass/cline-pass/glm-5.2`, `claude-opus-5` e `openrouter/~anthropic/claude-opus-latest` — o
# catálogo real do Pi traz 11 ids com `~` (revisão final da branch), e dentro do argumento citado
# por shlex.join o `~` não sofre expansão do shell. Colchete entra por causa de `opus[1m]`: é o
# formato que o próprio Claude Code usa pra marcar a janela de contexto e está no settings.json das
# contas do usuário — sem ele, retomar uma sessão dessas estourava. Barra é necessária (o Pi usa
# provider/id) e é inofensiva; espaço, `;`, `$`, crase e `|` não entram — a regex é a barreira.
ID_OK = re.compile(r"^[A-Za-z0-9._:~/\[\]-]{1,128}\Z")

# Valor que começa com `-` é recusado por HIGIENE, não por injeção de flag — medido em 12/08/2026,
# nesta máquina:
#
#     $ claude --model --version
#     "--version" is not a model this version of Claude Code recognizes, ...
#
# O parser CONSOME o token seguinte como valor de `--model`, mesmo começando com `-`; não há como
# fazer o binário ler o valor como opção dele. O que um id assim produz é uma sessão que sobe com
# modelo inválido e só reclama depois — e como o valor vem de catálogo de provedor, recusar aqui é
# mais barato que descobrir na sessão. Não trate isto como barreira de segurança: a barreira é a
# ID_OK acima.
def _e_flag(valor: str) -> bool:
    return valor.startswith("-")

# Listas FECHADAS, do --help de cada binário (medido em 10/08/2026). `ultracode` NÃO entra: é do
# picker interativo (`/effort ultracode`), não da flag de arranque.
EFFORT_CLAUDE = ("low", "medium", "high", "xhigh", "max")
EFFORT_PI = ("off", "minimal", "low", "medium", "high", "xhigh", "max")

_FLAG_ESFORCO = {"claude": "--effort", "pi": "--thinking"}
_NIVEIS = {"claude": EFFORT_CLAUDE, "pi": EFFORT_PI}


def validar(provider: str, model: str | None, effort: str | None) -> tuple[str | None, str | None]:
    """Devolve (model, effort) ou estoura ValueError.

    Nada pedido passa direto, seja qual for o provider: criar sessão Codex é caminho vivo e não pode
    virar 400 por causa de uma feature que ninguém acionou.
    """
    if model is None and effort is None:
        return None, None
    if provider not in _FLAG_ESFORCO:
        raise ValueError(f"provider {provider!r} não aceita escolha de modelo aqui")
    if model is not None and (not ID_OK.match(model) or _e_flag(model)):
        raise ValueError("model: use letras, números e . _ : / - ~ [ ] (até 128 caracteres, sem começar com -)")
    if effort is not None and effort not in _NIVEIS[provider]:
        raise ValueError(f"effort: use um de {', '.join(_NIVEIS[provider])}")
    return model, effort


def args_de(provider: str, model: str | None, effort: str | None) -> list[str]:
    """Argumentos prontos pro spawn_command. Revalida — barato, e fecha o caminho de quem chamar
    sem passar por validar()."""
    model, effort = validar(provider, model, effort)
    out: list[str] = []
    if model:
        out += ["--model", model]
    if effort:
        out += [_FLAG_ESFORCO[provider], effort]
    return out
