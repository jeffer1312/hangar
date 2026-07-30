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


def _run(args: list[str]) -> subprocess.CompletedProcess:
    # timeout: tmux travado nao pode prender o event loop / worker do threadpool pra sempre. Estouro ->
    # trata como falha (returncode=1), igual ao tmux recusar; os callers ja checam returncode != 0.
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
    return f"={name}:"


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


def has_session(name: str) -> bool:
    # `=NAME`: match EXATO, mesma pegadinha do _pane_target acima. Sem o `=`, o target-session do tmux
    # cai em exact -> fnmatch -> PREFIX match: com "pocket-2" viva, `has_session("pocket")` respondia
    # VIVO pra uma sessao que NUNCA existiu. Como o app fabrica nomes que colidem por prefixo
    # (`<base>`, `<base>-2`, `<base>-3`...), a sessao morta herdava o "vivo" da irma -> o /input
    # confirmava "entregue" digitando num pane inexistente e o state.py nunca marcava `dead`.
    # Sem `:` aqui (ao contrario do _pane_target): has-session so resolve SESSAO, nao pane/window.
    return _run(["tmux", "has-session", "-t", f"={name}"]).returncode == 0


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
    return _run(args).returncode == 0


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
    return not has_session(name)


def rename_session(old: str, new: str) -> bool:
    return _run(["tmux", "rename-session", "-t", old, new]).returncode == 0


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
# Quanto a fronteira pode avancar ALEM do chunk pra fugir de um hifen. Cabe porque o teto real e o
# colapso de paste (medido: 700 ok, 900 colapsa), nao os 512 — entao 180 mantem o pedaco em <=692,
# ainda abaixo do colapso.
_WIN_AVANCO = 180
# Placeholder da receita do hifen: uma letra qualquer que faca o argumento NAO comecar com "-".
_PLACEHOLDER = "x"


def _fronteiras(text: str) -> list[tuple[int, int]]:
    """Corta em pedacos de _WIN_CHUNK, mas NUNCA deixa um pedaco COMECAR com "-".

    O psmux nao honra o `--`: argumento que comeca com "-" e engolido em SILENCIO, com rc=0 e stderr
    vazio (medido pela sessao-irma em 5 casos; "controle x" e " - com espaco antes" chegam, "- direto"
    e "--algo" somem). O teste e o PRIMEIRO caractere.
    Prova de que isso atinge o fatiamento: um recado meu de 2332 chars chegou com 1820 — faltando
    EXATAMENTE 512, o tamanho do chunk, com o buraco alinhado em [512:1024] e o chunk 2 comecando com
    "- trunca no primeiro". As duas entregas perderam os MESMOS bytes, o que descarta perda aleatoria.
    E como o rc e 0, NENHUMA checagem nossa pega: nem o returncode por pedaco, nem a evidencia no
    composer (o pedaco some, o resto chega, e a cauda aparece normalmente).
    Conserto: puxar a fronteira 1 char pra tras ate o proximo pedaco comecar com outra coisa. O texto
    entregue continua BYTE-EXATO — muda so onde a divisao cai.
    Texto que COMECA com "-" na posicao 0 nao tem como ser dividido pra fora do problema; isso e
    exposicao PRE-EXISTENTE do send-keys (nao do fatiamento) e esta anotada pra medicao do lado
    Windows: ver se o psmux aceita `send-keys -H` (hex), que nao passa o texto por argv.
    ponytail: recuo simples de 1 em 1 com teto; em texto que seja uma parede de "-" o teto devolve o
    corte original (pior caso = comportamento de hoje, nunca pior).
    """
    cortes: list[tuple[int, int]] = []
    i = 0
    while i < len(text):
        fim = min(i + _WIN_CHUNK, len(text))
        if fim < len(text) and text[fim] == "-":
            # 1) pra TRAS primeiro: mantem o pedaco <= _WIN_CHUNK, que e o teto seguro conhecido.
            tras = fim
            while tras > i + 1 and text[tras] == "-":
                tras -= 1
            # 2) nao achou saida perto (regua markdown "-----", lista com varios itens seguidos —
            #    caso COMUM em conversa tecnica, e por isso o recuo sozinho nao basta): tenta pra
            #    FRENTE. Cabe porque o teto real nao e 512 e sim o COLAPSO de paste, medido em
            #    700 ok / 900 colapsa — ~190 chars de folga. Medicao da sessao-irma no Windows.
            if text[tras] == "-":
                frente = fim
                while frente < len(text) and text[frente] == "-" and frente - fim < _WIN_AVANCO:
                    frente += 1
                fim = frente if frente < len(text) and text[frente] != "-" else fim
            else:
                fim = tras
        cortes.append((i, fim))
        i = fim
    return cortes


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
    # Texto que COMECA com hifen: o psmux engole o argumento inteiro em silencio (rc=0), e isso NAO
    # depende do fatiamento — mensagem curta de uma chamada so ja se perdia. Como nao ha fronteira pra
    # mover no pedaco 1, vale a receita medida e validada pela sessao-irma no psmux: digita um
    # PLACEHOLDER antes, manda o texto, e no fim volta pro inicio (Home) e apaga o placeholder (DC).
    # Home+DC tem de ser o ULTIMO passo antes do Enter: feito antes, o cursor fica no comeco e o resto
    # do texto entra embaralhado.
    if os.name == "nt" and text.startswith("-"):
        _run(["tmux", "send-keys", "-t", target, "-l", "--", _PLACEHOLDER])
        ok = _enviar_pedacos(target, text)
        _run(["tmux", "send-keys", "-t", target, "Home"])
        _run(["tmux", "send-keys", "-t", target, "DC"])
        return ok
    return _enviar_pedacos(target, text)


def _enviar_pedacos(target: str, text: str) -> bool:
    """O envio em si, sem a receita do placeholder \u2014 separado pra ela envolver o envio INTEIRO
    (o Home+DC precisa rodar depois do ultimo pedaco, nao entre eles)."""
    # Linux (qualquer tamanho) e Windows dentro do teto: UMA chamada, comportamento de sempre.
    # Windows acima do teto: fatia com pausa pra nao disparar o modo paste da TUI que come o comeco.
    if os.name != "nt" or len(text) <= _WIN_CHUNK:
        cp = _run(["tmux", "send-keys", "-t", target, "-l", "--", text])
        if cp.returncode != 0:
            # Uma chamada so: falhou = NADA entrou (nao ha meia mensagem no input). Registra e devolve
            # False — sem log isso seria indistinguivel de entrega, porque `send_prompt` diria "sent".
            _log.warning("tmux send-keys -l falhou pra %r: %s",
                         target, (cp.stderr or "").strip()[:200])
            return False
        return True
    total = (len(text) + _WIN_CHUNK - 1) // _WIN_CHUNK
    for n, (i, fim) in enumerate(_fronteiras(text), start=1):
        cp = _run(["tmux", "send-keys", "-t", target, "-l", "--", text[i:fim]])
        if cp.returncode != 0:
            # PARA no primeiro erro: seguir mandando os pedacos seguintes entregaria 1+2+4+5 e o Enter
            # submeteria texto com um buraco no meio — a sessao trataria isso como pedido do usuario,
            # que e exatamente o estrago que o fatiamento existe pra evitar.
            _log.error("send-keys falhou no pedaco %d/%d de %r (%d de %d chars ja no input): %s",
                       n, total, target, i, len(text), (cp.stderr or "").strip()[:200])
            return False
        if fim < len(text):
            time.sleep(_WIN_CHUNK_PAUSE)
    return True


def send_keys(name: str, keys: str, literal: bool = False) -> bool:
    """False só no caso de envio literal que parou no meio (ver _send_literal). Quem ignora o retorno
    fica com o comportamento de antes."""
    if literal:
        return _send_literal(_pane_target(name), keys)
    return _run(["tmux", "send-keys", "-t", _pane_target(name), keys]).returncode == 0


_TRUNCA_BUFFER: bool | None = None   # cache do probe abaixo (uma vez por processo)


def buffer_trunca_no_newline() -> bool:
    """O multiplexador guarda `\\n` dentro de um paste-buffer, ou corta na primeira quebra?

    MEDIDO no psmux 3.3.7 (Windows): `set-buffer -- "ABC\\nDEF\\nGHI"` devolve rc=0 e grava 3 bytes —
    so o "ABC". Depois o `paste-buffer` tambem devolve rc=0 ENTREGANDO NADA no composer. Como o
    paste_text so caia no fallback quando o rc era != 0, o caminho que FUNCIONA (linha a linha com
    C-j) nunca rodava no Windows: o Enter submetia a primeira linha truncada — ou nada —, o
    reconcile nao achava o texto no transcript e REDIGITAVA, gerando rajadas de 3 entregas da MESMA
    primeira linha, ~8s entre elas (_CONFIRM_GRACE). Era isso, e nao um injetor externo, que
    aparecia como frase isolada repetida no pane. Diagnostico da sessao-irma no Windows, 3/3
    reprodutivel em sessao descartavel.
    Por CAPACIDADE e nao por nome de SO — mesma regra do _send_literal/procinfo: pergunta ao
    multiplexador o que ele faz, em vez de assumir pelo sistema. Um tmux que um dia passe a truncar
    (ou um psmux que conserte) e tratado certo sem ninguem tocar no codigo.
    Cacheado: o probe custa 3 chamadas e o comportamento nao muda durante a vida do processo.
    """
    global _TRUNCA_BUFFER
    if _TRUNCA_BUFFER is None:
        buf, amostra = "cp-probe-nl", "A\nB"
        try:
            _run(["tmux", "set-buffer", "-b", buf, "--", amostra])
            lido = _run(["tmux", "show-buffer", "-b", buf]).stdout
            _run(["tmux", "delete-buffer", "-b", buf])
            # Compara o CONTEUDO, nao o rc: o rc mente nos dois passos. show-buffer costuma devolver
            # com \n final; o que importa e se o "B" (depois da quebra) sobreviveu.
            _TRUNCA_BUFFER = "B" not in lido
            if _TRUNCA_BUFFER:
                _log.warning("multiplexador TRUNCA paste-buffer na primeira quebra de linha "
                             "(gravou %r de %r) — multi-linha vai direto pro envio linha a linha",
                             lido, amostra)
        except Exception:
            # Probe e best-effort: falhou -> assume o comportamento historico (nao trunca). Pior caso
            # e continuar como antes deste conserto, nunca pior que antes.
            _TRUNCA_BUFFER = False
    return _TRUNCA_BUFFER


def paste_text(name: str, text: str) -> None:
    # Envia texto MULTI-LINHA pro pane via bracketed paste: set-buffer + paste-buffer -p. O `-p` faz a
    # TUI (Ink) receber as quebras como newlines DENTRO do input (não submete cada linha). Buffer
    # nomeado (não suja os paste-buffers do usuário) e `-d` apaga depois. Quem submete e o Enter (caller).
    #
    # Multiplexador que TRUNCA o buffer na quebra de linha nem tenta o paste-buffer: ali ele devolve
    # rc=0 mentindo (entrega truncado ou nada), e confiar no rc foi o que manteve o fallback desligado
    # no Windows. Ver buffer_trunca_no_newline.
    if buffer_trunca_no_newline():
        _paste_linha_a_linha(name, text)
        return
    buf = "cp-prompt"
    _run(["tmux", "set-buffer", "-b", buf, "--", text])
    cp = _run(["tmux", "paste-buffer", "-t", _pane_target(name), "-b", buf, "-p", "-d"])
    if cp.returncode == 0:
        return
    _paste_linha_a_linha(name, text)


def _paste_linha_a_linha(name: str, text: str) -> None:
    """Plano B do multi-linha: uma chamada por linha, com C-j entre elas.

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
            _run(["tmux", "send-keys", "-t", alvo, "C-j"])
        if linha:
            # via _send_literal: uma LINHA comprida cai no mesmo teto do Windows (fatia com pausa).
            _send_literal(alvo, linha)


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
    cp = _run(["tmux", "list-panes", "-t", _pane_target(name), "-F", "#{pane_pid}"])
    if cp.returncode != 0:
        return None
    for line in cp.stdout.splitlines():
        if line.strip().isdigit():
            return int(line.strip())
    return None
