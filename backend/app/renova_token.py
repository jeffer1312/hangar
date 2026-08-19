"""Mantém vivo o token OAuth das contas Claude que ninguém está usando.

Por que existe: o `accessToken` de cada conta (`<config>/.credentials.json`, campo
`claudeAiOauth`) dura ~8h, e quem o renova é o PRÓPRIO Claude Code ao abrir uma sessão. Conta
parada não abre sessão, então o token vence e (a) a leitura de cota daquela conta morre e (b) o
`refreshToken` — que dura ~26 dias — caminha pro fim do prazo sem nada segurá-lo. Passado o prazo
do refresh, a conta exige login de novo, à mão.

O que foi MEDIDO em 18/08/2026, e é o que este módulo explora:

- Com o `expiresAt` marcado como vencido, abrir `claude` num pane tmux renovou sozinho em menos de
  20s, sem pedir login, reescrevendo o `.credentials.json` com token novo e +8h.
- `claude auth status --json` NÃO renova: o app rodou essa leitura por 6 dias seguidos numa conta e
  o token continuou vencido. Ou seja, não dá pra trocar a sessão tmux por uma chamada barata de CLI.
- Abrir `claude` numa pasta NÃO confiada trava na pergunta "Is this a project you created or one you
  trust?" — a sessão fica parada pra sempre esperando resposta. Por isso `pasta_confiada` lê as
  pastas já aceitas daquela conta (`<config>/.claude.json`, `projects[<caminho>]
  .hasTrustDialogAccepted`) e a conta sem nenhuma é PULADA, nunca aberta no escuro.

E a regra que protege quem está trabalhando: conta EM USO não é tocada. A Anthropic ROTACIONA o
refresh token na renovação — gravar um par novo por baixo de um CLI vivo pode derrubá-lo, e o
prejuízo (sessão do usuário caindo) é muito maior que o benefício (token de uma conta parada).
"""
import hashlib
import json
import logging
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

from app import contas, procinfo, tmux
from app.config import list_config_dirs

_log = logging.getLogger("claude_pocket.renova_token")

# Prefixo próprio: o `new_hidden_shell` ainda põe `term-` na frente, então a sessão real nasce como
# `term-cp-renova-<slug>-<hash>`. Nada mais no app usa esse nome, e o `_matar` daqui SÓ recebe alvo
# que esta função criou — matar sessão alheia é o erro que já custou o trabalho de sessões vivas.
_PREFIXO = "cp-renova-"

# Poll do arquivo de credencial. Curto porque a medição deu <20s pra renovação inteira; longo o
# bastante pra não fazer um stat por milissegundo enquanto o CLI sobe.
_POLL_S = 1.0

# Motivos que são "pulei de propósito", não "tentei e falhou" — separam `puladas` de `falhas` no
# relatório da rodada.
_MOTIVOS_DE_PULO = {"em-uso", "sem-pasta-confiada", "refresh-vencido"}


# ------------------------------------------------------------------ leitura do .credentials.json


def _oauth(dir_conta: Path) -> dict | None:
    """O bloco `claudeAiOauth` da conta, ou None quando não dá pra confiar no que está lá.

    Arquivo ausente (conta criada e nunca logada), ilegível, JSON inválido ou JSON válido do TIPO
    errado caem todos no mesmo None — precedente do `statusline.read()`: JSON do tipo errado não
    levanta ValueError, e o `.get()` lá na frente é que viraria AttributeError no meio de uma
    varredura de todas as contas.
    """
    try:
        dados = json.loads((dir_conta / ".credentials.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(dados, dict):
        return None
    oauth = dados.get("claudeAiOauth")
    return oauth if isinstance(oauth, dict) else None


def _epoch(oauth: dict | None, campo: str) -> float | None:
    """`expiresAt`/`refreshTokenExpiresAt` são MILISSEGUNDOS no arquivo; aqui saem em segundos.

    Ler o valor cru como segundos daria um vencimento em 1970 e a rotina tentaria renovar TODAS as
    contas, toda rodada — abrindo uma sessão tmux por conta em cima de contas que estão em dia.
    """
    if not isinstance(oauth, dict):
        return None
    v = oauth.get(campo)
    # `bool` é subclasse de int: `True` viraria o epoch 0,001s sem esta guarda.
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return None
    return v / 1000.0


def _assinatura(dir_conta: Path) -> tuple[float, float | None]:
    """(mtime do arquivo, vencimento em segundos). É o par que diz se a renovação aconteceu."""
    try:
        mtime = (dir_conta / ".credentials.json").stat().st_mtime
    except OSError:
        mtime = 0.0
    return mtime, _epoch(_oauth(dir_conta), "expiresAt")


def _renovou(antes: tuple[float, float | None], depois: tuple[float, float | None]) -> bool:
    """Renovou de verdade? O mtime é só o PORTÃO barato; quem decide é o vencimento.

    Só o mtime mentiria: o CLI reescreve o arquivo em outras ocasiões, e "mudou o arquivo" viraria
    "token novo" num relatório que o usuário lê pra saber se pode parar de se preocupar com a conta.
    """
    mt0, exp0 = antes
    mt, exp = depois
    if mt == mt0 or exp is None:
        return False
    return exp0 is None or exp > exp0


# ------------------------------------------------------------------------------ o que renovar


def contas_a_renovar(margem_s: float = 5400) -> list[Path]:
    """Contas cujo accessToken vence dentro da margem (ou já venceu).

    O filtro de quais pastas contam é o MESMO da aba Contas (`conta_estado.listar_contas`): conta
    carimbada pelo app ou a base dele. O catálogo `list_config_dirs` é mais largo de propósito
    (entra ali um `~/.claude-backup` com login legítimo, que a soma de custos precisa ver) — e
    abrir uma sessão dentro de uma pasta de backup do usuário seria mexer no que não é do app.

    Conta sem `expiresAt` legível fica de fora: não é conta com token velho, é conta sem token
    nenhum (criada e nunca logada). Abrir `claude` ali cairia na tela de login, que nenhuma rotina
    de fundo resolve.
    """
    agora = time.time()
    fora: list[Path] = []
    for c in list_config_dirs():
        p = Path(c.path)
        if not (contas.e_conta(p) or c.active):
            continue
        vence = _epoch(_oauth(p), "expiresAt")
        if vence is not None and vence - agora <= margem_s:
            fora.append(p)
    return fora


def pasta_confiada(dir_conta: Path) -> Path | None:
    """Primeira pasta confiada e AINDA EXISTENTE daquela conta, ou None.

    Confiança é por conta e por caminho: mora no `<config>/.claude.json` dela, em
    `projects[<caminho>].hasTrustDialogAccepted`. O `is True` é literal — um `"true"` string ou um
    `1` não é a resposta que o CLI grava, e tratar como sim colocaria a sessão exatamente no prompt
    de confiança que este módulo existe pra evitar.

    `is_dir()` porque o histórico guarda projeto que já foi apagado: nascer num diretório que sumiu
    é o CLI reclamando no lugar de renovar.
    """
    try:
        dados = json.loads((dir_conta / ".claude.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    projetos = dados.get("projects") if isinstance(dados, dict) else None
    if not isinstance(projetos, dict):
        return None
    for caminho, cfg in projetos.items():
        if not isinstance(cfg, dict) or cfg.get("hasTrustDialogAccepted") is not True:
            continue
        p = Path(caminho)
        if p.is_dir():
            return p
    return None


def esta_em_uso(dir_conta: Path) -> bool:
    """Algum processo VIVO já usa este config dir?

    Mesma consulta da borda destrutiva (o DELETE de conta): `procinfo._pids_com_config_dir` varre o
    ambiente dos processos atrás do `CLAUDE_CONFIG_DIR`. Um `claude` aberto fora do tmux não aparece
    no registry, mas aparece aqui.

    Varredura que FALHOU devolve True, não False: "não consegui olhar" não pode sair igual a "olhei
    e não tem ninguém" — o preço de errar pro lado do não é derrubar a sessão de alguém, e o preço
    de errar pro lado do sim é uma conta esperar a próxima rodada.

    O próprio processo do backend é descontado: ele pode ter herdado a variável de quem o subiu, e
    aí a conta base (`~/.claude`) nunca seria renovada — pra sempre, calada.
    """
    pids, varredura_ok = procinfo._pids_com_config_dir(dir_conta)
    if not varredura_ok:
        _log.warning("não consegui varrer os processos — trato %s como em uso", dir_conta)
        return True
    return any(pid != os.getpid() for pid in pids)


# ------------------------------------------------------------- a janela escondida (I/O do tmux)
#
# Toda fala com o tmux passa por estas três, e é o que os testes trocam por dublê — mesmo contrato
# das `_shell_*` do `login_conta`, que resolve o problema irmão (abrir uma janela escondida pra
# rodar um comando do `claude` no config dir de uma conta).


# Teto do subcomando barato. A medição deu poucos segundos; 20s é folga pra máquina carregada, e
# curto o bastante pra não segurar a leitura de cota que o chama (ela roda em lote).
_TIMEOUT_CLI_S = 20.0


def _bin_claude() -> str | None:
    """Caminho do BINÁRIO do claude, ou None.

    Nunca via shell: o `claude` do usuário é uma FUNÇÃO de shell (o wrapper que abre tmux), e cair
    nela aqui criaria uma sessão de tmux a cada chamada. `subprocess` com lista de argumentos não
    enxerga função de shell.
    """
    achado = shutil.which("claude")
    if achado:
        return achado
    # O serviço do systemd não herda o PATH do shell interativo (mesmo motivo do hangar.desktop).
    padrao = Path.home() / ".local" / "bin" / "claude"
    return str(padrao) if padrao.exists() else None


def renovar_por_cli(dir_conta: Path) -> bool:
    """Renovação BARATA: um subcomando local do CLI, sem tmux e sem gastar cota.

    MEDIDO em 18/08/2026, e é a diferença pro que o docstring do módulo registra sobre o
    `claude auth status --json` (que NÃO renova): `claude mcp list` autentica e regrava o
    `.credentials.json` com o par novo. `claude --version` não serve — nem tenta autenticar.

    Vale como atalho pro caminho caro (`renovar`, que abre sessão em tmux e leva ~20s): quando esta
    devolve False, aquele continua sendo o plano.

    NÃO checa `esta_em_uso` — quem chama decide, porque a resposta muda o que a tela diz. A regra
    de segurança (não renovar por baixo de sessão viva; o refresh ROTACIONA) continua sendo
    obrigação do chamador, igual nos dois caminhos.
    """
    binario = _bin_claude()
    if binario is None:
        _log.warning("renovação barata de %s: binário do claude não encontrado", dir_conta.name)
        return False
    antes = _assinatura(dir_conta)
    env = {**os.environ, "CLAUDE_CONFIG_DIR": str(dir_conta)}
    try:
        r = subprocess.run([binario, "mcp", "list"], env=env, timeout=_TIMEOUT_CLI_S,
                           stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
    except (OSError, subprocess.SubprocessError) as e:
        _log.warning("renovação barata de %s falhou: %r", dir_conta.name, e)
        return False
    if _renovou(antes, _assinatura(dir_conta)):
        return True
    # Rodou e não renovou: sem este log a conta só some da faixa com um motivo genérico e não sobra
    # pista nenhuma de por quê. stderr cortado — é diagnóstico, não despejo.
    erro = (r.stderr or b"").decode("utf-8", "replace").strip().replace("\n", " ")[:200]
    _log.info("renovação barata de %s não renovou (saída %s)%s",
              dir_conta.name, r.returncode, f": {erro}" if erro else "")
    return False


def _criar_janela(nome: str, cwd: str, dir_conta: str) -> str | None:
    """Janela escondida no cwd da pasta confiada, com o `CLAUDE_CONFIG_DIR` da conta.

    O config dir vai por `tmux new-session -e` porque é CAMINHO, não segredo — a regra dura do repo
    (chave nunca por linha de comando nem por `-e`, senão ela aterrissa no `/proc/<pid>/cmdline`,
    legível pela máquina inteira) não é violada aqui. E ele é obrigatório: sem o `-e`, o pane herda
    o ambiente do SERVIDOR tmux e o `claude` renovaria a conta errada, em silêncio.

    `new_hidden_shell` e não `new_session` por causa da marca `@cp_hidden`: sem ela o registry trata
    o pane como sessão Claude e a renovação vira um CARD nas três views do app, aparecendo e sumindo
    sozinho.
    """
    return tmux.new_hidden_shell(nome, cwd, config_dir=dir_conta)


def _submeter(alvo: str, comando: str) -> None:
    """Digita o comando e manda o Enter. Sem o Enter o `claude` fica escrito no pane e nunca sobe —
    e a espera abaixo estouraria o timeout inteiro achando que o CLI é lento."""
    if not tmux.send_keys(alvo, comando, literal=True):
        raise RuntimeError(f"não consegui digitar na janela de renovação {alvo}")
    if not tmux.send_keys(alvo, "Enter"):
        raise RuntimeError(f"não consegui enviar Enter para a janela de renovação {alvo}")


def _matar(alvo: str) -> None:
    tmux.kill_session(alvo)


def _nome_janela(dir_conta: Path) -> str:
    """Nome legível + hash curto do caminho.

    O tmux não aceita `.` no nome da sessão, e toda conta começa com um (`.claude-fulano`); o slug
    tira isso. O hash está aí porque `list_config_dirs` também aceita caminho arbitrário via
    `CP_CLAUDE_CONFIG_DIRS` — duas pastas de nomes parecidos em raízes diferentes viriam a colidir
    no mesmo nome de janela, e uma renovação mataria a janela da outra.
    """
    slug = re.sub(r"[^A-Za-z0-9_-]", "-", dir_conta.name.lstrip(".")) or "conta"
    curto = hashlib.sha1(str(dir_conta).encode("utf-8", "surrogateescape")).hexdigest()[:6]
    return f"{_PREFIXO}{slug}-{curto}"


def renovar(dir_conta: Path, espera_s: float = 45) -> tuple[bool, str]:
    """Abre `claude` numa janela escondida da conta e espera o token novo. (renovou?, motivo).

    A janela morre em TODO caminho — sucesso, timeout, erro do tmux, exceção no meio. Sessão órfã
    daqui é pane fantasma: um `claude` vivo segurando o config dir, que ainda por cima faria a
    própria rotina considerar a conta "em uso" nas rodadas seguintes.
    """
    if esta_em_uso(dir_conta):
        return False, "em-uso"
    pasta = pasta_confiada(dir_conta)
    if pasta is None:
        return False, "sem-pasta-confiada"

    antes = _assinatura(dir_conta)
    nome = _nome_janela(dir_conta)
    # Sobra de uma rodada que morreu no meio: a sessão tmux sobrevive ao processo do backend, e
    # reatá-la traria um `claude` já aberto (ou parado num prompt). Mesma decisão do `login_conta`.
    # Idempotente por contrato: matar sessão que não existe é sucesso.
    _matar(f"term-{nome}")
    alvo = _criar_janela(nome, str(pasta), str(dir_conta))
    if alvo is None:
        return False, "tmux-recusou"
    try:
        _submeter(alvo, "claude")
        limite = time.monotonic() + espera_s
        while time.monotonic() < limite:
            time.sleep(_POLL_S)
            if _renovou(antes, _assinatura(dir_conta)):
                return True, "renovado"
        return False, "timeout"
    finally:
        _matar(alvo)


def rodada(margem_s: float = 5400) -> dict:
    """Percorre as contas perto do vencimento e devolve o que aconteceu com cada uma.

    Nenhuma exceção escapa: isto roda em rotina de fundo, e uma conta com JSON estragado não pode
    impedir a renovação das outras nem derrubar quem chamou. O que falha vira LINHA no relatório —
    falha aqui aparece, não some.
    """
    relatorio: dict = {"renovadas": [], "puladas": [], "falhas": []}
    try:
        alvos = contas_a_renovar(margem_s)
    except Exception:
        _log.exception("não consegui listar as contas a renovar")
        return relatorio
    for dir_conta in alvos:
        conta = str(dir_conta)
        try:
            # Refresh token vencido = renovar é impossível: o `claude` abriria pedindo login e a
            # janela ficaria parada até o timeout. Pular com motivo é a resposta honesta — e é
            # justamente o estado que esta rotina existe pra ninguém alcançar.
            refresh = _epoch(_oauth(dir_conta), "refreshTokenExpiresAt")
            if refresh is not None and refresh <= time.time():
                relatorio["puladas"].append({"conta": conta, "motivo": "refresh-vencido"})
                continue
            ok, motivo = renovar(dir_conta)
            if ok:
                relatorio["renovadas"].append(conta)
            elif motivo in _MOTIVOS_DE_PULO:
                relatorio["puladas"].append({"conta": conta, "motivo": motivo})
            else:
                relatorio["falhas"].append({"conta": conta, "motivo": motivo})
        except Exception as e:
            _log.exception("renovação da conta %s falhou", conta)
            relatorio["falhas"].append({"conta": conta, "motivo": f"{type(e).__name__}: {e}"})
    return relatorio


# ------------------------------------------------------------------- agendamento (o app chama)

_INTERVALO_S = 6 * 3600
# 90 min de margem: o token dura ~8h, então a cada 6h ele tem no mínimo 2h de vida — a rodada só
# gasta uma abertura quando falta pouco, e não a cada ciclo. Uma conta parada há dias entra na
# primeira rodada porque já está vencida.
_MARGEM_S = 5400


async def laco(intervalo_s: float = _INTERVALO_S, margem_s: float = _MARGEM_S) -> None:
    """Roda na subida e a cada `intervalo_s`. Nunca levanta: `rodada` já é fail-soft, e o `except`
    de fora cobre o inesperado — este laço morrer derrubaria a renovação em silêncio até o próximo
    restart do backend, que é justo o cenário que ele existe pra cobrir (máquina ligada por dias).

    A rodada abre processo e espera arquivo mudar, então vai pra thread: no laço do asyncio ela
    seguraria o loop inteiro por dezenas de segundos.
    """
    import asyncio
    while True:
        try:
            rel = await asyncio.to_thread(rodada, margem_s)
            if rel.get("renovadas") or rel.get("falhas"):
                _log.info("renova_token: %s", rel)
            else:
                _log.debug("renova_token: nada a fazer (%s)", rel)
        except asyncio.CancelledError:
            raise
        except Exception:
            _log.exception("renova_token: rodada falhou inteira")
        await asyncio.sleep(intervalo_s)
