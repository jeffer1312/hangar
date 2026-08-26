"""Política de contas da máquina (`~/.claude/orquestracao-contas.md`): quais contas um executor
ou revisor pode usar, com quais modelos, e se pode trocar de modelo dentro dela.

O arquivo é do usuário e o árbitro o lê como texto; o app é dono de DUAS seções — a tabela em
`## O que pode` e a lista gerada em `## O que NÃO pode` — e nunca toca no resto (orq_md).
Conta fora da tabela é proibida: o painel não a oferece, e a lista gerada diz isso em texto pra
quem lê o arquivo.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from . import apelidos, config, contas, cotas, kimi_models, model_args, orq_md, pi_catalog

CABECALHO = ("conta", "provider", "apelido", "modelos", "trocar?")
SECAO_PODE = "O que pode"
SECAO_NAO_PODE = "O que NÃO pode"
PROVIDERS = ("claude", "kimi", "pi", "codex")
CONTA_PADRAO = "padrao"      # o ~/.claude
CONTA_CODEX = "openai-codex"


def caminho() -> Path:
    return contas.compartilhado() / "orquestracao-contas.md"


@dataclass(frozen=True)
class ContaPolitica:
    conta: str
    provider: str
    apelido: str = ""
    modelos: tuple[str, ...] = ("*",)   # ids; "*" = qualquer
    trocar: bool = True

    def libera(self, modelo: str) -> bool:
        return "*" in self.modelos or modelo in self.modelos


@dataclass(frozen=True)
class ContaInventario:
    conta: str
    provider: str
    apelido: str
    id_cota: str | None
    modelos: tuple[dict, ...] = field(default_factory=tuple)   # {id, name?, context_length?}
    reduced: bool = False   # Claude sem cache do picker: só os aliases mínimos


# ------------------------------------------------------------------ identidade

def _dir_claude(conta: str) -> Path:
    if conta in (CONTA_PADRAO, "", "~/.claude"):
        return contas.compartilhado()
    direto = Path.home() / f".claude-{conta}"
    # `claude-200-2` pode ser a pasta `~/.claude-claude-200-2` (grafia do arquivo vivo) ou o
    # usuário ter escrito o nome com o prefixo: quem decide é a pasta que existe.
    if not direto.is_dir() and conta.startswith("claude-"):
        curto = Path.home() / f".claude-{conta.removeprefix('claude-')}"
        if curto.is_dir():
            return curto
    return direto


def id_cota(provider: str, conta: str) -> str | None:
    """Id no /api/cotas. Claude aceita as duas grafias do arquivo vivo (`200-01` e
    `claude-200-2`): quem decide é a pasta que existe."""
    if provider == "claude":
        return f"claude:{_dir_claude(conta).resolve()}"
    if provider == "kimi":
        return f"kimi:{conta}"
    return None


def nome_conta_claude(path: str | Path) -> str:
    p = Path(path).resolve()
    if p == contas.compartilhado().resolve():
        return CONTA_PADRAO
    return p.name.removeprefix(".claude-")


# ------------------------------------------------------------------ inventário

def _modelos_claude_reduzidos(_dir_conta: Path) -> tuple[list[dict], bool]:
    return [{"id": "opus"}, {"id": "sonnet"}, {"id": "haiku"}], True


def inventario(catalogo_claude=_modelos_claude_reduzidos) -> list[ContaInventario]:
    """Tudo que a máquina conhece, na ordem Claude · Kimi · Pi · Codex. Falha de um provider não
    esconde os outros (sem `pi` no PATH, a lista Pi sai vazia).

    `catalogo_claude(dir) -> (modelos, reduced)`: a API injeta o leitor do cache do picker (o
    mesmo de /api/model-options); ler o picker daqui dirigiria N terminais."""
    nomes = apelidos.ler()
    out: list[ContaInventario] = []
    for cd in config.list_config_dirs():
        p = Path(cd.path)
        modelos, reduced = catalogo_claude(p)
        out.append(ContaInventario(nome_conta_claude(p), "claude", cd.label,
                                   f"claude:{p.resolve()}", tuple(modelos), reduced))
    cat = kimi_models.read_catalog() or {"models": []}
    for nome, _key, _base in cotas._providers_kimi():
        ms = tuple({"id": m["alias"], "name": m["name"], "context_length": m["context_length"],
                    "efforts": m["efforts"]} for m in cat["models"] if m["provider"] == nome)
        out.append(ContaInventario(nome, "kimi", nomes.get(f"kimi:{nome}", nome), f"kimi:{nome}", ms))
    try:
        por_prov: dict[str, list[dict]] = {}
        for m in pi_catalog.listar():
            por_prov.setdefault(m["provider"], []).append(
                {"id": m["id"], "context_length": m["context"], "thinking": m["thinking"]})
        for prov, ms in por_prov.items():
            out.append(ContaInventario(prov, "pi", prov, None, tuple(ms)))
    except Exception:  # noqa: BLE001 — pi ausente/quebrado não cega as outras contas
        pass
    out.append(ContaInventario(CONTA_CODEX, "codex", nomes.get("codex", "OpenAI Codex"), None, ()))
    return out


# ------------------------------------------------------------------ ler / gravar

def _de_linha(r: dict[str, str]) -> ContaPolitica | None:
    conta, prov = r.get("conta", ""), r.get("provider", "").lower()
    if not conta or prov not in PROVIDERS:
        return None
    modelos = tuple(m.strip().strip("`") for m in r.get("modelos", "*").split(",") if m.strip()) or ("*",)
    trocar = orq_md.normalizar(r.get("trocar?", "sim")) not in ("nao", "não", "no", "n")
    return ContaPolitica(conta, prov, r.get("apelido", ""), modelos, trocar)


def ler(texto: str | None = None) -> list[ContaPolitica]:
    if texto is None:
        texto, _ = orq_md.ler_arquivo(caminho())
    return [c for c in map(_de_linha, orq_md.ler_tabela(texto, CABECALHO)) if c]


def _para_linha(c: ContaPolitica) -> dict[str, str]:
    return {"conta": c.conta, "provider": c.provider, "apelido": c.apelido,
            "modelos": ", ".join(c.modelos), "trocar?": "sim" if c.trocar else "não"}


def _secao_nao_pode(texto: str) -> str:
    liberadas = {(c.provider, c.conta) for c in ler(texto)}
    fora = [i for i in inventario() if (i.provider, i.conta) not in liberadas]
    if not fora:
        return "- (todas as contas conhecidas estão liberadas)"
    return "\n".join(f"- `{i.conta}` ({i.provider}) — não liberada" for i in fora)


def gravar_conta(c: ContaPolitica, mtime_lido: float | None = None) -> float:
    texto, _ = orq_md.ler_arquivo(caminho())
    texto = orq_md.trocar_linha(texto, CABECALHO, c.conta, _para_linha(c), SECAO_PODE)
    texto = orq_md.trocar_secao(texto, SECAO_NAO_PODE, _secao_nao_pode(texto))
    return orq_md.gravar(caminho(), texto, mtime_lido)


def desligar(conta: str, mtime_lido: float | None = None) -> float:
    texto, _ = orq_md.ler_arquivo(caminho())
    texto = orq_md.remover_linha(texto, CABECALHO, conta)
    texto = orq_md.trocar_secao(texto, SECAO_NAO_PODE, _secao_nao_pode(texto))
    return orq_md.gravar(caminho(), texto, mtime_lido)


# ------------------------------------------------------------------ regra

def _niveis(provider: str, modelo: str) -> tuple[str, ...] | None:
    """None = aceita qualquer (Kimi sem catálogo, Codex)."""
    if provider == "claude":
        return model_args.EFFORT_CLAUDE
    if provider == "pi":
        return model_args.EFFORT_PI
    if provider == "kimi":
        cat = kimi_models.read_catalog()
        if cat:
            for m in cat["models"]:
                if m["alias"] == modelo and m["efforts"]:
                    return tuple(m["efforts"])
    return None


def permitido(provider: str, conta: str, modelo: str, esforco: str,
              politica: list[ContaPolitica] | None = None) -> str | None:
    """None = pode. Senão a chave i18n do motivo."""
    pol = politica if politica is not None else ler()
    if not pol:
        # Política vazia (arquivo sem tabela) = nada proibido AINDA: a regra dura só passa a
        # valer na primeira conta ligada/desligada pela tela — senão ninguém consegue trocar nada
        # antes de montar a política (medido em 26/08/2026).
        niveis = _niveis(provider, modelo)
        return "erro_orq_esforco_invalido" if esforco and niveis is not None and esforco not in niveis else None
    regra = next((c for c in pol if c.provider == provider
                  and orq_md.normalizar(c.conta) == orq_md.normalizar(conta)), None)
    if regra is None:
        return "erro_orq_conta_nao_liberada"
    if modelo and not regra.libera(modelo):
        return "erro_orq_conta_travada" if not regra.trocar else "erro_orq_modelo_nao_liberado"
    niveis = _niveis(provider, modelo)
    if esforco and niveis is not None and esforco not in niveis:
        return "erro_orq_esforco_invalido"
    return None
