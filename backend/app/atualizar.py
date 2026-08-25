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

import json
import logging
import os
import re
import shutil
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

from app import atomico, tmux

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
_FLAGS_DESTACADO_WINDOWS = 0x00000200 | 0x00000008   # CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS

# Etapas, na ordem em que acontecem. O rótulo é o que a tela mostra — em português, dizendo o que
# está acontecendo com a máquina, não o comando que roda.
ETAPAS = (
    ("resguardar", "Guardando o que estava aqui"),
    ("codigo",     "Puxando o código novo"),
    ("passos",     "Aplicando o que a versão pede"),
    ("instalar",   "Instalando dependências"),
    ("reiniciar",  "Reiniciando o servidor"),
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


# ─── Estado ────────────────────────────────────────────────────────────────────────────────────

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
    tmp.write_text(json.dumps(atual, ensure_ascii=False), encoding="utf-8")
    atomico.substituir(tmp, alvo)


def _etapa(chave: str, **extra) -> None:
    """Marca a etapa atual. `passo`/`total` alimentam a barra da tela."""
    idx = next((i for i, (k, _) in enumerate(ETAPAS) if k == chave), 0)
    _escrever(fase="rodando", etapa=chave, passo=idx + 1, total=len(ETAPAS),
              texto=ETAPAS[idx][1], **extra)


# ─── Rodar comando ─────────────────────────────────────────────────────────────────────────────

def _rodar(args: list[str], cwd: Path | None = None,
           timeout: float = _TIMEOUT_PADRAO) -> subprocess.CompletedProcess:
    """Um comando, com a saída capturada em texto.

    `errors="replace"` porque no Windows uma falha de decode estrita morre numa THREAD leitora:
    `run()` não levanta nada e `stdout` volta **None**, e o estouro aparece longe da causa (medido
    22/08/2026, registrado no CLAUDE.md). `LC_ALL=C` no git pela mesma razão do `git_ops._run`: a
    saída aqui é lida por código, e mensagem traduzida quebraria a leitura calada.
    """
    return subprocess.run(
        args, cwd=str(cwd or REPO), capture_output=True, text=True,
        encoding="utf-8", errors="replace", timeout=timeout,
        env={**os.environ, "LC_ALL": "C", "LANGUAGE": "C"},
    )


def _git(*args: str, timeout: float = _TIMEOUT_PADRAO) -> subprocess.CompletedProcess:
    return _rodar(["git", "-C", str(REPO), *args], timeout=timeout)


# Códigos de cor do terminal. O `install.sh` colore a saída dele (verde pra ok, vermelho pra erro),
# e esse texto vai INTEIRO pra tela do app — onde vira lixo visível no meio da frase
# (`\x1b[31mX\x1b[0m tmux faltando`). Medido ao vivo em 25/08/2026, apertando o botão.
_ANSI = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")


def _cauda(p: subprocess.CompletedProcess, linhas: int = 12) -> str:
    """As últimas linhas do erro, sem cor de terminal — é o que a tela mostra."""
    txt = _ANSI.sub("", (p.stderr or p.stdout or "")).strip()
    return "\n".join(txt.splitlines()[-linhas:])


# ─── Pré-voo ───────────────────────────────────────────────────────────────────────────────────

def _topologia() -> str:
    """Como esta máquina roda o servidor: `systemd`, `windows` ou `manual`.

    O terceiro caso **não é erro** — há instalação rodando na mão, e tratá-la como defeito seria
    recusar atualizar uma máquina que funciona. `systemctl list-unit-files` é consultado pelo exit
    code, e não pelo stdout, para "o systemctl falhou" (bus fora, timeout) não virar "a unit não
    existe" — precedente do `deploy.sh`.
    """
    if _E_WINDOWS:
        return "windows"
    if shutil.which("systemctl"):
        p = _rodar(["systemctl", "--user", "list-unit-files", "hangar-backend.service"], timeout=15)
        if p.returncode == 0:
            return "systemd"
    return "manual"


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
    return {
        "pode": not faltando,
        "faltando": faltando,
        "branch": branch,
        "sujo": len(sujo_rastreado),
        "ahead": ahead,
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

    Só faz alguma coisa quando há o que perder: arquivo rastreado modificado, commit local, ou uma
    branch que não é a main. Sem nada disso, devolve `None` e a atualização segue — criar branch de
    resgate a cada `git pull` de repo limpo só encheria o repo de refs mortas.

    **Confere que a ref existe antes de devolver.** É o que separa "automático" de "irreversível":
    quem chama só pode dar `reset --hard` depois desta função ter provado que há para onde voltar.
    """
    tem_o_que_perder = pre.get("sujo") or pre.get("ahead") or pre.get("branch") not in ("main", "master")
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

def _puxar(pre: dict) -> None:
    """`fetch` + fast-forward. Só reseta quando o ff é impossível — e o resgate já rodou."""
    f = _git("fetch", "origin", timeout=300)
    if f.returncode != 0:
        raise RuntimeError(f"nao consegui buscar o codigo novo: {_cauda(f)}")

    m = _git("merge", "--ff-only", "origin/main", timeout=120)
    if m.returncode == 0:
        return

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

    r = _git("reset", "--hard", "origin/main", timeout=120)
    if r.returncode != 0:
        raise RuntimeError(f"nao consegui alinhar com o codigo novo: {_cauda(r)}")


def _reaplicar(topologia: str) -> None:
    """O que o `git pull` não atualiza: units, deps, build. Já existe, por sistema."""
    if _E_WINDOWS:
        p = _rodar(["powershell", "-ExecutionPolicy", "Bypass", "-File",
                    str(REPO / "install.ps1"), "-Update"])
    else:
        p = _rodar(["bash", str(REPO / "install.sh"), "--update"])
    if p.returncode != 0:
        raise RuntimeError(f"a instalacao nao terminou: {_cauda(p)}")


def _avisar_sessoes() -> None:
    """Recado pras sessões vivas de que o backend vai reiniciar.

    Elas não morrem — rodam em tmux, fora do backend (medido 25/08/2026: as sessões seguiram vivas
    pelo restart). O que cai é o SSE, o WebSocket do terminal e os app-servers do Codex. Custa quase
    nada avisar, e uma sessão avisada pode se preparar em vez de estranhar a conexão sumindo.

    Fail-soft de ponta a ponta: máquina sem `hangar-send` instalado não pode ter a atualização
    barrada por causa de um aviso.
    """
    aviso = ("[hangar] o backend vai reiniciar agora por causa de uma atualização. "
             "Sua sessão continua viva; o app reconecta sozinho.")
    try:
        p = _rodar(["hangar-send", "--group", aviso], timeout=30)
        if p.returncode != 0:
            _log.debug("aviso de restart nao saiu: %s", _cauda(p, 3))
    except (OSError, subprocess.TimeoutExpired) as e:
        _log.debug("hangar-send indisponivel: %s", e)


def _reiniciar(topologia: str) -> None:
    if topologia == "systemd":
        unidades = ["hangar-backend.service"]
        if _rodar(["systemctl", "--user", "list-unit-files", "hangar-frontend.service"],
                  timeout=15).returncode == 0:
            unidades.append("hangar-frontend.service")
        p = _rodar(["systemctl", "--user", "restart", *unidades], timeout=120)
        if p.returncode != 0:
            raise RuntimeError(f"nao consegui reiniciar o servidor: {_cauda(p)}")
    elif topologia == "windows":
        # NÃO reinicia sozinho aqui, e isto é decisão, não pendência esquecida. No Linux quem leva
        # os app-servers do Codex junto com o backend é o `KillMode=control-group` do systemd
        # (conferido 25/08/2026: eles nascem como subprocessos diretos, sem escopo próprio). No
        # Windows não há cgroup, e matar a árvore de processos certa é justamente o que ninguém
        # mediu naquela máquina — escrever a sequência às cegas arrisca deixar app-server órfão
        # escutando em loopback, ou pior, matar o processo errado. Então a atualização faz TUDO
        # (código, passos, dependências, build) e para antes do restart, dizendo isso na tela.
        # Quando alguém puder medir lá, este ramo vira o `taskkill` da árvore + o `.vbs` de sempre.
        _escrever(reiniciar_manual=True)
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
    try:
        return _executar(porta)
    finally:
        # Solta a vez mesmo saindo por caminho de erro. O `finally` não cobre o processo ser MORTO
        # (é pra isso que o lock guarda o pid e o dono morto é recolhido), mas cobre todo o resto.
        _soltar_a_vez()


def _executar(porta: int) -> dict:
    pre = checar()
    de = pre.get("commit", "")
    _escrever(fase="rodando", ok=None, erro=None, resgate=None,
              commit_de=de, commit_para=None, pid=os.getpid(),
              reiniciar_manual=False)

    if not pre.get("pode"):
        falta = ", ".join(pre.get("faltando") or []) or pre.get("erro", "desconhecido")
        return _falhou(f"falta o que a atualizacao precisa: {falta}")

    try:
        _etapa("resguardar")
        resgate = resguardar(pre)
        if resgate:
            _escrever(resgate=resgate)

        _etapa("codigo")
        _puxar(pre)
        para = _git("rev-parse", "HEAD", timeout=30).stdout.strip()
        _escrever(commit_para=para)

        _etapa("passos")
        _aplicar_passos()

        _etapa("instalar")
        _reaplicar(pre["topologia"])

    except Exception as e:                           # noqa: BLE001 — ver abaixo: é deliberado
        # `Exception`, e não uma lista de tipos. Isto roda num processo DESTACADO cuja única forma
        # de falar com alguém é o arquivo de estado: uma exceção que escape daqui mata o processo
        # com traceback num stderr que ninguém lê, e deixa o estado preso em "rodando" — a tela
        # gira a barra pra sempre, e a pessoa não descobre nem que falhou nem por quê. Medido: a
        # lista de tipos que estava aqui não pegava `PassoFalhou`, e um passo com prova falha
        # produzia exatamente isso.
        return _falhou(str(e), de=de)

    # O restart fica FORA do try acima, e a diferença não é cosmética: tudo lá em cima falha com o
    # servidor velho ainda no ar (`no_ar=True` é verdade). Aqui não — `systemctl restart` para o
    # processo antigo ANTES de subir o novo, então um erro neste ponto deixa a máquina sem serviço
    # nenhum, com código novo no disco. Esse caso tem que ir pro rollback, não pro "falhou e está
    # tudo como estava".
    try:
        _etapa("reiniciar")
        _avisar_sessoes()
        _reiniciar(pre["topologia"])
    except Exception as e:                           # noqa: BLE001 — mesmo motivo do de cima
        return _voltar(de, f"o servidor nao reiniciou: {e}", pre["topologia"], porta)

    # Prova de vida só onde houve restart de verdade. Onde ele não acontece (Windows, instalação na
    # mão), o backend velho segue respondendo — checar aqui devolveria um "subiu" que não prova nada.
    if not estado().get("reiniciar_manual") and not _subiu(porta):
        return _voltar(de, "o servidor nao respondeu depois de reiniciar", pre["topologia"], porta)

    _escrever(fase="pronto", ok=True, texto="Atualizado")
    return estado()


def _falhou(msg: str, de: str = "", no_ar: bool = True) -> dict:
    """Falha sem reversão. `voltou=False` é literal: ninguém trouxe a máquina de volta.

    `no_ar` tem padrão `True` porque o caso comum é falhar ANTES do restart — ali o servidor velho
    nunca saiu do ar. Mas nem toda falha é dessas: um `systemctl restart` que falha já derrubou o
    processo antigo, e o rollback pode ele mesmo não conseguir voltar. Quem sabe disso passa
    `no_ar=False`, e a tela diz que o serviço precisa de atenção em vez de afirmar que está no ar.
    """
    _log.error("atualizacao falhou: %s", msg)
    _escrever(fase="pronto", ok=False, erro=msg, voltou=False, no_ar=no_ar)
    return estado()


def _voltar(commit: str, motivo: str, topologia: str, porta: int) -> dict:
    """Rollback: volta ao commit que esta máquina tinha minutos atrás e sobe de novo.

    Este `reset --hard` é o único que não passa pelo `resguardar`, e pode: o alvo é um commit da
    própria máquina, de minutos atrás, e o pré-voo já garantiu que não havia trabalho solto no
    caminho (se houvesse, `resguardar` guardou). A alternativa a fazê-lo é deixar a pessoa sem
    servidor.
    """
    _escrever(fase="rodando", texto="Voltando para a versão anterior")
    r = _git("reset", "--hard", commit, timeout=120)
    if r.returncode != 0:
        # O reset falhou (commit inválido, disco cheio, `.git` travado). Seguir daqui reinstalaria
        # e reiniciaria em cima do código NÃO revertido — e o estado diria "voltei pra versão
        # anterior" com a máquina rodando exatamente o que quebrou. Para aqui e conta a verdade.
        return _falhou(f"{motivo}; e nao consegui voltar pra versao anterior: {_cauda(r)}",
                       no_ar=False)
    try:
        _reaplicar(topologia)
        _reiniciar(topologia)
    except (RuntimeError, subprocess.TimeoutExpired, OSError) as e:
        # `no_ar=False` explícito: chegar aqui quer dizer que o restart que motivou o rollback já
        # matou o processo antigo, e o restart do próprio rollback também falhou — não há nada
        # rodando. O default `True` do `_falhou` vale pro caso comum (falha antes de qualquer
        # restart), e aqui ele diria "está no ar" com a máquina sem serviço nenhum.
        return _falhou(f"{motivo}; e a volta para a versao anterior tambem falhou: {e}",
                       no_ar=False)
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
    if not isinstance(pid, int) or pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except (OSError, ProcessLookupError):
        return False
    return True


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


def iniciar(porta: int = 8765) -> dict:
    """Lança a atualização fora deste processo e devolve na hora.

    Fora do processo porque ela reinicia o backend — dentro, ela se mataria no meio. `setsid` no
    POSIX e `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` no Windows, sem herdar os pipes daqui:
    o filho não pode morrer junto com o pai nem escrever no log do serviço que vai reiniciar.
    """
    if not _tomar_a_vez():
        return {"ok": False, "erro": "ja_rodando"}

    # `setsid` NÃO basta, e por pouco: ele tira o filho da sessão/grupo de processos, não do
    # **cgroup**. Como este `Popen` acontece dentro do worker do FastAPI, o processo da atualização
    # nasce no cgroup de `hangar-backend.service` — e a unit não declara `KillMode`, então o padrão
    # é `control-group`. Consequência: o `systemctl --user restart` que a própria atualização dispara
    # mata o cgroup inteiro e leva junto quem estava dando o comando, ANTES de ele escrever
    # `fase=pronto`. O estado ficaria travado em "rodando" para sempre e a barra giraria sem fim —
    # exatamente a falha que este módulo existe pra não ter. O repo já resolveu isso uma vez, pelo
    # mesmo motivo: `tmux._scope_prefix()`, que envolve o servidor tmux num escopo transiente.
    args = tmux._scope_prefix() + [sys.executable, "-m", "app.atualizar", str(porta)]
    extra: dict = {}
    if _E_WINDOWS:
        extra["creationflags"] = _FLAGS_DESTACADO_WINDOWS
    else:
        extra["start_new_session"] = True   # setsid: sai do grupo de processos do backend

    proc = subprocess.Popen(
        args, cwd=str(REPO / "backend"),
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
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
    _escrever(fase="rodando", ok=None, erro=None, pid=proc.pid,
              passo=0, total=len(ETAPAS), texto="Começando")
    return {"ok": True, "pid": proc.pid}


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    executar(int(sys.argv[1]) if len(sys.argv) > 1 else 8765)
