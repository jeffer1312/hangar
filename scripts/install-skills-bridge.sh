#!/usr/bin/env bash
# Espelha o ferramental do Claude Code nos outros agentes: skills, persona (CLAUDE.md) e hooks.
#
# POR QUE ISTO MORA NO HANGAR. Quem cria sessão Pi, Kimi e Codex é o app
# (`hangar-send --new … --provider pi`), e uma sessão nascida assim NÃO enxerga o que a mesma
# máquina já tem: cada agente varre o próprio diretório. Sem esta ponte, a sessão que o app abriu
# começa sem skill, sem a persona e sem os hooks — e o `scripts/checar-skills.sh` deste repo
# denuncia parte disso ("falta o symlink em ~/.pi/agent/skills-bridge") sem resolver.
#
# COMO ACRESCENTAR UM AGENTE NOVO: uma entrada em `AGENTES`, abaixo. Nada mais. O que varia entre
# eles é declarado ali (onde é a raiz, onde vão as skills, com que nome ele lê a persona, o que ele
# já varre sozinho); o resto do script não sabe o nome de agente nenhum. Até 29/08/2026 os symlinks
# de persona do Pi e do Kimi eram feitos À MÃO — numa máquina reinstalada eles se perdiam calados.
#
# O que ele NÃO faz: não instala skill nenhuma, não baixa nada, não mexe no `settings.json` do
# Claude. Só cria symlink e gera o `hooks.json` do Codex. Rodar duas vezes não muda nada.
#
# Idempotente e seguro de rodar sem os outros agentes instalados: cada ponte só é tocada se a raiz
# daquele agente existir.
set -euo pipefail

SETTINGS="$HOME/.claude/settings.json"
INSTALLED="$HOME/.claude/plugins/installed_plugins.json"

# Hook de ESTADO do app no `hooks.json` do Codex. Quem escreve é este instalador, e não o backend na
# subida: o comando carrega o caminho do venv DESTE checkout, então recriar venv, mover o repo ou
# subir de outra worktree mudaria o arquivo — e no Codex hook alterado é hook NÃO APROVADO, que não
# roda e não avisa. Com o instalador como dono, o arquivo só muda num momento explícito, onde
# re-aprovar na TUI faz sentido.
REPO="$(cd "$(dirname "$0")/.." && pwd)"
HOOK_ESTADO="$REPO/backend/hooks/state_hook.py"
PY_HOOK="$REPO/backend/.venv/bin/python"
[ -x "$PY_HOOK" ] || PY_HOOK="$(command -v python3 || true)"
export HOOK_ESTADO PY_HOOK

if [ ! -f "$SETTINGS" ] || [ ! -f "$INSTALLED" ]; then
  echo "pulado: sem $SETTINGS ou $INSTALLED (Claude Code não instalado nesta conta)"
  exit 0
fi

python3 - <<'PYEOF'
import glob, json, os, re

home = os.path.expanduser("~")
cache_prefix = os.path.join(home, ".claude/plugins/cache")
CLAUDE_MD = os.path.join(home, ".claude/CLAUDE.md")

settings = json.load(open(os.path.join(home, ".claude/settings.json")))
installed = json.load(open(os.path.join(home, ".claude/plugins/installed_plugins.json")))
enabled = {k for k, v in settings.get("enabledPlugins", {}).items() if v}

# Plugin que o Pi carrega como PACOTE em vez de skill em ponte, porque o package.json dele declara
# um bloco `pi` (extensão + skills). O `superpowers` precisa disso: a extensão dele injeta o
# bootstrap no início da sessão, coisa que a ponte de skill sozinha não faz — sem o pacote, o
# modelo acaba abrindo o SKILL.md na mão a cada sessão nova. O valor é o nome estável do link em
# ~/.pi/agent/packages/, pra o settings.json nunca apontar pra um diretório com versão dentro.
PI_PACKAGES = {
    "superpowers@claude-plugins-official": "superpowers",
    "ponytail@ponytail": "ponytail",
}
# Plugin que NÃO deve entrar na ponte. Dois casos: o que já chega ao Pi por outro caminho (o
# settings.json dele aponta direto pro marketplace) — bridgeá-lo de novo carregaria cada skill duas
# vezes, com colisão de nome; e o de PI_PACKAGES, que traz as próprias skills dentro do pacote.
#
# O primeiro caso é por máquina, não do app: cada um tem os seus. Um por linha em
# ~/.claude/skills-bridge-skip.txt, no formato `plugin@marketplace`; `#` comenta.
skip_file = os.path.join(home, ".claude/skills-bridge-skip.txt")
PI_SKIP = set(PI_PACKAGES)
if os.path.isfile(skip_file):
    with open(skip_file, encoding="utf-8") as fh:
        PI_SKIP |= {ln.strip() for ln in fh if ln.strip() and not ln.startswith("#")}

# Eventos de hook que o Codex tem. MEDIDO em 29/08/2026 contra codex-cli 0.146.1, com uma sonda que
# gravava o stdin: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop e SessionEnd
# dispararam num turno real; os quatro restantes estão na enumeração do binário mas não tiveram
# ocasião nesse turno. `Notification` e `MessageDisplay` do Claude NÃO existem lá — copiá-los
# escreveria hook que nunca roda, que é pior que hook ausente (parece ligado).
#
# O payload é o do Claude campo por campo (`session_id`, `transcript_path`, `cwd`,
# `hook_event_name`, `tool_name`, `tool_input`, `tool_response`, `last_assistant_message`), e o
# Codex ainda TRADUZ o nome das ferramentas dele pros nomes do Claude — o `exec` chega como
# `tool_name: "Bash"`. Por isso o `matcher` atravessa sem tradução.
EVENTOS_CODEX = ("SessionStart", "SessionEnd", "UserPromptSubmit", "PreToolUse", "PostToolUse",
                 "Stop", "PermissionRequest", "PreCompact", "PostCompact",
                 "SubagentStart", "SubagentStop")

def extra_skill_dirs_kimi():
    """Os diretórios que o Kimi já varre sozinho, lidos do `extra_skill_dirs` do config dele.

    Lido do arquivo e não escrito aqui porque a lista é da MÁQUINA (inclui os marketplaces de cada
    um). A própria ponte sai da lista: contá-la faria toda skill já bridgeada aparecer como "ele já
    tem", e a ponte congelava. Config ilegível cai no mínimo conhecido."""
    try:
        import tomllib
        with open(os.path.join(home, ".kimi-code/config.toml"), "rb") as fh:
            dirs = tomllib.load(fh).get("extra_skill_dirs") or []
    except Exception:
        return (".claude/skills",)
    fora = os.path.join(home, ".kimi-code/skills-bridge")
    out = []
    for d in dirs:
        d = os.path.expanduser(d).rstrip("/")
        if d.startswith(home + os.sep) and d != fora:
            out.append(os.path.relpath(d, home))
    return tuple(out) or (".claude/skills",)


# Um agente = uma entrada. `raiz` é o que decide se ele está instalado; `ponte` é onde entram os
# symlinks de skill; `persona` é o nome com que ELE lê o CLAUDE.md (o Kimi e o Codex leem
# AGENTS.md, o Pi lê CLAUDE.md); `ja_varre` são diretórios que o próprio agente já lê sozinho —
# bridgeá-los de novo só produz colisão de nome.
AGENTES = [
    # `ja_varre` do Kimi sai do `extra_skill_dirs` dele, lido do config — não chumbado aqui: a
    # lista tem os marketplaces de cada máquina. Sem isto a ponte duplicava as 22 skills pessoais,
    # que ele já lê direto de ~/.claude/skills.
    {"nome": "kimi", "raiz": ".kimi-code", "ponte": ".kimi-code/skills-bridge",
     "persona": "AGENTS.md", "ja_varre": extra_skill_dirs_kimi(),
     "skip": frozenset(), "hooks": None},
    # O Pi varre ~/.claude/skills sozinho e roda os hooks do Claude pelo adaptador nativo dele
    # (~/.pi/agent/claude-hooks-adapter.json, com allowlist) — nada a gerar aqui.
    {"nome": "pi", "raiz": ".pi/agent", "ponte": ".pi/agent/skills-bridge",
     "persona": "CLAUDE.md", "ja_varre": (".claude/skills",), "skip": PI_SKIP, "hooks": None},
    # O Codex varre ~/.agents/skills sozinho (inclusive seguindo os symlinks de lá pro repo de
    # skills pessoais). O que falta são as de plugin.
    {"nome": "codex", "raiz": ".codex", "ponte": ".codex/skills",
     "persona": "AGENTS.md", "ja_varre": (".agents/skills",), "skip": frozenset(),
     "hooks": "hooks.json"},
]


def skill_name(d):
    """Nome do frontmatter, caindo no nome do diretório — a mesma regra que o Pi usa."""
    try:
        with open(os.path.join(d, "SKILL.md"), encoding="utf-8") as fh:
            for i, line in enumerate(fh):
                if i > 40 or (i and line.strip() == "---"):
                    break
                m = re.match(r"^name:\s*(\S+)", line)
                if m:
                    return m.group(1)
    except (OSError, UnicodeDecodeError):
        pass
    return os.path.basename(d)


def skill_dirs(install_path):
    """Um plugin traz skills/<nome>/SKILL.md, ou um SKILL.md único na raiz."""
    if os.path.isfile(os.path.join(install_path, "SKILL.md")):
        return [install_path]
    return [d for d in sorted(glob.glob(os.path.join(install_path, "skills", "*")))
            if os.path.isfile(os.path.join(d, "SKILL.md"))]


def skills_de_dir(raiz):
    """{nome: caminho} das skills soltas num diretório (o ~/.claude/skills e afins)."""
    return {skill_name(d): d for d in sorted(glob.glob(os.path.join(raiz, "*")))
            if os.path.isfile(os.path.join(d, "SKILL.md"))}


def collect(plugins):
    wanted = {}
    for name, entries in installed.get("plugins", {}).items():
        if name not in plugins:
            continue
        # O mesmo plugin pode estar instalado em mais de um config dir (~/.claude e
        # ~/.claude-<conta>): quem sobrescreve é o ÚLTIMO, então a ordenação estável põe o do home
        # principal por último e ele ganha. Sem isto as skills espelhadas para Pi, Kimi e Codex
        # ficavam apontando para uma conta secundária — se ela limpa ou atualiza o cache dela, as
        # skills somem dos outros agentes sem explicação. Empate (nenhum ou os dois sob o
        # principal) segue resolvido pela última entrada, como sempre foi.
        ordenadas = sorted(entries, key=lambda e: os.path.abspath(
            e["installPath"]).startswith(cache_prefix + os.sep))
        for e in ordenadas:
            for d in skill_dirs(e["installPath"]):
                wanted[skill_name(d)] = d
    return wanted


def ligar(link, target):
    """Symlink idempotente: cria, reaponta se mudou de alvo, e NUNCA sobrescreve arquivo de
    verdade (um AGENTS.md escrito à mão é do usuário, não nosso)."""
    if os.path.islink(link):
        if os.readlink(link) != target:
            os.unlink(link)
            os.symlink(target, link)
        return True
    if os.path.exists(link):
        return False
    os.symlink(target, link)
    return True


def _nosso(alvo):
    """Link que ESTE script cria — o único que ele pode remover; um posto à mão apontando pra
    outro lugar não é nosso.

    O cache de plugin conta em QUALQUER config dir do Claude (`~/.claude` e `~/.claude-<conta>`),
    não só no principal: link pra conta secundária foi feito por nós antes do desempate de
    `collect`, e reconhecer só o principal deixava-o parado pra sempre — foi o caso medido de um
    plugin de marketplace no Kimi, que ele já varre sozinho e por isso nunca volta pro `wanted`."""
    if alvo.startswith(os.path.join(home, ".claude/skills") + os.sep):
        return True
    return bool(re.match(re.escape(home + os.sep) + r"\.claude[^/]*/plugins/cache/", alvo))


def sync(bridge, wanted):
    os.makedirs(bridge, exist_ok=True)
    for name, target in wanted.items():
        ligar(os.path.join(bridge, name), target)
    # Remove link de plugin desabilitado, apagado ou que mudou de versão. Conjunto vazio quase
    # sempre significa que o JSON acima não disse nada (mudou de formato, arquivo ilegível) e quase
    # nunca "todo plugin está desligado" — então não poda, em vez de esvaziar a ponte inteira por
    # causa de uma surpresa de parse.
    if not wanted:
        return
    for f in os.listdir(bridge):
        link = os.path.join(bridge, f)
        if os.path.islink(link) and _nosso(os.readlink(link)) and f not in wanted:
            os.unlink(link)


def _e_do_hangar(cmd):
    """Hook do próprio app. Ele é instalado por agente, pelo instalador dele (hook_installer.py no
    Claude, kimi_hook_installer.py no Kimi) — atravessar por aqui daria dois donos pro mesmo hook.

    Casa o caminho do SCRIPT (`backend/hooks/`), não o do interpretador: um hook do usuário pode
    rodar no `backend/.venv/bin/python3` deste repo e continuar sendo dele — é o caso do
    `guard_tmux.py`, que mora em ~/.claude/hooks. Filtrar pelo python levaria o hook alheio junto."""
    return "backend/hooks/" in cmd.replace("\\", "/")


def hooks_para(eventos_suportados):
    """Os hooks do settings.json do Claude que fazem sentido noutro agente, no mesmo formato.
    Devolve (config, descartados) — o que não atravessa é DITO, nunca sumido em silêncio."""
    out, fora = {}, []
    for ev, grupos in (settings.get("hooks") or {}).items():
        if ev not in eventos_suportados:
            fora.append(f"{ev} (evento não existe lá)")
            continue
        novos = []
        for g in grupos:
            entradas = [e for e in g.get("hooks", [])
                        if e.get("type") == "command" and e.get("command")
                        and not _e_do_hangar(e["command"])]
            if not entradas:
                continue
            novo = {"hooks": entradas}
            if g.get("matcher"):
                novo["matcher"] = g["matcher"]
            novos.append(novo)
        if novos:
            out[ev] = novos
    return out, fora


# Eventos do Codex em que o hook de ESTADO do app entra. `Stop` grava "ociosa"; os outros, "em
# execução". `Notification` NÃO existe lá — e é por isso que sessão Codex nunca reporta "aguardando".
_ESTADO_EVENTOS = ("SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop")


def com_hook_de_estado(cfg):
    """Acrescenta o hook de estado do app a `cfg` (o que já foi espelhado do Claude).

    Sem ele a sessão Codex aparece ociosa para sempre: é este marcador que a lista lê. O comando
    sai daqui com caminho absoluto do python do venv deste checkout — ver o cabeçalho do script."""
    hook = os.environ.get("HOOK_ESTADO") or ""
    py = os.environ.get("PY_HOOK") or ""
    if not (hook and py and os.path.isfile(hook)):
        return cfg, "sem o state_hook.py deste checkout"
    # `|| true`: hook que falha NÃO pode bloquear o turno de quem está trabalhando. Mesma regra do
    # `_FALHA_NAO_BLOQUEIA` do hook_installer.py do lado Claude.
    comando = f'"{py}" "{hook}" || true'
    for ev in _ESTADO_EVENTOS:
        grupos = cfg.setdefault(ev, [])
        ja = any(e.get("command") == comando for g in grupos for e in g.get("hooks", []))
        if not ja:
            grupos.append({"hooks": [{"type": "command", "command": comando}]})
    return cfg, None


def gravar_hooks(raiz, arquivo, eventos):
    """Gera o hooks.json do agente. Guarda uma cópia do que escrevemos em `.hangar-hooks.json`:
    se o arquivo vivo divergir dela, alguém editou à mão e nós NÃO passamos por cima."""
    alvo = os.path.join(raiz, arquivo)
    espelho = os.path.join(raiz, ".hangar-hooks.json")
    cfg, fora = hooks_para(eventos)
    cfg, sem_estado = com_hook_de_estado(cfg)
    if sem_estado:
        fora = fora + [f"hook de estado do app ({sem_estado})"]
    if not cfg:
        return None, fora
    texto = json.dumps({"hooks": cfg}, indent=2, ensure_ascii=False) + "\n"
    if os.path.exists(alvo):
        vivo = open(alvo, encoding="utf-8").read()
        anterior = open(espelho, encoding="utf-8").read() if os.path.isfile(espelho) else None
        if vivo == texto:
            return 0, fora
        if anterior is None or vivo != anterior:
            return "mao", fora
    tmp = alvo + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write(texto)
    os.replace(tmp, alvo)
    with open(espelho, "w", encoding="utf-8") as fh:
        fh.write(texto)
    return sum(len(g.get("hooks", [])) for gs in cfg.values() for g in gs), fora


pessoais = skills_de_dir(os.path.join(home, ".claude/skills"))
feito, avisos = [], []

for ag in AGENTES:
    raiz = os.path.join(home, ag["raiz"])
    if not os.path.isdir(raiz):
        continue

    # Skills: as de plugin habilitado + as pessoais soltas em ~/.claude/skills, menos o que aquele
    # agente já varre por conta própria (senão o nome colide e ele avisa duplicata).
    wanted = dict(pessoais)
    wanted.update(collect(enabled - set(ag["skip"])))
    taken = set()
    for d in ag["ja_varre"]:
        taken |= set(skills_de_dir(os.path.join(home, d)))
    sync(os.path.join(home, ag["ponte"]), {n: t for n, t in wanted.items() if n not in taken})

    # Persona: o mesmo ~/.claude/CLAUDE.md, com o nome que cada agente procura.
    if os.path.isfile(CLAUDE_MD) and not ligar(os.path.join(raiz, ag["persona"]), CLAUDE_MD):
        avisos.append(f"{ag['nome']}: {ag['persona']} já existe como arquivo de verdade — não mexi")

    if ag["hooks"]:
        n, fora = gravar_hooks(raiz, ag["hooks"], EVENTOS_CODEX)
        if n == "mao":
            avisos.append(f"{ag['nome']}: {ag['hooks']} foi editado à mão — não sobrescrevi")
        elif n:
            avisos.append(f"{ag['nome']}: {n} hooks espelhados em {ag['hooks']}")
            if ag["nome"] == "codex":
                # A confiança do Codex é por hash E POR ÍNDICE (`[hooks.state."<arquivo>:<evento>:
                # <i>:<j>"]` no config.toml), então QUALQUER escrita aqui desaprova os hooks
                # afetados. Enquanto não forem reaprovados, a TUI abre em "Hooks need review" e
                # fica parada esperando escolha — e uma sessão Codex criada pelo app nesse estado
                # estoura o tempo do handshake e NÃO nasce (medido em 30/08/2026).
                avisos.append("codex: ABRA o `codex` no terminal uma vez e aprove os hooks — até "
                              "lá, a sessão Codex criada pelo app não nasce")
        if fora:
            avisos.append(f"{ag['nome']}: fora do espelho — " + ", ".join(sorted(set(fora))))

    feito.append(ag["nome"])

    if ag["nome"] == "pi":
        # Aponta ~/.pi/agent/packages/<nome> pro install atual de cada PI_PACKAGES. O settings.json
        # do Pi lista esses caminhos estáveis, então subir de versão move o symlink em vez de
        # quebrar a entrada.
        pi_packages = os.path.join(home, ".pi/agent/packages")
        os.makedirs(pi_packages, exist_ok=True)
        for key, link_name in PI_PACKAGES.items():
            entries = installed.get("plugins", {}).get(key) or []
            if entries:
                ligar(os.path.join(pi_packages, link_name), entries[0]["installPath"])

print("ponte: " + (", ".join(feito) if feito else "nenhum agente instalado, nada a fazer"))
for a in avisos:
    print("  " + a)
PYEOF
