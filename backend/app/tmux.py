import os
import shutil
import logging
import subprocess
import time

RUN = subprocess.run


_SCOPE = ["systemd-run", "--user", "--scope", "--collect", "-q", "--"]

# Cache do probe abaixo: None = ainda nao testado. Por processo — se o systemd voltar ao normal,
# um restart do backend re-testa. Nao vale re-testar a cada sessao: o gerenciador raramente muda
# de estado e o probe custa um fork.
_scope_usavel: bool | None = None


def _scope_probe() -> bool:
    # `systemd-run` pode ESTAR instalado e ainda assim falhar: o gerenciador systemd do usuario
    # recusa criar scope transiente ("Failed to start transient scope unit: Unit run-pN.scope not
    # found") e o comando sai 1 sem rodar nada. Aconteceu nesta maquina, 5/5 tentativas, com o
    # binario e o gerenciador na mesma versao — logo NAO da pra inferir do `which`.
    # Sem este probe, toda criacao de sessao morre com "falha ao criar sessao no tmux": o app fica
    # sem abrir sessao por causa de um detalhe de cgroup que e OPCIONAL.
    global _scope_usavel
    if _scope_usavel is None:
        # ponytail: check-then-act sem lock. Duas POST /api/sessions concorrentes no frio (backend
        # recem-subido) podem rodar o probe as duas — mas o probe é idempotente (mesmo comando,
        # mesmo resultado booleano), então o pior caso é um fork() de sobra, nunca um valor
        # inconsistente. Lock só entra se esse fork extra virar custo medido.
        _scope_usavel = _run([*_SCOPE, "true"]).returncode == 0
        if not _scope_usavel:
            # Falha aparece, nao some: sem o scope, uma sessao que TENHA de iniciar o servidor tmux
            # nasce no cgroup do backend, e ai um `systemctl restart` do backend derruba as sessoes.
            # Com o servidor tmux ja de pe (caso normal) o pane herda o cgroup DELE e nada muda.
            _log.warning("systemd-run --user --scope indisponivel; criando sessoes sem scope "
                         "proprio. Se o servidor tmux precisar ser iniciado por aqui, um restart "
                         "do backend pode derrubar as sessoes.")
    return _scope_usavel


def _scope_prefix() -> list[str]:
    # Spawn the tmux SERVER in its OWN transient systemd scope so it does NOT inherit the
    # backend service's cgroup. Without this, `systemctl restart claude-pocket-backend`
    # SIGTERMs the whole control-group -> kills the tmux server and every session (incl. the
    # one driving this app). ponytail: gated on systemd-run + a user runtime dir; on non-systemd
    # hosts returns [] and spawns plainly, where the cgroup teardown problem doesn't exist.
    if os.name == "posix" and os.environ.get("XDG_RUNTIME_DIR") and shutil.which("systemd-run"):
        return _SCOPE if _scope_probe() else []
    return []


def _wayland_display() -> str | None:
    # Paste de IMAGEM no Claude Code depende do wl-paste, que precisa de WAYLAND_DISPLAY. O backend
    # roda como servico systemd (env de boot, sem a var) -> detecta o socket do compositor no
    # runtime dir (ex: wayland-1 no Hyprland; o fallback wayland-0 do wl-paste NAO acha esse).
    if os.environ.get("WAYLAND_DISPLAY"):
        return os.environ["WAYLAND_DISPLAY"]
    run_dir = os.environ.get("XDG_RUNTIME_DIR")
    if not run_dir:
        return None
    try:
        socks = sorted(f for f in os.listdir(run_dir)
                       if f.startswith("wayland-") and not f.endswith(".lock"))
    except OSError:
        return None
    return socks[0] if socks else None


_log = logging.getLogger("claude_pocket.tmux")


def _run(args: list[str], input: bytes | None = None) -> subprocess.CompletedProcess:
    # timeout: tmux travado nao pode prender o event loop / worker do threadpool pra sempre. Estouro ->
    # trata como falha (returncode=1), igual ao tmux recusar; os callers ja checam returncode != 0.
    #
    # `input=` existe pro `load-buffer -`: o texto vai pela STDIN e escapa do teto de 16344 bytes do
    # COMANDO (medido 07/08/2026: `set-buffer -- <texto>` acima disso devolve rc=1 "command too
    # long"). `text=True` e `input=bytes` sao incompativeis, entao o modo texto sai quando ha stdin e
    # a saida e decodificada aqui — o retorno continua sendo `str` pra todos os chamadores de hoje.
    if input is not None:
        try:
            cp = subprocess.run(args, capture_output=True, timeout=5, input=input)
            # bytes de verdade num run real; str num mock de teste que ja devolve texto — os dois
            # caminhos tem de sair como str, igual ao retorno do modo texto abaixo.
            out = cp.stdout.decode("utf-8", "replace") if isinstance(cp.stdout, bytes) else (cp.stdout or "")
            err = cp.stderr.decode("utf-8", "replace") if isinstance(cp.stderr, bytes) else (cp.stderr or "")
            return subprocess.CompletedProcess(args, cp.returncode, out, err)
        except (subprocess.TimeoutExpired, OSError) as e:
            return subprocess.CompletedProcess(args, 1, stdout="", stderr=str(e))
    try:
        return RUN(args, capture_output=True, text=True, encoding="utf-8", errors="replace",
                   timeout=5)
    except (subprocess.TimeoutExpired, OSError) as e:
        # OSError = tmux ausente (FileNotFoundError) / sem permissao; timeout = travado. Trata como
        # falha (returncode=1) em vez de 500 com traceback — os callers ja checam returncode != 0.
        return subprocess.CompletedProcess(args, 1, stdout="", stderr=str(e))


def _pane_target(name: str) -> str:
    # Alvo de SESSAO exato pra comandos pane/window-scoped (send-keys, paste-buffer, capture-pane,
    # list-panes). Sem isto, um nome de sessao NUMERICO (0/1/2 — auto-numerado pelo tmux quando cria
    # sem `-s nome`) colide com INDICE de window: `-t 0` vira "window 0 da sessao anexada", nao
    # "sessao 0" -> resolvia o pane ERRADO e vazava conversa/preview entre sessoes. `=NAME:` forca
    # match exato de sessao (`=`) escopado a sessao (`:`, janela/pane ativo). Nomes nao-numericos
    # ja funcionavam; isto cobre os dois casos.
    #
    # E os dois-pontos finais significam "janela/pane ATIVO": o app fala com o que estiver na frente,
    # nao com o agente. O agentpane resolve por processo; None = nao sei, vale o comportamento antigo.
    #
    # Import TARDIO porque agentpane importa este modulo — em cima seria ciclo. Depois da 1a chamada
    # e uma busca em dict.
    from app.agentpane import resolve_target
    return resolve_target(name) or f"={name}:"


def list_sessions() -> list[dict]:
    cp = _run(["tmux", "list-sessions", "-F", "#{session_name}\t#{pane_current_path}"])
    if cp.returncode != 0:
        return []
    out = []
    for line in cp.stdout.splitlines():
        if not line.strip():
            continue
        name, _, cwd = line.partition("\t")
        out.append({"name": name, "cwd": cwd})
    return out


def list_panes_active() -> list[dict]:
    # UMA chamada traz nome + pane_pid + cwd + pane_id da pane ATIVA de TODAS as sessoes. Substitui o
    # list_sessions() + um pane_pid() por sessao (S+1 forks -> 1) no caminho da listagem. pane_id
    # (ex: "%9") identifica o bilhete da extensao Pi (Task 3), que e por-pane, nao por-sessao.
    cp = _run(["tmux", "list-panes", "-a", "-F",
               "#{session_name}\t#{pane_active}\t#{pane_pid}\t#{pane_current_path}\t#{pane_id}"])
    if cp.returncode != 0:
        return []
    out: dict[str, dict] = {}
    for line in cp.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) != 5:
            continue
        name, active, pid, cwd, pane_id = parts
        if active != "1" or name in out:
            continue
        out[name] = {"name": name, "pid": int(pid) if pid.isdigit() else None, "cwd": cwd,
                     "pane_id": pane_id}
    return list(out.values())


def list_panes_all() -> dict[str, list[dict]]:
    # Task 5.5: MESMA chamada `list-panes -a` do list_panes_active, so que sem descartar os panes
    # NAO ativos -> quem precisa escolher, por sessao, o pane do AGENTE (nao so o ativo) usa esta,
    # sem pagar nenhum fork a mais (list_panes_active continua existindo, intocada, pra quem so quer
    # o ativo). Agrupado por nome de sessao porque e assim que o caller (registry.list()) itera.
    #
    # Task 6: `#{@cp_hidden}` a mais no MESMO `-F` -- opcao de usuario por SESSAO (tmux set-option),
    # nao convencao de nome (colidiria com o filtro por sidecar do Codex, que ja e por nome). Um
    # campo a mais no format nao custa fork nenhum: e a MESMA chamada de sempre. Sessao sem a
    # opcao seta devolve string vazia ("" != "1" -> hidden=False), entao toda sessao pre-existente
    # continua aparecendo normalmente.
    #
    # O parse aceita 5 campos OU MAIS (achado da revisao final, C1): esta funcao alimenta as tres
    # views, o list_with_state, o _pane_info e o _cwd_has_siblings -- se o 6o campo nao vier, o app
    # nao pode ficar SEM SESSAO NENHUMA por causa de uma opcao de usuario no `-F`. No Windows quem
    # responde e o psmux, cuja compatibilidade e MEDIDA (scripts/test-psmux.py, secao 4) e nao
    # inferida: interpolar `#{@opcao}` nao estava na lista ate esta rodada. Multiplexador que
    # devolve o campo vazio ou o literal cru -> hidden=False, tudo aparece; multiplexador que
    # RECUSA o comando -> rc != 0 -> {} -> ai nenhum parse salva, e por isso a sonda tambem cobre
    # o formato de 6 campos.
    cp = _run(["tmux", "list-panes", "-a", "-F",
               "#{session_name}\t#{pane_active}\t#{pane_pid}\t#{pane_current_path}\t#{pane_id}"
               "\t#{@cp_hidden}"])
    if cp.returncode != 0:
        return {}
    out: dict[str, list[dict]] = {}
    for line in cp.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < 5:
            continue
        name, active, pid, cwd, pane_id = parts[:5]
        hidden = parts[5] if len(parts) > 5 else ""
        out.setdefault(name, []).append({
            "name": name, "pid": int(pid) if pid.isdigit() else None, "cwd": cwd,
            "pane_id": pane_id, "active": active == "1", "hidden": hidden == "1",
        })
    return out


def list_panes_of(name: str) -> list[dict]:
    # TODOS os panes de UMA sessao (list_panes_active devolve so o ATIVO, de todas). `-s` = escopo
    # sessao; `=NAME` = match exato (mesma pegadinha documentada no _pane_target).
    #
    # O `:` final NAO e cosmetico aqui: medido nesta maquina, `-s -t =0` (sem ele) para um nome
    # NUMERICO sem sessao correspondente devolve rc=0 e os panes da sessao ATTACHED do servidor —
    # nao falha. `-s -t =0:` falha corretamente com "can't find session". `-s` continua devolvendo
    # TODOS os panes da sessao com o `:` (testado com sessao de 2 janelas); o `:` so fecha a mesma
    # fresta de nome numerico que o _pane_target ja documenta acima.
    #
    # `#{pane_active}` entrou pro desempate do agentpane ficar igual ao do registry._agent_pane
    # (I2 da revisao final): com 2+ panes de agente, os dois tem que escolher o MESMO. Campo
    # ausente (multiplexador que nao interpola) -> active=False em todos e a ordem e a da
    # varredura, exatamente o que era antes.
    cp = _run(["tmux", "list-panes", "-s", "-t", f"={name}:", "-F",
               "#{pane_id}\t#{pane_pid}\t#{pane_active}"])
    if cp.returncode != 0:
        return []
    out = []
    for line in cp.stdout.splitlines():
        partes = line.split("\t")
        if len(partes) < 2:
            continue
        pane_id, pid = partes[0], partes[1]
        if pane_id.startswith("%") and pid.isdigit():
            out.append({"pane_id": pane_id, "pid": int(pid),
                        "active": len(partes) > 2 and partes[2] == "1"})
    return out


def is_hidden(name: str) -> bool:
    """A sessao `name` existe e tem a marca @cp_hidden (Task 6)?

    Consulta DIRETA no tmux, nao inferida de `registry.list()` (achado da revisao, rodada 2 do
    Task 6a): a lista tambem filtra sessao com sidecar Codex de mesmo nome (registry.py, filtro
    logo abaixo do de hidden) -- um `term-<nome>` que fosse uma sessao Codex de verdade "sumiria"
    da lista por ESSE motivo, nao por estar marcada, e quem perguntasse "esta na lista?" concluiria
    (errado) que o nome esta livre. `:` final obrigatorio -- mesma pegadinha do `set-option`/
    `_pane_target`: sem ele, `show-options -t "=nome"` devolve "no such session" mesmo com a
    sessao viva nesta versao do tmux.
    """
    cp = _run(["tmux", "show-options", "-v", "-t", f"={name}:", "@cp_hidden"])
    return cp.returncode == 0 and cp.stdout.strip() == "1"


def has_session(name: str) -> bool:
    # `=NAME`: match EXATO, mesma pegadinha do _pane_target acima. Sem o `=`, o target-session do tmux
    # cai em exact -> fnmatch -> PREFIX match: com "pocket-2" viva, `has_session("pocket")` respondia
    # VIVO pra uma sessao que NUNCA existiu. Como o app fabrica nomes que colidem por prefixo
    # (`<base>`, `<base>-2`, `<base>-3`...), a sessao morta herdava o "vivo" da irma -> o /input
    # confirmava "entregue" digitando num pane inexistente e o state.py nunca marcava `dead`.
    # Sem `:` aqui (ao contrario do _pane_target): has-session so resolve SESSAO, nao pane/window.
    return _run(["tmux", "has-session", "-t", f"={name}"]).returncode == 0


def session_created(name: str) -> float:
    # Epoch (s) do NASCIMENTO da sessao tmux atual com esse nome. Entrada de fila mais velha que
    # isto pertence a uma VIDA ANTERIOR da sessao (mesmo nome de pasta, tmux recriado) e nao deve
    # ser entregue — sessao morreu devendo, a divida morre junto (regra do dono). O ts do transcript
    # sozinho nao cobre o resume (`pi -c`): transcript velho, tmux novo. 0.0 = nao sei (sessao
    # sumida/erro) -> sem poda extra, comportamento de hoje.
    cp = _run(["tmux", "display-message", "-p", "-t", f"={name}", "#{session_created}"])
    if cp.returncode != 0:
        return 0.0
    try:
        return float(cp.stdout.strip())
    except ValueError:
        return 0.0


def new_session(name: str, cwd: str, command: str, config_dir: str | None = None) -> bool:
    # -e: cores corretas do Claude Code DENTRO do tmux (o claude e spawnado via `exec`, virando o
    # processo do pane sem shell intermediario). COLORTERM=24-bit + CLAUDE_CODE_TMUX_TRUECOLOR curto-circuita o downgrade pra 256
    # (gate pink). O TERM nao-tmux (gate teal) vem do default-terminal no ~/.tmux.conf.
    # Ver docs/tmux-truecolor-setup.md.
    # Retorna False quando o tmux recusa (ex: nome duplicado) -> o caller NAO pode mapear a sessao
    # nova pra um jsonl, senao reusaria a sessao existente de mesmo nome (= "sessao nova foi pra 0").
    cfg = config_dir or os.environ.get("CLAUDE_CONFIG_DIR")
    # CP_SESSION_NAME: identidade CARIMBADA no nascimento — "quem sou eu" pra tudo que roda dentro do
    # pane (o cp-send usa pra assinar recado, parear e desparear). Antes o cp-send perguntava
    # `tmux display-message -p '#S'`, que NAO e propriedade de quem chama: e a "sessao corrente",
    # resolvida pelo CLIENTE anexado — estado global do servidor. Com um unico cliente anexado (medido
    # nesta maquina: `list-clients` devolvia so `/dev/pts/9067: jeffer1312`), TODA sessao que
    # perguntava recebia o mesmo nome, o da sessao do cliente. Resultado: a sessao B rodava
    # `cp-send --unpair` pra sair do grupo, se identificava como A e o backend desparava A — o
    # `--unpair` de uma sessao desfazia o vinculo da OUTRA, e o grupo de 2 se dissolvia inteiro
    # (aconteceu 2x seguidas, sem ninguem pedir). Ancorar em `$TMUX_PANE` NAO resolve no Windows: o
    # psmux numera pane id por sessao, entao `%1` existe nas duas e o alvo fica ambiguo (o tmux real
    # numera por servidor, e por isso o bug nao aparece no Linux). O env herdado do pane e imune as
    # duas coisas. Fica obsoleto se a sessao for renomeada (rename_session abaixo) -> quem le valida
    # contra o list-sessions e cai no fallback.
    args = _scope_prefix() + [
        "tmux", "new-session", "-d", "-s", name, "-c", cwd, "-x", "200", "-y", "50",
        "-e", "COLORTERM=truecolor",
        "-e", "CLAUDE_CODE_TMUX_TRUECOLOR=1",
        "-e", f"CP_SESSION_NAME={name}",
    ]
    wl = _wayland_display()
    if wl:
        # sem isto o wl-paste dentro do pane nao conecta -> paste de imagem no Claude Code morre.
        args += ["-e", f"WAYLAND_DISPLAY={wl}"]
    if cfg:
        # sessao app-criada usa o MESMO config dir que o backend (ou o escolhido), em vez de cair
        # no ~/.claude default (deslogado -> tela de boas-vindas).
        args += ["-e", f"CLAUDE_CONFIG_DIR={cfg}"]
    # `exec`: o tmux SEMPRE roda o comando via `$SHELL -c` (fish aqui). Sem exec, o fish fica como
    # dono do tty/grupo de foreground e o `send-keys` (input do app) NAO chega no claude -> ele
    # renderiza mas nunca le o teclado. Com exec o fish vira o claude (dono do tty) -> input chega.
    # SO no POSIX: o psmux (Windows) roda o comando direto no ConPTY, sem shell no meio, e o
    # `exec` viraria um argumento que nenhum shell do Windows conhece — o pane nasce e morre na
    # hora, com o new-session ainda devolvendo 0, ou seja, o app reportaria sessao criada.
    args.append(f"exec {command}" if os.name == "posix" else command)
    ok = _run(args).returncode == 0
    if ok:
        # Servidor tmux reiniciado zera o contador de `%N`: uma sessao recriada com o MESMO nome
        # dentro da janela de 60s do cache do agentpane mandaria a mensagem pro pane de OUTRA sessao
        # (achado 1 da revisao, rodada 1) — a mesma classe de estrago que a Task 1 existe pra evitar.
        # Import tardio: agentpane importa este modulo (mesmo motivo do _pane_target).
        from app.agentpane import invalidate
        invalidate(name)
    return ok


def new_hidden_shell(name: str, cwd: str) -> str | None:
    """Cria (ou reata) a sessao de shell ESCONDIDA do botao "+" do painel de terminal (Task 6).

    Sessao SEPARADA, nao janela dentro da sessao do agente: sessao separada tem JANELA PROPRIA,
    entao o painel e o terminal nativo do usuario param de disputar qual janela esta na frente,
    qual e o tamanho, e qual e o alvo do attach -- o atrito que a versao anterior desta task (uma
    `new-window` na sessao do agente) tinha.

    Devolve o nome da sessao (criada ou ja existente), ou None se o tmux recusou.
    """
    alvo = f"term-{name}"
    # NAO usa `new-session -A` (reatar): MEDIDO no tmux 3.7b que `-A` numa sessao JA existente falha
    # com "open terminal failed: not a terminal" quando quem chama nao tem tty controlador (rc=1) --
    # e o backend, rodando como servico ou via subprocess capturado, nunca tem. Com `-A` o SEGUNDO
    # POST /shell (idempotencia, o comando ainda rodando ao recarregar a pagina) sempre devolvia
    # None. `has_session` + `new-session` PLAIN (sem `-A`) evita o caminho de attach por completo:
    # so cria quando de fato NAO existe, e o `_scope_prefix()` so entra nesse caso -- e o mesmo
    # motivo do new_session: se esta for a chamada que inicia o servidor tmux, sem o scope ele nasce
    # no cgroup do backend e um `systemctl --user restart` derruba tudo. ponytail: check-then-act
    # sem lock -- duas POST /shell concorrentes pro MESMO nome podem colidir; o perdedor da corrida
    # recebe "duplicate session" (rc!=0), mas a sessao EXISTE de verdade (o vencedor criou) -- por
    # isso o `has_session` de novo abaixo, achado da revisao (minor): sem ele o perdedor devolvia
    # None -> 500 pra um shell vivo e saudavel.
    criou_agora = False
    # Reatar so vale se for o shell DAQUELE repo (achado I3 da revisao final). `term-<nome>` fica
    # vivo quando a sessao do agente morre FORA do app (usuario digita `exit`, ou um
    # `tmux kill-session` na mao) -- o `_kill_hidden_shell` so roda pelo `kill()` daqui. Criar
    # depois outra sessao com o MESMO nome noutro repo e abrir a aba Shell devolvia o shell do
    # repo ANTIGO, com o rotulo da sessao nova: silencioso e perigoso (um `rm`/`git` digitado no
    # diretorio errado). Compara `#{session_path}` -- a pasta de NASCIMENTO da sessao, medido que
    # nao muda quando o usuario faz `cd` dentro do pane (isso e o `pane_current_path`): um `cd` NAO
    # pode derrubar o shell de ninguem. Diferente -> mata e recria, que e aceitavel porque a sessao
    # e do app. Exige a MARCA antes de matar (mesmo cuidado do registry._kill_hidden_shell): um
    # `term-<nome>` de terceiro nunca pode ser derrubado por este caminho.
    if has_session(alvo):
        cp = _run(["tmux", "display", "-p", "-t", f"={alvo}:", "#{session_path}"])
        antigo = cp.stdout.strip() if cp.returncode == 0 else ""
        if cp.returncode != 0:
            # "Conferi e bate" e "nao consegui conferir" nao podem sair iguais (achado da revisao):
            # com o `display` falhando (tmux ocupado, timeout de 5s do `_run`), `antigo` vira "" e o
            # bloco de diretorio divergente abaixo nem roda — o shell existente e devolvido como se o
            # diretorio batesse. Reaproveitar continua sendo o comportamento (matar um shell com
            # trabalho por causa de uma leitura transiente seria pior), mas agora com registro.
            _log.warning("nao consegui ler o diretorio do shell escondido %r (rc=%d): reaproveito "
                         "sem conferir se ele e mesmo de %r", alvo, cp.returncode, cwd)
        if antigo and os.path.normpath(antigo) != os.path.normpath(cwd) and is_hidden(alvo):
            _log.info("shell escondido %r era do diretorio %r e agora a sessao e de %r — "
                      "recriando", alvo, antigo, cwd)
            # O retorno NAO e descartado (o docstring do kill_session e explicito: False = quem
            # chama nao pode reportar sucesso). Falhando o kill (tmux travado -> timeout de 5s do
            # `_run`), o `has_session` abaixo segue True, a criacao e pulada e esta funcao
            # devolveria o shell do diretorio ANTIGO -- exatamente o `rm`/`git` no lugar errado que
            # o bloco existe pra impedir, agora com o log dizendo "recriando". Desiste em vez disso:
            # None vira 500 na rota e o usuario tenta de novo.
            if not kill_session(alvo):
                _log.warning("shell escondido %r nao saiu; NAO reaproveito o shell do diretorio "
                             "antigo %r", alvo, antigo)
                return None
    if not has_session(alvo):
        cp = _run([*_scope_prefix(), "tmux", "new-session", "-d", "-s", alvo, "-c", cwd])
        if cp.returncode != 0:
            return alvo if has_session(alvo) else None
        criou_agora = True
    # A marca. Roda SEMPRE, nao so na criacao: sem ela a sessao vira CARD nas tres views do app,
    # porque o registry trata pane nao reconhecido como Claude por padrao -- inclusive quando a
    # sessao ja existia (reatada acima) e sobrou de uma versao anterior sem a marca. Idempotencia
    # vale pra marca tambem, nao so pra criacao. O `:` final NAO e cosmetico (mesma pegadinha
    # documentada em `_pane_target`/`list_panes_of`): MEDIDO que `set-option -t "=alvo"` sem ele
    # devolve "no such session" nesta versao do tmux, mesmo com a sessao existindo de verdade.
    #
    # O rc NAO e mais descartado (achado da revisao, rodada 2): antes, um `set-option` que falhasse
    # (tmux ocupado, timeout de 5s do `_run`) deixava a sessao viva e VISIVEL -- card nas tres
    # views. Se a sessao acabou de nascer AGORA, ninguem tinha trabalho rodando nela ainda -- mata
    # em vez de deixar o fantasma visivel. Se ja existia (reatada), NAO mata: matar um shell com
    # trabalho de verdade por causa de um `set-option` transiente seria pior que o card fantasma.
    #
    # L68 da revisao final: a versao anterior deste comentario prometia que o fantasma "se
    # autocorrige no proximo POST". NAO se autocorrige -- o gate de /shell (api.abrir_shell)
    # recusa com 409 ANTES de chamar esta funcao, justamente porque a sessao existe sem a marca.
    # Quem tira o fantasma da tela e o usuario, encerrando a sessao `term-<nome>`; o texto do 409
    # diz isso, sem afirmar que a sessao e de terceiro.
    if _run(["tmux", "set-option", "-t", f"={alvo}:", "@cp_hidden", "1"]).returncode != 0:
        if criou_agora:
            kill_session(alvo)
        return None
    return alvo


def kill_session(name: str) -> bool:
    """True = a sessao NAO existe mais depois desta chamada (inclui "ja nao existia"). False = ela
    sobreviveu, e quem chama NAO pode reportar sucesso nem apagar estado duravel dela."""
    # `=` (match EXATO) so no POSIX. Era o unico alvo de sessao do modulo SEM o `=`: has_session e
    # _pane_target ja o usam justamente porque o tmux resolve target-session em exact -> fnmatch ->
    # PREFIX, e o app fabrica nomes que colidem por prefixo (`<base>`, `<base>-2`, ...). Sem ele,
    # kill_session("pocket") com "pocket" ja morta derruba "pocket-2" — matar a sessao ERRADA custa
    # o trabalho de quem estava nela.
    # O comportamento CORRETO nao muda: `=nome` e `nome` resolvem igual sempre que a sessao existe.
    # A diferenca aparece so no caso quebrado (nome ausente + irma por prefixo): antes matava a irma,
    # agora nao mata nada.
    # No WINDOWS fica o nome cru, medido no psmux 3.3.7:
    #   - `-t "=nome"` FALHA (rc=1, "kill-session: session 'nome' still present after 5s") e ainda
    #     bloqueia 5s antes de desistir — o psmux nao interpreta o `=` aqui, so em has-session/
    #     display/send-keys. 5s e o teto do _run: viraria timeout.
    #   - o nome cru ja e exato la: com SO "zz-alvo-2" viva, `kill-session -t zz-alvo` nao derrubou
    #     nada (rc=0). Ou seja, no Windows nao ha prefix match a se defender.
    alvo = f"={name}" if os.name == "posix" else name
    _run(["tmux", "kill-session", "-t", alvo])
    # Devolve "a sessao SAIU?", nao "o comando deu 0" — sao coisas diferentes e a que importa e a
    # primeira. Dois casos reais em que o rc engana: (1) no psmux o kill-session devolve 0 e a sessao
    # continua de pe (medido; o instalador contorna matando por PID); (2) no caso quebrado descrito
    # acima o comando falha mas a sessao ja estava morta — e "morta" e exatamente o que o caller quer.
    # Idempotente de proposito: apagar sessao que nao existe e sucesso.
    saiu = not has_session(name)
    if saiu:
        # A sessao morreu de verdade -> o pane que o agentpane tinha cacheado pra este nome tambem
        # morreu. Sem isto, um kill+new_session (resume) na mesma funcao (registry.py, adapter.py)
        # ficaria ate 60s mandando mensagem pro pane VELHO (achado 1 da revisao, rodada 1). Import
        # tardio: agentpane importa este modulo (mesmo motivo do _pane_target).
        from app.agentpane import invalidate
        invalidate(name)
    return saiu


def rename_session(old: str, new: str) -> bool:
    ok = _run(["tmux", "rename-session", "-t", old, new]).returncode == 0
    if ok:
        # `old` nao existe mais sob esse nome; `new` pode ja ter um cache velho de uma vida anterior
        # (mesmo nome, sessao diferente) — os dois precisam sumir. Mesma razao do kill_session/
        # new_session acima. Import tardio: agentpane importa este modulo.
        from app.agentpane import invalidate
        invalidate(old)
        invalidate(new)
    return ok


# No Windows a TUI do Claude Code entra em "modo paste" quando UM `send-keys -l` entrega mais que
# ~1120 chars de uma vez (medido, psmux 3.3.7 + claude v2.1.218: 1120 chega inteiro, 1140 ja perde
# o INICIO no submit) — o Enter envia so a CAUDA e o comeco some. Foi o corte do prompt de
# pareamento: 1220 chars -> so os ~300 finais, SEM o "[de: claude-pocket]" do inicio, e a sessao
# obedeceu o pedaco achando que era o usuario. O send-keys/psmux entregam 100% (o buffer do input
# fica integro, medido via Home ate 1600 chars); quem corta e a TUI no SUBMIT. Fatiar em pedacos
# abaixo do cliff, com pausa entre eles, faz a TUI ver DIGITACAO normal (nunca vira paste) e o texto
# inteiro submete num Enter so — medido: pedaco 1024 + pausa 0.3s recupera 100% do inicio.
# SO no Windows (os.name == "nt"), por decisao do dono do repo: no Linux o bug NAO acontece (prompts
# ate 1083 chars entram inteiros numa chamada) e mexer no caminho que funciona so arriscaria quebra-lo
# -> o ramo posix fica BYTE-IDENTICO a hoje. Mesma pegada de os.name que o new_session ja usa.
# O teto NAO e o ponto onde a TUI corta (~1120) e sim onde ela COLAPSA o burst em paste: medido
# nesta maquina, 700 chars entram como digitacao normal e 900 ja viram "paste again to expand". Um
# pedaco colapsado e DESCARTADO no submit (o Enter manda so o que foi digitado depois), entao um
# chunk de 1024 — abaixo do cliff, mas ACIMA do colapso — perdia o 1o pedaco inteiro e entregava so
# a cauda: foi o que aconteceu com o anuncio de pareamento (1340 chars -> so os 330 finais, 4x
# seguidas porque o reconcile via entrega parcial e redigitava). 512 fica com folga sob os 700
# medidos; 1340 chars viram 3 chamadas (~0.6s a mais), custo irrelevante perto de perder o inicio.
_WIN_CHUNK = 512         # < limiar de colapso de paste (medido: 700 ok, 900 colapsa)
_WIN_CHUNK_PAUSE = 0.3   # pausa entre pedacos (medida: recupera 100% do inicio)
# Teto DURO do pedaco. O 512 acima e o alvo; este e ate onde da pra esticar a fronteira sem cair no
# colapso de paste (medido: 700 entra como digitacao, 900 colapsa). A folga de ~190 chars e o que
# permite EMPURRAR a fronteira pra frente em vez de pra tras ao fugir de um hifen (ver _fatiar_win).
_WIN_CHUNK_MAX = 700


def _fatiar_win(text: str) -> list[str]:
    """Fatia pro psmux garantindo que NENHUM pedaco comece com '-'.

    O psmux NAO honra o `--`: argumento que comeca com '-' e engolido em SILENCIO, com rc=0 e stderr
    vazio (medido: '-X' e '--X' nao chegam; 'xX' e ' -X' chegam -> o teste e o PRIMEIRO caractere).
    Como o rc mente, nenhuma checagem de erro pega: o pedaco some e o Enter submete o resto como se
    fosse a mensagem inteira. Aconteceu de verdade entre duas sessoes aqui: recado de 2332 chars
    chegou com 1820, faltando exatamente 512 alinhados no chunk 2, que comecava com '-'.

    A fronteira anda PRA FRENTE (absorve a corrida de hifens no pedaco atual), nunca pra tras:
    encolher o pedaco atual numa corrida longa converge pra fronteira zero, que e o caso degenerado.
    Pra frente cabe na folga ate _WIN_CHUNK_MAX, e corrida de hifens em texto real e curta (uma linha
    '-----' de separador tem dezenas de chars, nao centenas). Estourado o teto, aceita a fronteira no
    hifen: o pedaco seguinte comecaria com '-' e o CALLER resolve com o placeholder.
    """
    pedacos: list[str] = []
    i, n = 0, len(text)
    while i < n:
        fim = min(i + _WIN_CHUNK, n)
        # Enquanto o PROXIMO pedaco comecaria com '-', engole o hifen neste aqui.
        while fim < n and text[fim] == "-" and (fim - i) < _WIN_CHUNK_MAX:
            fim += 1
        pedacos.append(text[i:fim])
        i = fim
    return pedacos


def _send_literal(target: str, text: str) -> bool:
    """False = o envio parou no meio e parte do texto ficou no input do pane.

    Devolve em vez de LEVANTAR de propósito. Uma exceção aqui atravessaria todos os call sites de
    `send_keys(literal=True)` — `answer_questions` (resposta de texto livre num AskUserQuestion),
    `send_text` (espelho do terminal) e o `drain` — e nenhum deles a trataria: as rotas /answer e
    /term-input só capturam ValueError (viraria 500 cru, e no picker o texto parcial ficaria digitado
    sem Escape de limpeza), e o `drain` tem um `except Exception` cego que NÃO loga e deixa a entrada
    como delivered=True (o `claim_undelivered` marca antes do envio). Ou seja: a exceção trocava um
    silêncio por outro pior. Bool não escapa por acidente — quem não confere segue como sempre.
    """
    # POSIX: caminho de sempre, BYTE-IDENTICO. O bug do '-' e do psmux; o tmux real honra o `--`.
    if os.name != "nt":
        cp = _run(["tmux", "send-keys", "-t", target, "-l", "--", text])
        if cp.returncode != 0:
            _log.warning("tmux send-keys -l falhou pra %r: %s",
                         target, (cp.stderr or "").strip()[:200])
            return False
        return True

    # WINDOWS. Dois problemas do psmux se somam aqui:
    #   1. texto acima do teto vira "paste" na TUI e o comeco e descartado no submit -> fatia;
    #   2. pedaco que COMECA com '-' e engolido em silencio (rc=0) -> ver _fatiar_win.
    # O (2) NAO e efeito do (1): mensagem CURTA de uma chamada so, comecando com '-', tambem some.
    # Por isso o placeholder abaixo vale pros dois caminhos, nao so pro fatiado.
    #
    # PLACEHOLDER: quando o TEXTO INTEIRO comeca com '-' nao existe fronteira pra mover - o 1o pedaco
    # comeca no indice 0. Manda-se um 'x' na frente (o argumento deixa de comecar com '-' e passa) e
    # apaga-se o 'x' no FIM, com Home + DC.
    # A ORDEM IMPORTA e foi medida: o Home+DC tem que ser o ULTIMO passo antes do Enter. Feito logo
    # apos o 1o pedaco, o cursor fica na posicao 0 e os pedacos seguintes entram NO INICIO,
    # embaralhando o texto. O Enter submete com o cursor na posicao 0 sem problema.
    placeholder = text.startswith("-")
    envio = ("x" + text) if placeholder else text

    pedacos = _fatiar_win(envio) if len(envio) > _WIN_CHUNK else [envio]
    total = len(pedacos)
    entregue = 0
    for n, pedaco in enumerate(pedacos, start=1):
        cp = _run(["tmux", "send-keys", "-t", target, "-l", "--", pedaco])
        if cp.returncode != 0:
            # PARA no primeiro erro: seguir mandando entregaria 1+2+4+5 e o Enter submeteria texto com
            # um buraco no meio — a sessao trataria isso como pedido do usuario, que e exatamente o
            # estrago que o fatiamento existe pra evitar. (Com o bug do '-' o rc vinha 0 e isto nao
            # disparava; por isso a defesa de verdade e o _fatiar_win, nao esta checagem.)
            _log.error("send-keys falhou no pedaco %d/%d de %r (%d de %d chars ja no input): %s",
                       n, total, target, entregue, len(envio), (cp.stderr or "").strip()[:200])
            return False
        entregue += len(pedaco)
        if n < total:
            time.sleep(_WIN_CHUNK_PAUSE)

    if placeholder:
        # Home + DC: cursor pro inicio e apaga o 'x'.
        #
        # LIMITE MEDIDO, e ele importa: isto so funciona em pane cuja TUI honre Home/DC. Na TUI do
        # Claude Code (Ink) funciona — verificado, o composer fica byte-exato e sem residuo. Num pane
        # de SHELL nao: no PowerShell/PSReadLine as duas teclas voltam rc=0 e NAO apagam nada, e o
        # texto fica com um 'x' na frente. O `send_text` do espelho do terminal pode mirar um shell,
        # entao esse caso existe de verdade.
        # Ainda assim vale a troca: sem o placeholder o texto seria descartado INTEIRO e em silencio;
        # com ele, no pior caso, chega com um 'x' visivel na frente. Corrupcao visivel > perda muda.
        # O rc nao serve de gate (volta 0 mesmo sem apagar), entao a checagem e por CAPTURA.
        for tecla in ("Home", "DC"):
            cp = _run(["tmux", "send-keys", "-t", target, tecla])
            if cp.returncode != 0:
                _log.error("send-keys %s falhou pra %r ao remover o placeholder: %s",
                           tecla, target, (cp.stderr or "").strip()[:200])
                return False
        # So no caminho do placeholder (texto comecando com '-', raro) -> a captura extra nao pesa no
        # caminho comum. Nao devolve False: o texto ESTA no input, e um requeue duplicaria a mensagem.
        cap = _run(["tmux", "capture-pane", "-p", "-t", target])
        if cap.returncode == 0 and ("x" + text[:24]) in cap.stdout:
            _log.warning("placeholder 'x' NAO foi removido em %r (a TUI ignora Home/DC — pane de "
                         "shell?): a mensagem entrou com um 'x' na frente", target)
    return True


def send_keys(name: str, keys: str, literal: bool = False) -> bool:
    """False só no caso de envio literal que parou no meio (ver _send_literal). Quem ignora o retorno
    fica com o comportamento de antes."""
    if literal:
        return _send_literal(_pane_target(name), keys)
    # Enter SEMPRE como CR cru (-l), nunca como nome de tecla: com `extended-keys on` no servidor
    # tmux (necessário pro Shift+Enter do Pi), o send-keys "Enter" pode sair codificado no protocolo
    # estendido (CSI-u/modifyOtherKeys) pra apps que o negociaram — e o composer do Claude Code
    # engolia o submit: o texto ficava parado no pane e o send virava 400 "envio incompleto"
    # (regressão medida em 31/07, sessão Config-pi, print do dono). CR cru é o encoding legado que
    # toda TUI aceita como Enter, com ou sem extended-keys.
    # POSIX apenas: no psmux (Windows) o CR literal e engolido dentro de -l (medido — ver
    # buffer_trunca_no_newline: "as linhas chegam GRUDADAS"), e la extended-keys nem e ligado
    # (docs/tmux.conf.windows.example), entao o Enter nomeado segue sendo o caminho que funciona.
    if keys == "Enter" and os.name != "nt":
        return _run(["tmux", "send-keys", "-t", _pane_target(name), "-l", "--", "\r"]).returncode == 0
    return _run(["tmux", "send-keys", "-t", _pane_target(name), keys]).returncode == 0


_TRUNCA_BUFFER: bool | None = None   # cache do probe abaixo (uma vez por processo)


def buffer_trunca_no_newline() -> bool:
    """O multiplexador guarda `\n` dentro de um paste-buffer, ou corta na primeira quebra?

    MEDIDO no psmux 3.3.7 (Windows): `set-buffer -- "ABC\nDEF\nGHI"` devolve rc=0 e grava 3 bytes —
    so o "ABC". Depois o `paste-buffer` tambem devolve rc=0 ENTREGANDO NADA no composer. Como o
    paste_text so caia no fallback quando o rc era != 0, o caminho que FUNCIONA (linha a linha com
    C-j) nunca rodava no Windows: o Enter submetia a primeira linha truncada — ou nada —, o reconcile
    nao achava o texto no transcript e REDIGITAVA, em rajadas de 3 (attempts=2 + _CONFIRM_GRACE de 8s).
    Era isso, e nao um injetor externo, que aparecia como a mesma frase isolada chegando sozinha num
    pane que ninguem estava operando. Diagnostico da sessao-irma, 3/3 reprodutivel.
    O rc do paste-buffer NAO serve de gate: elas reproduziram rc=0 e rc=1 na mesma rodada mudando so o
    nome do buffer. A truncagem do set-buffer e o unico fato estavel, e e nela que este probe se apoia.
    Por CAPACIDADE e nao por nome de SO — mesma regra do procinfo: pergunta ao multiplexador o que ele
    faz. Cacheado: custa 3 chamadas e o comportamento nao muda na vida do processo.
    """
    global _TRUNCA_BUFFER
    if _TRUNCA_BUFFER is None:
        buf, amostra = "cp-probe-nl", "A\nB"
        try:
            _run(["tmux", "set-buffer", "-b", buf, "--", amostra])
            lido = _run(["tmux", "show-buffer", "-b", buf]).stdout
            _run(["tmux", "delete-buffer", "-b", buf])
            _TRUNCA_BUFFER = "B" not in lido
            if _TRUNCA_BUFFER:
                _log.warning("multiplexador TRUNCA paste-buffer na primeira quebra de linha "
                             "(gravou %r de %r) — multi-linha vai direto pro envio linha a linha",
                             lido, amostra)
        except Exception:
            _TRUNCA_BUFFER = False   # best-effort: falhou -> comportamento historico, nunca pior
    return _TRUNCA_BUFFER


def paste_text(name: str, text: str) -> bool:
    """True = o texto foi entregue por completo ao composer (via paste-buffer OU plano B linha a
    linha). False = alguma etapa CONFIRMOU falha no meio — quem chama nao pode mandar Enter nem
    confiar so na leitura da tela pra decidir (achado da review 02/08/2026, CRITICO: antes deste
    conserto o retorno era descartado nos dois pontos que o produzem — aqui e em
    `_paste_linha_a_linha` — e o `send_prompt` so tinha a leitura do pane como prova; com a prova
    por COMECO do texto (ver terminal_input._RESIDUO_INICIO), um envio que parou na linha 2 de 3
    ainda mostra o comeco no composer, `_entrou_no_composer` lia "entrou", o Enter ia, e o
    `send_prompt` devolvia "sent" CALADO pra um texto pela metade. Vale no fallback do Windows
    (psmux sempre cai aqui) e no POSIX quando o paste-buffer falha e cai no mesmo fallback).

    Envia texto MULTI-LINHA pro pane via bracketed paste: set-buffer + paste-buffer -p. O `-p` faz a
    TUI (Ink) receber as quebras como newlines DENTRO do input (não submete cada linha). Buffer
    nomeado (não suja os paste-buffers do usuário) e `-d` apaga depois. Quem submete e o Enter (caller).
    Multiplexador que TRUNCA o buffer na quebra nem tenta o paste-buffer: ali ele devolve rc=0
    mentindo (entrega truncado ou nada), e confiar nesse rc foi o que manteve o fallback DESLIGADO
    no Windows justamente onde ele era necessario. Ver buffer_trunca_no_newline.
    """
    if buffer_trunca_no_newline():
        return _paste_linha_a_linha(name, text)
    buf = "cp-prompt"
    # `load-buffer -` (texto pela STDIN), nao `set-buffer -- <texto>` (texto no argv): o teto de
    # 16344 bytes e do COMPRIMENTO DO COMANDO, e era o set-buffer que o pagava. Medido 08/08/2026:
    # load-buffer aceitou 1,088 MB e o paste-buffer entregou em 0,32s, byte a byte identico e com os
    # marcadores de bracketed paste intactos.
    lb = _run(["tmux", "load-buffer", "-b", buf, "-"], input=text.encode())
    if lb.returncode != 0:
        # NAO segue pro paste-buffer: com o buffer vazio (ou com o conteudo ANTERIOR), o paste
        # colaria texto de outra mensagem no composer. Cai no plano B, que digita linha a linha.
        _log.warning("tmux load-buffer falhou pra %r (%s) — caindo no envio linha a linha",
                     name, (lb.stderr or "").strip()[:200])
        return _paste_linha_a_linha(name, text)
    cp = _run(["tmux", "paste-buffer", "-t", _pane_target(name), "-b", buf, "-p", "-d"])
    if cp.returncode == 0:
        return True
    return _paste_linha_a_linha(name, text)


def _paste_linha_a_linha(name: str, text: str) -> bool:
    """Plano B do multi-linha: uma chamada por linha, com C-j entre elas.

    True = as N linhas foram digitadas por completo. False = parou no meio — PARA no primeiro erro
    (linha ou C-j) em vez de seguir tentando as demais: continuar entregaria um texto com um BURACO
    no meio, que e exatamente o estrago que checar o retorno existe pra evitar (ver `paste_text`).

    Multiplexador sem `paste-buffer` (medido no psmux 3.3.7, o tmux nativo de Windows). Duas coisas
    aprendidas no teste, as duas contra-intuitivas:
      1. o bracketed paste na mao (`send-keys -l` com ESC[200~/201~) NAO resolve — o psmux aceita a
         sequencia mas engole tudo depois do primeiro \n do argumento;
      2. trocar o separador por \r tambem nao: as linhas chegam GRUDADAS numa so.
    O que funciona e nunca ter \n dentro do argumento: C-j (0x0A) vai como TECLA NOMEADA, que a TUI
    insere como quebra sem submeter.

    Custa 2N-1 chamadas pra N linhas, contra 2 do paste-buffer — por isso e plano B e nao padrao.
    NAO roda no Linux: la o paste-buffer devolve 0 e o caller sai antes de chegar aqui.
    """
    alvo = _pane_target(name)
    for i, linha in enumerate(text.split("\n")):
        if i:
            cp = _run(["tmux", "send-keys", "-t", alvo, "C-j"])
            if cp.returncode != 0:
                _log.error("tmux send-keys C-j falhou pra %r no meio do plano B (linha %d): %s",
                           alvo, i, (cp.stderr or "").strip()[:200])
                return False
        if linha:
            # via _send_literal: uma LINHA comprida cai no mesmo teto do Windows (fatia com pausa).
            if not _send_literal(alvo, linha):
                return False
    return True


def pane_scrollback(name: str) -> int:
    """Linhas de scrollback que o tmux REALMENTE tem pra este pane (0 = nenhuma).

    Um TUI de tela cheia (Claude Code) roda na TELA ALTERNADA, e nela o tmux nao acumula historico:
    `alternate_on=1` -> `history_size=0`, e `capture-pane -S -N` nunca devolve mais que a tela
    visivel por mais linhas que se peca. A TUI do Codex sobe com `--no-alt-screen`, entao ali o
    scrollback existe de verdade. Quem desenha a UI precisa saber a diferenca pra nao oferecer
    "carregar mais historico" onde nao ha historico nenhum.
    """
    cp = _run(["tmux", "display", "-p", "-t", _pane_target(name),
               "#{?alternate_on,0,#{history_size}}"])
    if cp.returncode != 0:
        # 0 aqui significaria "nao ha historico" — e tmux QUEBRADO (ausente, sem permissao, travado
        # no timeout, sessao zumbi) daria a MESMA resposta, escondendo a falha atras de uma UI que
        # so some com o botao. Ainda devolve 0 (a UI nao pode explodir), mas nao em silencio.
        _log.warning("tmux display falhou pra %r: %s", name, (cp.stderr or "").strip()[:200])
        return 0
    out = cp.stdout.strip()
    return int(out) if out.isdigit() else 0


def capture_pane(name: str, lines: int = 200) -> str:
    cp = _run(["tmux", "capture-pane", "-p", "-t", _pane_target(name), "-S", f"-{lines}"])
    if cp.returncode != 0:
        # stdout vazio numa falha e indistinguivel de um pane genuinamente vazio -> o /pane devolvia
        # 200 com texto "" e ninguem ficava sabendo que o tmux tinha falhado. Devolve "" (o caller
        # degrada), mas registra.
        _log.warning("tmux capture-pane falhou pra %r: %s", name, (cp.stderr or "").strip()[:200])
    return cp.stdout


def pane_pid(name: str) -> int | None:
    # PID do processo raiz do pane (shell ou o proprio claude). Ponto de partida pra achar qual
    # transcript .jsonl o claude da sessao tem aberto (resolucao autoritativa, nao newest-by-mtime).
    #
    # `display -p` resolve alvo de PANE exato; `list-panes -t %N` resolveria a JANELA dele e
    # devolveria o primeiro pane dela (achado do pass adversarial).
    cp = _run(["tmux", "display", "-p", "-t", _pane_target(name), "#{pane_pid}"])
    out = cp.stdout.strip()
    return int(out) if cp.returncode == 0 and out.isdigit() else None
