#!/usr/bin/env python3
"""Prova de fogo do psmux (tmux nativo de Windows) contra o contrato do backend.

Roda NUMA MAQUINA WINDOWS, com `claude` instalado e JA LOGADO. Nao precisa do repo
inteiro nem do backend: e stdlib pura, um arquivo so.

    py scripts\\test-psmux.py

O que ele testa: TODAS as chamadas que backend/app/tmux.py faz, flag por flag —
nao so as suspeitas. Se uma passar aqui, aquela funcao do tmux.py roda no Windows
sem editar nada.

O que ele NAO testa (e nao da pra testar daqui): o parser de tela do state.py.
Por isso ele SALVA os quadros capturados em test-psmux-frames.txt — mande esse
arquivo de volta e o classify() roda contra ele do lado Linux.

Saida: uma linha PASS/FAIL por checagem + um resumo. Exit 0 se tudo passou.
"""
import os
import subprocess
import sys
import time

SESS = "cptest"
COLS, ROWS = 200, 50
FRAMES = "test-psmux-frames.txt"
# Comando do pane. Trocavel pra (a) testar `codex` no lugar do claude e (b) rodar o proprio
# script contra o tmux ORIGINAL no Linux com um comando bobo — se ele falhar la, o bug e do
# script, nao do psmux.
CMD = os.environ.get("CP_TEST_CMD", "claude")
# Multiplicador de espera. WinBoat/VM desenha a TUI bem mais devagar que bare metal —
# `CP_TEST_SLOW=3` triplica todas as pausas em vez de virar FAIL por impaciencia.
SLOW = float(os.environ.get("CP_TEST_SLOW", "1"))


def espera(seg: float) -> None:
    time.sleep(seg * SLOW)

_falhas: list[str] = []
_detalhes: list[tuple[str, str]] = []
_frames: list[tuple[str, str]] = []


def run(args: list[str], timeout: int = 10) -> subprocess.CompletedProcess:
    try:
        return subprocess.run([BIN, *args], capture_output=True, text=True,
                              timeout=timeout, encoding="utf-8", errors="replace")
    except (subprocess.TimeoutExpired, OSError) as e:
        return subprocess.CompletedProcess(args, 1, stdout="", stderr=str(e))


def check(nome: str, ok: bool, detalhe: str = "") -> bool:
    print(f"  {'PASS' if ok else 'FAIL'}  {nome}")
    if not ok:
        _falhas.append(nome)
        if detalhe:
            for linha in detalhe.strip().splitlines()[:4]:
                print(f"        {linha}")
            # O detalhe tambem vai pro arquivo: no terminal ele sobe no scroll e some,
            # e ai a analise depende de alguem rolar a tela atras da linha certa.
            _detalhes.append((nome, detalhe))
    return ok


def frame(rotulo: str, texto: str) -> None:
    _frames.append((rotulo, texto))


def achar_binario() -> str:
    # psmux instala os tres nomes; `tmux` e o que o backend chama.
    for nome in ("tmux", "psmux", "pmux"):
        cp = subprocess.run([nome, "-V"], capture_output=True, text=True)
        if cp.returncode == 0:
            print(f"binario: {nome} -> {cp.stdout.strip()}")
            return nome
    sys.exit("nenhum de tmux/psmux/pmux respondeu a -V. Instale: winget install psmux")


BIN = achar_binario()
alvo = f"={SESS}:"          # alvo de PANE (send-keys, capture-pane, display)
alvo_s = f"={SESS}"         # alvo de SESSAO (has-session)

run(["kill-session", "-t", SESS])   # limpa sobra de rodada anterior

# ── 1. new-session com -e (variaveis de ambiente no pane) ────────────────────
print("\n1. new_session")
cp = run(["new-session", "-d", "-s", SESS, "-c", os.getcwd(),
          "-x", str(COLS), "-y", str(ROWS),
          "-e", "COLORTERM=truecolor",
          "-e", "CLAUDE_CODE_TMUX_TRUECOLOR=1",
          CMD])
if not check(f"new-session -d -s -c -x -y -e ... {CMD}", cp.returncode == 0,
             cp.stderr or cp.stdout):
    # ponytail: sem sessao nao ha o que testar adiante. Falha alta, nao 12 FAILs em cascata.
    sys.exit("\nnew-session falhou — o resto do teste depende dela. Saida acima.")

# `exec` de proposito AUSENTE acima: no Linux o tmux.py precisa dele (senao o fish
# fica dono do tty e o send-keys nao chega). No Windows nao ha shell no meio, e no
# Cygwin o exec e justamente o que impede o ConPTY de nascer. Se o psmux exigir
# `exec claude`, a checagem de pane_pid la embaixo denuncia.

espera(6)   # a TUI do Claude leva alguns segundos ate desenhar


def captura() -> str:
    """capture-pane com aviso alto se o pane morreu.

    Sem isto, um claude que saiu no meio devolve tela vazia e TODAS as checagens
    seguintes falham em cascata, apontando pro multiplexador quando a culpa foi de
    quem digitou. Aconteceu: o Escape do passo 9 caiu na tela de confianca, que o
    le como "Esc to cancel".
    """
    texto = run(["capture-pane", "-p", "-t", alvo, "-S", "-200"]).stdout
    if not texto.strip() and run(["has-session", "-t", alvo_s]).returncode != 0:
        print("  !!  a sessao MORREU — as checagens abaixo nao valem nada")
    return texto


# ── 1b. tela de confianca da pasta (e, de quebra, o picker de opcao) ────────
# Pasta nova -> o Claude Code abre "Is this a project you trust?" com um picker
# ❯ 1./2. ANTES do composer. Sem responder, o resto do teste digita dentro do
# picker: o texto nao entra e o Escape CANCELA. Responder aqui testa de graca o
# caminho de opcao do terminal_input.py.
print("\n1b. tela de confianca / picker de opcao")
tela = captura()
if "trust this folder" in tela.lower() or "❯ 1." in tela:
    frame("picker de confianca (esperado: ❯ 1. ... / 2. ...)", tela)
    run(["send-keys", "-t", alvo, "Enter"])
    espera(4)
    tela = captura()
    check("Enter respondeu o picker de opcao (saiu da tela de confianca)",
          "trust this folder" not in tela.lower(),
          f"continua na tela de confianca:\n{tela[:400]}")
    espera(3)   # a TUI real ainda desenha depois que o picker sai
else:
    print("  --  sem tela de confianca (pasta ja confiada)")

# ── 2. has-session, com e sem match exato ───────────────────────────────────
print("\n2. has_session")
check("has-session -t =NOME (exato, sessao viva)",
      run(["has-session", "-t", alvo_s]).returncode == 0)
check("has-session -t =NOME nao casa por PREFIXO",
      run(["has-session", "-t", f"={SESS[:-1]}"]).returncode != 0,
      "sem match exato, 'cptest-2' viva responderia VIVO para 'cptest' — o app "
      "fabrica nomes que colidem por prefixo (<base>, <base>-2, ...)")

# ── 3. list-sessions -F ─────────────────────────────────────────────────────
print("\n3. list_sessions")
cp = run(["list-sessions", "-F", "#{session_name}\t#{pane_current_path}"])
linhas = [l for l in cp.stdout.splitlines() if l.strip()]
check("list-sessions -F '#{session_name}\\t#{pane_current_path}'",
      cp.returncode == 0 and any(l.startswith(SESS + "\t") and "\t" in l for l in linhas),
      cp.stderr or repr(cp.stdout[:200]))

# ── 4. list-panes -a -F com 5 campos ────────────────────────────────────────
print("\n4. list_panes_active")
cp = run(["list-panes", "-a", "-F",
          "#{session_name}\t#{pane_active}\t#{pane_pid}\t#{pane_current_path}\t#{pane_id}"])
campos = [l.split("\t") for l in cp.stdout.splitlines() if l.strip()]
nossa = [c for c in campos if c and c[0] == SESS]
check("list-panes -a -F (5 campos)", cp.returncode == 0 and bool(nossa),
      cp.stderr or repr(cp.stdout[:200]))
if nossa:
    c = nossa[0]
    check("  campos completos (pane_active/pane_pid/pane_id)",
          len(c) == 5 and c[1] in ("0", "1") and c[2].isdigit() and c[4].startswith("%"),
          f"recebido: {c!r}")

# ── 4b. list-panes -a -F com 6 campos (tmux.list_panes_all) ─────────────────
# O `-F` REAL do app tem um 6o campo: `#{@cp_hidden}`, uma opcao de USUARIO (tmux set-option), nao
# uma variavel de formato conhecida. E dela que a lista das tres views, o list_with_state, o
# _pane_info e o _cwd_has_siblings dependem — se o multiplexador RECUSAR o comando por causa da
# interpolacao (rc != 0), o app fica com ZERO sessao, e nao so sem o painel de terminal. O parse do
# tmux.py ja aceita 5 campos ou mais (campo vazio / literal cru = "nao escondida"); o que so a
# sonda mede e se o comando roda.
print("\n4b. list_panes_all (6 campos, com a opcao de usuario @cp_hidden)")
run(["set-option", "-t", alvo, "@cp_hidden", "1"])
cp = run(["list-panes", "-a", "-F",
          "#{session_name}\t#{pane_active}\t#{pane_pid}\t#{pane_current_path}\t#{pane_id}"
          "\t#{@cp_hidden}"])
campos6 = [l.split("\t") for l in cp.stdout.splitlines() if l.strip()]
nossa6 = [c for c in campos6 if c and c[0] == SESS]
check("list-panes -a -F com '#{@cp_hidden}' NAO e recusado", cp.returncode == 0 and bool(nossa6),
      "rc != 0 aqui = TODA sessao some das tres views do app (nao so o shell escondido): "
      + (cp.stderr or repr(cp.stdout[:200])))
if nossa6:
    c = nossa6[0]
    check("  os 5 primeiros campos continuam intactos",
          len(c) >= 5 and c[1] in ("0", "1") and c[2].isdigit() and c[4].startswith("%"),
          f"recebido: {c!r}")
    # A marca em si e um EXTRA: sem ela o shell do botao `+` vira card na lista (chato), mas o app
    # inteiro continua de pe. Por isso e print, nao check.
    print(f"  --  6o campo = {c[5]!r} " + ("(marca lida -> shell escondido funciona)"
                                           if len(c) > 5 and c[5] == "1" else
                                           "(marca NAO lida -> o shell escondido apareceria "
                                           "como card; o resto do app segue normal)"))
run(["set-option", "-t", alvo, "-u", "@cp_hidden"])

# ── 5. pane_pid ─────────────────────────────────────────────────────────────
print("\n5. pane_pid")
cp = run(["list-panes", "-t", alvo, "-F", "#{pane_pid}"])
pids = [l.strip() for l in cp.stdout.splitlines() if l.strip().isdigit()]
check("list-panes -t =NOME: -F '#{pane_pid}'", cp.returncode == 0 and bool(pids),
      cp.stderr or repr(cp.stdout[:200]))
if pids:
    # O pane_pid tem que ser o claude, nao um shell que sobrou por cima. E disso que
    # o registry depende pra achar o transcript .jsonl da sessao.
    try:
        tl = subprocess.run(["tasklist", "/FI", f"PID eq {pids[0]}", "/NH"],
                            capture_output=True, text=True, errors="replace").stdout
    except OSError as e:
        tl = f"<tasklist indisponivel: {e}>"
    dono = tl.strip().split()[0] if tl.strip() else "?"
    print(f"  --  pane_pid {pids[0]} = {dono}")
    if CMD.lower() in tl.lower():
        check(f"  pane_pid E o proprio {CMD}", True)
    else:
        # No psmux o pane e o shell hospedeiro e o claude e FILHO dele — o registry.py
        # ja faz essa caminhada pai->filhos no Linux, so que aqui ela e sempre necessaria.
        # O que precisa ser verdade e o claude estar UM NIVEL abaixo; se nem isso, o
        # transcript da sessao fica inalcancavel a partir do pane.
        # wmic saiu do Windows 11 recente (deprecado e removido) -> CIM pelo PowerShell.
        try:
            f = subprocess.run(
                ["powershell", "-NoProfile", "-Command",
                 f"Get-CimInstance Win32_Process -Filter 'ParentProcessId={pids[0]}'"
                 " | ForEach-Object { $_.Name }"],
                capture_output=True, text=True, errors="replace").stdout
        except OSError as e:
            f = f"<powershell indisponivel: {e}>"
        check(f"  {CMD} e FILHO do pane_pid (registry precisa descer 1 nivel)",
              CMD.lower() in f.lower(),
              f"pane_pid {pids[0]} = {dono}; filhos: {' '.join(f.split())[:200]}")

# ── 6. display -p com formato CONDICIONAL ───────────────────────────────────
print("\n6. pane_scrollback")
cp = run(["display", "-p", "-t", alvo, "#{?alternate_on,0,#{history_size}}"])
saida = cp.stdout.strip()
check("display -p '#{?alternate_on,0,#{history_size}}' (formato condicional)",
      cp.returncode == 0 and saida.isdigit(),
      cp.stderr or f"esperado um numero, veio: {saida!r}")

# ── 7. capture-pane ─────────────────────────────────────────────────────────
print("\n7. capture_pane")
cp = run(["capture-pane", "-p", "-t", alvo, "-S", "-200"])
tela = cp.stdout
check("capture-pane -p -t =NOME: -S -200", cp.returncode == 0 and len(tela.strip()) > 0,
      cp.stderr or "tela vazia")
check("  a TUI do Claude desenhou (nao so cursor piscando)",
      any(m in tela for m in ("Claude", "claude", "?", ">")) and len(tela.strip()) > 40,
      f"primeiras linhas:\n{tela[:300]}")
frame("apos subir o claude (esperado: idle)", tela)

# ── 8. send-keys literal + Enter ────────────────────────────────────────────
print("\n8. send_keys")
marca = "psmux-probe-123"
check("send-keys -l -- <texto>",
      run(["send-keys", "-t", alvo, "-l", "--", marca]).returncode == 0)
espera(1.5)
tela = run(["capture-pane", "-p", "-t", alvo, "-S", "-200"]).stdout
check("  o texto digitado apareceu no pane", marca in tela,
      f"nao achei {marca!r} na tela:\n{tela[-500:]}")
frame("texto digitado, ainda nao submetido", tela)

# ── 9. set-buffer + paste-buffer -p (bracketed paste, multi-linha) ──────────
print("\n9. paste_text")
run(["send-keys", "-t", alvo, "Escape"])   # limpa o composer antes
espera(0.5)
multi = "linha-um-abc\nlinha-dois-xyz"
ok_buf = run(["set-buffer", "-b", "cp-prompt", "--", multi]).returncode == 0
check("set-buffer -b cp-prompt -- <multi-linha>", ok_buf)
check("paste-buffer -t =NOME: -b cp-prompt -p -d",
      run(["paste-buffer", "-t", alvo, "-b", "cp-prompt", "-p", "-d"]).returncode == 0)
espera(1.5)
tela = run(["capture-pane", "-p", "-t", alvo, "-S", "-200"]).stdout
check("  as DUAS linhas entraram no composer",
      "linha-um-abc" in tela and "linha-dois-xyz" in tela,
      f"tela:\n{tela[-600:]}")
# O "-p nao submeteu a primeira linha" NAO da pra afirmar por contagem: o capture traz
# scrollback, entao o texto aparecer 2x e normal e nao prova nada. Fica como quadro pra
# olho humano — se o bracketed paste falhar, o quadro mostra o Claude JA RESPONDENDO a
# "linha-um-abc" com "linha-dois-xyz" sozinha no composer.
frame("multi-linha colada via paste-buffer (JULGAR: 2 linhas? nada enviado?)", tela)

# ── 9b. plano B do multi-linha: bracketed paste na mao, sem buffer ──────────
# `paste-buffer -p` nao e magica: e o texto entre ESC[200~ e ESC[201~. Dando pra
# mandar isso por send-keys -l, o paste_text() do tmux.py deixa de depender de
# set-buffer/paste-buffer — e o mesmo codigo serve no tmux original e no psmux.
if "linha-um-abc" not in tela:
    print("\n9b. multi-linha por bracketed paste manual (plano B)")
    run(["send-keys", "-t", alvo, "Escape"])
    espera(0.5)
    bp = "\x1b[200~" + "linha-tres-def\nlinha-quatro-ghi" + "\x1b[201~"
    check("send-keys -l -- <ESC[200~ ... ESC[201~>",
          run(["send-keys", "-t", alvo, "-l", "--", bp]).returncode == 0)
    espera(1.5)
    tela = captura()
    check("  as DUAS linhas entraram (plano B)",
          "linha-tres-def" in tela and "linha-quatro-ghi" in tela,
          f"tela:\n{tela[-600:]}")
    frame("multi-linha via bracketed paste manual (plano B)", tela)

    # ── 9c. de onde vem a quebra: o \n no argumento do send-keys ────────────
    # No plano B a 1a linha entrou e a 2a sumiu, com os marcadores ESC[200~/201~
    # consumidos sem virar lixo. Logo o psmux ACEITA a sequencia e engasga no \n.
    # Dois jeitos de nao ter \n nenhum no argumento:
    #   C) separador \r dentro do bracketed paste — dentro dele o TUI insere quebra
    #      em vez de submeter, que e a razao de o bracketed paste existir;
    #   D) uma chamada por linha, com C-j (0x0A) entre elas como tecla NOMEADA.
    # Qual passar vira o paste_text() do tmux.py no Windows.
    print("\n9c. de onde vem a quebra do multi-linha")

    run(["send-keys", "-t", alvo, "C-u"])   # limpa o composer (Escape nao limpou)
    espera(0.8)
    bp_r = "\x1b[200~" + "linha-cinco-jkl\rlinha-seis-mno" + "\x1b[201~"
    run(["send-keys", "-t", alvo, "-l", "--", bp_r])
    espera(1.5)
    tela = captura()
    # "as duas strings estao na tela" NAO basta: medido, o \r e engolido e as linhas
    # chegam GRUDADAS ("linha-cinco-jkllinha-seis-mno"), o que passaria num teste frouxo
    # e entregaria prompt corrompido. Exige a quebra de verdade.
    check("C) bracketed paste com \\r no lugar do \\n",
          "linha-cinco-jkl" in tela and "linha-seis-mno" in tela
          and "linha-cinco-jkllinha-seis-mno" not in tela,
          f"tela:\n{tela[-500:]}")
    frame("plano C: bracketed paste separado por \\r", tela)

    run(["send-keys", "-t", alvo, "C-u"])
    espera(0.8)
    run(["send-keys", "-t", alvo, "-l", "--", "linha-sete-pqr"])
    run(["send-keys", "-t", alvo, "C-j"])   # 0x0A como tecla, nunca dentro do argumento
    run(["send-keys", "-t", alvo, "-l", "--", "linha-oito-stu"])
    espera(1.5)
    tela = captura()
    check("D) uma chamada por linha, com C-j entre elas",
          "linha-sete-pqr" in tela and "linha-oito-stu" in tela,
          f"tela:\n{tela[-500:]}")
    check("  D) e o C-j NAO submeteu (as duas linhas seguem no composer)",
          "linha-sete-pqr" in tela and "linha-oito-stu" in tela
          and "✻" not in tela.split("linha-sete-pqr")[-1],
          "se submeteu, a 1a linha virou prompt e o claude comecou a responder")
    frame("plano D: uma chamada por linha, C-j entre elas", tela)

# ── 10. teclas nomeadas (picker de opcao) ──────────────────────────────────
print("\n10. teclas nomeadas")
for tecla in ("Down", "Up", "Escape", "Enter"):
    check(f"send-keys {tecla}", run(["send-keys", "-t", alvo, tecla]).returncode == 0)

# ── 11. estado 'working' — o quadro que mais importa pro state.py ──────────
print("\n11. quadro do estado working")
run(["send-keys", "-t", alvo, "Escape"])
espera(0.5)
run(["send-keys", "-t", alvo, "-l", "--", "conte ate 200 devagar, um numero por linha"])
run(["send-keys", "-t", alvo, "Enter"])
espera(5)
tela = run(["capture-pane", "-p", "-t", alvo, "-S", "-200"]).stdout
glifos = "✻✽✶✺✢·∗✳✦✧"
check("  o spinner do Claude aparece no capture (glifo preservado)",
      any(g in tela for g in glifos),
      "sem glifo de spinner o state.py nunca classifica 'working'. "
      f"ultimas linhas:\n{tela[-400:]}")
frame("claude trabalhando (esperado: working + spinner)", tela)

# ── 12. rename + kill ──────────────────────────────────────────────────────
print("\n12. rename_session / kill_session")
check("rename-session", run(["rename-session", "-t", SESS, SESS + "b"]).returncode == 0)
run(["rename-session", "-t", SESS + "b", SESS])
check("kill-session", run(["kill-session", "-t", SESS]).returncode == 0)
espera(1)
check("  sessao sumiu depois do kill", run(["has-session", "-t", alvo_s]).returncode != 0)

# ── Resultado ──────────────────────────────────────────────────────────────
with open(FRAMES, "w", encoding="utf-8") as fh:
    if _detalhes:
        fh.write(f"{'=' * 70}\n### DETALHE DAS FALHAS\n{'=' * 70}\n")
        for nome, detalhe in _detalhes:
            fh.write(f"\n--- {nome}\n{detalhe}\n")
    for rotulo, texto in _frames:
        fh.write(f"\n{'=' * 70}\n### {rotulo}\n{'=' * 70}\n{texto}\n")

print(f"\n{'-' * 60}")
print(f"quadros de tela salvos em {FRAMES} ({len(_frames)} quadros)")
if _falhas:
    print(f"\n{len(_falhas)} FALHA(S):")
    for f in _falhas:
        print(f"  - {f}")
    sys.exit(1)
print("\nTUDO PASSOU — o tmux.py roda sobre psmux sem edicao.")
print(f"Mande o {FRAMES} pra validar o parser de tela do state.py.")
