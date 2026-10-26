"""Atualizar a máquina inteira por um botão: puxa o código, roda o que a versão exige, reinicia.

Três decisões que valem antes de ler o código:

**Roda DESTACADO do backend.** A atualização reinicia o backend; se quem a tocasse fosse a
requisição HTTP do botão, ela morreria no meio do próprio trabalho e ninguém terminaria o serviço —
a máquina ficaria com código novo no disco e processo velho no ar, que é exatamente o estado que
`install.ps1:1242` registra como o pior de todos (o `-Update` dizia "ok" com o processo antigo
vivo). Por isso `iniciar()` lança `python -m app.atualizar` fora do processo e devolve na hora.

**O progresso mora em ARQUIVO, não na conexão.** Consequência direta do de cima: a única coisa que
sobrevive ao restart é o disco. O front lê `estado.json` por polling, e é isso que deixa a tela
dizer "atualizando…" enquanto o servidor volta, em vez de "desconectado" — que é o que ela diria
lendo a conexão, e é a mesma frase que ela usa quando o servidor caiu de verdade.

**`reset --hard` é automático, mas nunca é irreversível.** O usuário pediu (25/08/2026) que o botão
faça tudo sozinho, reset incluído: quem usa não deve precisar saber que passos existem. A regra da
casa é não destruir sem ordem explícita, e as duas coisas convivem porque `resguardar()` roda
ANTES — o que estava no disco vai para uma branch de resgate e um stash, conferidos antes de
qualquer coisa destrutiva acontecer. Automático sim; perder trabalho, não.

Ordem de `executar()` copiada do `scripts/deploy.sh`, que já era a atualização inteira sem
interface: tudo que pode falhar acontece ANTES do restart. Aqui em Python porque o `deploy.sh` é
bash e o Windows também precisa disto; a etapa de reaplicar o que o `git pull` não atualiza segue
sendo o `install.sh --update` / `install.ps1 -Update`, que já existem e já sabem o que fazer em
cada sistema.
"""
from __future__ import annotations

import hashlib
import http.client
import json
import logging
import os
import queue
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
from pathlib import Path

from app import atomico, config, diag, log_paths, procinfo, rust_release, tmux

_log = logging.getLogger("hangar.atualizar")

REPO = Path(__file__).resolve().parents[2]

# Constante de módulo, e não `os.name` lido na hora, pelo precedente do `atomico.py` — mas aqui há
# um motivo a mais, e ele é de teste: `monkeypatch.setattr(os, "name", "nt")` leva o `pathlib`
# junto, e o primeiro `Path(...) / "x"` do código estoura com "cannot instantiate 'WindowsPath' on
# your system" (armadilha registrada no CLAUDE.md, e que este módulo pagou na primeira versão).
# Com a constante, o teste troca só a decisão, sem mexer no `pathlib` de ninguém.
_E_WINDOWS = os.name == "nt"

# Valores literais, não `subprocess.CREATE_NEW_PROCESS_GROUP`: esse atributo **só existe no
# Windows**, então referenciá-lo no corpo da função quebra o teste que exercita o ramo Windows a
# partir do Linux — que é o único lugar onde ele é testado por alguém deste projeto. Os números são
# os da API do Win32 e não mudam.
# Sem DETACHED_PROCESS de propósito: o `python.exe` do venv é um trampolim que roda o interpretador
# de base como FILHO, e um pai sem console nenhum faz esse filho alocar um — visível. Medido com
# sonda EnumWindows: uma janela do Windows Terminal subia 3s depois de cada Atualizar. O
# CREATE_NO_WINDOW sozinho já dá ao motor um console próprio (invisível) e independente do backend.
_FLAGS_MOTOR_WINDOWS = 0x00000200 | 0x08000000   # CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW

# CREATE_NO_WINDOW. Sem isto, cada comando de console que a atualização roda no Windows ABRE UMA
# JANELA na frente de quem está usando — visto em 25/08/2026: um console preto escrito "npm ci"
# subiu por cima do app no meio da atualização. No Linux não acontece (não há console a criar), e
# por isso o defeito só aparece na máquina onde ninguém estava olhando o código.
_SEM_JANELA_WINDOWS = 0x08000000 if os.name == "nt" else 0

# Etapas, na ordem em que acontecem. O rótulo é o que a tela mostra — em português, dizendo o que
# está acontecendo com a máquina, não o comando que roda.
ETAPAS = (
    ("resguardar", "Guardando o que estava aqui"),
    ("codigo",     "Puxando o código novo"),
    ("passos",     "Aplicando o que a versão pede"),
    ("instalar",   "Instalando dependências"),
    ("reiniciar",  "Reiniciando o servidor"),
    ("nativo",     "Atualizando o app desta máquina"),
)
# O reinício avulso (código já no disco) tem duas etapas próprias: numerar com a lista de cima
# mostraria "4 de 5" numa operação que só tem duas.
ETAPAS_REINICIO = (
    ("tela",      "Trocando a tela pelo build publicado"),
    ("reiniciar", "Reiniciando o servidor"),
)

_TIMEOUT_PADRAO = 600.0      # npm ci num repo frio passa de 3min; 10 é folga, não expectativa.
_TIMEOUT_SUBIR = 30.0        # quanto esperamos o backend responder depois do restart.


def _base() -> Path:
    """`<config>/.hangar-update/`. Mesma chave dos outros marcadores: o config dir em uso.

    Pasta nova — nunca existiu com o nome antigo, então não passa pela ponte de compatibilidade do
    `migracao_sidecars`.
    """
    raiz = Path(os.environ.get("CLAUDE_CONFIG_DIR") or (Path.home() / ".claude"))
    return raiz / ".hangar-update"


def _caminho_estado() -> Path:
    return _base() / "estado.json"


_PRINCIPAIS = ("main", "master")


def alvo() -> str:
    """Branch que o Atualizar segue: `CP_UPDATE_BRANCH`, ou a main."""
    return config.settings.update_branch or "main"


def _caminho_canal() -> Path:
    return _base() / "branch"


def _canal() -> str:
    """Branch fora da main em que a PRÓPRIA atualização pôs o checkout.

    Sem esta memória, esvaziar o `CP_UPDATE_BRANCH` transformaria a branch de teste em branch de
    trabalho, e o Atualizar recusaria justamente a volta pra main.
    """
    try:
        return _caminho_canal().read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        return ""
    except OSError as e:
        _log.warning("nao consegui ler a branch da atualizacao: %s", e)
        return ""


def _marcar_canal(branch: str) -> None:
    # Nunca levanta: quem chama é o motor (e o rollback), que só fala pelo estado.
    try:
        if branch in _PRINCIPAIS:
            _caminho_canal().unlink(missing_ok=True)
            return
        _caminho_canal().parent.mkdir(parents=True, exist_ok=True)
        _caminho_canal().write_text(branch, encoding="utf-8")
    except OSError as e:
        # `_avisar` põe na tela e no log; o log `hangar.*` chega ao diário pelo `DiaryHandler`.
        _avisar(f"nao consegui gravar a branch da atualizacao ({branch}): {e}")


# ─── Estado ────────────────────────────────────────────────────────────────────────────────────

_JANELA_NASCIMENTO_S = 120.0    # tempo que o motor tem pra subir e registrar o pid dele


def _recente(ts: str | None) -> bool:
    if not ts:
        return False
    try:
        idade = (datetime.now().astimezone() - datetime.fromisoformat(ts)).total_seconds()
    except (TypeError, ValueError):
        return False
    return 0 <= idade < _JANELA_NASCIMENTO_S


def estado_para_tela() -> dict:
    """O estado, com dono morto convertido em falha.

    Um `fase: rodando` só sai desse valor quando o próprio motor grava o desfecho. Se ele morre sem
    isso — kill, queda de energia, ou o instalador do Windows matando quem o invocou (medido em
    25/08/2026) —, o arquivo congela e a tela fica presa numa atualização que não existe mais, sem
    jeito de sair a não ser editando o JSON na mão. Aqui a ausência do processo VIRA o desfecho que
    ele não conseguiu escrever, e a pessoa vê a falha na tela como veria qualquer outra.

    Converte de uma vez (grava), pra a resposta não depender de quem perguntou nem repetir a
    conclusão a cada poll.
    """
    atual = estado()
    if atual.get("fase") != "rodando":
        return atual
    dono = atual.get("pid")
    if not dono:
        # O lançador pode morrer antes da primeira escrita do motor; a trava já guarda o pid devolvido ao cliente.
        try:
            dono = int((_base() / "rodando.lock").read_text(encoding="utf-8").strip())
        except (OSError, ValueError):
            dono = None
    if _vivo(dono):
        return atual
    if not dono and _recente(atual.get("ts")):
        # AINDA SEM PID e recém-escrito: é o motor subindo, não um órfão. `iniciar()` grava o
        # estado antes do `Popen` (pra não correr com o motor), e quem grava o pid é o próprio
        # motor, uns segundos depois — o tempo de o Python carregar. Sem esta guarda, o polling da
        # tela (2s) declararia morta toda atualização recém-lançada.
        return atual
    if _restarted_into_target(atual):
        _log.info("atualizacao concluida pelo backend novo: o motor %s morreu no reinicio que pediu", dono)
        try:
            nativo = _nativo_instalado()
        except (OSError, ValueError):
            nativo = True
        if nativo:
            reason = ("a atualizacao foi concluida pelo servidor novo" if atual.get("etapa") == "reiniciar"
                      else "o processo que o atualizava nao existe mais")
            _avisar(f"app nativo nao atualizou: {reason}")
        _dist_velho_apagar()
        _escrever(fase="pronto", ok=True, texto="Atualizado")
        _soltar_a_vez()
        return estado()
    _log.warning("atualizacao abandonada: o processo %s nao existe mais", dono)
    _escrever(fase="pronto", ok=False, voltou=False, no_ar=True, pid=dono,
              erro="a atualizacao foi interrompida — o processo que a executava nao existe mais")
    _soltar_a_vez()
    return estado()


_BACKEND_PID_ENV = "HANGAR_UPDATE_BACKEND_PID"
_RESTART_REQUEST_ENV = "HANGAR_RESTART_REQUEST_ID"


def _launching_backend() -> int | None:
    """O backend que lançou este motor; motor rodado à mão não tem, e não conta como reiniciado."""
    try:
        return int(os.environ.get(_BACKEND_PID_ENV, ""))
    except ValueError:
        return None


def _restarted_into_target(atual: dict) -> bool:
    """O motor morreu no reinício que ele mesmo pediu e o reinício valeu: quem responde é outro
    backend e o disco está no commit da atualização. Sem escopo próprio do systemd (gerenciador que
    recusa unidade transiente), o motor mora no cgroup do backend e o restart o leva junto."""
    backend = atual.get("backend_pid")
    para = atual.get("commit_para")
    if (atual.get("etapa") not in ("reiniciar", "nativo") or not isinstance(backend, int)
            or backend == os.getpid() or not para):
        return False
    try:
        return _git("rev-parse", "HEAD", timeout=30).stdout.strip() == para
    except (OSError, subprocess.SubprocessError):
        _log.warning("commit do disco ilegivel; a atualizacao interrompida fica como falha", exc_info=True)
        return False


def estado() -> dict:
    """O que a atualização está fazendo agora. `{}` quando nunca rodou.

    Exige **dict**: JSON válido do tipo errado (`null`, uma lista) não levanta `ValueError`, e o
    `.get()` de quem lê morreria com `AttributeError` — o mesmo furo que já derrubou a resolução de
    estado de TODAS as sessões em `statusline.read` (ver CLAUDE.md).
    """
    try:
        bruto = json.loads(_caminho_estado().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return bruto if isinstance(bruto, dict) else {}


def _escrever(**campos) -> None:
    """Grava o estado, mesclando com o que já está lá.

    O tmp leva o **pid** no nome: o processo da atualização e o backend (que também escreve, ao
    recusar um segundo início) podem gravar ao mesmo tempo, e um tmp de nome fixo faria o `rename`
    promover bytes entrelaçados — o furo que `hangar_panel_common.py` e a statusline já pagaram.
    """
    alvo = _caminho_estado()
    alvo.parent.mkdir(parents=True, exist_ok=True)
    atual = estado()
    atual.update(campos)
    atual["ts"] = datetime.now().astimezone().isoformat(timespec="seconds")
    tmp = alvo.with_suffix(f".{os.getpid()}.tmp")
    try:
        tmp.write_text(json.dumps(atual, ensure_ascii=False), encoding="utf-8")
        atomico.substituir(tmp, alvo)
    except OSError as e:
        # NUNCA deixa a escrita do estado derrubar quem chamou. No Windows o `atomico.substituir`
        # levanta `PermissionError [WinError 5]` quando outro processo segura o arquivo por mais de
        # ~0,42s, e isso subiu sem tratamento até o `GET /api/atualizacao`, derrubando o request —
        # ou seja, a tela perdia a resposta INTEIRA por causa de uma linha de log. Perder uma
        # gravação de progresso é aceitável: a próxima vem em segundos e o estado é cumulativo.
        # Perder o desfecho é o caso ruim, e contra ele há o `estado_para_tela`, que reconhece dono
        # morto. Falha aparece no log do processo, não em silêncio.
        _log.warning("nao consegui gravar o estado (%s): %s", ", ".join(campos), atomico.explicar(e))
        try:
            tmp.unlink(missing_ok=True)     # sem deixar tmp órfão pra trás
        except OSError:
            pass


def _etapa(chave: str, *, lista: tuple = ETAPAS, **extra) -> None:
    """Marca a etapa atual. `passo`/`total` alimentam a barra da tela.

    `etapa_inicio` existe pra tela poder mostrar um relógio correndo. Sem ele, uma etapa longa e
    MUDA — `npm ci --silent` é literalmente sem saída — fica indistinguível de uma travada: nem a
    barra anda, nem o log ganha linha. O relógio é o único sinal de vida que não depende do comando
    resolver falar.
    """
    idx = next((i for i, (k, _) in enumerate(lista) if k == chave), 0)
    _escrever(fase="rodando", etapa=chave, passo=idx + 1, total=len(lista),
              texto=lista[idx][1],
              etapa_inicio=datetime.now().astimezone().isoformat(timespec="seconds"), **extra)


# ─── Rodar comando ─────────────────────────────────────────────────────────────────────────────

_TETO_LOG = 400          # linhas guardadas no estado: o suficiente pra entender, sem inchar o JSON


# Só o processo da ATUALIZAÇÃO escreve no log. O backend também chama `_git` (o pré-voo e o
# changelog do `GET /api/atualizacao`, que a tela pede a cada 2s), e `_log_do_estado` é
# ler-mesclar-gravar sem trava: os dois processos disputavam o mesmo arquivo e o backend
# SOBRESCREVIA o que o motor tinha acabado de escrever. Efeito visto no Windows em 25/08/2026 — a
# caixa mostrava só os comandos do endpoint, nenhum do `fetch`/`merge`/`install`, e parecia travada
# na etapa 4. Pior que o log perdido: `_escrever` mescla o dict inteiro, então a disputa podia
# levar junto `fase`, `passo` e `etapa`.
_SOU_O_MOTOR = False


def _log_do_estado(texto: str) -> None:
    """Acrescenta uma linha ao log que a tela mostra.

    A pessoa que aperta o botão fica olhando "Instalando dependências" por mais de um minuto sem
    nenhum sinal de vida — a leitura natural disso é que travou (aconteceu, 25/08/2026). Guardar o
    comando e a saída dele no próprio estado deixa a tela abrir um terminalzinho: o `estado.json`
    já é lido por polling e sobrevive ao restart, então não precisa de canal novo.

    Só o processo da ATUALIZAÇÃO escreve aqui — ver `_SOU_O_MOTOR`.
    """
    if not _SOU_O_MOTOR:
        return
    # Guarda uma LINHA por item, mesmo recebendo um bloco. O teto conta itens, então blocos
    # multi-linha o furariam: medido, 400 blocos de 2 linhas viraram 801 linhas no estado que a
    # tela relê a cada 2s. Quebrar aqui mantém `_TETO_LOG` querendo dizer o que ele diz.
    atual = estado().get("log") or []
    atual.extend(texto.splitlines() or [texto])
    _escrever(log=atual[-_TETO_LOG:])


def _rodar(args: list[str], cwd: Path | None = None,
           timeout: float = _TIMEOUT_PADRAO,
           log: Callable[[str], None] | None = None) -> subprocess.CompletedProcess:
    """Um comando, com a saída capturada em texto.

    `errors="replace"` porque no Windows uma falha de decode estrita morre numa THREAD leitora:
    `run()` não levanta nada e `stdout` volta **None**, e o estouro aparece longe da causa (medido
    22/08/2026, registrado no CLAUDE.md). `LC_ALL=C` no git pela mesma razão do `git_ops._run`: a
    saída aqui é lida por código, e mensagem traduzida quebraria a leitura calada.

    `log` desvia a saída ao vivo pra outro dono. O `harness_install` precisa da mesma leitura linha
    a linha com prazo que existe aqui — e ela é medida, não óbvia (thread+fila porque o Windows não
    faz select em pipe; teto de 1s pra o prazo valer com o comando calado) —, mas o destino dela lá
    é o estado DELE, não o da atualização.
    """
    _emitir = log or _log_do_estado
    _emitir("$ " + " ".join(args))
    # Saída lida AO VIVO, linha a linha, e não com `capture_output` no fim. O `install.sh --update`
    # é um comando só que leva mais de um minuto (npm ci + build): esperando ele terminar pra
    # escrever, a caixinha da tela fica parada exatamente durante a espera que ela existe pra
    # explicar — que foi o que aconteceu com quem estava olhando, em 25/08/2026.
    linhas: list[str] = []
    limite = time.monotonic() + timeout
    proc = subprocess.Popen(
        args, cwd=str(cwd or REPO), stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, encoding="utf-8", errors="replace", bufsize=1,
        env={**os.environ, "LC_ALL": "C", "LANGUAGE": "C"},
        creationflags=_SEM_JANELA_WINDOWS,   # 0 no POSIX: `Popen` aceita e ignora
    )
    # A leitura vive numa THREAD, e o laço principal espera na fila com prazo. Iterar direto em
    # `proc.stdout` bloqueia dentro do `readline()`, então o relógio só era consultado quando uma
    # linha chegava — um processo que trava CALADO (npm esperando um prompt, rede pendurada) nunca
    # devolvia o controle e o timeout virava letra morta. Medido: `sh -c "sleep 5"` com prazo de 1s
    # voltava normal, em 5s. Thread+fila em vez de `select` porque o Windows não faz select em pipe.
    fila: queue.Queue[str | None] = queue.Queue()

    def _ler() -> None:
        try:
            assert proc.stdout is not None
            for bruta in proc.stdout:
                fila.put(bruta)
        finally:
            fila.put(None)      # sentinela de fim; o `finally` garante que ela sempre vai

    leitor = threading.Thread(target=_ler, name="atualizar-log", daemon=True)
    leitor.start()

    pendentes: list[str] = []
    ultimo_flush = time.monotonic()
    while True:
        restante = limite - time.monotonic()
        if restante <= 0:
            proc.kill()
            raise subprocess.TimeoutExpired(args, timeout)
        try:
            # Teto de 1s por espera: mesmo em silêncio total, o laço acorda pra reavaliar o prazo e
            # pra despejar o que ficou pendente.
            bruta = fila.get(timeout=min(1.0, restante))
        except queue.Empty:
            bruta = ""
        if bruta is None:
            break
        if bruta:
            linha = _ANSI.sub("", bruta).rstrip()
            linhas.append(linha)
            if linha:
                pendentes.append(linha)
        agora = time.monotonic()
        # Agrupa por ~1s: uma escrita de arquivo por linha faria centenas de tmp+rename num npm ci.
        if pendentes and agora - ultimo_flush >= 1.0:
            _emitir("\n".join(pendentes))
            pendentes = []
            ultimo_flush = agora
    if pendentes:
        _emitir("\n".join(pendentes))
    try:
        # COM prazo: o EOF do pipe não garante que o processo já saiu (um neto que herdou o fd pode
        # tê-lo fechado antes). Sem teto aqui, um `waitpid` que não retorna prenderia a atualização
        # no último ponto sem rede de segurança.
        proc.wait(timeout=max(1.0, limite - time.monotonic()))
    except subprocess.TimeoutExpired:
        proc.kill()
        raise
    if proc.returncode != 0:
        _emitir(f"[saiu com {proc.returncode}]")
    # Mesma forma de retorno de antes: quem chama lê `.returncode`, `.stdout` e `.stderr`. O stderr
    # foi fundido no stdout (a tela precisa dos dois em ORDEM), então `stderr` vem vazio e o
    # `_cauda` cai no stdout — que é o comportamento que ele já tinha quando não havia stderr.
    return subprocess.CompletedProcess(args, proc.returncode, "\n".join(linhas), "")


def _git(*args: str, timeout: float = _TIMEOUT_PADRAO) -> subprocess.CompletedProcess:
    return _rodar(["git", "-C", str(REPO), *args], timeout=timeout)


# Códigos de cor do terminal. O `install.sh` colore a saída dele (verde pra ok, vermelho pra erro),
# e esse texto vai INTEIRO pra tela do app — onde vira lixo visível no meio da frase
# (`\x1b[31mX\x1b[0m tmux faltando`). Medido ao vivo em 25/08/2026, apertando o botão.
_ANSI = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")

# Como os instaladores dizem "terminei bem, MAS algo ficou pra trás". Marca em texto puro, sem
# tradução e sem cor, justamente pra sobreviver a idioma e a `_ANSI`.
_MARCA_AVISO = "##HANGAR-AVISO##"


def _cauda(p: subprocess.CompletedProcess, linhas: int = 12) -> str:
    """As últimas linhas do erro, sem cor de terminal — é o que a tela mostra."""
    txt = _ANSI.sub("", (p.stderr or p.stdout or "")).strip()
    return "\n".join(txt.splitlines()[-linhas:])


# O instalador marca cada pendência e cada parada com o motivo. A marca é irmã da de aviso: texto
# puro, sem cor, pra sobreviver a idioma e a `_ANSI`.
_MARCA_FALHA = "##HANGAR-FALHA##"


def _motivo_da_falha(p: subprocess.CompletedProcess) -> str:
    """As linhas marcadas do instalador; sem marca, a cauda de antes.

    A cauda eram as 12 últimas linhas, e no `install.ps1` essas são o portão do fim ("NAO
    terminou: tarefas agendadas, backend no ar") — o motivo de verdade, impresso no passo que
    falhou, ficava acima do corte, e o usuário via a lista sem saber o porquê.
    """
    txt = _ANSI.sub("", (p.stdout or "") + (p.stderr or ""))
    motivos = [linha.split(_MARCA_FALHA, 1)[1].strip()
               for linha in txt.splitlines() if _MARCA_FALHA in linha]
    return "\n".join(motivos) if motivos else _cauda(p)


# ─── Pré-voo ───────────────────────────────────────────────────────────────────────────────────

_LAUNCHD_LABEL = "br.dev.hangar.backend"   # o LaunchAgent do scripts/services-launchd.sh


def _launchd_alvo() -> str:
    return f"gui/{os.getuid()}/{_LAUNCHD_LABEL}"


def _topologia() -> str:
    """Como esta máquina roda o servidor: `systemd`, `launchd`, `windows` ou `manual`.

    O terceiro caso **não é erro** — há instalação rodando na mão, e tratá-la como defeito seria
    recusar atualizar uma máquina que funciona. `systemctl list-unit-files` é consultado pelo exit
    code, e não pelo stdout, para "o systemctl falhou" (bus fora, timeout) não virar "a unit não
    existe" — precedente do `deploy.sh`.
    """
    if _E_WINDOWS:
        return "windows"
    if sys.platform == "darwin":
        if _rodar(["launchctl", "print", _launchd_alvo()], timeout=15).returncode == 0:
            return "launchd"
        return "manual"
    if shutil.which("systemctl"):
        p = _rodar(["systemctl", "--user", "list-unit-files", "hangar-backend.service"], timeout=15)
        if p.returncode == 0:
            return "systemd"
    return "manual"


# Alvos cuja contagem já falhou neste processo: o `checar` roda a cada poll da tela.
_CONTAGEM_FALHOU: set[str] = set()


def _falta(*programas: str) -> list[str]:
    return [p for p in programas if not shutil.which(p)]


def checar() -> dict:
    """O estado da máquina, antes de tocar em qualquer coisa.

    Devolve tudo que decide se e como a atualização pode rodar. `pode` é a resposta curta; os
    outros campos dizem por quê, e é isso que a tela mostra quando a resposta é não.

    Distingue rastreado modificado de **não-rastreado** de propósito: arquivo solto (um `.bak`, uma
    pasta de trabalho) não atrapalha um fast-forward e não é motivo pra bloquear nada, enquanto
    arquivo rastreado modificado é trabalho de alguém.
    """
    faltando = _falta("git", "node", "npm", "uv")
    p = _git("status", "--porcelain=v1", "--branch", timeout=30)
    if p.returncode != 0:
        return {"pode": False, "erro": "nao_e_repo", "detalhe": _cauda(p), "faltando": faltando}

    linhas = p.stdout.splitlines()
    cabecalho = linhas[0] if linhas and linhas[0].startswith("## ") else ""
    arquivos = linhas[1:]
    # Porcelain v1: coluna 0-1 é o status; `??` é não-rastreado. O resto (M, A, D, R…) é rastreado.
    sujo_rastreado = [ln for ln in arquivos if not ln.startswith("??")]
    ahead = behind = 0
    if "..." in cabecalho and "[gone]" not in cabecalho:
        ahead = int(m.group(1)) if (m := re.search(r"ahead (\d+)", cabecalho)) else 0
        behind = int(m.group(1)) if (m := re.search(r"behind (\d+)", cabecalho)) else 0

    branch = _git("rev-parse", "--abbrev-ref", "HEAD", timeout=30).stdout.strip()
    destino = alvo()
    ahead_incerto = False
    if not _troca_de_branch({"branch": branch, "alvo": destino}):
        # Contra o `origin/<alvo>`, que é o que o reset alcança, e não contra o upstream: branch sem
        # upstream dava ahead 0, ficava sem resgate e perdia o commit local no `reset --hard`.
        r = _git("rev-list", "--left-right", "--count", f"origin/{destino}...HEAD", timeout=30)
        if r.returncode == 0 and len(contas := r.stdout.split()) == 2:
            behind, ahead = int(contas[0]), int(contas[1])
            _CONTAGEM_FALHOU.discard(destino)
        elif _git("remote", "get-url", "origin", timeout=30).returncode == 0:
            # Sem a contagem, o número do upstream pode ser o 0 que perde commit: o motor resgata.
            ahead_incerto = True
            if destino not in _CONTAGEM_FALHOU:
                _CONTAGEM_FALHOU.add(destino)
                _log.warning("nao consegui contar os commits contra origin/%s: %s", destino, _cauda(r, 4))
    return {
        "pode": not faltando,
        "faltando": faltando,
        "branch": branch,
        "alvo": destino,
        # A atualização alinha o checkout com `origin/<alvo>` — inclusive por `reset --hard`, quando
        # o fast-forward é recusado. Numa branch de trabalho isso ARRASTA a branch: medido em
        # 25/08/2026 na máquina do desktop, a `mobile-expo` local passou a apontar pra um commit da
        # main e o disco ficou misturado (arquivos da main em alguns caminhos, sobras da outra
        # branch noutros). Nada se perdeu — o `resguardar` fez o trabalho dele —, mas atualizar o
        # app não pode decidir sobre a branch de ninguém, então aqui ele recusa. Só escapam a
        # branch configurada e a que a própria atualização pôs no checkout.
        "branch_de_trabalho": branch not in (*_PRINCIPAIS, destino, _canal() or destino),
        "sujo": len(sujo_rastreado),
        "ahead": ahead,
        "ahead_incerto": ahead_incerto,
        "behind": behind,
        "divergiu": ahead > 0 and behind > 0,
        "topologia": _topologia(),
        "commit": _git("rev-parse", "HEAD", timeout=30).stdout.strip(),
    }


# ─── Resgate ───────────────────────────────────────────────────────────────────────────────────

class FalhaDeResgate(Exception):
    """Não deu para guardar o que estava no disco. A atualização para aqui, e o disco fica intacto."""


def resguardar(pre: dict) -> str | None:
    """Guarda o que existe hoje numa branch de resgate + stash. Devolve o nome, ou `None`.

    Só faz alguma coisa quando há o que perder: arquivo rastreado modificado, commit local (ou
    contagem de commits que falhou), ou uma branch de trabalho. Sem nada disso, devolve `None` e a atualização segue — criar branch de
    resgate a cada `git pull` de repo limpo só encheria o repo de refs mortas.

    **Confere que a ref existe antes de devolver.** É o que separa "automático" de "irreversível":
    quem chama só pode dar `reset --hard` depois desta função ter provado que há para onde voltar.
    """
    tem_o_que_perder = (pre.get("sujo") or pre.get("ahead") or pre.get("ahead_incerto")
                        or pre.get("branch_de_trabalho"))
    if not tem_o_que_perder:
        return None

    nome = f"resgate/{datetime.now().strftime('%Y-%m-%d-%H%M')}"
    p = _git("branch", "--force", nome, "HEAD", timeout=60)
    if p.returncode != 0:
        raise FalhaDeResgate(f"nao consegui criar a branch de resgate: {_cauda(p)}")

    if pre.get("sujo"):
        s = _git("stash", "push", "--include-untracked", "-m", f"hangar {nome}", timeout=120)
        if s.returncode != 0:
            raise FalhaDeResgate(f"nao consegui guardar as mudancas: {_cauda(s)}")

    # A prova. Sem ela o resgate é uma intenção, e o `reset --hard` que vem depois seria uma aposta.
    v = _git("rev-parse", "--verify", f"refs/heads/{nome}", timeout=30)
    if v.returncode != 0:
        raise FalhaDeResgate(f"a branch de resgate {nome} nao existe depois de criada")
    return nome


# ─── As etapas ─────────────────────────────────────────────────────────────────────────────────

def _avisar(texto: str) -> None:
    _escrever(avisos=list(estado().get("avisos") or []) + [texto])
    _log.warning(texto)


def _troca_de_branch(pre: dict) -> bool:
    atual, destino = pre.get("branch"), pre.get("alvo") or "main"
    # Sem branch configurada, a master segue recebendo a main como sempre recebeu.
    return bool(atual) and atual != destino and not (destino == "main" and atual in _PRINCIPAIS)


def _trocar_para(destino: str, ref: str) -> None:
    """Põe o checkout na branch do alvo, com as mesmas proteções do alinhamento.

    A ref local do alvo é a que o `reset --hard` de `_puxar` alcança: com commit que o origin não
    tem, ela ganha resgate conferido antes, como no `resguardar`. Branch local nova nasce em `ref`,
    e não no topo: senão o fast-forward até o commit escolhido não teria o que fazer.
    """
    local = f"refs/heads/{destino}"
    if _git("rev-parse", "--verify", "--quiet", local, timeout=30).returncode == 0:
        if _git("merge-base", "--is-ancestor", local, f"origin/{destino}", timeout=30).returncode != 0:
            nome = f"resgate/{datetime.now().strftime('%Y-%m-%d-%H%M')}-{destino}"
            b = _git("branch", "--force", nome, local, timeout=60)
            if b.returncode != 0 or _git("rev-parse", "--verify", f"refs/heads/{nome}",
                                         timeout=30).returncode != 0:
                raise FalhaDeResgate(f"nao consegui guardar a branch local {destino}: {_cauda(b)}")
            _avisar(f"a branch local {destino} tinha commits fora do origin: guardados em {nome}")
        args = ("checkout", destino)
    else:
        args = ("checkout", "-b", destino, ref)
    c = _git(*args, timeout=120)
    if c.returncode != 0:
        # Arquivo solto que colide com a outra branch recusa o checkout: guarda e tenta de novo.
        etiqueta = f"hangar antes de trocar para {destino}"
        s = _git("stash", "push", "--include-untracked", "-m", etiqueta, timeout=120)
        if s.returncode != 0:
            raise RuntimeError(f"nao consegui guardar o que estava no disco: {_cauda(s)}")
        _avisar(f"arquivos soltos guardados no stash '{etiqueta}'")
        c = _git(*args, timeout=120)
        if c.returncode != 0:
            raise RuntimeError(f"nao consegui trocar para a branch {destino}: {_cauda(c)}")
    if args[1] == "-b":
        u = _git("branch", "--set-upstream-to", f"origin/{destino}", timeout=30)
        if u.returncode != 0:
            _avisar(f"a branch {destino} ficou sem upstream: {_cauda(u, 4)}")
    _marcar_canal(destino)


def _git_quiet(*args: str) -> subprocess.CompletedProcess:
    """Leitura de git que não vai ao log da tela (um `git show` encheria o terminalzinho).

    Falha vira código 1: quem lê aqui cai no comportamento de antes, nunca derruba a atualização.
    """
    args = ["git", "-C", str(REPO), *args]
    try:
        return subprocess.run(args, capture_output=True, text=True, encoding="utf-8",
                              errors="replace", timeout=30,
                              env={**os.environ, "LC_ALL": "C", "LANGUAGE": "C"},
                              creationflags=_SEM_JANELA_WINDOWS)
    except (OSError, subprocess.TimeoutExpired) as e:
        _log.warning("git %s falhou: %s", args[3], e)
        return subprocess.CompletedProcess(args, 1, "", str(e))


_PROTOCOL_FILE = "backend/app/rust_server.py"
_PROTOCOL_RE = re.compile(r"^RUST_SERVER_PROTOCOL = (\d+)", re.M)


def _protocol_at(rev: str) -> int | None:
    p = _git_quiet("show", f"{rev}:{_PROTOCOL_FILE}")
    m = _PROTOCOL_RE.search(p.stdout) if p.returncode == 0 else None
    return int(m.group(1)) if m else None


def _binary(release: tuple | None) -> tuple[str, int | None] | None:
    """(commit, contrato) do binário deste sistema na release; `None` sem build ou sem commit legível.

    `platforms.<sistema>` diz o build de cada sistema, que pode ser de um commit anterior quando o
    dele falhou; sem a chave (manifesto antigo), o `commit` do topo vale para todos.
    """
    manifest = release[2] if release else None
    plat = rust_release.platform_key()
    if not isinstance(manifest, dict) or plat is None or not rust_release.has_build(manifest, plat):
        return None
    if "platforms" in manifest:
        platforms = manifest["platforms"]
        entry = platforms.get(plat) if isinstance(platforms, dict) else None
        if not isinstance(entry, dict):
            return None
        commit, protocol = entry.get("commit"), entry.get("protocol")
    else:
        commit, protocol = manifest.get("commit"), None
    # Só sha completo: nome de ref ou texto com `-` viraria outra coisa no `git show`.
    if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
        return None
    if not isinstance(protocol, int) or isinstance(protocol, bool):
        protocol = _protocol_at(commit)
    return commit, protocol


def published_api(release: tuple | None) -> int:
    """A API de cliente do servidor publicado para este sistema (`platforms.<sistema>.api`).

    Manifesto de antes do campo, ilegível ou sem build deste sistema vale 1: o app nunca vê erro aqui.
    """
    manifest = release[2] if release else None
    plat = rust_release.platform_key()
    platforms = manifest.get("platforms") if isinstance(manifest, dict) else None
    entry = platforms.get(plat) if isinstance(platforms, dict) and plat else None
    api = entry.get("api") if isinstance(entry, dict) else None
    return api if isinstance(api, int) and not isinstance(api, bool) and api >= 1 else 1


def _behind_top() -> bool:
    """HEAD estritamente atrás de `origin/<alvo>`: commit local adiante não conta."""
    topo = _git_quiet("rev-parse", f"origin/{alvo()}").stdout.strip()
    head = _git_quiet("rev-parse", "HEAD").stdout.strip()
    return (bool(topo) and bool(head) and head != topo
            and _git_quiet("merge-base", "--is-ancestor", head, topo).returncode == 0)


_SERVER_WORKFLOW = ".github/workflows/server.yml"


def _caminhos_do_build(rev: str) -> list[str] | None:
    """Os `paths` do push no workflow que publica o hangar-server: mudar um deles pede binário novo."""
    p = _git_quiet("show", f"{rev}:{_SERVER_WORKFLOW}")
    if p.returncode != 0:
        return None
    caminhos, dentro = [], False
    for linha in p.stdout.splitlines():
        texto = linha.strip()
        if texto == "paths:":
            dentro = True
        elif dentro and texto.startswith("- "):
            caminhos.append(texto[2:].strip().strip("\"'"))
        elif dentro and texto and not texto.startswith("#"):
            break
    return caminhos or None


def _ultimo_com_o_binario(publicado: str, topo: str) -> str:
    """O commit mais novo do primeiro pai com o conteúdo do build publicado.

    Merge pode trazer o build por outro pai; o histórico de mudanças não prova que a árvore difere.
    Sem conferir a lista ou a comparação, para no próprio commit do binário.
    """
    caminhos = _caminhos_do_build(topo)
    if caminhos is None:
        _log.warning("paths do %s ilegiveis em %s: o codigo para no commit do binario %s", _SERVER_WORKFLOW, topo, publicado[:8])
        return publicado
    p = _git_quiet("rev-list", "--first-parent", f"{publicado}..{topo}")
    if p.returncode != 0:
        _log.warning("git log dos commits depois do binario falhou (%s): %s", p.returncode, p.stderr.strip()[-300:])
        return publicado
    for candidato in p.stdout.split():
        diff = _git_quiet("diff", "--quiet", publicado, candidato, "--", *(f":(glob){c}" for c in caminhos))
        if diff.returncode == 0:
            return candidato
        if diff.returncode != 1:
            _log.warning("git diff do build falhou (%s): %s", diff.returncode, diff.stderr.strip()[-300:])
            return publicado
    return publicado


def pinned_target(destino: str) -> tuple[str | None, tuple | None]:
    """Até onde o checkout pode ir, e a release lida para escolher.

    O commit mais novo de `origin/<destino>` (primeiro pai) cujo contrato interno é o do binário
    publicado para este sistema: o topo sem binário próprio deixaria o Supervisor recusando o
    hangar-server antigo. Sem Rust ou sem release própria de uma branch fora da main, devolve o
    topo, como antes. `None` = não deu para conferir (rede, git, release da main sumida, sem build
    deste sistema); quem chama decide. Depois do `fetch`: lê `origin/<destino>` local.
    """
    topo = f"origin/{destino}"
    plat = rust_release.platform_key()
    if not config.settings.rust_server or plat is None:
        return topo, None
    # A release da branch para onde o checkout vai, não da que ele está agora; o ambiente vence, como no `fetch`.
    if url := os.environ.get("HANGAR_SERVER_RELEASE_URL"):
        url, tag = url.rstrip("/"), None
    else:
        tag = rust_release.tag_for(destino)
        url = f"{rust_release.RELEASES_URL}/{tag}"
    try:
        manifest = rust_release.read_manifest(url)
    except urllib.error.HTTPError as e:
        # A release da main sempre existe: 404 nela é o CI regravando o manifesto (o `--clobber` apaga e sobe).
        if e.code != 404 or tag == rust_release.MAIN_TAG:
            _log.warning("manifesto do hangar-server em %s: HTTP %s", url, e.code)
            return None, None
        # Branch sem release própria: o `fetch` cai na da main, cujo commit não é desta branch.
        return topo, None
    except Exception as e:                           # noqa: BLE001 — rede, JSON torto
        _log.warning("manifesto do hangar-server em %s ilegivel: %s", url, e)
        return None, None
    release = (url, tag, manifest)
    want = binary[1] if (binary := _binary(release)) else None
    if want is None:
        entry = manifest.get("platforms", {}).get(plat) if isinstance(manifest.get("platforms"), dict) else None
        _log.warning("binario %s sem contrato conferido (entrada %r, commit do topo %r)",
                     plat, entry, manifest.get("commit"))
        return None, release
    # Mesmo contrato não basta: o conteúdo que produziu o binário também precisa coincidir.
    publicado = binary[0]
    if publicado == _git_quiet("rev-parse", "--verify", "--quiet", topo).stdout.strip():
        return topo, release
    if _git_quiet("merge-base", "--is-ancestor", publicado, topo).returncode == 0:
        return _ultimo_com_o_binario(publicado, topo), release
    # Binário de commit fora desta branch: o último commit dela que fala o mesmo contrato.
    return _newest_with_protocol(topo, want), release


def _newest_with_protocol(topo: str, want: int) -> str | None:
    """O commit mais novo de `topo` (primeiro pai) com o contrato `want`; `None` sem achar."""
    if _protocol_at(topo) == want:
        return topo
    # Só os commits que mexeram no número: o pai de cada um é o último com o valor anterior.
    # `-m` e não `--diff-merges=first-parent`, que só existe do git 2.31 em diante.
    p = _git_quiet("log", "-m", "--first-parent", "--no-patch", "--format=%H",
                   "-G", "^RUST_SERVER_PROTOCOL = ", topo, "--", _PROTOCOL_FILE)
    if p.returncode != 0:
        _log.warning("git log do protocolo falhou (%s): %s", p.returncode, p.stderr.strip()[-300:])
        return None
    for sha in p.stdout.split():
        anterior = _git_quiet("rev-parse", "--verify", "--quiet", f"{sha}^1").stdout.strip()
        if anterior and _protocol_at(anterior) == want:
            return anterior
    _log.warning("nenhum commit de %s fala o protocolo %s", topo, want)
    return None


def _puxar(pre: dict) -> tuple | None:
    """`fetch` + fast-forward até `pinned_target`. Só reseta quando o ff é impossível — e o
    resgate já rodou. Devolve a release lida na escolha, para o download usar a mesma."""
    destino = pre.get("alvo") or "main"
    f = _git("fetch", "origin", timeout=300)
    if f.returncode != 0:
        raise RuntimeError(f"nao consegui buscar o codigo novo: {_cauda(f)}")

    # A `dist-latest` o CI move a cada push, e sem `--force` a copia local fica presa no commit
    # em que nasceu. Fetch separado e nao fatal: ela so da nome a versao na tela; falhar aqui
    # nao pode segurar o codigo novo, que ja chegou.
    t = _git("fetch", "origin", "--force", "refs/tags/dist-latest:refs/tags/dist-latest", timeout=60)
    if t.returncode != 0:
        _log.warning("tag dist-latest nao atualizada: %s", _cauda(t))

    ref, release = pinned_target(destino)
    if ref is None:
        # Sem o binário conferido, vale o contrato do código no ar, o do binário que já está aqui.
        atual = _protocol_at("HEAD")
        if atual is None:
            _avisar("não consegui conferir o binário do Rust publicado para este sistema; o código foi ao topo")
            ref = f"origin/{destino}"
        else:
            _avisar("não consegui conferir o binário do Rust publicado para este sistema; o código "
                    "não troca de contrato com ele até conferir")
            ref = _newest_with_protocol(f"origin/{destino}", atual)
            if ref is None:
                return release              # nenhum commit nele: fica onde está
    if _troca_de_branch(pre):
        _trocar_para(destino, ref)

    # Commit já à frente de `ref` fica: o ff-only não recua.
    m = _git("merge", "--ff-only", ref, timeout=120)
    if m.returncode == 0:
        return release

    # ff-only falhou: ou divergiu, ou a árvore tem algo no caminho. Antes do reset, guarda também o
    # que NÃO está rastreado — e este stash é diferente do que o `resguardar` já pode ter feito.
    # Medido: um arquivo solto (um `.env` extra, um script) cujo nome COLIDE com um arquivo que
    # chega no commit novo faz o ff-only ser recusado, e o `reset --hard` seguinte sobrescreve esse
    # arquivo calado, com rc=0. Como `resguardar` só entra em ação quando há mudança em arquivo
    # RASTREADO, esse caso passava direto pela rede de proteção — e "automático nunca é
    # irreversível" deixava de valer justamente para o arquivo que ninguém versionou.
    s = _git("stash", "push", "--include-untracked", "-m", "hangar antes do reset", timeout=120)
    if s.returncode != 0:
        # Não seguir para o reset. Um stash que falhou (index.lock preso, disco cheio, permissão)
        # com o reset acontecendo mesmo assim reproduz o defeito original — o arquivo é sobrescrito
        # calado —, agora escondido atrás da linha que parece ser a correção. Mesma regra do
        # `resguardar`: sem prova de que dá pra voltar, nada destrutivo acontece.
        raise RuntimeError(f"nao consegui guardar o que estava no disco: {_cauda(s)}")

    r = _git("reset", "--hard", ref, timeout=120)
    if r.returncode != 0:
        raise RuntimeError(f"nao consegui alinhar com o codigo novo: {_cauda(r)}")
    return release


def _electron_instalado() -> bool:
    """Instalação nova não traz mais o Electron, e o passo de remoção o tira do Windows."""
    return (REPO / "shell" / "node_modules" / "electron").is_dir()


def _shell_mudou(de: str, para: str) -> bool:
    """A janela nativa (Electron) roda o `main.cjs` que estava no disco quando abriu: o restart do
    backend não a alcança, e só fechar e abrir o app traz o shell novo. A tela precisa dizer isso."""
    if not de or not para or de == para or not _electron_instalado():
        return False
    p = _git("diff", "--name-only", f"{de}..{para}", "--", "shell/", timeout=30)
    if p.returncode != 0:
        # Na dúvida, avisa: um "feche e abra" a mais custa um clique; um a menos deixa o shell
        # velho no ar sem ninguém saber.
        _log.warning("nao consegui saber se shell/ mudou (%s..%s): %s", de[:8], para[:8], _cauda(p))
        return True
    return any(linha and not linha.endswith(".test.cjs") for linha in p.stdout.splitlines())


def _hash_arquivo(caminho: Path) -> str:
    import hashlib
    try:
        return hashlib.sha256(caminho.read_bytes()).hexdigest()
    except FileNotFoundError:
        return ""
    except OSError:
        # Sem hash o `npm ci` é pulado; isso tem que aparecer, senão "Atualizado" com front velho.
        _log.warning("nao consegui ler %s; o npm ci nao vai rodar nesta atualizacao", caminho, exc_info=True)
        return ""


def _preparar(topologia: str, *, dist: bool = True, release: tuple | None = None) -> None:
    """O que o `git pull` não traz e o backend novo precisa pra subir — SEM o instalador.

    O instalador inteiro (`install.ps1 -Update`, 8 etapas) rodava a cada atualização, e cada etapa
    era uma chance de falhar sem ninguém ter pedido nada dela. O que muda de verdade entre dois
    commits e não vem no `git pull` é curto: a tela compilada, as dependências do backend e, às
    vezes, as do front. Wrapper, statusline, tarefa agendada e afins só mudam quando um commit os
    muda — e esse commit declara o passo em `docs/atualizacoes/`, que roda o instalador cirúrgico.

    `uv sync` sempre: é idempotente e sai em segundos com o lock igual. Comparar o lock entre
    `de..para` seria o intervalo de commits, que mente em máquina reclonada ou resetada (regra
    medida em `atualizacoes.py`). O `npm ci` não é barato, então esse compara o hash do lock
    gravado no sidecar — do que JÁ RODOU aqui, não do intervalo.
    """
    uv = shutil.which("uv")
    if not uv:
        raise RuntimeError("uv nao encontrado: nao da pra sincronizar as dependencias do backend")
    p = _rodar([uv, "sync"], cwd=REPO / "backend", timeout=600)
    if p.returncode != 0:
        raise RuntimeError(f"as dependencias do backend nao sincronizaram: {_cauda(p, 6)}")
    lock = REPO / "package-lock.json"
    marca = _base() / "package-lock.sha"
    atual = _hash_arquivo(lock)
    try:
        aplicado = marca.read_text(encoding="utf-8").strip()
    except OSError:
        # Sem marca ainda: o `node_modules` que existe é o do instalador, feito com o lock de
        # então. Rodar `npm ci` aqui só por falta de registro custou 5 min na primeira
        # atualização da VM; a marca nasce do que já está no disco.
        aplicado = atual
    if atual and atual != aplicado and (REPO / "node_modules").is_dir():
        npm = shutil.which("npm")
        if not npm:
            raise RuntimeError("package-lock.json mudou e nao achei o npm pra instalar as dependencias")
        if topologia == "windows":
            _stop_windows_front()
        p = _rodar([npm, "ci", "--workspace=@hangar/core", "--workspace=frontend"], cwd=REPO, timeout=900)
        if p.returncode != 0:
            if topologia == "windows":
                # O `_reiniciar` não chega a rodar nesta falha; sem isto o front ficaria parado.
                _rodar(["powershell", "-NoProfile", "-Command",
                        "Start-ScheduledTask -TaskName hangar-frontend -ErrorAction SilentlyContinue"],
                       timeout=30)
            raise RuntimeError(f"as dependencias do front nao instalaram: {_cauda(p, 6)}")
    if atual:
        marca.parent.mkdir(parents=True, exist_ok=True)
        marca.write_text(atual, encoding="utf-8")
    # Depois do `npm ci`: fora da main a tela é compilada aqui, já com as dependências dela.
    if dist:
        # Tela e binário em paralelo: são dois downloads independentes. Sem o hangar-server o
        # Python atende sozinho: falha do binário é aviso, nunca etapa quebrada. A volta
        # (`dist=False`) não baixa: a release é a mais nova, não a do commit de antes. O binário
        # só é gravado com a tela no lugar: tela que falha leva à volta do checkout, e um
        # binário novo sob o Python velho recusaria o protocolo.
        baixados: list = []
        with ThreadPoolExecutor(2) as ex:
            tela = ex.submit(_atualizar_dist)
            binario = ex.submit(rust_release.fetch, release=release, hold=baixados)
        erros: list[Exception] = []

        def _resultado(f):
            try:
                return f.result()
            except Exception as e:
                erros.append(e)
                return None
        aviso = _resultado(tela)
        if aviso:
            _log.warning(aviso)
        # Os avisos de quem terminou entram antes de a falha do outro parar a etapa.
        novos = ([aviso] if aviso else []) + (_resultado(binario) or [])
        if novos:
            _escrever(avisos=list(estado().get("avisos") or []) + novos)
        if erros:
            raise erros[0]
        if baixados and (falhas := rust_release.install_held(baixados)):
            _escrever(avisos=list(estado().get("avisos") or []) + falhas)
        _renovar_chromium()


def _renovar_chromium(agora: float | None = None) -> None:
    """O chrome-headless-shell baixado não se atualiza como o Chrome da distro, e é ele que abre
    qualquer site para o agente: cada Atualizar confere a versão estável e baixa de novo se mudou.
    Roda em toda máquina onde o script já rodou (a marca existe), para que um download que falhou
    seja tentado de novo. O script nunca falha; código diferente de zero ou marca de falha só vira
    log, sem derrubar a atualização. A versão estável muda no máximo uma vez por dia: marca boa
    regravada (pelo script) há menos de 24 h não roda de novo; marca de falha tenta sempre."""
    marca = Path.home() / ".hangar" / "native" / "chromium-ok"
    if not sys.platform.startswith("linux") or not marca.is_file():
        return
    agora = time.time() if agora is None else agora
    if (agora - marca.stat().st_mtime < 24 * 3600
            and "falhou" not in marca.read_text(encoding="utf-8", errors="replace")):
        return
    p = _rodar([str(REPO / "scripts" / "install-chromium.sh")], cwd=REPO, timeout=900)
    if p.returncode != 0:
        _log.warning("install-chromium.sh saiu com %s: %s", p.returncode, _cauda(p, 4))
    elif "falhou" in marca.read_text(encoding="utf-8", errors="replace"):
        _log.warning("install-chromium.sh: %s", _cauda(p, 4))


def _nativo_instalado() -> bool:
    marker = Path.home() / ".hangar" / "native" / "release.json"
    if not marker.is_file():
        return False
    installed = json.loads(marker.read_text(encoding="utf-8"))
    if not isinstance(installed, dict):
        raise ValueError("marca do app nativo nao e um objeto")
    return not installed.get("sem_build")


def _instalar_nativo(alvo: str, porta: int) -> None:
    """Atualiza só o app já instalado e compatível com o servidor que ficou no ar."""
    if alvo != "main":
        return
    try:
        if not _nativo_instalado():
            return
        release = os.environ.get("HANGAR_NATIVE_UPDATE_URL") or f"{rust_release.RELEASES_URL}/native-latest"
        manifest = json.loads(rust_release._get(f"{release.rstrip('/')}/native-latest.json", 30))
        if not isinstance(manifest, dict):
            raise ValueError("manifesto do app nativo nao e um objeto")
        required = manifest.get("requires_api", 1)
        request = urllib.request.Request(f"http://127.0.0.1:{porta}/api/client-api",
                                        headers={"Authorization": f"Bearer {config.settings.auth_token}"})
        try:
            with urllib.request.urlopen(request, timeout=3) as response:
                contract = json.load(response)
            api = contract.get("api") if isinstance(contract, dict) else None
        except urllib.error.HTTPError as e:
            if e.code != 404:
                raise
            api = 1
        if type(required) is not int or required < 1 or type(api) is not int or api < 1:
            raise ValueError("contrato de API do app ou do servidor desconhecido")
    except (OSError, ValueError, http.client.HTTPException) as e:
        diag.registrar("atualizacao.nativo", "aviso", codigo="contrato_ilegivel", **diag.erro_campos(e))
        return
    if required > api:
        diag.registrar("atualizacao.nativo", codigo="api_insuficiente", requires_api=required, api=api)
        return

    command = (["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
                str(REPO / "scripts" / "install-native.ps1")] if _E_WINDOWS
               else [str(REPO / "scripts" / "install-native.sh")])
    try:
        result = _rodar(command, cwd=REPO, timeout=900)
    except (OSError, subprocess.SubprocessError) as e:
        result = subprocess.CompletedProcess(command, 1, "", str(e))
    if result.returncode != 0:
        _avisar(f"app nativo nao atualizou: {_motivo_da_falha(result)}")


def _stop_windows_front() -> None:
    """Derruba o `vite preview` da tarefa `hangar-frontend` antes do `npm ci`.

    O `npm ci` apaga o `node_modules` antes de reinstalar, e o Windows não deixa apagar binário
    nativo mapeado em processo vivo: com o front de pé ele morre em `EPERM ... unlink` e deixa o
    `node_modules` pela metade. Quem sobe a tarefa de novo é o `_reiniciar`.
    """
    helper = str(REPO / "scripts" / "windows-tasks.ps1").replace("'", "''")
    raiz = str(REPO).replace("'", "''")
    p = _rodar(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command",
                f". '{helper}'; Stop-HangarFrontend '{raiz}'"], timeout=60)
    if p.returncode != 0:
        # Segue: sem front de pé o `npm ci` passa, e com ele de pé a falha do `npm ci` diz o motivo.
        _log.warning("nao consegui parar o front antes do npm ci: %s", _cauda(p, 4))


def _reaplicar(topologia: str) -> None:
    """O instalador inteiro (`-Update`/`--update`). Não é mais o caminho do botão — fica pro
    hook post-merge e pra quem chama na mão; o botão só roda o que um passo declarado pedir."""
    if _E_WINDOWS:
        p = _rodar(["powershell", "-ExecutionPolicy", "Bypass", "-File",
                    str(REPO / "install.ps1"), "-Update"])
    else:
        p = _rodar(["bash", str(REPO / "install.sh"), "--update"])
    if p.returncode != 0:
        raise RuntimeError(f"a instalacao nao terminou: {_motivo_da_falha(p)}")
    # Avisos: o instalador pode terminar bem e mesmo assim ter deixado algo pra trás (o `npm ci` da
    # janela nativa é o caso). Sem isto a tela dizia "Atualizado" e pronto — o que ficou quebrado
    # só aparecia como uma linha no meio do log, que ainda por cima pode ter rolado pra fora.
    avisos = [linha.split(_MARCA_AVISO, 1)[1].strip()
              for linha in (p.stdout or "").splitlines() if _MARCA_AVISO in linha]
    if avisos:
        _escrever(avisos=avisos)
        _log.warning("a instalacao terminou com avisos: %s", "; ".join(avisos))


def _porta_do_front() -> int:
    """`CP_FRONT_PORT` do `backend/.env`; 0 quando não há tarefa do front."""
    try:
        for ln in (REPO / "backend" / ".env").read_text(encoding="utf-8").splitlines():
            if ln.startswith("CP_FRONT_PORT="):
                return int(ln.split("=", 1)[1].strip() or 0)
    except (OSError, ValueError):
        pass
    return 0


def _reiniciar(topologia: str, porta: int = 8765) -> None:
    if topologia == "systemd":
        # O backend por último: o reinício dele começa assim que pedido e, sem escopo próprio, leva
        # junto este processo e o `systemctl`, que ainda nem teria pedido o do frontend.
        unidades = ["hangar-backend.service"]
        if _rodar(["systemctl", "--user", "list-unit-files", "hangar-frontend.service"],
                  timeout=15).returncode == 0:
            unidades.insert(0, "hangar-frontend.service")
        p = _rodar(["systemctl", "--user", "restart", *unidades], timeout=120)
        if p.returncode != 0:
            raise RuntimeError(f"nao consegui reiniciar o servidor: {_cauda(p)}")
    elif topologia == "windows":
        # Quem sabe derrubar a cadeia de processos e subir a tarefa é o `windows-tasks.ps1` (o
        # mesmo que a vigia usa) — sem passar pelo instalador. Este processo é destacado do
        # backend, e o `Restart-HangarTask` não encerra a árvore: o atualizador sobrevive.
        helper = str(REPO / "scripts" / "windows-tasks.ps1").replace("'", "''")
        raiz = str(REPO).replace("'", "''")
        p = _rodar(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command",
                    f". '{helper}'; Restart-HangarTasks '{raiz}' {int(porta)} {_porta_do_front()}"],
                   timeout=180)
        if p.returncode != 0:
            raise RuntimeError(f"nao consegui reiniciar as tarefas: {_cauda(p, 6)}")
    elif topologia == "launchd":
        # O launchd mata o grupo de processos do backend; este nasceu com setsid e fica de pé.
        p = _rodar(["launchctl", "kickstart", "-k", _launchd_alvo()], timeout=120)
        if p.returncode != 0:
            raise RuntimeError(f"nao consegui reiniciar o servico: {_cauda(p)}")
    else:
        # Instalação na mão: não há serviço para reiniciar, e inventar um `kill` no processo de
        # alguém seria pior que não reiniciar. A tela avisa que falta reiniciar à mão.
        _escrever(reiniciar_manual=True)


def _subiu(porta: int, teto: float = _TIMEOUT_SUBIR) -> bool:
    """O backend voltou? Prova de vida depois do restart — sem ela, "sucesso" é só "exit 0"."""
    import urllib.error
    import urllib.request
    limite = time.monotonic() + teto
    while time.monotonic() < limite:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{porta}/", timeout=3) as r:
                if r.status < 500:
                    return True
        except (urllib.error.URLError, OSError):
            pass
        time.sleep(1.0)
    return False


# ─── O motor ───────────────────────────────────────────────────────────────────────────────────

def executar(porta: int = 8765) -> dict:
    """A atualização inteira. Devolve o estado final; nunca levanta.

    Ordem herdada do `deploy.sh`: tudo que pode falhar acontece **antes** do restart. Uma falha no
    meio deixa a máquina na versão anterior, inteira — que é o requisito duro deste botão.
    """
    global _SOU_O_MOTOR
    _SOU_O_MOTOR = True     # daqui pra frente este processo é o dono do estado e do log
    try:
        return _executar(porta)
    finally:
        # Solta a vez mesmo saindo por caminho de erro. O `finally` não cobre o processo ser MORTO
        # (é pra isso que o lock guarda o pid e o dono morto é recolhido), mas cobre todo o resto.
        _soltar_a_vez()


def _executar(porta: int) -> dict:
    # Zera o log ANTES do pré-voo, não depois. O `checar()` roda `git status`, `rev-parse` e o
    # `systemctl list-unit-files` — todos passam pelo `_log_do_estado`, que lê, mescla e regrava o
    # estado. Zerando depois, todo esse trabalho ia pro lixo e a etapa de checagem nunca aparecia
    # no terminalzinho, mesmo tendo sido registrada.
    # `waiting_for_server_build` só é zerado: cliente antigo ainda lê o campo e mostraria um `True` velho.
    _escrever(fase="rodando", ok=None, erro=None, resgate=None, voltou=None, no_ar=None,
              commit_de="", commit_para=None, pid=os.getpid(), backend_pid=_launching_backend(),
              reiniciar_manual=False, shell_mudou=False, waiting_for_server_build=False,
              request_id=None, log=[], avisos=[])
    pre = checar()
    de = pre.get("commit", "")
    _escrever(commit_de=de)

    # A recusa da branch vem ANTES de tudo, e principalmente antes do `resguardar`: aqui nada
    # aconteceu ainda, e a saída é uma frase, não um resgate pra alguém desfazer depois.
    destino = pre.get("alvo") or "main"
    if pre.get("branch_de_trabalho"):
        return _falhou(
            f"este checkout esta na branch '{pre.get('branch')}', e a atualizacao alinha o disco "
            f"com origin/{destino} — troque para a {destino} antes de atualizar", porta=porta)

    if not pre.get("pode"):
        falta = ", ".join(pre.get("faltando") or []) or pre.get("erro", "desconhecido")
        return _falhou(f"falta o que a atualizacao precisa: {falta}", porta=porta)

    # Branch configurada que o origin não tem: recusa aqui, antes do resgate, com o disco intacto.
    if destino != "main":
        r = _git("ls-remote", "--exit-code", "origin", f"refs/heads/{destino}", timeout=60)
        if r.returncode != 0:
            motivo = ("nao existe no origin" if r.returncode == 2
                      else f"nao pode ser consultada no origin ({_cauda(r, 4)})")
            return _falhou(f"a branch '{destino}' (CP_UPDATE_BRANCH) {motivo}; nada foi alterado",
                           porta=porta)

    # Passos, instaladores e hooks do git herdam: o binário do Rust é só desta rodada.
    os.environ[rust_release.UPDATE_OWNS_RUST] = "1"
    try:
        # A cópia que a volta devolve tem que ser a desta rodada: uma de outra rodada que não sai
        # trocaria um binário atual por um velho, então para antes de mexer em qualquer coisa.
        if falhas := rust_release.discard_previous():
            raise RuntimeError("; ".join(falhas))

        _etapa("resguardar")
        resgate = resguardar(pre)
        if resgate:
            _escrever(resgate=resgate)

        _etapa("codigo")
        release = _puxar(pre)
        para = _git("rev-parse", "HEAD", timeout=30).stdout.strip()
        _escrever(commit_para=para, shell_mudou=_shell_mudou(de, para))
        if (binary := _binary(release)) and binary[1] is not None:
            if binary[1] != _protocol_at("HEAD"):
                _avisar(f"o binário do Rust publicado para este sistema (commit {binary[0][:8]}) fala outro "
                        "contrato que este código; o Python atende sozinho até sair o binário novo")
            elif _behind_top():
                # Topo ainda sem binário para este sistema: a atualização vai até o último publicado e está
                # completa; quem usa não tem o que fazer com um build pendente no CI.
                diag.registrar("atualizacao.limitada_ao_binario", detalhe=para[:8])

        _etapa("passos")
        _aplicar_passos()

        _etapa("instalar")
        _preparar(pre["topologia"], release=release)

    except Exception as e:                           # noqa: BLE001 — ver abaixo: é deliberado
        # `Exception`, e não uma lista de tipos. Isto roda num processo DESTACADO cuja única forma
        # de falar com alguém é o arquivo de estado: uma exceção que escape daqui mata o processo
        # com traceback num stderr que ninguém lê, e deixa o estado preso em "rodando" — a tela
        # gira a barra pra sempre, e a pessoa não descobre nem que falhou nem por quê. Medido: a
        # lista de tipos que estava aqui não pegava `PassoFalhou`, e um passo com prova falha
        # produzia exatamente isso.
        msg = str(e)
        # Código novo no disco com binário e tela velhos: o próximo reinício, de qualquer origem,
        # subiria versões que não se entendem. No Windows o passo pode até já ter reiniciado.
        if de and _git("rev-parse", "HEAD", timeout=30).stdout.strip() != de:
            return _voltar(de, msg, pre["topologia"], porta, branch=pre.get("branch", ""))
        atual = _git("rev-parse", "--abbrev-ref", "HEAD", timeout=30).stdout.strip()
        if atual and pre.get("branch") and atual != pre.get("branch"):
            msg += f" (o checkout ficou na branch {atual})"
        return _falhou(msg, de=de, porta=porta)

    # O restart fica FORA do try acima, e a diferença não é cosmética: tudo lá em cima falha com o
    # servidor velho ainda no ar (`no_ar=True` é verdade). Aqui não — `systemctl restart` para o
    # processo antigo ANTES de subir o novo, então um erro neste ponto deixa a máquina sem serviço
    # nenhum, com código novo no disco. Esse caso tem que ir pro rollback, não pro "falhou e está
    # tudo como estava".
    pid_antes = _pid_do_servidor(pre["topologia"], porta)
    try:
        _etapa("reiniciar")
        _reiniciar(pre["topologia"], porta)
    except Exception as e:                           # noqa: BLE001 — mesmo motivo do de cima
        return _voltar(de, f"o servidor nao reiniciou: {e}", pre["topologia"], porta,
                       branch=pre.get("branch", ""))

    # Prova de vida só onde houve restart de verdade. Na instalação na mão o backend velho segue
    # respondendo — checar aqui devolveria um "subiu" que não prova nada.
    if not estado().get("reiniciar_manual"):
        if not _subiu(porta):
            return _voltar(de, "o servidor nao respondeu depois de reiniciar", pre["topologia"], porta,
                           branch=pre.get("branch", ""))
        # HTTP < 500 o processo VELHO também responde. O pid na porta é a prova de que trocou:
        # foi assim que o `-Update` do Windows chegou a dizer ok com a instância anterior no ar.
        pid_depois = _pid_do_servidor(pre["topologia"], porta)
        if pid_antes and pid_depois == pid_antes:
            return _voltar(de, f"o servidor nao reiniciou: o processo na porta e o mesmo de antes (pid {pid_antes})",
                           pre["topologia"], porta, branch=pre.get("branch", ""))

    _etapa("nativo")
    try:
        _instalar_nativo(destino, porta)
    except Exception as e:                           # noqa: BLE001 — o servidor já está atualizado
        _avisar(f"app nativo nao atualizou: {e or type(e).__name__}")
    _dist_velho_apagar()
    for falha in rust_release.discard_previous():
        _avisar(falha)
    _escrever(fase="pronto", ok=True, texto="Atualizado")
    return estado()


def _pid_do_servidor(topologia: str, porta: int) -> int | None:
    """Quem está servindo agora. `None` quando não dá pra saber — e aí a prova por pid não se
    aplica, sem virar falha."""
    try:
        if topologia == "systemd":
            p = _rodar(["systemctl", "--user", "show", "-p", "MainPID", "--value",
                        "hangar-backend.service"], timeout=15)
            return int(p.stdout.strip() or 0) or None
        if topologia == "launchd":
            # Pela porta não dá: no macOS o psutil.net_connections() do sistema exige root.
            m = re.search(r"^\s*pid = (\d+)", _rodar(["launchctl", "print", _launchd_alvo()],
                                                     timeout=15).stdout, re.M)
            return int(m.group(1)) if m else None
        if topologia == "windows":
            import psutil
            for c in psutil.net_connections(kind="tcp"):
                if c.status == psutil.CONN_LISTEN and c.laddr and c.laddr.port == porta:
                    return c.pid or None
    except Exception:                                # noqa: BLE001 — prova opcional, nunca derruba
        _log.warning("pid do servidor indisponivel: a prova por pid fica desligada nesta rodada", exc_info=True)
    return None


def _falhou(msg: str, de: str = "", no_ar: bool | None = None, porta: int = 8765) -> dict:
    """Falha sem reversão. `voltou=False` é literal: ninguém trouxe a máquina de volta.

    `no_ar=None` (o padrão) MEDE, não supõe. O padrão anterior era `True`, apoiado em "o caso comum
    é falhar antes do restart, e ali o servidor velho nunca saiu do ar" — e isso é falso no Windows:
    lá quem reinicia é o `install.ps1 -Update`, chamado na etapa *instalar*, que derruba a instância
    velha ANTES de subir a nova. Uma falha dele deixa a máquina sem servidor nenhum, e a tela
    afirmava "o que estava no ar continua no ar" — o oposto do estado real, mandando a pessoa
    embora de um servidor morto. Perguntar custa um GET local com teto curto.
    """
    _log.error("atualizacao falhou: %s", msg)
    if no_ar is None:
        no_ar = _subiu(porta, teto=5.0)
    _escrever(fase="pronto", ok=False, erro=msg, voltou=False, no_ar=no_ar)
    return estado()


def _voltar(commit: str, motivo: str, topologia: str, porta: int, branch: str = "") -> dict:
    """Rollback: volta ao commit que esta máquina tinha minutos atrás e sobe de novo.

    `branch` é a do checkout antes da atualização: se ela trocou de branch, a volta passa por lá
    antes do reset, senão a branch nova é que receberia o commit antigo.

    Este `reset --hard` é o único que não passa pelo `resguardar`, e pode: o alvo é um commit da
    própria máquina, de minutos atrás, e o pré-voo já garantiu que não havia trabalho solto no
    caminho (se houvesse, `resguardar` guardou). A alternativa a fazê-lo é deixar a pessoa sem
    servidor.
    """
    _escrever(fase="rodando", texto="Voltando para a versão anterior")
    if branch and branch != _git("rev-parse", "--abbrev-ref", "HEAD", timeout=30).stdout.strip():
        c = _git("checkout", "-f", branch, timeout=120)
        if c.returncode != 0:
            return _falhou(f"{motivo}; e nao consegui voltar para a branch {branch}: {_cauda(c)}",
                           no_ar=False)
        _marcar_canal(branch)
    r = _git("reset", "--hard", commit, timeout=120)
    if r.returncode != 0:
        # O reset falhou (commit inválido, disco cheio, `.git` travado). Seguir daqui reinstalaria
        # e reiniciaria em cima do código NÃO revertido — e o estado diria "voltei pra versão
        # anterior" com a máquina rodando exatamente o que quebrou. Para aqui e conta a verdade.
        return _falhou(f"{motivo}; e nao consegui voltar pra versao anterior: {_cauda(r)}",
                       no_ar=False)
    # Código antigo com o binário novo é contrato diferente: o Python atenderia sozinho. Antes do
    # resto da volta, que pode falhar e pular o que vier depois.
    if falhas := rust_release.restore_previous():
        _escrever(avisos=list(estado().get("avisos") or []) + falhas)
    try:
        _dist_velho_voltar()
        _preparar(topologia, dist=False)
        _reiniciar(topologia, porta)
    except (RuntimeError, subprocess.TimeoutExpired, OSError) as e:
        # MEDE (`no_ar=None`), não supõe. No systemd o restart que motivou o rollback já matou o
        # processo antigo; no Windows o `Restart-HangarTasks` pode recusar ANTES de parar qualquer
        # coisa (mutex ocupado) — medido na VM em 14/09/2026: `no_ar=False` cravado, com o backend
        # anterior respondendo na porta.
        return _falhou(f"{motivo}; e a volta para a versao anterior tambem falhou: {e}",
                       no_ar=None, porta=porta)
    # Dois campos, não um. `voltou` diz que o código anterior está de volta no disco (aconteceu
    # aqui em cima, incondicionalmente); `no_ar` diz se o servidor respondeu depois disso. Com um
    # campo só, o caso "reverti e mesmo assim não subiu" era indistinguível de "não revertei", e a
    # tela escolhia entre duas frases das quais nenhuma era verdade.
    _escrever(fase="pronto", ok=False, erro=motivo, voltou=True, no_ar=_subiu(porta))
    return estado()


def _aplicar_passos() -> None:
    """Os passos que a versão exige.

    Import direto, sem `try/except ImportError`: o módulo existe no mesmo commit que este, e o
    guard só serviria para transformar um defeito de importação em `atualizacoes.py` numa etapa que
    passa em branco — a atualização terminaria `ok=True` com os passos pendentes intactos, dizendo
    ter feito o que não fez.
    """
    from app import atualizacoes
    ruins = atualizacoes.invalidos()
    if ruins:
        # Vai pro ESTADO, não só pro log: o log deste processo vai pro /dev/null, e um passo
        # descartado por erro de digitação não fica pendente — some de vez, em toda máquina.
        _escrever(passos_invalidos=ruins)
        _log.warning("passos ignorados por estarem malformados: %s", ", ".join(ruins))
    atualizacoes.aplicar_pendentes()


# ─── Lançar destacado ──────────────────────────────────────────────────────────────────────────

def _vivo(pid) -> bool:
    # `procinfo` e a unica camada que sabe em que sistema estamos: no Windows perguntar com
    # `os.kill(pid, 0)` MATA o processo (ver procinfo.pid_vivo) — e aqui o processo e a propria
    # atualizacao em curso.
    if not isinstance(pid, int) or pid <= 0:
        return False
    return procinfo.pid_vivo(pid)


def _tomar_a_vez() -> bool:
    """Só um lançamento por vez. `True` = a vez é sua.

    Ler o estado e depois escrever era check-then-act, com janela real entre os dois: dois toques no
    botão, um retry de rede ou duas abas lançavam DOIS processos, que rodariam `fetch`/`merge`/
    `reset --hard` no mesmo repo ao mesmo tempo e disputariam a escrita do mesmo `estado.json`
    (`_escrever` é ler-mesclar-gravar, sem lock). Aqui quem decide é o `O_CREAT|O_EXCL`, que é
    atômico no sistema de arquivos: quem criar o arquivo ganha, e não há janela nenhuma.

    O lock guarda o pid, então um processo morto (máquina desligada no meio) não trava a máquina
    para sempre — o dono some, o lock é recolhido e o próximo lançamento passa.
    """
    trava = _base() / "rodando.lock"
    trava.parent.mkdir(parents=True, exist_ok=True)
    for _ in range(2):                                # 2ª volta só quando limpamos um lock morto
        try:
            fd = os.open(trava, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError:
            try:
                dono = int(trava.read_text(encoding="utf-8").strip() or 0)
            except (OSError, ValueError):
                dono = 0
            if _vivo(dono):
                return False
            # Dono morto: recolhe e tenta de novo. `missing_ok` porque outro processo pode ter
            # chegado à mesma conclusão primeiro — aí ele leva a vez no `O_EXCL` da volta seguinte.
            trava.unlink(missing_ok=True)
            continue
        else:
            with os.fdopen(fd, "w") as f:
                f.write(str(os.getpid()))
            return True
    return False


def _soltar_a_vez() -> None:
    (_base() / "rodando.lock").unlink(missing_ok=True)


def _kw_destacado() -> dict:
    """Como o motor nasce fora do grupo de processos do backend, por sistema."""
    if _E_WINDOWS:
        # Grupo próprio + sem janela. O DETACHED_PROCESS que havia aqui era o que ABRIA a janela:
        # sem console nenhum, o interpretador de base que o trampolim do venv roda alocava um.
        return {"creationflags": _FLAGS_MOTOR_WINDOWS}
    return {"start_new_session": True}   # setsid: sai do grupo de processos do backend


def iniciar(porta: int = 8765, *, expected_branch: str | None = None) -> dict:
    """Lança a atualização fora deste processo e devolve na hora.

    Fora do processo porque ela reinicia o backend — dentro, ela se mataria no meio. `setsid` no
    POSIX e `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` no Windows, sem herdar os pipes daqui:
    o filho não pode morrer junto com o pai nem escrever no log do serviço que vai reiniciar.
    """
    if not _tomar_a_vez():
        return {"ok": False, "erro": "ja_rodando"}
    # O automático pode ter aprovado a main antes de o dono trocar de canal.
    if expected_branch is not None and alvo() != expected_branch:
        _soltar_a_vez()
        return {"ok": False, "erro": "canal_mudou"}

    # `setsid` NÃO basta, e por pouco: ele tira o filho da sessão/grupo de processos, não do
    # **cgroup**. Como este `Popen` acontece dentro do worker do FastAPI, o processo da atualização
    # nasce no cgroup de `hangar-backend.service` — e a unit não declara `KillMode`, então o padrão
    # é `control-group`. Consequência: o `systemctl --user restart` que a própria atualização dispara
    # mata o cgroup inteiro e leva junto quem estava dando o comando, ANTES de ele escrever
    # `fase=pronto`. O estado ficaria travado em "rodando" para sempre e a barra giraria sem fim —
    # exatamente a falha que este módulo existe pra não ter. O repo já resolveu isso uma vez, pelo
    # mesmo motivo: `tmux._scope_prefix()`, que envolve o servidor tmux num escopo transiente.
    args = tmux._scope_prefix() + [sys.executable, "-m", "app.atualizar", str(porta)]
    extra = _kw_destacado()

    # O estado inicial é escrito ANTES do Popen, e isso é o que impede a corrida por construção:
    # depois do lançamento, o motor já pode estar gravando `_etapa("resguardar")`, e um `_escrever`
    # do backend chegando em seguida reverteria `passo`/`texto` pra "Começando" — a mesma classe de
    # disputa que prendeu a barra na etapa 4, só que estreita e autocorrigível. O `pid` sai daqui de
    # propósito: quem o registra é o próprio motor (`_executar`), com o pid dele; o do filho, que é
    # o que importa pra saber se a atualização morreu, mora no lock.
    # `pid=None`: `_escrever` mescla, e o pid da atualização ANTERIOR (morto) faria o
    # `estado_para_tela` declarar esta recém-lançada como abandonada no primeiro poll.
    _escrever(fase="rodando", ok=None, erro=None, pid=None,
              passo=0, total=len(ETAPAS), texto="Começando")
    proc = subprocess.Popen(
        args, cwd=str(REPO / "backend"),
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        env={**os.environ, _BACKEND_PID_ENV: str(os.getpid())},
        **extra,
    )
    # O lock nasceu com o pid do BACKEND, que é quem tinha de vencer a corrida do `O_EXCL`. Agora
    # passa a apontar pro filho: é ele que fica vivo enquanto a atualização roda, e o backend (que
    # não morre) faria `_vivo` responder sim para sempre, travando a máquina no primeiro uso.
    # tmp+rename, e não `write_text`: este último trunca e depois escreve, então há um instante em
    # que o lock tem ZERO bytes no disco. Uma segunda chamada caindo bem aí leria "", concluiria
    # "dono morto" (pid 0 nunca está vivo), recolheria o lock e lançaria uma segunda atualização —
    # justamente o que o `O_EXCL` existe pra impedir. Janela de microssegundos, mas é a mesma classe
    # de defeito que o `_escrever` daqui já evita com `atomico.substituir`.
    try:
        trava = _base() / "rodando.lock"
        tmp = trava.with_suffix(f".{os.getpid()}.tmp")
        tmp.write_text(str(proc.pid), encoding="utf-8")
        atomico.substituir(tmp, trava)
    except OSError as e:
        # Falhando aqui, o lock fica com o pid do BACKEND — um processo que não morre. Deixá-lo
        # assim (o `pass` que estava aqui) travaria a máquina PARA SEMPRE: todo lançamento futuro
        # veria um dono vivo e recusaria, e só apagando o arquivo na mão pra destravar. Entre um
        # lock eterno e nenhum lock, nenhum é melhor — a exclusão já cumpriu o papel dela neste
        # lançamento, e o próximo depende de alguém apertar o botão de novo. O aviso vai pro log do
        # BACKEND, que é lido, e não pro do filho, que vai pro /dev/null.
        _log.warning("nao consegui marcar o dono do lock (%s); soltando pra nao travar a maquina", e)
        _soltar_a_vez()
    return {"ok": True, "pid": proc.pid}


def reiniciar_agora(porta: int = 8765) -> dict:
    """Reinicia o servidor SEM atualizar nada. Devolve na hora; o restart roda destacado.

    Existe pro caso em que o disco já está à frente do processo — um `git pull` feito à mão, ou uma
    atualização que terminou sem reiniciar. Até aqui a tela só sabia dizer "entra no próximo
    reinício" e a pessoa tinha de descobrir sozinha qual era o comando.

    Destacado pelo mesmo motivo do `iniciar` (e com o mesmo escopo transiente): o
    `systemctl --user restart` mata o cgroup da unit, e quem deu o comando de dentro dele morre
    antes de o comando terminar.

    `systemd`, `launchd` e `windows` (tarefa agendada, pelo mesmo `Restart-HangarTasks` da atualização). Na
    instalação `manual` não há serviço, e inventar um `kill` no processo de alguém seria pior que
    recusar.
    """
    topologia = _topologia()
    if topologia == "manual":
        return {"ok": False, "erro": "topologia", "topologia": topologia}
    # A mesma vez da atualização: os dois escrevem no mesmo estado.json, e um reinício por cima
    # de uma atualização em curso sobrescrevia pid/etapa dela — e, morrendo, soltava a trava dela.
    if not _tomar_a_vez():
        return {"ok": False, "erro": "ja_rodando"}
    # Limpa a falha e os avisos do reinício ANTERIOR: sem isso a tela mostraria pra sempre o erro
    # de uma tentativa que já passou, inclusive depois de um reinício que deu certo.
    request_id = uuid.uuid4().hex
    _escrever(fase="rodando", ok=None, erro=None, pid=None, etapa=None,
              passo=0, total=len(ETAPAS_REINICIO), texto="Começando o reinício",
              commit_de="", commit_para=None, backend_pid=None,
              reinicio_erro=None, avisos=[], waiting_for_server_build=False, request_id=request_id)
    diag.registrar("service.restart.requested", codigo=topologia)
    try:
        folder = log_paths.base() / "privado"
        folder.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd = os.open(folder / "atualizacao-launch.log", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        with os.fdopen(fd, "a", encoding="utf-8", errors="replace") as launch_log:
            proc = subprocess.Popen(
                tmux._scope_prefix() + [sys.executable, "-m", "app.atualizar", "--reiniciar", str(porta)],
                cwd=str(REPO / "backend"),
                stdin=subprocess.DEVNULL, stdout=launch_log, stderr=launch_log,
                env={**os.environ, _RESTART_REQUEST_ENV: request_id},
                **_kw_destacado(),
            )
    except OSError as exc:
        _escrever(fase="pronto", ok=False, erro="não consegui lançar o processo do reinício")
        diag.registrar("service.restart.launch_failed", "erro", **diag.erro_campos(exc))
        _soltar_a_vez()
        raise
    try:
        trava = _base() / "rodando.lock"
        tmp = trava.with_suffix(f".{os.getpid()}.tmp")
        tmp.write_text(str(proc.pid), encoding="utf-8")
        atomico.substituir(tmp, trava)
    except OSError as e:
        _log.warning("nao consegui marcar o dono do lock (%s); soltando pra nao travar a maquina", e)
        _soltar_a_vez()
    return {"ok": True, "pid": proc.pid, "request_id": request_id}


_DIST_URL = "https://github.com/jeffer1312/hangar/releases/download/dist-latest"
_DIST_COMMITS_URL = "https://github.com/jeffer1312/hangar/releases/download/dist-commits"
# De onde a tela compila: o `build` do front roda `i18n:compile` com `../project.inlang`, que lê
# `messages/`; o lock e o package.json da raiz decidem as dependências com que ela compila.
_FONTES_DA_TELA = ("frontend", "packages", "messages", "project.inlang",
                   "package.json", "package-lock.json")


def _chave_da_tela(rev: str) -> str | None:
    """sha256 das árvores das fontes da tela em `rev`: a mesma conta do `publica-dist` no ci.yml."""
    p = _git_quiet("rev-parse", *(f"{rev}:{d}" for d in _FONTES_DA_TELA))
    return hashlib.sha256(p.stdout.encode()).hexdigest() if p.returncode == 0 else None


def _trocar_pelo_tar(fileobj, modo: str) -> str | None:
    """Extrai o tar numa pasta ao lado e só então troca o `dist`. Devolve o problema, se houver."""
    import tarfile
    import tempfile
    tmp = Path(tempfile.mkdtemp(prefix=".dist-baixado.", dir=REPO / "frontend"))
    try:
        with tarfile.open(fileobj=fileobj, mode=modo) as tar:
            tar.extractall(tmp, filter="data")
        if not (tmp / "index.html").is_file():
            return "o dist do CI veio incompleto"
        # O dist velho só sai DEPOIS de o novo estar no lugar: um rename que falha no meio não
        # pode deixar a máquina sem tela nenhuma. E fica guardado até o fim: o rollback devolve
        # a tela do commit anterior junto com o código dele.
        dist = REPO / "frontend" / "dist"
        velho = _DIST_VELHO()
        shutil.rmtree(velho, ignore_errors=True)
        if dist.exists():
            dist.rename(velho)
        tmp.rename(dist)
        return None
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def _baixar_tela_da_chave(chave: str) -> tuple[str, str] | None:
    """Troca o `dist` pelo `dist-commits/frontend-dist-<chave>.tar.gz` conferido.

    Devolve `(codigo, detalhe)` quando não trocou; `sem_chave` é o 404 (chave nunca publicada).
    """
    import http.client
    import io
    import tarfile
    import urllib.request
    import zlib
    url = f"{_DIST_COMMITS_URL}/frontend-dist-{chave}.tar.gz"
    try:
        with urllib.request.urlopen(f"{url}.sha256", timeout=15) as r:
            esperado = r.read().decode("ascii", "replace").split()[:1]
        with urllib.request.urlopen(url, timeout=180) as r:
            corpo = r.read()
        if [hashlib.sha256(corpo).hexdigest()] != esperado:
            return "sha256_divergente", ""
        problema = _trocar_pelo_tar(io.BytesIO(corpo), "r:gz")
        return ("tar_incompleto", problema) if problema else None
    except urllib.error.HTTPError as e:
        return ("sem_chave" if e.code == 404 else "download_falhou"), str(e)
    # Conexão que cai no meio do corpo (`IncompleteRead`) e gzip truncado não são `OSError`.
    except (OSError, tarfile.TarError, http.client.HTTPException, EOFError, zlib.error) as e:
        return "download_falhou", repr(e)


def _DIST_VELHO() -> Path:
    return REPO / "frontend" / ".dist-velho"


def _dist_velho_apagar() -> None:
    shutil.rmtree(_DIST_VELHO(), ignore_errors=True)


def _dist_velho_voltar() -> None:
    """Rollback da tela: o dist anterior volta ao lugar, se ainda existe."""
    velho, dist = _DIST_VELHO(), REPO / "frontend" / "dist"
    if not velho.is_dir():
        return
    shutil.rmtree(dist, ignore_errors=True)
    velho.rename(dist)


def _voltar_tela() -> str:
    """Devolve a tela guardada ao `dist`. Vazio quando voltou (ou não havia o que voltar)."""
    try:
        _dist_velho_voltar()
        return ""
    except OSError as e:
        return str(e)


def _atualizar_dist() -> str | None:
    """Troca o `frontend/dist` pelo build que o CI publicou. Devolve o aviso quando não deu.

    Espelha o `baixar_dist` do `install.sh`: um reinício sem isto sobe o backend novo servindo a
    tela velha — repo == processo, a tela diz "em dia", e o navegador continua no bundle de antes.
    As mesmas regras de lá: commit diferente do CI não cancela, só avisa. Árvore com edição em
    `frontend`/`packages` é quem está desenvolvendo e quer o SEU código na tela: aí compila local,
    como o `install.sh` interativo — recusar deixava a máquina de desenvolvimento sem jeito de
    atualizar a tela pelo app, porque a árvore dela está sempre suja. Fora da main também compila:
    o CI só publica o dist da main. Com o checkout atrás do topo (a atualização parou antes dele
    por falta do binário do Rust) o `dist-latest` é o do topo: a tela vem da `dist-commits` pela
    chave das fontes do commit alcançado, e compila aqui quando essa chave não foi publicada.
    """
    import tarfile
    import urllib.request
    p = _git("status", "--porcelain", "--", *_FONTES_DA_TELA, timeout=30)
    fora_da_main = _git("rev-parse", "--abbrev-ref", "HEAD", timeout=30).stdout.strip() not in _PRINCIPAIS
    compilar = p.returncode != 0 or bool(p.stdout.strip()) or fora_da_main
    if not compilar and _behind_top():
        chave = _chave_da_tela("HEAD")
        falha = _baixar_tela_da_chave(chave) if chave else ("chave_ilegivel", "")
        if falha is None:
            return None
        codigo, detalhe = falha
        # Só o diário: artefato do CI que falta ou falha não é aviso para quem atualiza.
        if codigo == "sem_chave":
            _log.info("tela da chave %s não publicada; compilando aqui", chave)
        else:
            _log.warning("tela da chave %s não veio do CI (%s: %s); compilando aqui", chave, codigo, detalhe)
            diag.registrar("atualizacao.tela_da_chave", "aviso", codigo=codigo, detalhe=detalhe)
        compilar = True
    if compilar:
        npm = shutil.which("npm")
        if not npm:
            return "tela não atualizada: ela precisa ser compilada aqui e não achei o npm"
        # Cópia, e não rename: o build escreve no `dist`, e o servidor no ar segue servindo dele.
        # Sem a cópia o rollback voltaria o código anterior servindo a tela nova.
        dist, velho = REPO / "frontend" / "dist", _DIST_VELHO()
        try:
            shutil.rmtree(velho, ignore_errors=True)
            if dist.is_dir():
                shutil.copytree(dist, velho)
        except OSError as e:
            # Cópia pela metade no `.dist-velho` viraria a tela do rollback.
            shutil.rmtree(velho, ignore_errors=True)
            return f"tela não atualizada: não consegui guardar a tela anterior ({e})"
        try:
            b = _rodar([npm, "--prefix", "frontend", "run", "build"], cwd=REPO, timeout=600)
        except Exception:
            if erro := _voltar_tela():
                _log.warning("a tela anterior nao voltou depois do build interrompido: %s", erro)
            raise
        if b.returncode != 0 or not (dist / "index.html").is_file():
            erro = _voltar_tela()
            return (f"tela não atualizada: o build local do frontend falhou: {_cauda(b, 4)}"
                    + (f"; e a tela anterior não voltou: {erro}" if erro else ""))
        return None
    sha_local = _git("rev-parse", "HEAD", timeout=30).stdout.strip()
    try:
        with urllib.request.urlopen(f"{_DIST_URL}/frontend-dist.sha", timeout=15) as r:
            sha_remoto = r.read().decode("ascii", "replace").strip()
        with urllib.request.urlopen(f"{_DIST_URL}/frontend-dist.tar.gz", timeout=180) as r:
            if problema := _trocar_pelo_tar(r, "r|gz"):
                return f"tela não atualizada: {problema}"
    except (OSError, tarfile.TarError) as e:
        return f"tela não atualizada: não consegui baixar o dist do CI ({e})"
    if sha_remoto != sha_local:
        return f"a tela é do commit {sha_remoto[:8]}; este checkout está em {sha_local[:8]}"
    return None


def executar_reinicio(porta: int = 8765) -> None:
    """O reinício em si, já dentro do processo destacado.

    O estado leva o desfecho à tela mesmo depois de o serviço reiniciar. A saída do lançamento
    fica no log privado; uma exceção do motor também precisa concluir o pedido no estado.
    """
    global _SOU_O_MOTOR
    _SOU_O_MOTOR = True
    # Mesma tela de progresso da atualização (etapa, barra, log): o botão dizia só "Reiniciando…"
    # e a pessoa não via nem o dist descendo nem o servidor caindo.
    _escrever(fase="rodando", ok=None, erro=None, voltou=None, no_ar=None, resgate=None,
              pid=os.getpid(), reinicio_erro=None, avisos=[], log=[],
              commit_de="", commit_para=None, reiniciar_manual=False, shell_mudou=False,
              waiting_for_server_build=False, request_id=os.environ.get(_RESTART_REQUEST_ENV))
    try:
        _etapa("tela", lista=ETAPAS_REINICIO)
        aviso = _atualizar_dist()
        _escrever(avisos=[aviso] if aviso else [])
        _etapa("reiniciar", lista=ETAPAS_REINICIO)
        topologia = _topologia()
        pid_antes = _pid_do_servidor(topologia, porta)
        _reiniciar(topologia, porta)
        # `ok=True` só com o servidor respondendo: a tela recarrega ao ver `ok`, e sem a prova
        # recarregava antes do novo subir.
        if not _subiu(porta):
            raise RuntimeError("o servidor nao respondeu depois de reiniciar")
        if pid_antes and _pid_do_servidor(topologia, porta) == pid_antes:
            raise RuntimeError(f"o servidor nao reiniciou: o processo na porta e o mesmo de antes (pid {pid_antes})")
        _dist_velho_apagar()
        _escrever(fase="pronto", ok=True, texto="Reiniciado")
    except Exception as e:                           # noqa: BLE001 — ver docstring
        erro = f"{type(e).__name__}: {e}"
        _escrever(fase="pronto", ok=False, erro=erro, reinicio_erro=erro, voltou=False)
    finally:
        _soltar_a_vez()


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    from app import diag_logging
    diag_logging.instalar("atualizacao.log")
    if len(sys.argv) > 1 and sys.argv[1] == "--reiniciar":
        executar_reinicio(int(sys.argv[2]) if len(sys.argv) > 2 else 8765)
    else:
        executar(int(sys.argv[1]) if len(sys.argv) > 1 else 8765)
