import logging
import os
import re
import socket
import time
from pathlib import Path
from pydantic import AliasChoices, BaseModel, Field, ValidationInfo, field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

from app import contas, user_data

_LOOPBACK = {"127.0.0.1", "localhost", "::1", "0.0.0.0", "auto"}
_NOME_DE_BRANCH = re.compile(r"[A-Za-z0-9._/-]{1,100}")


def valid_update_branch(value: str) -> bool:
    return not value or bool(_NOME_DE_BRANCH.fullmatch(value) and ".." not in value
                             and not value.startswith("-"))


class ConfigDirInfo(BaseModel):
    path: str
    label: str
    active: bool


def _backend_config_base() -> Path:
    return Path(os.environ.get("CLAUDE_CONFIG_DIR") or (Path.home() / ".claude"))


def _label_for(path: Path) -> str:
    stripped = path.name.removeprefix(".claude-").removeprefix(".claude")
    return stripped or "default"


def _is_config_dir(p: Path) -> bool:
    if (p / ".hangar-account-pending").exists():
        return False
    # Conta recém-criada ainda NÃO tem .credentials.json — quem grava esse arquivo é o /login, e o
    # /login só roda DENTRO de uma sessão dela. Sem o marcador aqui, a conta sumia da lista
    # justamente entre criar e logar, e não havia onde abrir a sessão: impasse.
    if (p / contas.MARCADOR).is_file():
        # Quem decide sobre pasta carimbada é o contas.e_conta endurecido (diretório REAL,
        # marcador REAL): o .is_file() raso segue symlink, e um ~/.claude-evil -> /tmp/fora com
        # um marcador do lado de lá entraria na lista como conta — a reconciliação remexeria, e
        # o apagar destruiria, o diretório externo.
        return contas.e_conta(p)
    return (p / ".credentials.json").is_file() and (p / "projects").is_dir()


# Chave = caminho do config dir; cresce so com a quantidade de ~/.claude*, entao nao precisa poda.
_MTIME_TTL = 60.0
_mtime_cache: dict[str, tuple[float, float]] = {}


def _projects_mtime(p: Path) -> float:
    # Varre recursivamente pra pegar o arquivo mais recente (nao so o dir imediato). CARO: medido em
    # 28/08/2026 nesta maquina, 9.370 arquivos e ~55ms por chamada — e `list_config_dirs` esta em
    # caminho quente (cotas._fontes por requisicao, statusline e previa por leitura de sidecar),
    # onde apareceu como ~10% do CPU do backend num perfil de py-spy. O valor so ORDENA a lista de
    # contas por recencia, entao um minuto de atraso na ordem nao se ve; a varredura por chamada, sim.
    agora = time.monotonic()
    chave = str(p)
    hit = _mtime_cache.get(chave)
    if hit is not None and agora - hit[0] < _MTIME_TTL:
        return hit[1]
    try:
        valor = max((f.stat().st_mtime for f in (p / "projects").rglob("*") if f.is_file()), default=0.0)
    except OSError:
        # Falha NAO entra no cache. Cachear o 0.0 congelaria por 60s um engasgo de 200ms de disco
        # (rede lenta, arquivo sendo reescrito), e como o valor so serve pra ORDENAR, a conta que a
        # pessoa acabou de usar afundaria pro fim da lista sem nenhuma pista do porque. Sem gravar,
        # a proxima chamada tenta de novo — que era o comportamento antes do cache.
        logging.getLogger("hangar.config").warning("_projects_mtime falhou em %s", p, exc_info=True)
        return 0.0
    _mtime_cache[chave] = (agora, valor)
    return valor


def list_config_dirs(ordered: bool = True) -> list[ConfigDirInfo]:
    """Config dirs do Claude pra escolher na criacao. Hibrido: CP_CLAUDE_CONFIG_DIRS ('label:path'
    por virgula) tem prioridade; senao auto-scan de ~/.claude* com login + projects/, label pelo
    basename, ordenado por recencia (backup/abandonado afundam). `ordered=False` pula a ordem, que
    varre projects/ inteiro: quem so busca ou confere pertinencia nao precisa dela."""
    active_base = _backend_config_base().resolve()
    raw = os.environ.get("CP_CLAUDE_CONFIG_DIRS", "").strip()
    entries: list[tuple[str, Path]] = []
    if raw:
        for item in raw.split(","):
            item = item.strip()
            if not item:
                continue
            label, sep, path = item.partition(":")
            # No Windows um caminho SEM rótulo já tem ':' dentro — o do drive. `C:\Users\x` era
            # partido em label="C" e path="\Users\x": o rótulo virava a letra do drive E o caminho
            # perdia o drive, então o resolve() abaixo o reancorava no drive do cwd. Duas linhas
            # erradas de uma vez, caladas. Rótulo de UMA letra seguido de barra é sempre drive:
            # rótulo de verdade com esse formato não existe, e se existisse o caminho depois dele
            # não começaria com barra. Só no `nt` — no POSIX `t:/home/u` é rótulo `t`, e este ramo
            # fica byte-idêntico ao de sempre.
            if os.name == "nt" and sep and len(label) == 1 and label.isalpha() \
                    and path[:1] in ("\\", "/"):
                sep = ""
            if not sep:
                path, label = item, ""
            p = Path(os.path.expanduser(path)).resolve()
            entries.append((label.strip() or _label_for(p), p))
    else:
        found = [p.resolve() for p in Path.home().glob(".claude*") if p.is_dir() and _is_config_dir(p)]
        if ordered:
            found.sort(key=_projects_mtime, reverse=True)
        # A base do app entra mesmo deslogada: numa máquina nova ela é a primeira conta, e sem ela
        # a aba Contas só oferecia criar uma `~/.claude-<nome>`, deixando o `claude` do terminal
        # (que usa a base) preso na tela de boas-vindas.
        if active_base not in found:
            found.insert(0, active_base)
        entries = [(_label_for(p), p) for p in found]
    # Apelido da aba Contas (id `claude:<path>`) vence o nome derivado do disco — é o mesmo nome
    # que a faixa de cota e a lista de contas mostram. Import local só porque apenas esta função
    # usa (não há ciclo hoje: apelidos→contas não voltam pra cá — conferido no review).
    from app import apelidos
    nomes = apelidos.ler()
    out, seen = [], set()
    for label, p in entries:
        if (p / ".hangar-account-pending").exists():
            continue
        s = str(p)
        if s in seen:
            continue
        seen.add(s)
        out.append(ConfigDirInfo(path=s, label=nomes.get(f"claude:{s}") or label, active=(p == active_base)))
    return out


def visible_accounts(ordered: bool = True) -> list[ConfigDirInfo]:
    """As contas Claude que a tela de contas mostra: as criadas pelo app e a base dele. Pasta de
    backup fica fora, porque a tela não teria como apagá-la."""
    return [c for c in list_config_dirs(ordered) if c.active or contas.e_conta(Path(c.path))]


def _default_projects_dir() -> Path:
    # Claude Code writes transcripts under $CLAUDE_CONFIG_DIR/projects when that env
    # is set, else ~/.claude/projects. Don't hardcode — CLAUDE_CONFIG_DIR varies per
    # machine/user. CP_PROJECTS_DIR still overrides this when set.
    base = os.environ.get("CLAUDE_CONFIG_DIR") or (Path.home() / ".claude")
    return Path(base) / "projects"


# Allowlist padrao do scanner de pastas: estas raizes sao o PERIMETRO DE SEGURANCA da
# varredura. Editavel via CP_SCAN_ROOTS (lista separada por virgula, ~ expandido). Edicao
# das raizes dentro do app fica pra depois: por ora o env e a superficie editavel.
#
# O padrao muda por SISTEMA porque `~/pessoal,~/sistemas` sao pastas do dono deste repo, que so
# existem no Linux dele. Num Windows recem-instalado nenhuma das duas existe, e o
# resolve_scan_roots abaixo descarta raiz inexistente de proposito (um typo nunca pode alargar o
# perimetro) — resultado medido nesta VM: a lista resolvia pra VAZIA, `GET /api/fs/roots` devolvia
# `[]` e TODO `GET /api/fs/scan` respondia 403 "root not allowed". Ou seja, o seletor de pasta do
# modal de sessao nova nascia inutil, sem nada na tela explicando por que.
#
# E o padrao, nao um fallback: cair pra `~` quando a lista resolve vazia seria o app ALARGANDO
# sozinho o perimetro que o usuario configurou, que e exatamente o que o descarte acima existe
# pra impedir. Quem apontar CP_SCAN_ROOTS pra caminho errado continua com lista vazia.
def _default_scan_roots() -> str:
    if os.name == "nt":
        # As tres que existem em toda instalacao do Windows e onde projeto de fato mora. Sem `C:\`:
        # a raiz do drive lista Windows/, Program Files/ e ProgramData/ — ruido pra um seletor de
        # projeto, e perimetro grande demais pra vir ligado por padrao.
        return "~,~/Documents,~/Desktop"
    return "~/pessoal,~/sistemas"


_DEFAULT_SCAN_ROOTS = _default_scan_roots()


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="CP_", extra="ignore")

    # A migração leva o `.env` para `~/.hangar/config/`; quem não migrou o tem na `backend/`.
    def __init__(self, **values):
        values.setdefault("_env_file", user_data.path(".env"))
        super().__init__(**values)

    # Loopback by default: portable (exists on every machine) and safe (not exposed to
    # the network unless you opt in). Set CP_LAN_BIND_IP to your LAN IP for phone access.
    lan_bind_ip: str = "127.0.0.1"
    port: int = 8765
    auth_token: str = "change-me"
    projects_dir: Path = _default_projects_dir()
    # CP_SCAN_ROOTS: raizes que o fs-scanner pode listar (string crua; resolvida por
    # resolve_scan_roots). Mantida como str pra aceitar o formato "a,b" direto do env.
    scan_roots: str = _DEFAULT_SCAN_ROOTS
    # CP_DIAG_TERM_INPUT=1: registra no log privado as RESPOSTAS de terminal que o painel manda pro
    # pty (CPR, DA, XTVERSION) — nunca a digitacao. Existe pro defeito do Windows em que caractere
    # que ninguem digitou aparece no composer da TUI com o painel aberto (`termsock._diag_entrada`).
    # Default off: ligado, um terminal movimentado enche o log.
    diag_term_input: bool = False
    # CP_FRONT_PORT: onde o PWA é servido — entra no QR e no painel de alcance. VAZIO (0) = o
    # próprio servidor Rust, que serve o `frontend/dist` na raiz; é a topologia
    # padrão desde que o serviço separado de front deixou de ser instalado. Só quem mantém o
    # `vite preview` (instalação antiga, ou reverse proxy apontado pra lá) grava 5173 aqui — e os
    # instaladores gravam por ele. Cravar 5173 como default fazia o QR apontar pra uma porta que
    # ninguém escuta assim que o serviço do front saiu de cena.
    front_port: int = 0

    @field_validator("front_port", mode="before")
    @classmethod
    def _front_port_vazio_e_ausencia(cls, v: object) -> object:
        # `CP_FRONT_PORT=` (chave presente, valor vazio) é a forma natural de alguém "desligar" isso
        # editando o .env à mão — e o pydantic levanta ValidationError num `int`, o que derruba o
        # backend INTEIRO no import (`settings = Settings()` é module-level). Ausência e vazio têm de
        # significar a mesma coisa: use a porta do backend.
        return 0 if isinstance(v, str) and not v.strip() else v

    public_url: str = ""     # CP_PUBLIC_URL: overrides the auto-built pairing base URL
    # CP_TERM_ORIGINS: origens EXTRAS de onde o login de convidado abre o terminal, separadas por
    # virgula (ex: "https://pocket.exemplo.com"). O dono com `?token=` nao confere Origin. Nao ha
    # default permissivo de proposito: o login do convidado e cookie, e aceitar origem arbitraria
    # deixaria qualquer site abrir um terminal com ele.
    term_origins: str = ""
    # CP_SERVER_ID: id DESTA máquina no peers.json (mesmo que o hangar-send usa). Vazio = grupo
    # entre máquinas desligado; quando setado, vira o endereço de resposta 'srv::sessao' e o nome
    # desta máquina nos grupos entre máquinas. Recado 1:1 entre máquinas segue só no hangar-send.
    server_id: str = ""
    # Web Push (notificacao quando uma sessao fica awaiting_input). Par VAPID COMPARTILHADO entre os
    # servidores (single-user controla todos -> uma inscricao do celular serve os 3). Vazio = push
    # desligado (degrada gracioso: subscribe vira no-op). CP_VAPID_PUBLIC/PRIVATE/SUBJECT.
    vapid_public: str = ""
    vapid_private: str = ""
    # O py_vapid recusa `mailto:` sem domínio com ponto: `hangar@local` derrubava todo envio.
    vapid_subject: str = "mailto:push@hangar.dev.br"
    # Push extras (feature #2): "terminou" (working->idle apos turno longo) e "caiu" (dead). Cada um
    # com seu flag on/off; default ligado (mesmo padrao do awaiting, que nao tem flag proprio).
    notify_finished: bool = True   # CP_NOTIFY_FINISHED
    notify_dead: bool = True       # CP_NOTIFY_DEAD
    finish_min_seconds: int = 45   # CP_FINISH_MIN_SECONDS: debounce — turno mais curto que isso nao avisa
    # Watchdog de sessao travada (feature #7): "working" silencioso ha muito tempo (loop infinito de
    # ferramenta, subprocesso esperando stdin) nunca vira awaiting/finished/dead -> nunca pinga sozinho.
    # 300s de default: ha chamada de ferramenta longa legitima (build, teste) — muito baixo geraria
    # falso-positivo. CP_STALL_POLL_SECONDS: intervalo do watchdog que varre a lista.
    stall_seconds: int = 300         # CP_STALL_SECONDS
    stall_poll_seconds: int = 30     # CP_STALL_POLL_SECONDS
    # Auto-resume (feature #8, opt-in): quando uma sessao LIMITED tem fila nao-entregue e o reset
    # parseia num horario confiavel, arma um Timer pra drenar sozinha no reset. Default OFF -- e uma
    # acao UNATTENDED (manda prompt sem o usuario olhar); liga so quem quer. CP_AUTO_RESUME.
    auto_resume: bool = False
    # Importação/sincronização OMP é opt-in e também respeita o kill-switch mestre.
    omp_plugin_sync_enabled: bool = False
    omp_plugin_sync_interval: float = Field(300, gt=0, allow_inf_nan=False)
    omp_claude_context_enabled: bool = False

    @field_validator("omp_plugin_sync_interval", mode="before")
    @classmethod
    def _intervalo_vazio_e_ausencia(cls, v: object) -> object:
        # Vazio no .env preserva o padrão, como em front_port; números inválidos continuam recusados.
        return 300 if isinstance(v, str) and not v.strip() else v

    # Sync opt-in: CP_SYNC=1 ou ativação persistida pela tela, sem reiniciar o backend.
    # Stores only salt + auth verifier + ciphertext (zero-knowledge; tokens are encrypted client-side).
    sync: bool = False
    sync_bootstrap: str = ""        # CP_SYNC_BOOTSTRAP: one-time secret to gate first registration
    # ~/.hangar/ (era ~/.claude-pocket/): quem move a pasta é migracao_sidecars.migrar, na subida,
    # deixando link no caminho antigo. É o COFRE do cloud-sync — perder de vista este arquivo é a
    # tela "Criar acesso" aparecendo pra quem já tinha conta.
    sync_data: Path = Path.home() / ".hangar" / "sync-vault.json"
    sync_session_secret: str = ""   # CP_SYNC_SESSION_SECRET: HMAC key for the session cookie; empty -> random at boot
    sync_rate_max: int = 10         # CP_SYNC_RATE_MAX: failed sync logins allowed in the window before 429
    sync_rate_window: int = 300     # CP_SYNC_RATE_WINDOW: rate-limit window in seconds (default 5 min)
    # CP_FORWARDED_ALLOW_IPS: proxy IP(s) uvicorn trusts for X-Forwarded-For/-Proto, so the real client
    # IP (rate limiter) and the https scheme (cookie Secure) are seen behind a TLS proxy. "*" trusts any
    # upstream (only safe when nothing untrusted can reach the port directly).
    forwarded_allow_ips: str = "127.0.0.1"
    # CP_DEPLOY_SECRET: shared secret do webhook do GitHub (HMAC-SHA256). Vazio = endpoint de auto-deploy
    # desligado (retorna 404). O push na main dispara /api/deploy/github-webhook -> valida a assinatura ->
    # start (nao-bloqueante) da unit systemd 'hangar-deploy.service' (pull + build + restart).
    deploy_secret: str = ""
    # CP_EDITOR: binario do editor pro "abrir pasta no editor" (menu da sessao). So-desktop: abre na
    # maquina que roda o backend. Binario unico (sem args/shell) -> exec seguro com o cwd da sessao.
    editor: str = "code"
    # Kill-switch MESTRE das acoes autonomas: encadeamento de sessao, tick do loop, auto-resume,
    # auto-update, sync do Codex e do omp, ask-history (quem chama automations_enabled()). Default ON;
    # e portao ADICIONAL, nao substitui o flag proprio de cada uma (ex: CP_AUTO_RESUME). Manutencao
    # (renovacao de token, limpeza, hooks no boot, varredura de grupo) fica fora de proposito: desligar
    # a chave nao pode deixar login vencer nem sidecar orfao acumular. CP_AUTOMATIONS=0 desliga tudo.
    automations: bool = True
    # Resumo em pt-BR dos blocos de raciocínio (pensamento_pt). Desligado, o front nem pede e a
    # rota devolve o texto original: em link fraco cada abertura de chat custava 3-5 chamadas.
    traduzir_pensamento: bool = True
    # Reconciliacao automatica do Codex (codex_integracao.acompanhar + abertura de TUI). Portao
    # PROPRIO por cima do `automations`, editavel pela tela (card do Codex em Harnesses).
    codex_sync: bool = True
    # Desligar o 1M do Codex fica gravado aqui: sem a chave no config.toml, o padrão é ligado.
    codex_extended_context: bool = True
    # Organização é opt-in, independentemente de credenciais antigas de LLM.
    dictation_organization_mode: str = "none"
    dictation_claude_model: str = ""
    dictation_codex_model: str = ""
    dictation_include_recent_messages: bool = False
    # A preferência salva também é lida pelos instaladores, sem depender do backend em execução.
    claude_statusline_update: bool = True
    # Caminho do plugin do Hangar nas sessões Claude (mods do Claude Code). Nasce ligado; desligado,
    # toda entrega volta ao caminho de tecla. Vale para sessão NOVA.
    claude_function_hooks: bool = True
    # Como a sessão nova nasce quando ninguém escolhe o Jev. Default aqui, e não só em
    # `runtime_config.EDITAVEIS`, porque sem ele o campo responde `None` — e um interruptor que
    # nasce sem valor nenhum na tela não sabe dizer se está ligado ou desligado.
    jev_padrao: bool = False
    headless_default: bool = True
    # Fileira de atalhos do painel de sessão (JSON numa string; ver runtime_config._validate_shortcuts).
    # Vazio = conjunto nativo. Default aqui pelo mesmo motivo do jev_padrao: sem ele o get responde None.
    shortcuts: str = ""
    # Chave da Groq pra transcricao de audio (whisper-large-v3-turbo). Aceita CP_GROQ_API_KEY (padrao
    # do .env, com prefixo) OU GROQ_API_KEY (convencao do Groq/OpenAI SDK, ex: no Environment do systemd).
    # Vazio = transcricao desligada (o endpoint /transcribe responde 503). Ver docs/USAGE.md.
    groq_api_key: str = Field("", validation_alias=AliasChoices("CP_GROQ_API_KEY", "GROQ_API_KEY"))
    # Dias que um anexo fica em ~/.hangar/uploads/<projeto>/<sessao>/. Sem limpeza a pasta cresce
    # pra sempre (video sobe ate 100 MiB por arquivo). 0 = nunca apagar.
    # Anexo apagado que ainda aparece numa conversa antiga vira o chip "não carregou" — visível,
    # não some calado.
    upload_retention_days: int = Field(30, validation_alias=AliasChoices("CP_UPLOAD_RETENTION_DAYS",))
    # CP_UPDATE_BRANCH: branch que o Atualizar segue no lugar da main, pra testar uma versão em
    # desenvolvimento no app de verdade. Vazio = main.
    update_branch: str = ""
    update_last_branch: str = ""

    @field_validator("update_branch", "update_last_branch", mode="before")
    @classmethod
    def _branch_valida(cls, v: object, info: ValidationInfo) -> object:
        # O nome vai pro argv do git: inválido vira vazio (main) e fica registrado, nunca derruba o import.
        texto = v.strip() if isinstance(v, str) else ""
        if valid_update_branch(texto):
            return texto
        name = f"CP_{info.field_name.upper()}"
        logging.getLogger("hangar.config").warning("%s inválido (%r): valor ignorado", name, texto)
        from app import diag
        diag.registrar("atualizacao.branch_invalida", "aviso", detalhe=f"{name} ignorado")
        return ""


settings = Settings()


def detect_lan_ip() -> str:
    """Best-effort primary LAN IP. Opens a UDP socket toward a public address to find
    which local interface egress traffic would use — no packet is actually sent."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("8.8.8.8", 80))
            return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"


def resolve_bind_ip(s: "Settings") -> str:
    """The address uvicorn should bind. 'auto' -> the detected LAN IP."""
    return detect_lan_ip() if s.lan_bind_ip == "auto" else s.lan_bind_ip


# Porta do convidado: quem escuta é o hangar-server (Rust); o Python nunca a abre.
GUEST_PORT = 8766
