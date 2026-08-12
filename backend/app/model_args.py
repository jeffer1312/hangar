"""Modelo e nível de esforço: validar antes de virar comando, montar por provider.

Por que a validação mora aqui e não no front: o comando que sobe a sessão é montado por
CONCATENAÇÃO (`" ".join(...)` em registry.py:1114) e executado como `exec {command}` por um
`$SHELL -c` (tmux.py:391). Não há quoting nesse caminho. O id vem de JSON de provedor (292 ids na
omniroute) ou de parse de tabela — nenhum dos dois é fonte confiável, e o front é só um cliente
entre outros. O repo já trata assim todo caminho comparável: uuid validado em registry.py:1404
("vai DIRETO pro comando do shell"), _PROIBIDO_NO_VALOR no engines.py, _clean no pi_models.py.

Por que flags diferentes por provider: o binário do Pi não conhece `--effort` e o do Claude não
conhece `--thinking`. A errada mata o processo no arranque com o pane já criado — o app reportaria
uma sessão aberta que não existe.
"""
import re

# Cobre tudo que os provedores medidos usam: `k3-256k`, `cx/gpt-5.6-sol-high`,
# `clinepass/cline-pass/glm-5.2`, `claude-opus-5`. Barra é necessária (o Pi usa provider/id) e é
# inofensiva; espaço, `;`, `$`, crase e `|` não entram.
ID_OK = re.compile(r"^[A-Za-z0-9._:/-]{1,128}\Z")

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
    if model is not None and not ID_OK.match(model):
        raise ValueError("model: use letras, números e . _ : / - (até 128 caracteres)")
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
