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
from contextlib import contextmanager
from pathlib import Path
from uuid import uuid4

try:
    import fcntl
except ImportError:      # Windows: sem flock. A trava vira no-op; ver _trava.
    fcntl = None


class ContaError(Exception):
    """status HTTP junto porque a API é o principal chamador; o CLI só imprime o detail."""

    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


_NOME_OK = re.compile(r"^[a-z0-9][a-z0-9_-]{0,31}$")

# Projeto sanitizado (registry.sanitize_cwd): só letras, dígitos e hífen. Regra separada da do
# nome de conta porque o sanitize preserva maiúsculas e a conta não.
_PROJETO_OK = re.compile(r"[A-Za-z0-9_-]+")

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
    if not _NOME_OK.match(nome or ""):
        raise ContaError(400, "nome: use minúsculas, números, '-' ou '_' (até 32 caracteres)")
    return Path.home() / f".claude-{nome}"


def e_conta(p: Path) -> bool:
    """Pasta é conta só se o marcador for real e a raiz não for symlink.

    Seguir symlink aqui aceitaria `~/.claude-evil -> /tmp/fora` com um marcador plantado lá dentro:
    listar() mostraria a conta, reconciliar() remexeria em /tmp/fora e apagar() derrubaria com erro
    bruto de rmtree em symlink. A raiz tem que ser diretório de verdade da conta.
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

    A reconciliação também escreve no `~/.claude` compartilhado (o arquivo local alterado sobe pro
    compartilhado). A _trava por conta não cobre isso: duas contas reconciliando ao mesmo tempo
    passam o filecmp contra o MESMO estado e cada uma copia a sua versão — o último copyfile
    vence e a alteração da primeira morre sem aviso. O lock mora no HOME, fora de qualquer conta,
    e é tomado SEMPRE antes da _trava da conta (ordem fixa nas duas pontas, senão deadlock).
    Mesma ressalva do Windows da _trava.
    """
    if fcntl is None:
        yield
        return
    with open(Path.home() / ".hangar-contas.lock", "a+", encoding="utf-8") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(fh, fcntl.LOCK_UN)


def _ligar(destino: Path, alvo: Path) -> None:
    """Cria o atalho por troca atômica: sem janela em que o caminho não existe.

    A forma ingênua (`unlink` e depois `symlink`) deixa um instante sem o caminho — e um CLI vivo
    daquela conta lendo `skills/` nesse instante recebe ENOENT. O temporário tem nome único por
    chamada: um arquivo do usuário chamado `<nome>.hangar-novo` (o nome fixo antigo) era apagado
    na religação. Órfão de crash fica com nome claro e inócuo — nenhuma chamada futura o acha.
    """
    tmp = destino.with_name(f"{destino.name}.hangar-novo-{uuid4().hex}")
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
        # Gaveta apontando pra fora: o move mandaria a pasta colidida pro alvo externo, e a poda
        # poderia rmtree lá. Recusar é a única saída que não mexe fora da conta.
        raise ContaError(500, "a gaveta .drift desta conta é inválida (symlink ou não-diretório)")
    gaveta.mkdir(exist_ok=True)
    n = 1
    while (gaveta / f"{destino.name}.{n}").exists():
        n += 1
    shutil.move(str(destino), str(gaveta / f"{destino.name}.{n}"))
    antigas = sorted(gaveta.iterdir(), key=lambda p: p.stat().st_mtime, reverse=True)
    for velha in antigas[DRIFT_TETO:]:
        shutil.rmtree(velha, ignore_errors=True) if velha.is_dir() else velha.unlink()
    return f"'{destino.name}' era local nesta conta; movido pra .drift/{destino.name}.{n}"


def _resolver_colisao(dir_conta: Path, destino: Path, alvo: Path) -> str | None:
    """O caminho existe e NÃO é o atalho esperado. Decide o que fazer sem perder dado.

    Symlink inesperado (o esperado foi filtrado em reconciliar): seguir com filecmp/copyfile
    LERIA o alvo do link — que pode estar fora da conta — e sobrescreveria o compartilhado com
    ele. Link não é dado: vai pra gaveta sem tocar no alvo.

    Arquivo: quem grava por tmp+rename (`os.replace`) substitui o ATALHO por um arquivo comum
    dentro da conta. A mudança é real e é do usuário — e como o arquivo é compartilhado por
    desenho, devolvê-la pro compartilhado é o que "compartilhado" quer dizer. Mandar pra `.drift`
    perderia a mudança nas DUAS contas, com um log como único rastro.

    Pasta, ou incompatibilidade de tipo (arquivo contra alvo que virou pasta): não dá pra fundir,
    e descartar o arquivo perderia dado. Vai pra gaveta.
    """
    if destino.is_symlink():
        return _gavetar(dir_conta, destino)
    if destino.is_dir() or not destino.is_file() or not alvo.is_file():
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
    `registry.sanitize_cwd` (fonte única dessa regra).

    Colisão de memória usa a MESMA regra de preservação do resto: memória local real vai pra
    gaveta e o atalho é refeito — deixar quieto faria a conta divergir do compartilhado calada.
    Symlink de memória apontando pra projeto que sumiu do compartilhado é link morto, não dado:
    é removido, sem gaveta. Devolve avisos (mesmo contrato do reconciliar).
    """
    raiz = compartilhado() / "projects"
    raiz.mkdir(parents=True, exist_ok=True)
    nomes = {p.name for p in raiz.iterdir() if p.is_dir()}
    if projeto:
        (raiz / projeto / "memory").mkdir(parents=True, exist_ok=True)
        nomes.add(projeto)
    avisos: list[str] = []
    for nome in nomes:
        alvo = raiz / nome / "memory"
        if not alvo.is_dir():
            continue
        local = dir_conta / "projects" / nome
        local.mkdir(parents=True, exist_ok=True)
        destino = local / "memory"
        if destino.is_symlink() and os.readlink(destino) == str(alvo):
            continue
        if destino.exists() or destino.is_symlink():
            aviso = _resolver_colisao(dir_conta, destino, alvo)
            if aviso:
                avisos.append(aviso)
        _ligar(destino, alvo)
    raiz_local = dir_conta / "projects"
    if raiz_local.is_dir():
        for local in raiz_local.iterdir():
            memo = local / "memory"
            if memo.is_symlink() and not memo.exists():
                memo.unlink()
    return avisos


def reconciliar(nome: str, projeto: str | None = None) -> list[str]:
    """Refaz os atalhos da conta. Idempotente — roda a cada abertura de sessão.

    É isto que impede a deriva: pasta que aparecer no `~/.claude` depois entra na conta no próximo
    uso, sem ninguém rodar nada à mão. Devolve avisos (lista vazia = nada fora do lugar).
    """
    dir_conta = caminho(nome)
    if not e_conta(dir_conta):
        raise ContaError(404, f"{dir_conta} não é uma conta criada pelo hangar")
    if projeto is not None and not _PROJETO_OK.fullmatch(projeto):
        # `projeto` vira caminho logo abaixo: absoluto, `..`, barra ou barra invertida escapariam
        # de ~/.claude. A regra aceita exatamente o que registry.sanitize_cwd produz.
        raise ContaError(400, "projeto inválido")
    avisos: list[str] = []
    with _trava_compartilhada(), _trava(dir_conta):
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


def _semear_claude_json(dir_conta: Path) -> None:
    """Copia o `~/.claude.json` SEM o `oauthAccount`.

    Copiar em vez de começar do zero salva o que dói perder: as permissões já aceitas por diretório
    e os MCP de escopo usuário moram nesse arquivo. O `oauthAccount` é o único campo que PRECISA
    ser diferente — mantê-lo faria o CLI abrir dizendo estar logado numa conta cujo token não tem.

    Só a AUSÊNCIA do arquivo conta como máquina nova. Arquivo truncado por escrita concorrente,
    sem permissão ou com JSON inválido não pode virar conta "criada" sem MCP nem permissões, com
    o problema escondido — isso é falha de criação, não começo limpo.
    """
    origem = Path.home() / ".claude.json"
    try:
        lido = json.loads(origem.read_text(encoding="utf-8"))
    except FileNotFoundError:
        dados: dict = {}
    except (OSError, ValueError) as e:
        raise ContaError(500, f"não consegui ler {origem}: {e}") from None
    else:
        if not isinstance(lido, dict):
            raise ContaError(500, f"{origem} não é um objeto JSON — conta não criada")
        dados = lido
    dados.pop("oauthAccount", None)
    (dir_conta / ".claude.json").write_text(json.dumps(dados, indent=2), encoding="utf-8")


def criar(nome: str) -> Path:
    """Cria a pasta pronta pro `/login`. NÃO loga — o OAuth abre navegador e é interativo.

    Rollback explícito: se qualquer passo falhar (sem `~/.claude` pra iterar, symlink recusado no
    Windows), a pasta inteira some. Conta pela metade apareceria no seletor e travaria o cadastro
    com 409 na tentativa seguinte.
    """
    dir_conta = caminho(nome)
    if dir_conta.exists():
        raise ContaError(409, f"já existe {dir_conta}")
    dir_conta.mkdir(parents=True)
    ok = False
    try:
        (dir_conta / MARCADOR).write_text("", encoding="utf-8")
        _semear_claude_json(dir_conta)
        (dir_conta / "projects").mkdir()
        compartilhado().mkdir(parents=True, exist_ok=True)
        reconciliar(nome)
        ok = True
    finally:
        if not ok and dir_conta.is_dir() and not dir_conta.is_symlink():
            shutil.rmtree(dir_conta)
    return dir_conta


def apagar(nome: str) -> None:
    """Some com a conta. Existe porque um nome digitado errado no cadastro ficaria pra sempre no
    seletor — `criar` só recusa sobrescrever, não desfaz.

    Os `.jsonl` daquela conta vão junto: são dela, e o gasto histórico dela sai do painel. Quem
    chama (a API) é quem checa se há sessão viva usando esta conta.
    """
    dir_conta = caminho(nome)
    if not e_conta(dir_conta):
        raise ContaError(404, f"{dir_conta} não é uma conta criada pelo hangar")
    shutil.rmtree(dir_conta)
