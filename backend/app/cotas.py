"""Cota por conta lida NA FONTE do provedor (janelas 5h/7d), não no sidecar de statusline.

Por que existe (medido 18/08/2026): a faixa do rodapé tirava o limite do último sidecar de
statusline DENTRO da pasta da conta. Numa máquina onde `<conta>/.hangar-status` é um
symlink pra pasta da conta padrão — o caso desta aqui — as três contas liam o MESMO arquivo e
desenhavam o MESMO número; e mesmo sem o symlink, conta sem sessão aberta nunca teve leitura
nenhuma. Cota não é propriedade da sessão, é da CREDENCIAL: quem responde tem que ser o provedor.

Fontes, todas verificadas contra a API real em 18/08/2026:

 - Claude: GET https://api.anthropic.com/api/oauth/usage com o `accessToken` de
   `<conta>/.credentials.json`. Devolve `five_hour`/`seven_day` com `utilization` (percentual) e
   `resets_at` (ISO). NÃO é chamada de inferência — não consome cota nenhuma.
 - Kimi: GET <base_url>/usages com a `api_key` de cada provider `type = "kimi"` do
   `~/.kimi-code/config.toml`. A janela curta vem em `limits[]` (`window.duration == 300`
   minutos) e a longa em `usage`. Os dois trazem `limit`/`remaining` como STRING, e o usado é
   `limit - remaining`: campo `used` não existe nesta API (o refresher da statusline em
   `scripts/omniroute-statusline.js` lê `detail.used` e por isso nunca desenhou a cota do Kimi).
 - OpenCode Zen: fora, de propósito. `/v1/usage`, `/v1/usages`, `/v1/me`, `/v1/balance` e
   `/v1/account` respondem 404 (só `/v1/models` existe) — faltar a linha é melhor que inventar
   número.
 - CommandCode: GET https://api.commandcode.ai/alpha/billing/credits com a `api_key` do engine
   (verificado 21/08/2026). Rota NÃO documentada (a doc oficial não expõe usage; quem a usa é o
   painel do site e o CodexBar) e fora do prefixo `/provider` do base_url — por isso a URL é
   fixa. Devolve `windowLimits.fiveHour`/`weekly` com `used`/`cap` em USD e `resetAt` em
   epoch-MILISSEGUNDOS, mais `credits` (sem teto na resposta, então não vira janela).

Token expirado NÃO é renovado aqui. O refresh token da Anthropic rotaciona: gravar o par novo por
baixo de um CLI vivo derrubaria a sessão dele. `expiresAt` no passado vira estado `expirada` sem
nem gastar a requisição; 401/403 caem no mesmo estado.
"""
import json
import logging
import threading
import time
import tomllib
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Callable, Literal

from fastapi import APIRouter, Depends
from pydantic import BaseModel

from app import apelidos, codex_appserver, contas, engines, opencode_cota, renova_token
from app.adapters.kimi import sessions as kimi_sessions
from app.auth import require_auth
from app.config import list_config_dirs

_log = logging.getLogger("hangar.cotas")

cotas_router = APIRouter(prefix="/api/cotas")

# 5 min: é a régua que o usuário pediu e o que a janela de 5h suporta sem mentir (1% de erro no
# pior caso). Conta EM USO não depende deste TTL pra parecer viva — a statusline da sessão dela
# continua desenhando o número no chat; aqui o que importa é a conta parada ter algum número.
_TTL_S = 300.0
_HTTP_TIMEOUT = 8.0
_URL_CLAUDE = "https://api.anthropic.com/api/oauth/usage"
# Mesmo cabeçalho que o CLI manda no endpoint OAuth; sem ele a rota responde, mas mandá-lo é o
# contrato documentado do token `sk-ant-oat`.
_BETA_CLAUDE = "oauth-2025-04-20"

Estado = Literal["lida", "sem_credencial", "expirada", "indisponivel"]
Provedor = Literal["claude", "kimi", "opencode", "commandcode", "codex"]


class JanelaCota(BaseModel):
    """Uma janela de limite. `rotulo` é dado do provedor ("5h"/"7d"), não texto de interface."""

    rotulo: str
    pct: float
    reset_ts: float | None = None


class CotaConta(BaseModel):
    """Cota de UMA credencial. `estado` distingue os quatro casos que a tela precisa separar:
    lida, conta sem credencial no disco, credencial expirada e falha de leitura."""

    id: str
    label: str
    provedor: Provedor
    # Conta-base do app (o `~/.claude` de `list_config_dirs`) — é a que uma sessão nova nasce
    # usando, e a faixa a marca. Não é "a conta da sessão em foco": isso é do chat, não da faixa.
    ativa: bool = False
    estado: Estado
    janelas: list[JanelaCota] = []
    ts: float | None = None
    idade_s: float | None = None
    motivo: str | None = None


# Resultado cru de um leitor: (estado, janelas, motivo).
_Leitura = tuple[Estado, list[JanelaCota], str | None]


@dataclass(frozen=True)
class _Fonte:
    chave: str
    label: str
    provedor: Provedor
    ler: Callable[[], _Leitura]
    ativa: bool = False


# ------------------------------------------------------------------------------------ HTTP


def _get_json(url: str, headers: dict[str, str]) -> tuple[int, object]:
    """GET -> (status, json). Status 0 = nem chegou a ter resposta (rede/timeout/DNS).

    Nada aqui levanta: uma conta que não responde não pode derrubar a lista das outras.
    """
    req = urllib.request.Request(url, headers=headers, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=_HTTP_TIMEOUT) as r:
            return r.status, json.loads(r.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        return e.code, None
    except (urllib.error.URLError, OSError, ValueError, TimeoutError) as e:
        _log.debug("cota: %s falhou: %r", url, e)
        return 0, None


def _iso_ts(v: object) -> float | None:
    """ISO-8601 do provedor -> epoch. Formato estranho vira None (a janela some, o pct fica)."""
    if not isinstance(v, str) or not v:
        return None
    try:
        return datetime.fromisoformat(v.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


# ---------------------------------------------------------------------------------- Claude


def _janela_claude(o: object, rotulo: str) -> JanelaCota | None:
    if not isinstance(o, dict):
        return None
    pct = o.get("utilization")
    if not isinstance(pct, (int, float)) or isinstance(pct, bool):
        return None
    return JanelaCota(rotulo=rotulo, pct=float(pct), reset_ts=_iso_ts(o.get("resets_at")))


def _token_claude(dir_conta: Path) -> tuple[str | None, _Leitura | None]:
    """Token OAuth da conta, ou o motivo de não dar pra usar. `expiresAt` vem em MILISSEGUNDOS."""
    try:
        o = json.loads((dir_conta / ".credentials.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None, ("sem_credencial", [], "credencial-ilegivel")
    oauth = o.get("claudeAiOauth") if isinstance(o, dict) else None
    tok = oauth.get("accessToken") if isinstance(oauth, dict) else None
    if not isinstance(tok, str) or not tok:
        return None, ("sem_credencial", [], "sem-token")
    exp = oauth.get("expiresAt")
    if isinstance(exp, (int, float)) and not isinstance(exp, bool) and exp / 1000 <= time.time():
        return None, ("expirada", [], "token-expirado")
    return tok, None


def _refresh_vivo(dir_conta: Path) -> bool:
    """O refresh token da conta ainda vale? (`refreshTokenExpiresAt`, em MILISSEGUNDOS)

    Separa os dois avisos que a tela precisa dar: refresh vivo = a conta esta so PARADA (basta
    uma sessao nela); refresh vencido ou ausente = ai sim e login de verdade.
    """
    try:
        o = json.loads((dir_conta / ".credentials.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    oauth = o.get("claudeAiOauth") if isinstance(o, dict) else None
    if not isinstance(oauth, dict) or not oauth.get("refreshToken"):
        return False
    exp = oauth.get("refreshTokenExpiresAt")
    if isinstance(exp, (int, float)) and not isinstance(exp, bool):
        return exp / 1000 > time.time()
    return True   # sem prazo no arquivo: trata como vivo (o pior caso e uma tentativa a toa)


def _tentar_renovar(dir_conta: Path, ativa: bool) -> str | None:
    """Renova a conta pelo caminho barato do `renova_token`. None = renovou; senão, o MOTIVO.

    O motivo é código, não texto: a tela escolhe a frase por ele (`lib/cota.ts`). São três, e a
    diferença entre eles é o que o usuário tem que fazer — nada (`sessao-viva`, volta sozinho no
    próximo turno da sessão), login de verdade (`login-necessario`) ou olhar o log
    (`renovacao-falhou`).

    A conta ATIVA (o `~/.claude`) nunca é renovada aqui, e é limitação assumida: processo que usa
    a pasta padrão NÃO define `CLAUDE_CONFIG_DIR`, então a varredura por ambiente não o enxerga —
    e renovar por baixo de uma sessão viva a deixaria com um refresh que já rodou. Ela também é a
    que mais tem sessão aberta, e a que se conserta sozinha assim que o usuário digita.
    """
    if not _refresh_vivo(dir_conta):
        return "login-necessario"
    if ativa or renova_token.esta_em_uso(dir_conta):
        return "sessao-viva"
    return None if renova_token.renovar_por_cli(dir_conta) else "renovacao-falhou"


def _ler_claude(dir_conta: Path, ativa: bool = False, renovou_agora: bool = False) -> _Leitura:
    """`renovou_agora` corta a recursão: com o par recém-gravado, um 401 é recusa de verdade."""
    tok, falha = _token_claude(dir_conta)
    if falha is not None and falha[2] == "token-expirado" and not renovou_agora:
        # Vencido pelo relógio não é o fim: o refresh costuma estar vivo (medido: access de 8h,
        # refresh de ~26 dias). Quem renova é o CLI, e só quando ninguém está usando a conta.
        motivo = _tentar_renovar(dir_conta, ativa)
        if motivo is not None:
            return "expirada", [], motivo
        tok, falha = _token_claude(dir_conta)
        renovou_agora = True
    if falha is not None:
        return falha
    status, j = _get_json(_URL_CLAUDE, {
        "Authorization": f"Bearer {tok}",
        "Accept": "application/json",
        "anthropic-beta": _BETA_CLAUDE,
    })
    if status in (401, 403):
        # O relógio dizia que o token valia e o provedor recusou (revogado, girado noutra máquina).
        # Passa pela MESMA porta do vencido: sem isso o motivo "sessao-viva" saía sem ninguém ter
        # olhado processo nenhum, e a tela mandava abrir uma sessão para uma conta que precisava de
        # login — sem nunca tentar renovar, a cada 5 min, para sempre.
        if not renovou_agora:
            motivo = _tentar_renovar(dir_conta, ativa)
            if motivo is None:
                return _ler_claude(dir_conta, ativa, renovou_agora=True)
            return "expirada", [], motivo
        return "expirada", [], "login-necessario"
    if not isinstance(j, dict):
        return "indisponivel", [], (f"http-{status}" if status else "sem-resposta")
    janelas = [w for w in (_janela_claude(j.get("five_hour"), "5h"),
                           _janela_claude(j.get("seven_day"), "7d")) if w is not None]
    if not janelas:
        return "indisponivel", [], "formato-desconhecido"
    return "lida", janelas, None


# ------------------------------------------------------------------------------------ Kimi


def _num(v: object) -> float | None:
    """O Kimi manda limite/restante como STRING ("100"). Aceita os dois, recusa o resto."""
    if isinstance(v, bool):
        return None
    if isinstance(v, (int, float)):
        return float(v)
    if isinstance(v, str):
        try:
            return float(v)
        except ValueError:
            return None
    return None


def _janela_kimi(detalhe: object, rotulo: str) -> JanelaCota | None:
    """`limit`/`remaining` -> pct USADO. Sem `used` nesta API: usar `limit - remaining`."""
    if not isinstance(detalhe, dict):
        return None
    lim, rest = _num(detalhe.get("limit")), _num(detalhe.get("remaining"))
    if lim is None or rest is None or lim <= 0:
        return None
    pct = max(0.0, min(100.0, (lim - rest) / lim * 100.0))
    return JanelaCota(rotulo=rotulo, pct=pct, reset_ts=_iso_ts(detalhe.get("resetTime")))


def _rotulo_janela(minutos: object) -> str:
    """Duração em minutos -> rótulo curto ("300" -> "5h"). Desconhecida vira "janela"."""
    n = _num(minutos)
    if n is None or n <= 0:
        return "janela"
    if n >= 1440 and n % 1440 == 0:
        return f"{int(n // 1440)}d"
    if n >= 60 and n % 60 == 0:
        return f"{int(n // 60)}h"
    return f"{int(n)}min"


def _base_usages_kimi(base: str) -> str:
    """Base do `api.kimi.com` sem `/v1` ganha o `/v1` — só pra cota, nunca pro motor."""
    if "api.kimi.com" in base and not base.rstrip("/").endswith("/v1"):
        return base.rstrip("/") + "/v1"
    return base


def _ler_kimi(api_key: str, base_url: str) -> _Leitura:
    """Forma do Kimi (`GET <base>/usages`), tentada em qualquer provedor de chave.

    Resposta ruim aqui NUNCA vira `expirada`, e isso é decisão, não descuido: só a rota OAuth do
    Claude tem semântica de credencial: um 401/403/404 aqui quase sempre quer dizer "esta URL não
    é essa rota" — medido 18/08 com a chave do OpenCode Zen, que devolve 403 num caminho que não
    existe. Chamar isso de "chave vencida" mandaria a pessoa refazer um login que está inteiro.
    Tudo que não for uma leitura boa cai em `indisponivel`, que a tela desenha como "não informa
    cota", com o código HTTP no motivo pra quem for investigar."""
    status, j = _get_json(base_url.rstrip("/") + "/usages", {
        "Authorization": f"Bearer {api_key}",
        "Accept": "application/json",
    })
    if not isinstance(j, dict):
        return "indisponivel", [], (f"http-{status}" if status else "sem-resposta")
    janelas: list[JanelaCota] = []
    limites = j.get("limits")
    for w in limites if isinstance(limites, list) else []:
        if not isinstance(w, dict):
            continue
        janela = w.get("window")
        rot = _rotulo_janela(janela.get("duration") if isinstance(janela, dict) else None)
        item = _janela_kimi(w.get("detail"), rot)
        if item is not None:
            janelas.append(item)
    # A janela larga do Kimi não traz duração: é a do plano (7 dias, medido pelo resetTime).
    longa = _janela_kimi(j.get("usage"), "7d")
    if longa is not None:
        janelas.append(longa)
    if not janelas:
        return "indisponivel", [], "formato-desconhecido"
    return "lida", janelas, None


# ------------------------------------------------------------------------------ CommandCode

_URL_COMMANDCODE = "https://api.commandcode.ai/alpha/billing/credits"
# O Cloudflare desta rota devolve 403 (error code 1010) pro User-Agent de lib HTTP — medido
# 21/08/2026: urllib puro 403, o mesmo GET com UA de navegador 200. O UA não é fingir browser
# por esporte: sem ele a rota simplesmente não responde.
_UA_NAVEGADOR = ("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
                 "Chrome/126.0.0.0 Safari/537.36")


def _janela_commandcode(o: object, rotulo: str) -> JanelaCota | None:
    """`used`/`cap` em USD -> pct. `resetAt` vem em epoch-MILISSEGUNDOS; 0 = sem reset marcado."""
    if not isinstance(o, dict):
        return None
    usado, teto = _num(o.get("used")), _num(o.get("cap"))
    if usado is None or teto is None or teto <= 0:
        return None
    reset = _num(o.get("resetAt"))
    return JanelaCota(rotulo=rotulo, pct=max(0.0, min(100.0, usado / teto * 100.0)),
                      reset_ts=reset / 1000 if reset else None)


def _ler_commandcode(api_key: str) -> _Leitura:
    """CommandCode (`GET /alpha/billing/credits`). Mesma semântica de erro do `_ler_kimi`: nada
    aqui vira `expirada` — um 403 nesta rota costuma ser o Cloudflare, não chave vencida."""
    status, j = _get_json(_URL_COMMANDCODE, {
        "Authorization": f"Bearer {api_key}",
        "Accept": "application/json",
        "User-Agent": _UA_NAVEGADOR,
    })
    if not isinstance(j, dict):
        return "indisponivel", [], (f"http-{status}" if status else "sem-resposta")
    limites = j.get("windowLimits")
    limites = limites if isinstance(limites, dict) else {}
    janelas = [w for w in (_janela_commandcode(limites.get("fiveHour"), "5h"),
                           _janela_commandcode(limites.get("weekly"), "7d")) if w is not None]
    if not janelas:
        return "indisponivel", [], "formato-desconhecido"
    return "lida", janelas, None


# Presença da credencial do Codex, cacheada pelo mtime do `auth.json` (ver _tem_credencial_codex).
_cred_codex_cache: tuple[tuple[float, ...], bool] | None = None


def _auth_codex() -> Path:
    """O `auth.json` do Codex. A pasta sai do `codex_appserver.home()` — `CODEX_HOME` é respeitado
    pelo mesmo motivo do lançador: quem move a pasta move a credencial junto."""
    return codex_appserver.home() / "auth.json"


def _tem_credencial_codex() -> bool:
    """Par OAuth presente no disco. Sem isto não há o que perguntar — e perguntar custa um processo
    de ~1,2s, então a checagem vem antes.

    Cache pelo mtime do arquivo, mesma razão do `_mapa_pi`: `id_conta_codex` roda POR SESSÃO Codex
    a cada varredura da lista, e ler+parsear um JSON de 4KB nesse laço é o tipo de custo que o tick
    do SSE não pode pagar (o `_mtimes` sobra um `stat`).
    """
    global _cred_codex_cache
    chave = _mtimes(_auth_codex())
    if _cred_codex_cache and _cred_codex_cache[0] == chave:
        return _cred_codex_cache[1]
    try:
        auth = json.loads(_auth_codex().read_text(encoding="utf-8"))
        tokens = auth.get("tokens")
        tem = isinstance(tokens, dict) and bool(tokens.get("access_token"))
    except (OSError, ValueError):
        tem = False
    _cred_codex_cache = (chave, tem)
    return tem


def id_conta_codex() -> str | None:
    """O id desta credencial no `/api/cotas`, ou None quando não há credencial.

    Uma função só porque o id vive em DOIS lugares: a fonte, aqui, e o campo `conta` da sessão
    Codex (`registry.list`). Ids diferentes fariam a pílula do topo procurar uma linha que a faixa
    desenha com outro nome, e cair no pior-geral sem ninguém entender — o mesmo cuidado que o
    comentário do `chave:<motor>` já registra.
    """
    return f"codex:{_auth_codex().parent}" if _tem_credencial_codex() else None


def _janela_codex(o: object) -> JanelaCota | None:
    """O percentual já vem PRONTO (`usedPercent`), e a janela se identifica pela duração em minutos
    — o mesmo `_rotulo_janela` do Kimi. `resetsAt` é epoch em SEGUNDOS, ao contrário do
    CommandCode: dividir por 1000 aqui poria o reset em 1970."""
    if not isinstance(o, dict):
        return None
    pct = _num(o.get("usedPercent"))
    if pct is None:
        return None
    reset = _num(o.get("resetsAt"))
    return JanelaCota(rotulo=_rotulo_janela(o.get("windowDurationMins")),
                      pct=max(0.0, min(100.0, pct)), reset_ts=reset or None)


def _ler_codex() -> _Leitura:
    """Cota da conta do Codex, pelo `account/rateLimits/read` de um app-server efêmero.

    Não é HTTP como as outras porque a credencial é um par OAuth do ChatGPT e o endpoint que a
    traduz em cota não é público — quem sabe fazer essa conta é o próprio binário. Medido em
    30/08/2026 (codex-cli 0.151.0): o método responde sem thread aberta, sem pane e sem sessão
    viva, em 1,2s. É por credencial, que é exatamente o que este painel pede.

    Nada aqui levanta, mesma regra do `_get_json`: um provedor que não responde não pode derrubar a
    lista das outras contas.
    """
    # A fonte só nasce com credencial (ver `_fontes`), então isto cobre a corrida: um logout entre
    # a montagem da fonte e a leitura pagaria o processo à toa e voltaria "falhou" no lugar de
    # "não há credencial".
    if not _tem_credencial_codex():
        return "sem_credencial", [], None
    try:
        # Mesmo teto das fontes HTTP: `_atualizar` espera TODAS as leituras juntas, então uma fonte
        # com teto maior que as outras vira o tempo de resposta do `/api/cotas` inteiro.
        r = codex_appserver.perguntar("account/rateLimits/read", timeout=_HTTP_TIMEOUT)
    except codex_appserver.CodexAusente:
        return "indisponivel", [], "codex-ausente"
    except (RuntimeError, OSError) as e:
        _log.debug("cota: codex nao respondeu: %r", e)
        return "indisponivel", [], "sem-resposta"
    limites = r.get("rateLimits")
    limites = limites if isinstance(limites, dict) else {}
    janelas = [j for j in (_janela_codex(limites.get("primary")),
                           _janela_codex(limites.get("secondary"))) if j is not None]
    if not janelas:
        return "indisponivel", [], "formato-desconhecido"
    return "lida", janelas, None


def _ler_opencode(cfg: dict[str, str]) -> _Leitura:
    """Adapta o leitor do painel do OpenCode ao formato de leitura deste módulo."""
    estado, janelas, motivo = opencode_cota.ler(cfg["workspace_id"], cfg["auth_cookie"])
    return estado, [JanelaCota(**j) for j in janelas], motivo


def _providers_kimi() -> list[tuple[str, str, str]]:
    """(nome, api_key, base_url) de cada provider Kimi com chave. Provider por OAuth fica de fora:
    o arquivo de storage é vazio nesta máquina e adivinhar o formato dele seria inventar."""
    cfg = kimi_sessions.kimi_home() / "config.toml"
    try:
        dados = tomllib.loads(cfg.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError, ValueError):
        return []
    provedores = dados.get("providers")
    if not isinstance(provedores, dict):
        return []
    out = []
    for nome, p in provedores.items():
        if not isinstance(p, dict) or p.get("type") != "kimi":
            continue
        key, base = p.get("api_key"), p.get("base_url")
        if isinstance(key, str) and key and isinstance(base, str) and base:
            out.append((str(nome), key, base))
    return out


# Provider do default_model do Kimi ("apikey/k3" -> "apikey") — a conta que uma sessão Kimi sem
# motor gasta. Cache por mtime: a registry chama por sessão a cada varredura de lista, e reler o
# config a cada poll seria I/O por sessão por tick sem ganho nenhum.
_padrao_kimi: tuple[float, str | None] | None = None


def provider_padrao_kimi() -> str | None:
    global _padrao_kimi
    cfg = kimi_sessions.kimi_home() / "config.toml"
    try:
        mt = cfg.stat().st_mtime
    except OSError:
        return None
    if _padrao_kimi and _padrao_kimi[0] == mt:
        return _padrao_kimi[1]
    try:
        dados = tomllib.loads(cfg.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError, ValueError):
        # Falha também cacheia (por mtime): sem isto um config ruim era relido por sessão por tick.
        _padrao_kimi = (mt, None)
        return None
    modelo = dados.get("default_model")
    prov = modelo.split("/", 1)[0] if isinstance(modelo, str) and "/" in modelo else None
    # O id "kimi:<nome>" só existe pra provider COM chave (type kimi + api_key + base_url, ver
    # _providers_kimi): default_model apontando pra OAuth/sem-chave não tem cota — None, e a
    # pílula cai no pior-geral em vez de carregar um id que nunca casa.
    if prov is not None and prov not in {nome for nome, _, _ in _providers_kimi()}:
        prov = None
    _padrao_kimi = (mt, prov)
    return prov


# Provider do Pi -> conta desta lista, casado pela CHAVE e não pelo nome: o Pi chama de
# "kimi-coding" a MESMA credencial que o Kimi Code chama de "apikey" (verificado: a chave é byte a
# byte a mesma), e cota é da credencial, não do rótulo que cada CLI deu pra ela. Casar por nome
# exigiria uma tabela de sinônimos que envelhece a cada provedor novo.
# Provider sem chave conhecida aqui (OAuth do Codex, provedor que só o Pi tem) devolve None e a
# pílula cai no pior-geral — o comportamento de antes, nunca um id que não casa com nada.
_PI_AGENT = Path.home() / ".pi" / "agent"
_mapa_pi_cache: tuple[tuple[float, ...], dict[str, str]] | None = None


def _mtimes(*caminhos: Path) -> tuple[float, ...]:
    out = []
    for p in caminhos:
        try:
            out.append(p.stat().st_mtime)
        except OSError:
            out.append(0.0)
    return tuple(out)


def _chaves_do_pi() -> dict[str, str]:
    """provider do Pi -> api key. Dois arquivos: `auth.json` (o que o `/login` do Pi grava) e os
    provedores manuais de `models.json`, que trazem a chave no próprio bloco."""
    out: dict[str, str] = {}
    try:
        auth = json.loads((_PI_AGENT / "auth.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        auth = {}
    for nome, d in (auth if isinstance(auth, dict) else {}).items():
        k = d.get("key") if isinstance(d, dict) else None
        if isinstance(k, str) and k:
            out[str(nome)] = k
    try:
        mods = json.loads((_PI_AGENT / "models.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        mods = {}
    provs = mods.get("providers") if isinstance(mods, dict) else None
    for nome, d in (provs if isinstance(provs, dict) else {}).items():
        k = d.get("apiKey") if isinstance(d, dict) else None
        if isinstance(k, str) and k:
            out.setdefault(str(nome), k)
    return out


def _mapa_pi() -> dict[str, str]:
    """provider do Pi -> id de conta. Cache pelos mtimes dos quatro arquivos porque isto roda por
    sessão Pi a cada varredura da lista (mesma razão do cache do `provider_padrao_kimi`)."""
    global _mapa_pi_cache
    chave = _mtimes(_PI_AGENT / "auth.json", _PI_AGENT / "models.json",
                    kimi_sessions.kimi_home() / "config.toml", engines.caminho())
    if _mapa_pi_cache and _mapa_pi_cache[0] == chave:
        return _mapa_pi_cache[1]
    por_chave: dict[str, str] = {}
    for nome, key, _base in _providers_kimi():
        por_chave.setdefault(key, f"kimi:{nome}")
    for nome, dados in engines.listar().items():
        key = dados.get("api_key")
        if isinstance(key, str) and key:
            por_chave.setdefault(key, f"chave:{nome}")
    mapa = {prov: por_chave[key] for prov, key in _chaves_do_pi().items() if key in por_chave}
    _mapa_pi_cache = (chave, mapa)
    return mapa


def conta_de_provider_pi(provider: str | None) -> str | None:
    return _mapa_pi().get(provider) if provider else None


# ------------------------------------------------------------------------- fontes e cache

_cache: dict[str, tuple[float, CotaConta]] = {}
_lock = threading.Lock()


def _fontes() -> list[_Fonte]:
    """Uma fonte por CREDENCIAL. As contas Claude usam o mesmo filtro da aba Contas (conta de
    verdade ou a base do app) — pasta de backup não vira linha na faixa."""
    out: list[_Fonte] = []
    for c in list_config_dirs():
        p = Path(c.path)
        if contas.e_conta(p) or c.active:
            out.append(_Fonte(f"claude:{c.path}", c.label, "claude",
                              lambda p=p, at=bool(c.active): _ler_claude(p, at), bool(c.active)))
    # Codex: UMA credencial por máquina (o `auth.json` do CODEX_HOME), e ela só vira linha quando
    # existe — quem não usa Codex não ganha uma linha vazia nem paga o processo que a leitura custa.
    cid_codex = id_conta_codex()
    if cid_codex:
        out.append(_Fonte(cid_codex, "Codex", "codex", _ler_codex))
    for nome, key, base in _providers_kimi():
        # CommandCode plugado como provider do Kimi Code: o `<base>/usages` dele é 403 — a rota
        # de cota é a do CommandCode, escolhida pela base_url, igual ao ramo das chaves abaixo.
        if "commandcode.ai" in base:
            out.append(_Fonte(f"kimi:{nome}", nome, "commandcode",
                              lambda k=key: _ler_commandcode(k)))
        else:
            out.append(_Fonte(f"kimi:{nome}", nome, "kimi",
                              lambda k=key, b=base: _ler_kimi(k, b)))
    # Chaves cadastradas no app (engines.json). O id casa com o da lista unificada
    # (`chave:<nome>`) de propósito: é a MESMA credencial nas duas telas, e ids diferentes fariam
    # a tela mostrar a linha sem cota enquanto a faixa mostra a cota, sem ninguém entender.
    # Só a forma do Kimi é tentada; provedor que não a responde vira `indisponivel` (que a tela
    # desenha como "não informa cota"), nunca um número inventado.
    for nome, dados in engines.listar().items():
        key, base = dados.get("api_key"), dados.get("base_url")
        if not (isinstance(key, str) and key and isinstance(base, str) and base):
            continue
        cid, rotulo = f"chave:{nome}", dados.get("label") or nome
        # OpenCode Go não tem rota de cota nenhuma (medido; ver app/opencode_cota.py): a leitura
        # dele é a página do painel com o cookie de sessão, e só existe se a pessoa colou o cookie.
        # Sem cookie a credencial continua na lista, sem número — nunca zero.
        cfg = opencode_cota.config_de(cid) if "opencode.ai" in base else None
        if cfg is not None:
            out.append(_Fonte(cid, rotulo, "opencode",
                              lambda c=cfg: _ler_opencode(c)))
        elif "commandcode.ai" in base:
            out.append(_Fonte(cid, rotulo, "commandcode",
                              lambda k=key: _ler_commandcode(k)))
        else:
            # Motor Kimi aponta pro endpoint formato-Anthropic (`/coding`, sem `/v1`) — é o certo
            # pra RODAR a sessão, mas o `/usages` só existe sob `/v1` (medido 21/08/2026: a mesma
            # chave lia cota pelo config.toml, que traz `/coding/v1`, e dava 404 pelo motor).
            out.append(_Fonte(cid, rotulo, "kimi",
                              lambda k=key, b=_base_usages_kimi(base): _ler_kimi(k, b)))
    return out


def _seguro(f: _Fonte) -> _Leitura:
    try:
        return f.ler()
    except Exception:                                        # noqa: BLE001 - fail-soft por fonte
        # `exception` e não `debug`: falha de REDE já é tratada dentro do `_get_json` (e lá o debug
        # é certo, porque é ruído esperado). Chegar aqui significa defeito no leitor — e um defeito
        # em debug seria relido a cada 5 min, pra sempre, sem ninguém ver.
        _log.exception("cota: leitor de %s levantou", f.chave)
        return "indisponivel", [], "erro-leitor"


def _atualizar(fontes: list[_Fonte], forcar: bool = False) -> None:
    """Relê em paralelo as fontes fora do TTL. Falha de REDE não apaga leitura boa (o número
    envelhece e a tela mostra a idade); `expirada`/`sem_credencial` são fato sobre a conta e
    sobrescrevem — deixar número velho ali faria conta deslogada parecer em uso.

    `forcar` ignora o TTL: é o botão "atualizar" da aba Contas — quem aperta quer a leitura
    de AGORA, não a do cache de 5 min (o poll da faixa continua sem ele)."""
    agora = time.monotonic()
    with _lock:
        vencidas = [f for f in fontes
                    if forcar or (h := _cache.get(f.chave)) is None or agora - h[0] >= _TTL_S]
    if not vencidas:
        return
    with ThreadPoolExecutor(max_workers=min(8, len(vencidas))) as ex:
        leituras = list(ex.map(_seguro, vencidas))
    with _lock:
        for f, (estado, janelas, motivo) in zip(vencidas, leituras):
            anterior = _cache.get(f.chave)
            if estado == "indisponivel" and anterior is not None and anterior[1].estado == "lida":
                # Mantém a leitura boa mas deixa o carimbo do TTL novo: sem isto uma queda de rede
                # faria as 8 requisições voltarem a cada poll de 60s.
                _cache[f.chave] = (time.monotonic(), anterior[1])
                continue
            _cache[f.chave] = (time.monotonic(), CotaConta(
                id=f.chave, label=f.label, provedor=f.provedor, ativa=f.ativa, estado=estado,
                janelas=janelas, ts=time.time() if estado == "lida" else None, motivo=motivo,
            ))


@cotas_router.get("", dependencies=[Depends(require_auth)], response_model=list[CotaConta])
def listar_cotas(forcar: bool = False) -> list[CotaConta]:
    """Cota de cada credencial da máquina, com no máximo 5 min de idade.

    A rota é síncrona de propósito (o FastAPI já a roda em thread): quem chama é um poll de 60s
    da faixa, e dentro do TTL ela não toca a rede — o custo real é uma rodada de requisições a
    cada 5 minutos, em paralelo. `?forcar=true` pula o TTL: é o botão "atualizar" da aba
    Contas (a faixa segue chamando sem ele).
    """
    fontes = _fontes()
    _atualizar(fontes, forcar)
    agora = time.time()
    # O nome exibido é o apelido, quando a pessoa deu um: sem isto a faixa mostra o nome que o
    # disco impôs — foi como uma conta chamada "apikey" foi parar no rodapé.
    nomes = apelidos.ler()
    saida = []
    with _lock:
        for f in fontes:
            hit = _cache.get(f.chave)
            if hit is None:
                continue
            c = hit[1]
            saida.append(c.model_copy(update={
                "label": nomes.get(c.id) or c.label,
                "idade_s": (agora - c.ts) if c.ts is not None else None}))
    return saida
