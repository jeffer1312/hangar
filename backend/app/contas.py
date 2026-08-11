"""Contas Claude: um config dir por conta, com o ambiente compartilhado por atalho.

Por que uma pasta por conta: o Claude Code guarda a sessão logada em DOIS arquivos dentro do config
dir — `.credentials.json` (token OAuth, renovado sozinho a cada ~8h) e `.claude.json` (o bloco
`oauthAccount`). Com um arquivo só, duas sessões vivas em contas diferentes se atropelam: quem
renovar por último sobrescreve a outra, e a primeira passa a mandar o token da conta errada.

Por que atalho e não cópia: config dir separado normalmente significa AMBIENTE separado — skills,
plugins, hooks, settings. Aqui quase tudo é link pro `~/.claude` de sempre, então editar uma skill
vale nas contas todas no mesmo instante.

Por que `projects/` é a exceção: é onde ficam os `.jsonl`, e o painel de custo soma UMA VEZ POR
CONFIG DIR (`costs_sources._config_dirs`). Compartilhar a pasta faria o mesmo gasto ser contado
uma vez por conta e ainda aparecer em conta que nunca rodou nada. Pasta real por conta = gasto
honesto por conta. O preço, aceito: `claude --resume` de uma conta não lista conversa da outra.

Por que `memory/` volta a ser atalho: memória não custa (o leitor de gasto varre `*.jsonl`, e
memória é `.md`) e o usuário quer a mesma memória de projeto valendo em qualquer conta.

Por que `~/.claude-<nome>`: `config.list_config_dirs()` já varre `~/.claude*` procurando config
dir. Nomear assim faz a conta nova aparecer na tela de criação sem inventar registro novo.
"""
import filecmp
import json
import os
import re
import shutil
import uuid
from contextlib import contextmanager
from pathlib import Path

try:
    import fcntl
except ImportError:      # Windows: sem flock. A trava da conta vira no-op; ver _trava.
    fcntl = None

try:
    import msvcrt
except ImportError:      # Linux/macOS: sem locking do Windows; só a _trava_compartilhada usa.
    msvcrt = None


class ContaError(Exception):
    """status HTTP junto porque a API é o principal chamador; o CLI só imprime o detail."""

    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


_NOME_OK = re.compile(r"^[a-z0-9][a-z0-9_-]{0,31}$")

# Carimbo de "esta pasta foi criada aqui". reconciliar()/apagar() só mexem em pasta carimbada —
# sem isto, um `~/.claude-backup` do usuário entraria na poda de atalhos e no apagar.
MARCADOR = ".hangar-conta"

# Quantas versões a gaveta guarda. Sem teto ela cresce pra sempre e ninguém olha.
DRIFT_TETO = 3

# Nunca viram atalho. `projects` está aqui por causa do custo (ver docstring); `.drift` e o
# marcador são nossos e não existem no compartilhado.
_NAO_LIGAR = {MARCADOR, ".drift", ".claude.json", ".credentials.json", "projects"}


def compartilhado() -> Path:
    """SEMPRE o ~/.claude real, nunca CLAUDE_CONFIG_DIR.

    Se o backend já estiver rodando dentro de uma conta, derivar do env faria a conta nova apontar
    pra outra conta e os atalhos virariam corrente — dois saltos até o arquivo de verdade, e a
    remoção do elo do meio quebrando tudo em silêncio.
    """
    return Path.home() / ".claude"


def caminho(nome: str) -> Path:
    # fullmatch e não match+$: o $ casa antes de uma quebra de linha FINAL, então
    # 'conta2\n' passava e a pasta nascia com controle de linha no nome.
    if not _NOME_OK.fullmatch(nome or ""):
        raise ContaError(400, "nome: use minúsculas, números, '-' ou '_' (até 32 caracteres)")
    return Path.home() / f".claude-{nome}"


def e_conta(p: Path) -> bool:
    """Pasta de conta de verdade: diretório REAL (não symlink) com marcador real.

    Sem as duas guardas, um `~/.claude-evil -> /tmp/fora` com um `.hangar-conta` do lado de lá
    seria listado como conta, e a reconciliação remexeria — e o apagar destruiria — o diretório
    externo.
    """
    if p.is_symlink() or not p.is_dir():
        return False
    marcador = p / MARCADOR
    return marcador.is_file() and not marcador.is_symlink()


def listar() -> list[str]:
    return sorted(p.name.removeprefix(".claude-") for p in Path.home().glob(".claude-*")
                  if e_conta(p))


@contextmanager
def _trava(dir_conta: Path):
    """Serializa reconciliações da MESMA conta.

    Sem isto, duas criações de sessão simultâneas (o app roda em thread, e o terminal chama o
    `cp-conta --prep` por fora) caem na janela entre remover e recriar o link: uma leva
    FileExistsError e a criação de sessão morre com 500 intermitente. No Windows não há flock;
    a trava vira no-op e a corrida volta a ser possível — está na doc de limitações.
    """
    if fcntl is None:
        yield
        return
    with open(dir_conta / MARCADOR, "r+", encoding="utf-8") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(fh, fcntl.LOCK_UN)


@contextmanager
def _trava_compartilhada():
    """Serializa reconciliações de contas DIFERENTES.

    A conta não é dona do compartilhado: duas contas reconciliando ao mesmo tempo podem devolver
    versões diferentes do MESMO arquivo pro ~/.claude (o `_resolver_colisao` copia pro
    compartilhado). Cada uma passa o seu filecmp e o último copyfile vence — a trava por conta
    não impede a corrida, porque o recurso disputado é de TODAS. O lock mora na home
    (`.hangar-contas.lock`), fora de qualquer conta, e usa o locking nativo de cada sistema.
    """
    if fcntl is None and msvcrt is None:
        yield
        return
    with open(Path.home() / ".hangar-contas.lock", "a+", encoding="utf-8") as fh:
        if fcntl is not None:
            fcntl.flock(fh, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(fh, fcntl.LOCK_UN)
        else:
            fh.seek(0)
            msvcrt.locking(fh.fileno(), msvcrt.LK_LOCK, 1)
            try:
                yield
            finally:
                fh.seek(0)
                msvcrt.locking(fh.fileno(), msvcrt.LK_UNLCK, 1)


def _ligar(destino: Path, alvo: Path) -> None:
    """Cria o atalho por troca atômica: sem janela em que o caminho não existe.

    A forma ingênua (`unlink` e depois `symlink`) deixa um instante sem o caminho — e um CLI vivo
    daquela conta lendo `skills/` nesse instante recebe ENOENT. O temporário leva pid + uuid:
    com nome fixo, um arquivo legítimo `skills.hangar-novo` do usuário seria apagado na próxima
    reconciliação, antes de existir o que criar no lugar.
    """
    tmp = destino.with_name(f"{destino.name}.hangar-novo.{os.getpid()}.{uuid.uuid4().hex[:8]}")
    if tmp.is_symlink() or tmp.exists():
        raise ContaError(500, f"{tmp.name} já existe — reconciliação anterior deixou lixo?")
    try:
        os.symlink(alvo, tmp, target_is_directory=alvo.is_dir())
    except OSError as e:
        raise ContaError(
            500,
            f"não consegui criar o atalho {destino.name}: {e}. No Windows isso exige o Modo "
            "Desenvolvedor ligado (Configurações → Sistema → Para desenvolvedores). Sem ele a "
            "conta ficaria com uma CÓPIA, que passa a divergir do original sem ninguém perceber.",
        ) from e
    os.replace(tmp, destino)


def _gavetar(dir_conta: Path, destino: Path) -> str:
    """Move pra `.drift/`, mantendo só as DRIFT_TETO mais novas."""
    gaveta = dir_conta / ".drift"
    if gaveta.is_symlink() or (gaveta.exists() and not gaveta.is_dir()):
        # A gaveta é caminho interno da conta: symlinkada pra fora, as entradas — e a poda, que
        # pode rmtree — iriam parar no diretório externo.
        raise ContaError(500, "a gaveta .drift desta conta é um symlink ou não é uma pasta")
    gaveta.mkdir(exist_ok=True)
    n = 1
    while (gaveta / f"{destino.name}.{n}").exists():
        n += 1
    shutil.move(str(destino), str(gaveta / f"{destino.name}.{n}"))
    # lstat e não stat: entradas podem ser symlinks (colisão de symlink inesperado vai pra
    # gaveta), e stat seguiria o alvo — quebrado, levantaria no meio da poda.
    antigas = sorted(gaveta.iterdir(), key=lambda p: p.lstat().st_mtime, reverse=True)
    for velha in antigas[DRIFT_TETO:]:
        if velha.is_symlink() or velha.is_file():
            velha.unlink()
        else:
            shutil.rmtree(velha, ignore_errors=True)
    return f"'{destino.name}' era local nesta conta; movido pra .drift/{destino.name}.{n}"


def _resolver_colisao(dir_conta: Path, destino: Path, alvo: Path) -> str | None:
    """O caminho existe e NÃO é o atalho esperado. Decide o que fazer sem perder dado.

    Regra única: incompatibilidade de tipo vai pra gaveta. Arquivo contra arquivo pode fundir (a
    mudança sobe pro compartilhado — é o que "compartilhado" quer dizer); pasta não dá pra
    fundir; e symlink inesperado NÃO pode ser seguido: filecmp/copyfile leriam o alvo externo e
    vazariam o conteúdo dele pra dentro do compartilhado.
    """
    if destino.is_symlink() or destino.is_dir() or not destino.is_file() or not alvo.is_file():
        return _gavetar(dir_conta, destino)
    if not filecmp.cmp(destino, alvo, shallow=False):
        shutil.copyfile(destino, alvo)
        destino.unlink()
        return f"'{destino.name}' foi alterado dentro desta conta; a mudança subiu pro ~/.claude"
    destino.unlink()
    return None


def _ligar_memoria(dir_conta: Path, projeto: str | None) -> list[str]:
    """`projects/` é real por conta; só o `memory/` de cada projeto é atalho.

    Duas passadas: uma varre o que já existe no compartilhado (cobre tudo que a máquina conhece),
    e a outra atende o projeto que está subindo agora — que pode ser novo e ainda não ter memória
    nenhuma. Quem chama com `projeto` é o backend, que sabe o cwd da sessão e sanitiza com
    `registry.sanitize_cwd` (fonte única dessa regra). Devolve os avisos das colisões.
    """
    avisos: list[str] = []
    raiz = compartilhado() / "projects"
    raiz.mkdir(parents=True, exist_ok=True)
    nomes = {p.name for p in raiz.iterdir() if p.is_dir()}
    if projeto:
        (raiz / projeto / "memory").mkdir(parents=True, exist_ok=True)
        nomes.add(projeto)
    for nome in nomes:
        alvo = raiz / nome / "memory"
        if not alvo.is_dir():
            continue
        local = dir_conta / "projects" / nome
        local.mkdir(parents=True, exist_ok=True)
        destino = local / "memory"
        if destino.is_symlink() and os.readlink(destino) == str(alvo):
            continue
        if destino.is_symlink() or destino.exists():
            # Mesma regra do topo: o que não é o atalho esperado é deriva. Memória local de
            # verdade vai pra gaveta (não dá pra fundir) e o atalho é refeito — deixar quieto
            # faria esta conta guardar uma memória que nenhuma outra vê, em silêncio.
            aviso = _resolver_colisao(dir_conta, destino, alvo)
            if aviso:
                avisos.append(aviso)
        _ligar(destino, alvo)
    # Poda: atalho de memória apontando pra projeto que sumiu do compartilhado. A poda do topo
    # da reconciliar não alcança projects/ — sem esta passada, o link morto ficava pra sempre.
    if (dir_conta / "projects").is_dir():
        for local in (dir_conta / "projects").iterdir():
            memo = local / "memory"
            if memo.is_symlink() and not memo.exists():
                memo.unlink()
    return avisos


def _reconciliar(dir_conta: Path, projeto: str | None) -> list[str]:
    """Corpo da reconciliação, SEM as travas — quem chama (reconciliar público ou o ciclo da
    conta) já as segura. Validar o projeto aqui também protege o caminho do ciclo, que recebe
    o projeto do backend sem passar pelo público."""
    if projeto is not None and not re.fullmatch(r"[A-Za-z0-9_-]+", projeto):
        # projeto entra em caminhos (raiz / projeto / memory): absoluto, `..` ou barra
        # escapariam do ~/.claude/projects. O regex aceita exatamente o que o
        # registry.sanitize_cwd produz, e nada além.
        raise ContaError(400, "projeto inválido")
    avisos: list[str] = []
    for alvo in sorted(compartilhado().iterdir()):
        if alvo.name in _NAO_LIGAR:
            continue
        destino = dir_conta / alvo.name
        if destino.is_symlink() and os.readlink(destino) == str(alvo):
            continue
        if destino.is_symlink() or destino.exists():
            aviso = _resolver_colisao(dir_conta, destino, alvo)
            if aviso:
                avisos.append(aviso)
        _ligar(destino, alvo)
    for p in dir_conta.iterdir():
        # Atalho apontando pra coisa que sumiu do compartilhado.
        if p.is_symlink() and not p.exists():
            p.unlink()
    avisos.extend(_ligar_memoria(dir_conta, projeto))
    return avisos


def reconciliar(nome: str, projeto: str | None = None) -> list[str]:
    """Refaz os atalhos da conta. Idempotente — roda a cada abertura de sessão.

    É isto que impede a deriva: pasta que aparecer no `~/.claude` depois entra na conta no próximo
    uso, sem ninguém rodar nada à mão. Devolve avisos (lista vazia = nada fora do lugar).
    """
    dir_conta = caminho(nome)
    if not e_conta(dir_conta):
        raise ContaError(404, f"{dir_conta} não é uma conta criada pelo hangar")
    # Compartilhada primeiro, a da conta depois — sempre nesta ordem, em toda operação.
    with _trava_compartilhada(), _trava(dir_conta):
        return _reconciliar(dir_conta, projeto)


class _Ciclo:
    """A conta sob a trava do ciclo: as operações internas que a API roda DENTRO da janela em
    que a conta não pode sumir. Só o `ciclo_conta` fabrica — a API não adquire `_trava` por
    conta própria."""

    def __init__(self, dir_conta: Path):
        self.dir_conta = dir_conta

    def reconciliar(self, projeto: str | None = None) -> list[str]:
        return _reconciliar(self.dir_conta, projeto)

    def apagar(self) -> None:
        _apagar(self.dir_conta)


@contextmanager
def ciclo_conta(nome: str):
    """Trava do ciclo de vida da conta: da reconciliação até o `registry.create` (abrir sessão),
    e ao redor da checagem + rmtree do apagar.

    Por quê: a criação de sessão é assíncrona (roda em thread) e a reconciliação tem efeito no
    disco. Sem esta janela, um DELETE da MESMA conta no meio dela via a lista de sessões ainda
    vazia e apagava a pasta embaixo da sessão que estava subindo — o CLI passaria a escrever num
    caminho que sumiu. Quem abre sessão e quem apaga disputam o mesmo recurso; as duas pontas
    usam esta mesma trava, então uma espera a outra.

    O `apagar` público adquire a mesma trava sozinho (o `cp-conta` não conhece o ciclo); a API
    usa as operações do `_Ciclo` devolvido para não se trancar duas vezes.
    """
    dir_conta = caminho(nome)
    if not e_conta(dir_conta):
        raise ContaError(404, f"{dir_conta} não é uma conta criada pelo hangar")
    # Compartilhada primeiro, a da conta depois — sempre nesta ordem, em toda operação.
    with _trava_compartilhada(), _trava(dir_conta):
        yield _Ciclo(dir_conta)


def _semear_claude_json(dir_conta: Path) -> None:
    """Copia o `~/.claude.json` SEM o `oauthAccount`.

    Copiar em vez de começar do zero salva o que dói perder: as permissões já aceitas por diretório
    e os MCP de escopo usuário moram nesse arquivo. O `oauthAccount` é o único campo que PRECISA
    ser diferente — mantê-lo faria o CLI abrir dizendo estar logado numa conta cujo token não tem.
    """
    try:
        lido = json.loads((Path.home() / ".claude.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        dados: dict = {}   # máquina nova: a conta começa limpa, não quebra
    except (OSError, ValueError) as e:
        # Truncado por escrita concorrente, sem permissão, JSON inválido: engolir viraria uma
        # conta criada com cara de sucesso e sem MCP nem permissões — falha silenciosa.
        raise ContaError(500, f"não consegui ler o ~/.claude.json pra semear a conta: {e}") from e
    else:
        if not isinstance(lido, dict):
            raise ContaError(500, "o ~/.claude.json não é um objeto JSON — não dá pra semear a conta")
        dados = lido
    dados.pop("oauthAccount", None)
    (dir_conta / ".claude.json").write_text(json.dumps(dados, indent=2), encoding="utf-8")


def criar(nome: str) -> Path:
    """Cria a pasta pronta pro `/login`. NÃO loga — o OAuth abre navegador e é interativo."""
    dir_conta = caminho(nome)
    if dir_conta.is_symlink() or dir_conta.exists():
        # is_symlink cobre o link QUEBRADO: `exists()` o segue e mente, e o mkdir em cima
        # estouraria FileExistsError bruto em vez do 409 de "já existe".
        raise ContaError(409, f"já existe {dir_conta}")
    # Máquina nova não tem ~/.claude ainda; sem o mkdir o reconciliar morreria no meio e a conta
    # parcial carimbada sobraria — aparecendo no seletor e travando o cadastro com 409.
    compartilhado().mkdir(parents=True, exist_ok=True)
    ok = False
    try:
        dir_conta.mkdir(parents=True)
        (dir_conta / MARCADOR).write_text("", encoding="utf-8")
        _semear_claude_json(dir_conta)
        (dir_conta / "projects").mkdir()
        reconciliar(nome)
        ok = True
        return dir_conta
    finally:
        # Rollback: falhou no meio (symlink recusado no Windows, ~/.claude.json ilegível) e a
        # conta parcial não pode sobrar carimbada.
        if not ok and dir_conta.is_dir() and not dir_conta.is_symlink():
            shutil.rmtree(dir_conta)


def _apagar(dir_conta: Path) -> None:
    """rmtree sob a trava — quem chama (apagar público ou o ciclo da conta) já validou e já
    segura as travas."""
    shutil.rmtree(dir_conta)


def apagar(nome: str) -> None:
    """Some com a conta. Existe porque um nome digitado errado no cadastro ficaria pra sempre no
    seletor — `criar` só recusa sobrescrever, não desfaz.

    Os `.jsonl` daquela conta vão junto: são dela, e o gasto histórico dela sai do painel. Quem
    chama (a API) é quem checa se há sessão viva usando esta conta.

    Adquire a mesma trava da reconciliação e do ciclo: o `cp-conta` (que não conhece o ciclo)
    também não pode apagar a conta no meio de uma abertura de sessão.
    """
    dir_conta = caminho(nome)
    if not e_conta(dir_conta):
        raise ContaError(404, f"{dir_conta} não é uma conta criada pelo hangar")
    with _trava_compartilhada(), _trava(dir_conta):
        _apagar(dir_conta)
