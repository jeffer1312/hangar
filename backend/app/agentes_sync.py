"""Espalha UMA credencial (nome + base_url + api_key + modelos) pra config de cada agente da máquina.

Por que existe: quem cadastra um provedor no app já disse tudo que o Pi, o Kimi Code e o Codex
precisam saber — repetir isso à mão em três arquivos de formato diferente é onde nasce a chave
colada errada num deles. O Claude Code NÃO entra aqui: pra ele a credencial JÁ É o
`~/.claude/engines.json` que o app grava (app/engines.py); reimplementar seria uma segunda fonte
da verdade pro mesmo arquivo.

Três decisões que valem pros três alvos:

- **Agente não instalado não é erro.** Pasta inexistente devolve `ok=False, motivo="nao-instalado"`,
  distinto de qualquer falha de escrita: numa máquina que só tem o Pi, "o Kimi falhou" faria o
  usuário caçar um problema que não existe.
- **O arquivo é do usuário, não nosso.** JSON dá pra reescrever inteiro (é o formato do Pi e o
  próprio CLI dele faz isso); TOML não — `tomllib` só LÊ e o repo não tem lib de escrita. Então
  para os dois TOML a estratégia é a mesma do `kimi_hook_installer`: bloco delimitado por
  sentinelas, o resto do arquivo preservado byte a byte, backup `.bak-hangar` antes da primeira
  mexida e `tmp+rename` com PID no nome (dois writes sobrepostos com nome fixo promovem bytes
  entrelaçados).
- **Campo que o provedor não informou não é inventado.** `engine_probe.listar_modelos` devolve só
  `{id, context_length, vision}`; custo, `maxTokens` e `reasoning` ficam de fora em vez de virarem
  zero — um custo 0 chutado aparece na tela do Pi como fato.

Permissão: o novo arquivo herda o modo do original. O `config.toml` do Kimi e o `models.json` do Pi
guardam a API key em texto puro (é o formato deles); um 0600 que voltasse 0644 depois da nossa
escrita seria vazamento causado por nós.
"""
import json
import logging
import os
import shutil
import tomllib
from pathlib import Path
from typing import Any

from app import atomico
from app.adapters.kimi.sessions import kimi_home
from app.engines import _NOME_OK  # mesma regra de nome do motor — uma fonte só (precedente: engine_probe)

_log = logging.getLogger("hangar.agentes_sync")

ALVOS = ("pi", "kimi", "codex")


# ---------------------------------------------------------------- caminhos

def base_openai(base_url: str) -> str:
    """A base no dialeto OpenAI: a raiz do provedor mais `/v1`.

    O app guarda a RAIZ, sem `/v1`, porque é isso que os dois consumidores do lado Claude pedem: o
    Claude Code monta `{raiz}/v1/messages` e o probe de modelos monta `{raiz}/v1/models`. Os três
    agentes daqui querem a OUTRA forma — o Pi (`openai-completions`) e o Codex (`wire_api = "chat"`)
    montam `{base}/chat/completions`, e o próprio `config.toml` do Kimi Code escreve
    `https://api.kimi.com/coding/v1`.

    Escrevendo a raiz crua, a credencial nascia apontando pra `https://opencode.ai/zen/chat/completions`
    em vez de `.../zen/v1/chat/completions`. Relatado em 25/08/2026: a conta do OpenCode Zen cadastrada
    pelo app aparecia no Pi com o endereço errado, e nada falhava até a primeira chamada. Conferido na
    documentação dos seis provedores do catálogo — OpenCode Zen `/zen/v1`, Kimi Code `/coding/v1`,
    OpenRouter `/api/v1`, Groq `/openai/v1`, DeepSeek `/v1`, Anthropic `/v1` — a regra é a mesma pra
    todos, e é a mesma que o probe já assume ao montar `{raiz}/v1/models`.

    Idempotente: o campo aceita as duas formas (a pessoa pode colar a URL como o provedor documenta),
    então uma raiz que já termina em `/v1` não vira `/v1/v1`.
    """
    base = base_url.strip().rstrip("/")
    return base if base.endswith("/v1") else f"{base}/v1"


def _pi_dir(home: Path | None) -> Path:
    return (home / ".pi" / "agent") if home else (Path.home() / ".pi" / "agent")


def _kimi_dir(home: Path | None) -> Path:
    # Sem `home` respeita KIMI_CODE_HOME, que o CLI respeita (kimi_home).
    return (home / ".kimi-code") if home else kimi_home()


def _codex_dir(home: Path | None) -> Path:
    if home:
        return home / ".codex"
    return Path(os.environ.get("CODEX_HOME") or (Path.home() / ".codex"))


def nome_da_variavel(nome: str) -> str:
    """Variável de ambiente onde o Codex vai buscar a chave (ele é o único que não a guarda em disco)."""
    return "HANGAR_" + nome.upper().replace("-", "_") + "_API_KEY"


# ---------------------------------------------------------------- escrita

def _validar(nome: str, base_url: str, api_key: str) -> str | None:
    if not _NOME_OK.match(nome or ""):
        return "nome-invalido"
    if not base_url or not api_key:
        return "faltando-base-url-ou-chave"
    return None


def _escrever_restrito(alvo: Path, conteudo: str, modo: int) -> None:
    """Cria o arquivo JÁ com a permissão final e só então escreve.

    `write_text` seguido de `chmod` deixa uma JANELA: o arquivo nasce com o umask (0644 nesta
    máquina) e fica legível por qualquer usuário local enquanto a chave de API já está lá dentro.
    A janela é curta, mas o conteúdo é justo o que não pode vazar — e no caso mais comum (primeira
    sincronização, arquivo de destino ainda inexistente) ela acontece SEMPRE. `os.open` com o modo
    fecha isso na criação, que é o único ponto onde dá pra fechar.
    """
    fd = os.open(alvo, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, modo)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(conteudo)
    except BaseException:
        alvo.unlink(missing_ok=True)
        raise
    # O umask pode ter apertado o modo na criação; o chmod aqui só AFROUXA de volta pro pedido,
    # nunca abre uma janela (o arquivo já existe com o conteúdo e permissão no máximo mais restrita).
    os.chmod(alvo, modo)


def _gravar_preservando(caminho: Path, conteudo: str) -> None:
    """tmp+rename preservando o modo do arquivo original (0600 tem que continuar 0600)."""
    modo = (caminho.stat().st_mode & 0o777) if caminho.exists() else 0o600
    tmp = caminho.with_name(f"{caminho.name}.tmp-hangar-{os.getpid()}")
    _escrever_restrito(tmp, conteudo, modo)
    atomico.substituir(tmp, caminho)


def _backup(caminho: Path) -> None:
    bak = caminho.with_name(f"{caminho.name}.bak-hangar")
    if caminho.exists() and not bak.exists():
        # Mesma razão do _escrever_restrito: o backup carrega o config INTEIRO do usuário, com todas
        # as chaves já cadastradas. `copyfile` criaria o destino pelo umask e só depois seria
        # apertado — copiar por dentro de um descritor já restrito não tem essa janela.
        _escrever_restrito(bak, caminho.read_text(encoding="utf-8"),
                           caminho.stat().st_mode & 0o777)


def _ts(valor: str) -> str:
    """Valor como basic string do TOML. Serve também pra CHAVE entre aspas (mesmas regras)."""
    esc = str(valor).replace("\\", "\\\\").replace('"', '\\"')
    return '"' + "".join(c if c >= " " and c != "\x7f" else "\\u%04X" % ord(c) for c in esc) + '"'


def _sentinelas(nome: str) -> tuple[str, str]:
    return (f"# >>> hangar: provedor {nome} (app/agentes_sync) — gerado, não editar à mão",
            f"# <<< hangar: fim do provedor {nome}")


def _gravar_bloco_toml(
    cfg: Path, nome: str, bloco: str, tabelas: dict[str, list[str]],
) -> tuple[bool, str]:
    """Insere/substitui NOSSO bloco no TOML do usuário, preservando o resto.

    `tabelas` = o que o bloco define, por seção (ex.: {"providers": ["kimi"], "models": ["kimi/k3"]}).
    Serve pro check de conflito: se uma dessas tabelas já existe FORA do nosso bloco, apendar seria
    redefinição e o arquivo pararia de abrir — melhor recusar que corromper config de terceiro.
    """
    ini, fim = _sentinelas(nome)
    try:
        raw = cfg.read_text(encoding="utf-8") if cfg.exists() else ""
    except UnicodeDecodeError:
        # Config do usuario que nao e UTF-8 (editor antigo, arquivo vindo de outra maquina, cp1252
        # no Windows). Sem isto o UnicodeDecodeError subia CRU: o irmao do Pi logo abaixo ja
        # traduzia esse mesmo caso pra "config-invalido", que e a CHAVE que a tela traduz — a
        # mensagem crua do Python aparecia no lugar do texto do app, em ingles e sem acao. E o
        # arquivo de terceiro continua intocado, que e a regra desta funcao.
        return False, "config-invalido"
    i = raw.find(ini)
    if i >= 0:
        j = raw.find(fim, i)
        if j < 0:
            # Alguém apagou metade do bloco; adivinhar onde ele acabava é como corromper o arquivo.
            return False, "bloco-incompleto"
        j = raw.find("\n", j)
        j = len(raw) if j < 0 else j + 1
        resto, antes, depois = raw[:i] + raw[j:], raw[:i], raw[j:]
    else:
        resto, antes, depois = raw, raw, ""
    if resto.strip():
        try:
            dados = tomllib.loads(resto)
        except tomllib.TOMLDecodeError:
            return False, "config-invalido"
        for secao, chaves in tabelas.items():
            existentes = dados.get(secao)
            if isinstance(existentes, dict) and any(c in existentes for c in chaves):
                return False, "ja-existe-fora-do-bloco"
    if i < 0:
        # Bloco novo vai pro FIM: tabela no meio de outra tabela roubaria as chaves seguintes dela.
        antes = raw + ("" if not raw or raw.endswith("\n") else "\n")
        depois = ""
    _backup(cfg)
    _gravar_preservando(cfg, antes + bloco + depois)
    return True, str(cfg)


# ---------------------------------------------------------------- alvos

# Provedores que o Pi JÁ conhece: ele traz o catálogo embutido e só precisa da credencial. A chave é
# o começo da URL (sem esquema); o valor é o nome que o `auth.json` do Pi usa. Ordem importa —
# `/zen/go` tem de ser testado antes de `/zen`, que é prefixo dele.
#
# Tirado da tabela "API Keys" de `docs/providers.md` do próprio Pi (0.84.2), e conferido ao vivo em
# 25/08/2026: com um `auth.json` de UMA linha (`opencode-go`) e `models.json` VAZIO, o
# `pi --list-models opencode-go` devolveu 13 modelos com contexto, saída máxima, raciocínio e
# imagem — tudo certo, sem a gente escrever nada disso.
_PI_EMBUTIDOS: tuple[tuple[str, str], ...] = (
    ("opencode.ai/zen/go", "opencode-go"),
    ("opencode.ai/zen", "opencode"),
    ("api.kimi.com/coding", "kimi-coding"),
    ("api.anthropic.com", "anthropic"),
    ("openrouter.ai/api", "openrouter"),
    ("api.groq.com/openai", "groq"),
    ("api.deepseek.com", "deepseek"),
    ("api.x.ai", "xai"),
    ("api.minimax.io", "minimax"),
    ("api.mistral.ai", "mistral"),
    ("api.cerebras.ai", "cerebras"),
    ("api.together.xyz", "together"),
)


def provedor_embutido_do_pi(base_url: str) -> str | None:
    """Nome que o Pi usa pra este provedor, ou None se ele não conhecer o endereço."""
    alvo = base_url.strip().rstrip("/")
    alvo = alvo.split("://", 1)[-1].removeprefix("www.")
    if alvo.endswith("/v1"):
        alvo = alvo[:-3].rstrip("/")
    for prefixo, nome_pi in _PI_EMBUTIDOS:
        if alvo == prefixo or alvo.startswith(prefixo + "/"):
            return nome_pi
    return None


def _gravar_auth_pi(pi_dir: Path, nome_pi: str, api_key: str) -> tuple[bool, str]:
    """Grava SÓ a credencial em `auth.json`, sob o nome que o Pi usa pro provedor.

    É o caminho bom: o Pi tem o catálogo dele e monta a configuração inteira sozinho — modelo,
    janela, raciocínio e leitura de imagem. Escrever a lista de modelos por fora, como o caminho do
    `models.json` faz, é sempre pior, porque a lista sai do `/v1/models` do provedor e ele pode não
    dizer capacidade nenhuma (o OpenCode Zen devolve `{id, object, created, owned_by}` e nada mais —
    foi assim que o muse-spark, que lê imagem, chegou ao Pi como texto puro).

    NÃO sobrescreve credencial que não seja de chave de API. Uma entrada `oauth` ali é uma
    assinatura logada (Claude Pro, ChatGPT, OpenRouter por OAuth): trocá-la por uma chave desloga a
    pessoa daquele provedor no Pi, e sincronizar uma credencial nunca pode ter esse preço.
    """
    cfg = pi_dir / "auth.json"
    dados: dict[str, Any] = {}
    if cfg.exists():
        try:
            dados = json.loads(cfg.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, ValueError, UnicodeDecodeError):
            return False, "auth-invalido"
        if not isinstance(dados, dict):
            return False, "auth-invalido"
    atual = dados.get(nome_pi)
    if isinstance(atual, dict) and atual.get("type") != "api_key":
        return False, f"credencial-nao-substituivel ({nome_pi} já está logado de outro jeito no Pi)"
    dados[nome_pi] = {"type": "api_key", "key": api_key}
    _backup(cfg)
    _gravar_preservando(cfg, json.dumps(dados, indent=2, ensure_ascii=False) + "\n")
    return True, f"{cfg} (provedor {nome_pi}; o Pi resolve os modelos)"


def gravar_pi(
    nome: str, base_url: str, api_key: str, modelos: list[dict],
    *, home: Path | None = None,
) -> tuple[bool, str]:
    """`~/.pi/agent/models.json` — JSON, chave em texto puro (formato deles).

    Dois caminhos, e o primeiro é o preferido: provedor que o Pi já conhece recebe SÓ a credencial
    em `auth.json` e se configura sozinho (ver `_gravar_auth_pi`). Endereço desconhecido — gateway
    próprio, Ollama, um provedor novo — cai no `models.json`, onde a lista de modelos tem de vir de
    nós mesmos.
    """
    try:
        erro = _validar(nome, base_url, api_key)
        if erro:
            return False, erro
        d = _pi_dir(home)
        if not d.is_dir():
            return False, "nao-instalado"
        nome_pi = provedor_embutido_do_pi(base_url)
        if nome_pi:
            return _gravar_auth_pi(d, nome_pi, api_key)
        cfg = d / "models.json"
        dados: dict[str, Any] = {}
        if cfg.exists():
            try:
                dados = json.loads(cfg.read_text(encoding="utf-8"))
            except (json.JSONDecodeError, ValueError, UnicodeDecodeError):
                return False, "config-invalido"
            if not isinstance(dados, dict):
                return False, "config-invalido"
        provedores = dados.get("providers")
        if not isinstance(provedores, dict):
            if "providers" in dados:
                return False, "config-invalido"
            provedores = {}
        # Lido UMA vez (o arquivo passa de 240 KB aqui) e não por modelo.
        catalogo = _catalogo_do_pi(d)
        # Substituição pelo nome já é a idempotência aqui (e é o que "atualizar a credencial"
        # significa) — em JSON não há o risco de redefinição que o TOML tem.
        provedores[nome] = {
            "baseUrl": base_openai(base_url),
            # O app descobre modelo pelo /v1/models (dialeto OpenAI, engine_probe), então é esse o
            # dialeto que sabemos que a chave fala. "anthropic-messages" seria chute.
            "api": "openai-completions",
            "apiKey": api_key,
            "models": [_modelo_pi(m, catalogo)
                       for m in modelos if isinstance(m, dict) and m.get("id")],
        }
        dados["providers"] = provedores
        _backup(cfg)
        _gravar_preservando(cfg, json.dumps(dados, indent=2, ensure_ascii=False) + "\n")
        return True, str(cfg)
    except Exception as e:
        _log.exception("pi: falha ao gravar credencial %r", nome)
        return False, f"erro: {e}"


def _catalogo_do_pi(pi_dir: Path) -> dict[str, dict]:
    """Índice `id do modelo -> entrada` do catálogo que o PRÓPRIO Pi mantém (`models-store.json`).

    Existe por causa de uma perda medida em 25/08/2026: o `muse-spark-1.2-contributor` lê imagem, e
    a credencial publicada por aqui chegou ao Pi como texto puro. Não foi bug de parse — o
    `/v1/models` do OpenCode Zen devolve o shape OpenAI pelado (`{id, object, created, owned_by}`,
    conferido ao vivo, 30 modelos), então a capacidade simplesmente NÃO está na resposta do
    provedor, e nenhum campo a mais que o probe leia mudaria isso.

    Isto não fere a regra de "campo que o provedor não informou não é inventado": não é chute, é a
    MESMA tabela de onde o Pi tira as capacidades dos provedores dele (o `pi update` a atualiza), e
    ela já traz o modelo certo — `opencode-go / muse-spark-1.2-contributor` com
    `input: ["text", "image"]` e `contextWindow: 1048576`.

    Casamento por id EXATO, nunca aproximado: `meta/muse-spark-1.2` e `muse-spark-1.2-contributor`
    são entradas diferentes do catálogo, e colar a capacidade de uma na outra seria justamente o
    chute que a regra proíbe. Catálogo ausente/ilegível devolve `{}` — volta a valer só o que o
    provedor disse, que é o comportamento de antes.
    """
    try:
        bruto = json.loads((pi_dir / "models-store.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, ValueError, UnicodeDecodeError):
        return {}
    if not isinstance(bruto, dict):
        return {}
    fora: dict[str, dict] = {}
    for entrada in bruto.values():
        if not isinstance(entrada, dict):
            continue
        for m in entrada.get("models") or []:
            # `setdefault`: o mesmo id pode aparecer em mais de um provedor do catálogo. A primeira
            # ocorrência vence — são o mesmo modelo, e escolher entre cópias iguais não vale um
            # critério de desempate.
            if isinstance(m, dict) and isinstance(m.get("id"), str):
                fora.setdefault(m["id"], m)
    return fora


def _modelo_pi(m: dict, catalogo: dict[str, dict] | None = None) -> dict[str, Any]:
    # O que o PROVEDOR disse vence sempre: ele reflete esta chave e este deployment, e o catálogo é
    # uma tabela geral. O catálogo só preenche o que veio como "não sei" (vision None, sem
    # context_length) e o que o probe nunca informa (reasoning).
    cat = (catalogo or {}).get(m["id"]) or {}
    vision = m.get("vision")
    if vision is None:
        entrada = cat.get("input")
        vision = isinstance(entrada, list) and "image" in entrada
    out: dict[str, Any] = {
        "id": m["id"],
        # `name` do catálogo quando existe (é o rótulo bonito que o Pi já mostra pros provedores
        # dele); senão o id, que é a identidade que já temos e não é chute.
        "name": cat.get("name") if isinstance(cat.get("name"), str) else m["id"],
        # vision desconhecida nas DUAS fontes = texto puro. Declarar image aí faria o Pi mandar
        # imagem pra um modelo que talvez recuse.
        "input": ["text", "image"] if vision is True else ["text"],
    }
    janela = m.get("context_length")
    if not isinstance(janela, int):
        janela = cat.get("contextWindow")
    if isinstance(janela, int):
        out["contextWindow"] = janela
    if isinstance(cat.get("reasoning"), bool):
        out["reasoning"] = cat["reasoning"]
    # cost / maxTokens seguem de fora: custo chutado aparece na tela do Pi como fato.
    return out


def gravar_kimi(
    nome: str, base_url: str, api_key: str, modelos: list[dict],
    *, home: Path | None = None,
) -> tuple[bool, str]:
    """`~/.kimi-code/config.toml` — bloco marcado, resto do arquivo intacto."""
    try:
        erro = _validar(nome, base_url, api_key)
        if erro:
            return False, erro
        d = _kimi_dir(home)
        if not d.is_dir():
            return False, "nao-instalado"
        ini, fim = _sentinelas(nome)
        linhas = [ini, f"[providers.{_ts(nome)}]", 'type = "kimi"',
                  f"api_key = {_ts(api_key)}", f"base_url = {_ts(base_openai(base_url))}", ""]
        chaves_modelo = []
        for m in modelos:
            if not isinstance(m, dict) or not m.get("id"):
                continue
            chave = f"{nome}/{m['id']}"
            chaves_modelo.append(chave)
            linhas += [f"[models.{_ts(chave)}]", f"provider = {_ts(nome)}",
                       f"model = {_ts(m['id'])}"]
            if isinstance(m.get("context_length"), int):
                linhas.append(f"max_context_size = {m['context_length']}")
            linhas += [f"display_name = {_ts(m['id'])}", ""]
        # `capabilities` e `support_efforts` ficam de fora: o probe não diz quais o modelo tem, e
        # uma lista chutada aqui vira o Kimi mandando imagem pra quem não lê imagem.
        linhas += [fim, ""]
        return _gravar_bloco_toml(d / "config.toml", nome, "\n".join(linhas),
                                  {"providers": [nome], "models": chaves_modelo})
    except Exception as e:
        _log.exception("kimi: falha ao gravar credencial %r", nome)
        return False, f"erro: {e}"


def gravar_codex(
    nome: str, base_url: str, api_key: str, modelos: list[dict],
    *, home: Path | None = None,
) -> tuple[bool, str]:
    """`~/.codex/config.toml` — o ÚNICO que não guarda a chave: só o NOME da variável de ambiente.

    Por isso o motivo de sucesso carrega a variável: sem ela exportada, o provedor existe na config
    e o Codex falha na primeira chamada sem dizer o porquê.
    """
    try:
        erro = _validar(nome, base_url, api_key)
        if erro:
            return False, erro
        d = _codex_dir(home)
        if not d.is_dir():
            return False, "nao-instalado"
        var = nome_da_variavel(nome)
        ini, fim = _sentinelas(nome)
        # Sem tabela de modelos: no Codex o modelo é escolhido na hora (`model` + `model_provider`),
        # não cadastrado. `wire_api = "chat"` casa com o dialeto que o probe usa (/v1/models OpenAI).
        bloco = "\n".join([ini, f"[model_providers.{_ts(nome)}]", f"name = {_ts(nome)}",
                           f"base_url = {_ts(base_openai(base_url))}", f"env_key = {_ts(var)}",
                           'wire_api = "chat"', fim, ""])
        ok, motivo = _gravar_bloco_toml(d / "config.toml", nome, bloco,
                                        {"model_providers": [nome]})
        return (True, f"{motivo} (exporte {var})") if ok else (ok, motivo)
    except Exception as e:
        _log.exception("codex: falha ao gravar credencial %r", nome)
        return False, f"erro: {e}"


_FUNCOES = {"pi": gravar_pi, "kimi": gravar_kimi, "codex": gravar_codex}


def sincronizar(
    nome: str, base_url: str, api_key: str, modelos: list[dict],
    alvos: tuple[str, ...] = ALVOS, *, homes: dict[str, Path] | None = None,
) -> dict[str, dict[str, Any]]:
    """Grava a credencial em cada alvo. NUNCA levanta — um agente quebrado não pode derrubar os outros."""
    out: dict[str, dict[str, Any]] = {}
    for alvo in alvos:
        fn = _FUNCOES.get(alvo)
        if fn is None:
            out[alvo] = {"ok": False, "motivo": "alvo-desconhecido"}
            continue
        try:
            ok, motivo = fn(nome, base_url, api_key, modelos,
                            home=(homes or {}).get(alvo))
        except Exception as e:  # cinto e suspensório: cada gravar_* já é fail-soft
            _log.exception("%s: falha inesperada", alvo)
            ok, motivo = False, f"erro: {e}"
        out[alvo] = {"ok": ok, "motivo": motivo}
    return out
