"""Índice em SQLite do uso lido de todas as fontes (Claude, Codex, Pi, omp, Kimi).

Os transcripts só crescem no fim. Cada arquivo guarda até onde foi lido (`offset`) e o estado da
leitura naquele ponto (`estado`, a "dobra" em pickle): a próxima coleta lê só o que foi
acrescentado. Arquivo que encolheu, trocou de inode ou mudou antes do offset é relido do zero.

As linhas resultantes (custo e uso) ficam em tabelas e os relatórios as leem sob demanda; nada
do histórico fica em memória entre coletas. O índice é descartável: esquema diferente apaga e
refaz. Os `.json` do cache antigo na mesma pasta não são mais lidos.
"""
from __future__ import annotations

import itertools
import logging
import os
import pickle
import sqlite3
import threading
import time
import zlib
from collections import OrderedDict
from collections.abc import Callable, Iterable
from dataclasses import fields
from pathlib import Path
from typing import Protocol

from app import uso_areas
from app.uso_claude import UsoLinha

_log = logging.getLogger(__name__)

def _pasta_padrao() -> Path:
    # No Windows fica fora do perfil que roaming/OneDrive sincronizam: SQLite em WAL não
    # sobrevive a cópia no meio da escrita.
    if os.name == "nt":
        return Path(os.environ.get("LOCALAPPDATA") or (Path.home() / "AppData" / "Local")) / "hangar" / "custos"
    return Path.home() / ".claude" / ".hangar-custos"


_CACHE_DIR = _pasta_padrao()
_ARQUIVO = "custos.sqlite3"
# st_ino do Windows (ReFS/Dev Drive, NTFS com número de sequência) passa de 63 bits e estoura o
# INTEGER do SQLite.
_MASCARA_63 = 0x7FFF_FFFF_FFFF_FFFF
# Suba ao mudar tabelas, colunas ou o formato de qualquer dobra: o índice é apagado e refeito.
ESQUEMA = 1
# Bytes antes do offset conferidos na retomada: prova barata de que o começo não foi reescrito.
# ponytail: reescrita no mesmo inode que cresce e preserva esses bytes passa como anexo; os
# transcripts só crescem. Hash do prefixo inteiro se algum dia aparecer fonte que reescreve.
_CAUDA = 64
# Transação por lote na varredura: commit por arquivo seria um fsync de WAL por arquivo.
_LOTE_S = 1.0

# Ordem das colunas de custo = campos do `costs_sources.UsageRow`.
CAMPOS_CUSTO = ("ts", "source", "provider", "model", "project", "session_id", "input", "output",
                "cache_write", "cache_read", "subagente", "account_id", "codex_long_context",
                "cache_write_1h", "fast", "regravado", "regravado_1h")
# `conta` fica de fora: é carimbada na leitura, pela conta dona do escopo.
CAMPOS_USO = tuple(f.name for f in fields(UsoLinha) if f.name != "conta")
_BOOL_USO = tuple(i for i, f in enumerate(CAMPOS_USO) if f in ("fast", "subagente"))

# escopo -> (lidos, total) da coleta corrente; é o que o 202 "aquecendo" mostra.
progresso: dict[str, tuple[int, int]] = {}
# Sem disco (somente leitura, cheio): índice em memória compartilhada, preso por esta conexão.
_MEMORIA = "file:hangar-custos?mode=memory&cache=shared"
_ancora: sqlite3.Connection | None = None
_ancora_lock = threading.Lock()


class Dobra(Protocol):
    """Leitura retomável de um arquivo. Vai inteira pro pickle entre coletas."""

    def linha(self, bruta: bytes) -> None: ...

    def fechar(self) -> tuple[list, list[UsoLinha], tuple | None]:
        """(linhas de custo, linhas de uso sem área, entradas de área). Pode estragar a dobra."""
        ...


def zerar_progresso() -> None:
    progresso.clear()


def progresso_total() -> tuple[int, int]:
    """(lidos, total) somados de todas as varreduras em andamento."""
    lidos = total = 0
    for a, b in list(progresso.values()):
        lidos += a
        total += b
    return lidos, total


def invalidar() -> None:
    """Nada do índice mora em memória; existe pra quem simulava restart do cache antigo."""
    with _relatorios_lock:
        _relatorios.clear()


# -- relatórios prontos ------------------------------------------------------------------------

# Sobe a cada gravação que mudou alguma linha. ponytail: contador do processo; só o backend
# escreve no índice. Outro processo escrevendo exigiria um contador na tabela `meta`.
_seq = itertools.count(1)
_versao_dados = 0
# Montar o /api/uso lê e agrega o histórico inteiro; a tela repete o mesmo pedido a cada troca
# de aba ou filtro. Só entradas da versão corrente ficam, até `_RELATORIOS_MAX`: um relatório
# de custos inteiro pesa megabytes em objetos.
_RELATORIOS_MAX = 8
_relatorios: OrderedDict = OrderedDict()
_relatorios_lock = threading.Lock()


def mudou() -> None:
    """Invalida os relatórios prontos (dados ou o conjunto de escopos lidos mudaram)."""
    global _versao_dados
    _versao_dados = next(_seq)


def _talvez_mudou(conn: sqlite3.Connection) -> None:
    if conn.total_changes:
        mudou()


def relatorio(chave: tuple, montar: Callable[[], object]):
    """`montar()` memorizado por `chave` + versão dos dados + mapa de áreas. A versão é lida
    ANTES de montar: gravação no meio vira outra chave, nunca um relatório velho na nova."""
    chave = (_versao_dados, uso_areas.assinatura(), *chave)
    with _relatorios_lock:
        pronto = _relatorios.get(chave)
        if pronto is not None:
            _relatorios.move_to_end(chave)
            return pronto
    r = montar()
    with _relatorios_lock:
        for velha in [k for k in _relatorios if k[0] != chave[0]]:
            del _relatorios[velha]
        _relatorios[chave] = r
        while len(_relatorios) > _RELATORIOS_MAX:
            _relatorios.popitem(last=False)
    return r


# -- conexão -----------------------------------------------------------------------------------

_TABELAS = f"""
CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT);
CREATE TABLE files(id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, scope TEXT NOT NULL,
    versao TEXT NOT NULL, dev INTEGER, ino INTEGER, size INTEGER, mtime_ns INTEGER,
    offset INTEGER, cauda BLOB, estado BLOB, areas BLOB, areas_sig TEXT);
CREATE INDEX files_scope ON files(scope);
CREATE TABLE custo(file_id INTEGER NOT NULL, dia TEXT, {", ".join(CAMPOS_CUSTO)});
CREATE INDEX custo_file ON custo(file_id);
CREATE TABLE uso(file_id INTEGER NOT NULL, {", ".join(CAMPOS_USO)});
CREATE INDEX uso_file ON uso(file_id);
"""


def _preparar(conn: sqlite3.Connection) -> None:
    conn.execute("PRAGMA synchronous=NORMAL")
    try:
        atual = conn.execute("SELECT v FROM meta WHERE k='esquema'").fetchone()
    except sqlite3.OperationalError as e:
        if "no such table" not in str(e):
            raise
        atual = None
    if atual and atual[0] == str(ESQUEMA):
        return
    conn.execute("BEGIN IMMEDIATE")
    try:
        atual = None
        try:
            atual = conn.execute("SELECT v FROM meta WHERE k='esquema'").fetchone()
        except sqlite3.OperationalError as e:
            if "no such table" not in str(e):
                raise
        if not (atual and atual[0] == str(ESQUEMA)):
            for (nome,) in conn.execute(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'").fetchall():
                conn.execute(f'DROP TABLE "{nome}"')
            # `executescript` faria COMMIT implícito no meio da transação.
            for comando in _TABELAS.split(";"):
                if comando.strip():
                    conn.execute(comando)
            conn.execute("INSERT INTO meta VALUES ('esquema', ?)", (str(ESQUEMA),))
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise


def _abrir_arquivo(caminho: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(caminho, timeout=30, isolation_level=None, check_same_thread=False)
    try:
        conn.execute("PRAGMA journal_mode=WAL")
        _preparar(conn)
    except BaseException:
        conn.close()
        raise
    return conn


def _abrir() -> sqlite3.Connection:
    """Uma conexão por operação, fechada por quem abriu: nada fica preso a thread nem segura o
    arquivo depois (o Windows não apaga arquivo aberto)."""
    global _ancora
    caminho = _CACHE_DIR / _ARQUIVO
    try:
        _CACHE_DIR.mkdir(parents=True, exist_ok=True)
        try:
            return _abrir_arquivo(caminho)
        except sqlite3.OperationalError:
            raise
        except sqlite3.DatabaseError as e:
            # Arquivo que não é banco (corrompido, lixo): o índice é descartável. A conexão já
            # foi fechada em `_abrir_arquivo`; aberto em outra, o Windows recusa o unlink e cai
            # na memória abaixo.
            _log.warning("índice de custos ilegível (%r); refazendo", e)
            for sufixo in ("", "-wal", "-shm"):
                Path(f"{caminho}{sufixo}").unlink(missing_ok=True)
            return _abrir_arquivo(caminho)
    except sqlite3.OperationalError as e:
        # Outro escritor segurando o banco não é falta de disco: quem chamou tenta de novo.
        if "locked" in str(e) or "busy" in str(e):
            raise
        _log.warning("índice de custos sem disco (%r); usando memória", e)
    except (OSError, sqlite3.Error) as e:
        _log.warning("índice de custos sem disco (%r); usando memória", e)
    with _ancora_lock:
        if _ancora is None:
            _ancora = sqlite3.connect(_MEMORIA, uri=True, isolation_level=None, check_same_thread=False)
            _preparar(_ancora)
    conn = sqlite3.connect(_MEMORIA, uri=True, timeout=30, isolation_level=None, check_same_thread=False)
    _preparar(conn)
    return conn


# -- ingestão ----------------------------------------------------------------------------------

def _linha_custo(r) -> tuple:
    return (r.ts.strftime("%Y-%m-%d"), r.ts.isoformat(), *(getattr(r, c) for c in CAMPOS_CUSTO[1:]))


def _linha_uso(l: UsoLinha) -> tuple:
    return tuple(getattr(l, c) for c in CAMPOS_USO)


_INS_CUSTO = f"INSERT INTO custo VALUES (?, ?, {', '.join('?' * len(CAMPOS_CUSTO))})"
_INS_USO = f"INSERT INTO uso VALUES (?, {', '.join('?' * len(CAMPOS_USO))})"


def _ler_novo(p: Path, st: os.stat_result, reg: tuple | None, nova_dobra: Callable[[Path], Dobra],
              versao: str):
    """Lê o que falta de `p` e devolve (offset, cauda, estado, saída, tamanho lido)."""
    try:
        return _ler(p, st, reg, nova_dobra, versao)
    except OSError:
        raise
    except Exception:
        if reg is None:
            raise
        # Dobra que despicklou mas não serve mais (classe mudou sem subir a versão): relê inteiro.
        _log.warning("retomada de %s falhou; relendo do zero", p, exc_info=True)
        return _ler(p, st, None, nova_dobra, versao)


def _ler(p: Path, st: os.stat_result, reg: tuple | None, nova_dobra: Callable[[Path], Dobra],
         versao: str):
    dobra = None
    offset = 0
    with open(p, "rb") as f:
        if reg is not None:
            _id, _scope, r_versao, dev, ino, _size, _mtime, r_offset, cauda, estado = reg[:10]
            if (estado is not None and r_versao == versao and (dev, ino) == _identidade(st)
                    and r_offset <= st.st_size):
                f.seek(r_offset - len(cauda))
                if f.read(len(cauda)) == cauda:
                    try:
                        dobra = pickle.loads(zlib.decompress(estado))
                        offset = r_offset
                    except Exception:
                        _log.warning("estado de %s ilegível; relendo do zero", p, exc_info=True)
                        dobra = None
        if dobra is None:
            dobra = nova_dobra(p)
            offset = 0
        f.seek(offset)
        fragmento = b""
        for bruta in f:
            if bruta.endswith(b"\n"):
                dobra.linha(bruta)
                offset += len(bruta)
            else:
                fragmento = bruta
        inicio = max(0, offset - _CAUDA)
        f.seek(inicio)
        cauda = f.read(offset - inicio)
    # O estado é salvo ANTES da linha sem `\n`: ela pode estar no meio da escrita e voltar maior.
    estado = zlib.compress(pickle.dumps(dobra, protocol=pickle.HIGHEST_PROTOCOL), 1)
    if fragmento:
        dobra.linha(fragmento)
    return offset, cauda, estado, dobra.fechar(), offset + len(fragmento)


def _areas_blob(entradas) -> bytes | None:
    return None if entradas is None else zlib.compress(pickle.dumps(entradas, protocol=pickle.HIGHEST_PROTOCOL), 1)


def _gravar_arquivo(conn: sqlite3.Connection, p: Path, st: os.stat_result, scope: str,
                    versao: str, lido, manter_escopo: bool = False) -> None:
    offset, cauda, estado, (custos, usos, entradas), tamanho = lido
    sig = uso_areas.assinatura()
    # Upsert, não "INSERT se não conhecia": o custo de uma sessão (`sincronizar_arquivo`) grava
    # fora da varredura e pode ter inserido o mesmo caminho entre a leitura e a escrita dela.
    file_id = conn.execute(
        "INSERT INTO files(scope, versao, dev, ino, size, mtime_ns, offset, cauda, estado, areas,"
        " areas_sig, path) VALUES (?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(path) DO UPDATE SET"
        " scope=IIF(?, files.scope, excluded.scope), versao=excluded.versao, dev=excluded.dev, ino=excluded.ino,"
        " size=excluded.size, mtime_ns=excluded.mtime_ns, offset=excluded.offset,"
        " cauda=excluded.cauda, estado=excluded.estado, areas=excluded.areas,"
        " areas_sig=excluded.areas_sig RETURNING id",
        (scope, versao, *_identidade(st), tamanho, st.st_mtime_ns, offset, cauda, estado,
         _areas_blob(entradas), sig, str(p), manter_escopo)).fetchone()[0]
    conn.execute("DELETE FROM custo WHERE file_id=?", (file_id,))
    conn.execute("DELETE FROM uso WHERE file_id=?", (file_id,))
    conn.executemany(_INS_CUSTO, [(file_id, *_linha_custo(r)) for r in custos])
    conn.executemany(_INS_USO, [(file_id, *_linha_uso(l)) for l in usos])


_SEL_FILE = "SELECT id, scope, versao, dev, ino, size, mtime_ns, offset, cauda, estado FROM files"


def _identidade(st: os.stat_result) -> tuple[int, int]:
    """(dev, ino) do arquivo. No Windows o ino pode vir 0: aí sobram o tamanho que encolheu e os
    bytes antes do offset pra provar que é o mesmo arquivo."""
    return st.st_dev & _MASCARA_63, st.st_ino & _MASCARA_63


def _em_dia(reg: tuple, st: os.stat_result, versao: str) -> bool:
    return reg[2] == versao and (reg[3], reg[4], reg[5], reg[6]) == (
        *_identidade(st), st.st_size, st.st_mtime_ns)


def sincronizar(scope: str, arquivos: Iterable[Path], nova_dobra: Callable[[Path], Dobra],
                versao: str) -> None:
    """Deixa o índice do escopo igual aos `arquivos`: lê o que cresceu, relê o que mudou e
    esquece o que sumiu. Nunca vai à rede. Arquivo que falha na leitura vira log e mantém as
    linhas anteriores."""
    lista = list(arquivos)
    versao = f"{ESQUEMA}:{versao}"
    conn = _abrir()
    try:
        # Sem o estado (BLOB grande): ele só é lido de quem mudou.
        conhecidos = {row[0]: row[1:] for row in conn.execute(
            "SELECT path, id, scope, versao, dev, ino, size, mtime_ns FROM files WHERE scope=?",
            (scope,))}
        progresso[scope] = (0, len(lista))
        em_transacao = False
        pendentes: list[tuple] = []
        inicio_lote = time.monotonic()

        def gravar_pendentes() -> None:
            # A leitura fica fora da transação: segurar o lock de escrita enquanto se lê o
            # próximo arquivo travaria o custo da sessão aberta (`sincronizar_arquivo`).
            nonlocal em_transacao
            if not pendentes:
                return
            conn.execute("BEGIN IMMEDIATE")
            em_transacao = True
            for args in pendentes:
                _gravar_arquivo(conn, *args)
            conn.execute("COMMIT")
            em_transacao = False
            pendentes.clear()
            mudou()

        try:
            for i, p in enumerate(lista):
                progresso[scope] = (i, len(lista))
                chave = str(p)
                leve = conhecidos.pop(chave, None)
                try:
                    st = os.stat(p)
                except FileNotFoundError:
                    if leve is not None:
                        conhecidos[chave] = leve
                    continue
                except OSError:
                    # Falha passageira não é sumiço: apagar aqui jogaria fora o histórico dele.
                    continue
                if leve is not None and _em_dia(leve, st, versao):
                    continue
                reg = conn.execute(_SEL_FILE + " WHERE path=?", (chave,)).fetchone()
                if reg is not None and _em_dia(reg, st, versao):
                    # Veio de outro escopo (outra conta leu o mesmo arquivo antes).
                    conn.execute("UPDATE files SET scope=? WHERE id=?", (scope, reg[0]))
                    continue
                try:
                    lido = _ler_novo(p, st, reg, nova_dobra, versao)
                except OSError as e:
                    _log.warning("custos: %s não pôde ser lido: %r", p, e)
                    continue
                except Exception:
                    # Um arquivo que derruba o leitor não pode parar a varredura de todos.
                    _log.warning("custos: %s não pôde ser lido", p, exc_info=True)
                    continue
                pendentes.append((p, st, scope, versao, lido))
                if time.monotonic() - inicio_lote > _LOTE_S:
                    gravar_pendentes()
                    inicio_lote = time.monotonic()
            conn.execute("BEGIN IMMEDIATE")
            em_transacao = True
            for args in pendentes:
                _gravar_arquivo(conn, *args)
            sumidos = [(v[0],) for v in conhecidos.values()]
            conn.executemany("DELETE FROM custo WHERE file_id=?", sumidos)
            conn.executemany("DELETE FROM uso WHERE file_id=?", sumidos)
            conn.executemany("DELETE FROM files WHERE id=?", sumidos)
            conn.execute("COMMIT")
            em_transacao = False
        finally:
            if em_transacao:
                conn.execute("ROLLBACK")
            # Fica marcado como concluído (não some): a barra da tela soma todas as fontes.
            progresso[scope] = (len(lista), len(lista))
    finally:
        _talvez_mudou(conn)
        conn.close()


def sincronizar_arquivo(p: Path, nova_dobra: Callable[[Path], Dobra], versao: str,
                        scope: str) -> int | None:
    """Um arquivo só (custo de uma sessão aberta), fora da varredura. Mantém o escopo que ele já
    tinha; `scope` vale só pra quem ainda não está no índice. Devolve o id, ou None se sumiu."""
    versao = f"{ESQUEMA}:{versao}"
    conn = _abrir()
    try:
        st = os.stat(p)
        reg = conn.execute(_SEL_FILE + " WHERE path=?", (str(p),)).fetchone()
        if reg is not None and _em_dia(reg, st, versao):
            return reg[0]
        lido = _ler_novo(p, st, reg, nova_dobra, versao)
        try:
            conn.execute("BEGIN IMMEDIATE")
        except sqlite3.OperationalError as e:
            # Índice ocupado além do timeout: estimativa indisponível agora, não erro 500.
            _log.warning("custos: índice ocupado ao gravar %s: %s", p, e)
            return None
        try:
            # A varredura pode ter dado dono ao caminho enquanto este lia: o escopo dela vence.
            _gravar_arquivo(conn, p, st, scope, versao, lido, manter_escopo=True)
            conn.execute("COMMIT")
        except BaseException:
            conn.execute("ROLLBACK")
            raise
        return conn.execute("SELECT id FROM files WHERE path=?", (str(p),)).fetchone()[0]
    except OSError:
        return None
    finally:
        _talvez_mudou(conn)
        conn.close()


def esquecer_fora(ativos: set[str]) -> None:
    """Apaga do índice o arquivo que sumiu do disco e não está em nenhum escopo varrido (conta
    removida, sessão avulsa apagada). Os escopos varridos já esquecem os seus em `sincronizar`."""
    conn = _abrir()
    try:
        velhos = [(i,) for i, scope, path in conn.execute("SELECT id, scope, path FROM files")
                  if scope not in ativos and not os.path.exists(path)]
        if not velhos:
            return
        conn.execute("BEGIN IMMEDIATE")
        try:
            conn.executemany("DELETE FROM custo WHERE file_id=?", velhos)
            conn.executemany("DELETE FROM uso WHERE file_id=?", velhos)
            conn.executemany("DELETE FROM files WHERE id=?", velhos)
            conn.execute("COMMIT")
        except BaseException:
            conn.execute("ROLLBACK")
            raise
    finally:
        _talvez_mudou(conn)
        conn.close()


# -- leitura -----------------------------------------------------------------------------------

def ler_custos(scope: str | None = None, desde: str | None = None, file_id: int | None = None) -> list[tuple]:
    """Linhas de custo (colunas `CAMPOS_CUSTO`, `ts` em ISO) de um escopo ou de um arquivo, na
    ordem do caminho e, dentro dele, como o arquivo as produziu. `desde` = primeiro dia
    (YYYY-MM-DD) incluído."""
    sql = f"SELECT {', '.join('c.' + c for c in CAMPOS_CUSTO)} FROM custo c"
    where, args = _filtro(scope, desde, file_id, "c")
    conn = _abrir()
    try:
        return conn.execute(f"{sql} WHERE {where} ORDER BY (SELECT path FROM files WHERE id=c.file_id), c.rowid", args).fetchall()
    finally:
        conn.close()


def ler_usos(scope: str, conta: str, desde: str | None = None) -> list[UsoLinha]:
    a, b = _BOOL_USO
    return [UsoLinha(*t[:a], bool(t[a]), *t[a + 1:b], bool(t[b]), *t[b + 1:-1], conta)
            for t in iter_usage_rows(scope, conta, desde)]


# Colunas na ordem dos campos de `UsoLinha`, com a conta entrando como parâmetro.
_SELECT_USAGE = "SELECT " + ", ".join("?" if f.name == "conta" else f"u.{f.name}" for f in fields(UsoLinha))


def iter_usage_rows(scope: str, conta: str, desde: str | None = None) -> Iterable[tuple]:
    """Linhas de uso como tuplas na ordem dos campos de `UsoLinha` (booleanos como 0/1), sem um
    objeto por linha. Lidas de uma vez: leitura aberta durante a soma segura o checkpoint do WAL."""
    where, args = _filtro(scope, desde, None, "u")
    conn = _abrir()
    try:
        return conn.execute(f"{_SELECT_USAGE} FROM uso u WHERE {where} ORDER BY (SELECT path FROM files WHERE id=u.file_id), u.rowid", (conta, *args)).fetchall()
    finally:
        conn.close()


def _filtro(scope, desde, file_id, t: str) -> tuple[str, tuple]:
    # Subconsulta em vez de JOIN: o plano lê só as linhas dos arquivos do escopo pelo índice.
    where, args = [], []
    if scope is not None:
        where.append(f"{t}.file_id IN (SELECT id FROM files WHERE scope = ?)")
        args.append(scope)
    if file_id is not None:
        where.append(f"{t}.file_id = ?")
        args.append(file_id)
    if desde:
        where.append(f"{t}.dia >= ?")
        args.append(desde)
    return " AND ".join(where) or "1", tuple(args)


def listar(raiz: Path, casa: Callable[[str], bool]) -> list[Path]:
    """Arquivos sob `raiz` cujo nome `casa`, sem seguir link de pasta — o `rglob` sem o custo de
    montar e casar um Path por entrada."""
    out: list[Path] = []
    pilha = [os.fspath(raiz)]
    while pilha:
        try:
            it = os.scandir(pilha.pop())
        except OSError:
            continue
        with it:
            for e in it:
                try:
                    if e.is_dir(follow_symlinks=False):
                        pilha.append(e.path)
                    elif casa(e.name):
                        out.append(Path(e.path))
                except OSError:
                    continue
    # A ordem do filesystem não pode alterar o arredondamento das somas.
    return sorted(out, key=str)
