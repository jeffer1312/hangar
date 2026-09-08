"""Instalar um harness que falta, pelo botão do painel de Harnesses.

Até aqui um CLI ausente aparecia como "não instalado" e mais nada — quem usa tinha de ir procurar o
comando na documentação do fornecedor. Aqui o botão roda **o instalador oficial do fornecedor**, e
só ele: nada de inventar passo de instalação.

Quatro decisões, com o motivo de cada uma:

**Só comando conferido no site do fornecedor entra em `_COMANDOS`.** Chutar nome de pacote instala
software errado na máquina de quem usa, e o npm tem homônimo pra quase tudo: `oh-my-pi` no npm está
na casa do 0.2 e não é o omp (que é o `can1357/oh-my-pi`, na casa do 18); `kimi-code` no npm é um
proxy de terceiro que roda o `claude` por baixo; e `kimi` é uma biblioteca de máquina de estados.
Harness sem comando conferido **para este sistema** não ganha botão: ganha o link em `MANUAL`.

**Instalar o CLI não basta: sem o wrapper o app não o enxerga.** É o `install-claude-wrapper.sh`
que escreve o bloco do rc que carrega os wrappers de shell (`claude`, `codex`, `pi`, `omp`, `kimi`)
e os lançadores de `~/.local/bin` — `hangar-codex-tui` inclusive, que é o que o backend chama pra
abrir uma sessão Codex. Sem essa etapa, quem instalasse o Codex por aqui ganharia um CLI que roda no
terminal e some do app, que é o oposto do que o botão promete. Ele é idempotente, então rodá-lo
inteiro é mais barato e mais seguro do que escolher a parte relativa àquele CLI.

**Fora isso, nada de etapa nova:** roda os MESMOS consertos que o card daquele harness já expõe
(hooks, extensões, ponte de skills), na ordem em que já aparecem. Quem sabe o que falta num CLI
recém-instalado é o diagnóstico; uma lista paralela envelheceria à parte dele.

**Quem diz se instalou é o diagnóstico relendo o disco, não o `rc` do comando.** Um `npm install -g`
sai 0 e deixa o binário fora do PATH, e no Windows o `shutil.which` só acha o que casa PATHEXT. O
`diagnosticar()` já responde "binário OU pasta de config", que é a pergunta certa, e é a resposta
dele que a tela mostra.

**Roda DENTRO do backend**, ao contrário do `app/atualizar.py`. O que obriga a atualização a se
destacar num processo à parte é ela reiniciar o backend — dentro, ela se mataria no meio. Instalar
um CLI não reinicia nada, então o precedente certo é o `codex_integracao.SERVICO`, que esta mesma
tela já consulta: trabalho numa thread, estado consultável, front por polling.
"""
from __future__ import annotations

import asyncio
import logging
import os
import shutil

from app import atualizar, harness_saude

_log = logging.getLogger("hangar.harness_install")

# Constante de módulo, e não `os.name` lido na hora: `monkeypatch.setattr(os, "name", "nt")` leva o
# `pathlib` junto e estoura no primeiro `Path(...)`. Mesmo motivo do `atualizar._E_WINDOWS`.
_E_WINDOWS = os.name == "nt"

_TETO_LOG = 400
# O instalador do omp baixa um binário de ~190 MB e o do Kimi um de ~174 MB; um `npm install -g`
# com cache frio passa de 3 min. 15 é folga, não expectativa.
_TIMEOUT = 900.0

ETAPAS = ("comando", "conferir", "wrapper", "ajustes")


def _ps(linha: str) -> list[str]:
    return ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", linha]


def _sh(linha: str) -> list[str]:
    """`pipefail` nas FLAGS do bash, e não dentro do script.

    Sem ele um `curl … | sh` MENTE: o `sh` lê um stdin vazio e sai 0 quando o `curl` falhou, e a
    instalação seria dada como feita. Nas flags o `rc` fica honesto e o comando que a pessoa lê na
    confirmação continua sendo o do fornecedor, sem enfeite nosso no meio dele.
    """
    return ["bash", "-o", "pipefail", "-c", linha]


# Comando oficial de cada fornecedor, conferido na documentação deles em 07/09/2026. `None` = não há
# comando conferido PARA ESTE SISTEMA — o card mostra o link de `MANUAL` e nenhum botão.
_COMANDOS: dict[str, list[str] | None] = {
    "codex": ["npm", "install", "-g", "@openai/codex"],
    "pi": ["npm", "install", "-g", "--ignore-scripts", "@earendil-works/pi-coding-agent"],
    "omp": (_ps("irm https://omp.sh/install.ps1 | iex") if _E_WINDOWS
            else _sh("curl -fsSL https://omp.sh/install | sh")),
    "kimi": (None if _E_WINDOWS
             else _sh("curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash")),
}

# Onde ler a instrução quando não há comando para este sistema. Fica para TODOS, inclusive os que
# têm botão: quem prefere instalar na mão continua tendo para onde ir.
MANUAL = {
    "codex": "https://github.com/openai/codex",
    "pi": "https://pi.dev/docs/latest",
    "omp": "https://github.com/can1357/oh-my-pi",
    "kimi": "https://kimi.com/code",
}


def _exibir(argv: list[str]) -> str:
    """O comando como a pessoa lê antes de aprovar: o do fornecedor, não o embrulho que o roda."""
    return argv[-1] if argv[0] in ("bash", "powershell") else " ".join(argv)


def comandos() -> dict[str, str]:
    """O que dá para instalar por botão nesta máquina, por harness."""
    return {cli: _exibir(argv) for cli, argv in _COMANDOS.items() if argv}


def _resolver(argv: list[str]) -> list[str]:
    """O CAMINHO do executável, não o nome dele.

    No Windows o `CreateProcess` não aplica PATHEXT, então `["npm", ...]` (que lá é `npm.cmd`)
    levanta `FileNotFoundError` — a mesma armadilha que deixou o painel sem versão nenhuma dos CLIs
    instalados por npm (ver `harness_saude._versao`).
    """
    exe = shutil.which(argv[0])
    if not exe:
        raise ValueError(f"{argv[0]} não está nesta máquina")
    return [exe, *argv[1:]]


class EmCurso(Exception):
    """Já há OUTRA instalação rodando. O argumento é o harness que está com a vez."""


class Instalador:
    """Uma instalação por vez, com o progresso lido por polling."""

    def __init__(self) -> None:
        self._task: asyncio.Task | None = None
        self._estado: dict[str, object] = self._zerado()

    @staticmethod
    def _zerado(**campos: object) -> dict[str, object]:
        return {"fase": "ocioso", "harness": None, "etapa": None, "passo": 0,
                "total": len(ETAPAS), "log": [], "avisos": [], "ok": None, "erro": None, **campos}

    def status(self) -> dict[str, object]:
        return {**self._estado, "comandos": comandos(), "manual": MANUAL}

    async def iniciar(self, cli: str) -> dict[str, object]:
        argv = _COMANDOS.get(cli)
        if argv is None:
            raise ValueError(cli)
        # Uma por vez, e quem manda é a FASE, não o `Task`. Duas armadilhas que a checagem por
        # `_task.done()` tinha, e as duas terminavam em sucesso falso na tela — que é justamente o
        # que este módulo não pode fazer: (1) pedir a instalação de OUTRO harness enquanto uma roda
        # devolvia o estado do primeiro, então a tela lia `ok=True` de uma instalação que nunca
        # começou; (2) entre `fase="pronto"` e o `Task` fechar, `done()` ainda é falso, e até o
        # mesmo harness recebia o resultado da rodada anterior como se fosse desta.
        if self._estado["fase"] == "rodando":
            if self._estado["harness"] != cli:
                raise EmCurso(str(self._estado["harness"]))
            return self.status()
        self._estado = self._zerado(fase="rodando", harness=cli, etapa=ETAPAS[0], passo=1)
        self._task = asyncio.create_task(asyncio.to_thread(self._executar, cli, argv))
        # Rede pro que o `except Exception` do `_executar` não pega (um `BaseException` na saída do
        # processo, por exemplo): sem isto a fase ficaria "rodando" para sempre e — agora que é ela
        # quem guarda a vez — nenhuma instalação futura conseguiria começar.
        self._task.add_done_callback(self._encerrou)
        return self.status()

    def _encerrou(self, task: asyncio.Task) -> None:
        # `task is not self._task` é a mesma janela que o `iniciar` fecha: entre a thread escrever
        # `fase="pronto"` e o `Task` dela fechar, uma instalação NOVA pode ter começado. Sem esta
        # linha o callback da rodada velha pintava a rodada nova como interrompida — e, como a vez
        # é guardada pela fase, ainda soltava a tranca com a thread nova trabalhando.
        if task is not self._task or self._estado["fase"] != "rodando":
            return
        try:
            falha = task.exception()
        except asyncio.CancelledError:
            falha = None
        self._falhou(str(self._estado["etapa"] or ETAPAS[0]),
                     f"a instalação foi interrompida{f': {falha}' if falha else ''}")

    # ── o trabalho, já dentro da thread ────────────────────────────────────────────────────────

    def _executar(self, cli: str, argv: list[str]) -> None:
        try:
            p = atualizar._rodar(_resolver(argv), timeout=_TIMEOUT, log=self._anotar)
            if p.returncode != 0:
                self._falhou("comando", f"o instalador saiu com {p.returncode}")
                return

            self._passo("conferir")
            # O `--version` está em cache por 10 min: sem esquecer, a releitura devolveria a
            # resposta de ANTES da instalação e o card diria "não instalado" com o CLI no disco.
            harness_saude.esquecer_versao(cli)
            card = self._card(cli)
            if not card or not card.get("instalado"):
                # O PATH que o `which` lê é o do SERVIÇO, congelado na subida — um instalador que
                # grava fora dele deixa o CLI no disco e invisível aqui. Dizer só "não apareceu"
                # mandava a pessoa procurar no escuro, então a mensagem nomeia onde se olhou.
                self._falhou("conferir",
                             "o comando terminou, mas o CLI não aparece nem no PATH deste serviço "
                             f"({os.environ.get('PATH', '')}) nem na pasta de config dele")
                return

            self._passo("wrapper")
            if not self._wrapper(cli):
                return

            self._passo("ajustes")
            # Depois do instalador: ele já ligou extensões e ponte de skills, então o que sobrar
            # aqui é o que só o diagnóstico sabe (credenciais, hooks do Kimi). Idempotente dos dois
            # lados, então o que já ficou pronto vira uma linha de "nada a fazer".
            card = self._card(cli) or card
            pendentes = [i for i in card["itens"] if i.get("conserto")]
            for item in pendentes:
                try:
                    feito = harness_saude.consertar(item["conserto"])
                except Exception as e:  # noqa: BLE001 — um conserto sabe falhar de muitos jeitos
                    # Para aqui, e DIZ o que ficou pra trás: só o nome do que falhou deixava a
                    # pessoa sem saber que os seguintes nem foram tentados.
                    restantes = [i["conserto"] for i in pendentes[pendentes.index(item) + 1:]]
                    faltou = f"; não cheguei a rodar: {', '.join(restantes)}" if restantes else ""
                    self._falhou("ajustes", f"{item['conserto']}: {e}{faltou}")
                    return
                self._anotar(f"$ {item['conserto']}\n{feito}")
            self._pub(fase="pronto", ok=True, erro=None)
        except Exception as e:  # noqa: BLE001 — ver abaixo
            # `Exception`, e não uma lista de tipos. Isto roda numa thread cujo único canal com a
            # tela é este estado: uma exceção que escape daqui morre num `Task` que ninguém lê e
            # deixa `fase=rodando` para sempre — a barra gira sem fim e ninguém descobre por quê.
            _log.exception("instalação de %s falhou", cli)
            self._falhou(self._estado.get("etapa") or ETAPAS[0], f"{type(e).__name__}: {e}")

    def _wrapper(self, cli: str) -> bool:
        """Os wrappers do Hangar e os lançadores de `~/.local/bin`. `False` = parou aqui.

        No Windows não há o que rodar: os wrappers de lá são do `install.ps1` (que dot-sourceia
        `claude.ps1` do perfil do PowerShell) e não há wrapper de `codex`. Aí a etapa é PULADA, e o
        pulo vira aviso no estado — não só uma linha perdida no meio do log, porque a manchete da
        tela promete "ligado ao app" e nessa máquina isso não aconteceu.
        """
        try:
            comando = harness_saude.cmd_instalador()
        except ValueError as e:
            self._anotar(f"[wrapper pulado] {e}")
            self._pub(avisos=[*self._avisos(), f"a etapa do wrapper foi pulada: {e}"])
            return True
        p = atualizar._rodar(comando, cwd=harness_saude._REPO,
                             timeout=harness_saude.TIMEOUT_INSTALADOR, log=self._anotar)
        if p.returncode != 0:
            self._falhou("wrapper", f"o instalador dos wrappers saiu com {p.returncode}")
            return False
        # O `rc` não prova, aqui como em `conferir`: o instalador só escreve nos shells que ELE
        # detecta (com o PATH do serviço) e sai 0 tendo coberto menos do que a pessoa usa. A
        # releitura já existe e é de graça.
        item = harness_saude._wrapper(cli)
        if item["ok"] is False:
            self._falhou("wrapper", "o instalador rodou, mas o wrapper continua faltando em: "
                                    f"{item['params'].get('lista', '?')}")
            return False
        return True

    def _avisos(self) -> list:
        atual = self._estado.get("avisos")
        return list(atual) if isinstance(atual, list) else []

    def _card(self, cli: str) -> dict | None:
        return next((h for h in harness_saude.diagnosticar() if h["id"] == cli), None)

    def _pub(self, **campos) -> None:
        # Troca o dict inteiro em vez de mutá-lo: quem lê é o loop de eventos, noutra thread, e uma
        # leitura no meio de várias mutações veria um estado pela metade.
        self._estado = {**self._estado, **campos}

    def _passo(self, chave: str) -> None:
        self._pub(fase="rodando", etapa=chave, passo=ETAPAS.index(chave) + 1)

    def _anotar(self, texto: str) -> None:
        """Uma LINHA por item, mesmo recebendo um bloco: o teto conta itens, e um bloco
        multi-linha o furaria (mesma conta do `atualizar._log_do_estado`)."""
        linhas = [*self._estado.get("log", []), *(texto.splitlines() or [texto])]
        self._pub(log=linhas[-_TETO_LOG:])

    def _falhou(self, etapa: str, msg: str) -> None:
        _log.error("instalação parou em %s: %s", etapa, msg)
        self._pub(fase="pronto", ok=False, etapa=etapa, erro=msg)


INSTALADOR = Instalador()
