"""Arvore de arquivos do repo DA SESSAO.

Nao reusa a allowlist do fs.py de proposito: la a raiz e a lista de projetos (o seletor
de pasta na criacao de sessao), aqui a raiz e o cwd da sessao. Mesma trava de caminho,
politica de raiz diferente.
"""

import hashlib
import os
import tempfile
from pathlib import Path

from app import atomico

MAX_ENTRADAS = 1000
MAX_BYTES = 512 * 1024


class FileError(Exception):
    def __init__(self, status: int, code: str, msg: str):
        super().__init__(msg)
        self.status, self.code, self.msg = status, code, msg


def _real(p: str) -> Path:
    return Path(os.path.realpath(os.path.expanduser(p)))


def _within(child: Path, root: Path) -> bool:
    return child == root or root in child.parents


def _protege_git(raiz: Path, alvo: Path) -> None:
    """Nenhum caminho que passe por uma pasta chamada `.git` e listado nem lido.

    Comparacao por COMPONENTE sobre o caminho JA RESOLVIDO (realpath): `.gitignore` e
    `.github/` continuam legiveis, e `atalho -> .git` nao escapa — a regua e do caminho
    fisico, nao da string que o cliente mandou. Decisao do usuario (15/08): um
    gerenciador de arquivos NAO descobre onde fica o git-dir — e a pasta de nome `.git`,
    e so. Consequencias aceitas, documentadas: sessao aberta DENTRO do proprio `.git`
    le (o terminal da sessao ja alcanca esses arquivos); `.git` quebrada a ponto do
    proprio git nao se reconhecer fica fora de escopo."""
    if alvo == raiz:
        return
    if ".git" in alvo.relative_to(raiz).parts:
        raise FileError(403, "erro_arq_area_do_git", "area interna do git")


def _resolver(cwd: str, path: str | None) -> tuple[Path, Path]:
    """Devolve (raiz, alvo) com o alvo provado dentro da raiz da sessao."""
    raiz = _real(cwd)
    # Recusado ANTES de virar caminho: um path assim acabaria como flag num comando git.
    if path and path.startswith("-"):
        raise FileError(400, "erro_arq_caminho_invalido", "caminho invalido")
    # NUL quebra realpath() e subprocess com ValueError solto — recusar antes dos dois.
    if path and "\x00" in path:
        raise FileError(400, "erro_arq_caminho_invalido", "caminho invalido")
    # So caminho RELATIVO: absoluto dentro do cwd tambem e recusado (o front so manda relativo).
    if path and os.path.isabs(path):
        raise FileError(400, "erro_arq_caminho_invalido", "caminho absoluto nao aceito")
    alvo = _real(os.path.join(cwd, path)) if path else raiz
    if not _within(alvo, raiz):
        raise FileError(400, "erro_arq_fora_da_raiz", "caminho sai da raiz da sessao")
    _protege_git(raiz, alvo)
    if not alvo.exists():
        raise FileError(404, "erro_arq_inexistente", "caminho nao existe")
    return raiz, alvo


def _prefixo_no_repo(cwd: str) -> str:
    """Onde o cwd fica DENTRO do repo, com barra no fim ('' se for o topo).

    Existe porque o git devolve caminho relativo ao TOPO do repositorio, e esta arvore
    lista relativo ao cwd DA SESSAO. Uma sessao aberta em /repo/backend recebia
    {"backend/app/x.py": "M"} do git e comparava com "app/x.py" — nunca casava, e a arvore
    voltava VAZIA no modo so_modificados (que e o padrao). Bug encontrado na auditoria do
    plano, antes de custar uma Task.
    """
    from app import git_ops
    try:
        p = git_ops._run(cwd, "rev-parse", "--show-prefix")
    except git_ops.GitError as e:
        # Falha de subprocesso (git ausente/timeout) NAO e "fora de repo" — e erro.
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    return p.stdout.strip() if p.returncode == 0 else ""


def _e_repo(cwd: str) -> bool:
    from app import git_ops
    try:
        p = git_ops._run(cwd, "rev-parse", "--is-inside-work-tree")
    except git_ops.GitError as e:
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    return p.returncode == 0 and p.stdout.strip() == "true"


def _marcas(cwd: str) -> dict[str, str]:
    """path RELATIVO AO CWD -> letra do porcelain. Fora de repo git, ERRO no envelope:
    arvore vazia escondia a falha (antes: except Exception -> {}, e so_modificados
    devolvia 200 [] como se nada tivesse mudado)."""
    from app import git_ops
    if not _e_repo(cwd):
        raise FileError(409, "erro_arq_nao_e_repo_git", "a arvore precisa de um repositorio git")
    pref = _prefixo_no_repo(cwd)
    try:
        brutas = git_ops.changed_files(cwd)
    except git_ops.GitError as e:
        # Status do git PRESERVADO (504 de timeout chega como 504): 500 fixo comeria o
        # status que o cliente usa pra decidir retry. O detalhe vai so pro log.
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    fora = {}
    for c in brutas:
        p = c["path"]
        if pref:
            if not p.startswith(pref):
                continue                    # mudou fora do cwd desta sessao: nao e da arvore
            p = p[len(pref):]
        fora[p.rstrip("/")] = c["code"].strip()[:1] or "?"
    return fora


def _numstat(cwd: str) -> dict[str, tuple[int, int]]:
    """path RELATIVO AO CWD -> (add, del). UMA chamada pro repo inteiro, nunca uma por arquivo.
    Contagem so sai quando o comando terminou bem: zero por falha seria resposta FALSA
    (marca M com add=0/del=0 mentirosos, medido no parecer 2ac646c)."""
    from app import git_ops
    pref = _prefixo_no_repo(cwd)
    # Sem HEAD nao ha o que contar. "Sem HEAD" e SO returncode 1 sem stderr (ausencia
    # normal); 128 com stderr e HEAD quebrado/falha real — virar lista vazia seria
    # esconder a falha (medido no parecer 72e866e3). GitError do probe escapa do envelope
    # se ficar fora do try — esta dentro, com o status preservado.
    try:
        probe = git_ops._run(cwd, "rev-parse", "--verify", "-q", "HEAD")
    except git_ops.GitError as e:
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    if probe.returncode == 1 and not probe.stderr.strip():
        return {}
    if probe.returncode != 0:
        raise FileError(500, "erro_arq_lista_falhou",
                        (probe.stderr or "rev-parse HEAD falhou").strip())
    try:
        p = git_ops._run(cwd, "-c", "core.quotePath=false", "diff", "--numstat", "HEAD")
    except git_ops.GitError as e:
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    if p.returncode != 0:
        raise FileError(500, "erro_arq_lista_falhou",
                        (p.stderr or "git diff --numstat falhou").strip())
    fora = {}
    for linha in p.stdout.splitlines():
        partes = linha.split("\t")
        if len(partes) != 3 or partes[0] == "-":       # "-" = binario
            continue
        cam = partes[2]
        if pref:
            if not cam.startswith(pref):
                continue
            cam = cam[len(pref):]
        fora[cam] = (int(partes[0]), int(partes[1]))
    return fora


def list_dir(cwd: str, path: str | None = None, so_modificados: bool = True) -> dict:
    raiz, alvo = _resolver(cwd, path)
    if not alvo.is_dir():
        raise FileError(400, "erro_arq_nao_e_pasta", "nao e uma pasta")

    # Fora de repo git a arvore FUNCIONA: lista tudo, sem marca e sem soma (regra do
    # usuario). so_modificados vira falso nesta chamada: sem marcas nao ha o que filtrar.
    if _e_repo(cwd):
        marcas, nums = _marcas(cwd), _numstat(cwd)
    else:
        marcas, nums, so_modificados = {}, {}, False
    entradas, cortou = [], False
    try:
        bruto = sorted(os.scandir(alvo), key=lambda e: (not e.is_dir(), e.name.lower()))
    except PermissionError:
        raise FileError(403, "erro_arq_sem_permissao", "sem permissao de leitura")
    except FileNotFoundError:
        raise FileError(404, "erro_arq_inexistente", "a pasta sumiu")
    except OSError:
        raise FileError(500, "erro_arq_lista_falhou", "nao deu pra ler a pasta")
    for e in bruto:
        filho = _real(e.path)
        if not _within(filho, raiz):        # symlink apontando pra fora
            continue
        try:
            _protege_git(raiz, filho)
        except FileError:
            continue                        # .git e atalho pra dentro dele nao aparecem
        try:
            tam = 0 if e.is_dir() else e.stat().st_size
        except OSError:                      # symlink quebrado: aparece, sem tamanho
            tam = 0
        rel = os.path.relpath(e.path, raiz)   # caminho LOGICO: o nome do link, nao o do alvo
        # Pasta herda a marca e SOMA o +N -M dos descendentes.
        dentro = [p for p in marcas if p == rel or p.startswith(rel + "/")]
        marca = marcas.get(rel) or (marcas[dentro[0]] if e.is_dir() and dentro else None)
        add = sum(nums.get(p, (0, 0))[0] for p in dentro)
        rem = sum(nums.get(p, (0, 0))[1] for p in dentro)
        if so_modificados and marca is None:
            continue
        if len(entradas) >= MAX_ENTRADAS:
            cortou = True
            break
        entradas.append({
            "name": e.name, "path": rel, "is_dir": e.is_dir(), "size": tam,
            "changed": marca, "add": add, "del": rem,
        })
    return {"entries": entradas, "truncated": cortou}


def read_file(cwd: str, path: str) -> dict:
    _raiz, alvo = _resolver(cwd, path)
    if alvo.is_dir():
        raise FileError(400, "erro_arq_e_pasta", "isso e uma pasta")
    if not alvo.is_file():
        raise FileError(400, "erro_arq_nao_e_arquivo", "nao e um arquivo comum")
    try:
        with alvo.open("rb") as fh:
            cabeca = fh.read(8192)
            if b"\x00" in cabeca:
                raise FileError(415, "erro_arq_binario", "arquivo binario")
            resto = fh.read(MAX_BYTES - len(cabeca) + 1)
            if b"\x00" in resto:
                raise FileError(415, "erro_arq_binario", "arquivo binario")
    except PermissionError:
        raise FileError(403, "erro_arq_sem_permissao", "sem permissao de leitura")
    bruto = cabeca + resto
    cortou = len(bruto) > MAX_BYTES
    return {
        "path": path,
        "text": bruto[:MAX_BYTES].decode("utf-8", errors="replace"),
        "size": alvo.stat().st_size,
        "truncated": cortou,
        # Impressão do que foi lido. Quem for gravar devolve isto e a gravação recusa se o
        # arquivo mudou no disco no meio — e aqui isso não é hipótese de manual: o agente da
        # sessão edita os mesmos arquivos o tempo todo, enquanto a tela está aberta.
        "digest": None if cortou else _digest(bruto),
    }


def _digest(dados: bytes) -> str:
    return hashlib.sha256(dados).hexdigest()


def write_file(cwd: str, path: str, texto: str, digest_lido: str | None) -> dict:
    """Grava `texto` no arquivo, recusando se ele mudou no disco desde a leitura.

    Mesmas travas de caminho da leitura (`_resolver` + `_protege_git`): sem isso a escrita seria
    um caminho novo pra sair da raiz da sessão ou tocar no `.git`.
    """
    _raiz, alvo = _resolver(cwd, path)
    if alvo.is_dir():
        raise FileError(400, "erro_arq_e_pasta", "isso e uma pasta")
    if not alvo.is_file():
        raise FileError(400, "erro_arq_nao_e_arquivo", "nao e um arquivo comum")
    if "\x00" in texto:
        raise FileError(415, "erro_arq_binario", "arquivo binario")
    novo = texto.encode("utf-8")
    if len(novo) > MAX_BYTES:
        raise FileError(413, "erro_arq_grande_demais", "arquivo grande demais")

    # Sem digest o cliente está gravando às cegas (leitura truncada, ou cliente antigo): recusa.
    # Um "salvar" que apaga em silêncio o que o agente acabou de escrever é o pior desfecho aqui.
    # Esta checagem vem ANTES de tocar no disco: rejeitar não precisa ler arquivo nenhum.
    if not digest_lido:
        raise FileError(409, "erro_arq_sem_digest", "sem a impressao da leitura")
    # Teto ANTES de ler: sem ele, um POST apontando pra um arquivo de gigabytes (dump, artefato de
    # build) fazia o backend puxar tudo pra memória só pra depois recusar o pedido. O `read_file`
    # nunca lê além de MAX_BYTES; a escrita não pode ser a porta que falta.
    try:
        tamanho_atual = alvo.stat().st_size
    except OSError as e:
        raise FileError(409, "erro_arq_sumiu", f"o arquivo sumiu do disco: {e}") from e
    if tamanho_atual > MAX_BYTES:
        raise FileError(413, "erro_arq_grande_demais", "arquivo grande demais")

    try:
        atual = alvo.read_bytes()
        modo = alvo.stat().st_mode
    except PermissionError as e:
        raise FileError(403, "erro_arq_sem_permissao", "sem permissao de leitura") from e
    except OSError as e:
        # Apagado entre o _resolver e agora: é o cenário que esta feature documenta (o agente da
        # sessão mexe nos mesmos arquivos), e sair cru daqui vira 500 sem o envelope de erro.
        raise FileError(409, "erro_arq_sumiu", f"o arquivo sumiu do disco: {e}") from e
    if _digest(atual) != digest_lido:
        raise FileError(409, "erro_arq_mudou_no_disco", "o arquivo mudou no disco")

    # tmp+rename no MESMO diretório (rename entre sistemas de arquivos falha), preservando o modo:
    # um arquivo executável não pode voltar sem o bit de execução.
    fd, tmp = tempfile.mkstemp(dir=str(alvo.parent), prefix=".hangar-escrita-")
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(novo)
            fh.flush()
            os.fsync(fh.fileno())
        os.chmod(tmp, modo & 0o7777)
        # Última conferência, colada no rename: entre a primeira leitura e aqui houve fsync, e o
        # agente escreve sem lock nenhum (ele usa o próprio Edit, direto no arquivo). Isto NÃO
        # fecha a janela — fechar de verdade exigiria o agente cooperar —, só a encolhe pro
        # menor tamanho que dá sem ele.
        # ponytail: teto conhecido, um flock aqui só protegeria contra nós mesmos.
        if _digest(alvo.read_bytes()) != digest_lido:
            raise FileError(409, "erro_arq_mudou_no_disco", "o arquivo mudou no disco")
        atomico.substituir(tmp, alvo)
    except FileError:
        os.unlink(tmp)
        raise
    except PermissionError as e:
        os.unlink(tmp)
        # No Windows o MESMO errno cobre duas coisas diferentes, e so uma delas e permissao:
        # `os.replace` levanta PermissionError quando outro processo esta com o destino ABERTO,
        # ainda que so pra leitura (medido — no POSIX o rename por cima de arquivo aberto sempre
        # funciona). Dizer "sem permissao" ali manda a pessoa conferir o ACL de um arquivo que ela
        # pode escrever: diagnostico errado, e o `atomico.substituir` ja retentou antes de chegar
        # aqui, entao quem sobrou e um processo segurando o arquivo de verdade.
        if atomico.em_uso(e):
            raise FileError(409, "erro_arq_em_uso", "arquivo aberto por outro programa") from e
        raise FileError(403, "erro_arq_sem_permissao", "sem permissao de escrita") from e
    except OSError as e:
        # Disco cheio, sistema de arquivos diferente, arquivo sumido: erro do app, com envelope,
        # nunca um 500 cru — mesma régua do `_run` do git_ops.
        os.unlink(tmp)
        raise FileError(409, "erro_arq_escrita_falhou", f"nao consegui gravar: {e}") from e
    return {"path": path, "size": len(novo), "digest": _digest(novo)}
