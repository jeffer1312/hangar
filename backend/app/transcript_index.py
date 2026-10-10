"""Índice SQLite (FTS5) dos transcripts do Claude, para a busca não varrer GBs a cada tecla.

Transcript é append-only: cada arquivo guarda o byte até onde já foi lido e só as linhas completas
depois dele entram. Arquivo que encolheu ou trocou de inode é relido do zero. O backend é o único
escritor (uma thread em segundo plano); leitores abrem conexão própria, o WAL os deixa ler durante
a escrita. Enquanto a primeira construção não termina, `ready` fica falso e a busca usa o `rg`.

Também guarda o cabeçalho (cwd e 1ª mensagem) de cada arquivo e uma cópia da varredura de
Pi/Kimi/Codex, que é o que o Arquivo relia a cada listagem."""
import json
import logging
import os
import sqlite3
import threading
import time
from pathlib import Path
from typing import Optional

from app import log_paths
from app.transcript import parse_obj

_log = logging.getLogger("hangar.transcript_index")

SCHEMA_VERSION = 2
# Mesmo teto do `rg -M` da busca antiga: linha maior é saída de ferramenta ou contexto injetado.
_MAX_LINE_BYTES = 40000
_HEAD_LINES = 60        # o que archive._head_info lê
_INTERNAL_LINES = 30    # o que search._interno lê
_PER_FILE = 3
_INTERVAL = 45.0
# Boot já paga sondas de CLI e a coleta de custos; a construção entra depois, e a busca usa o rg até lá.
_START_DELAY = 90.0
# Busca com índice mais velho que isto dispara uma passada em segundo plano: a conversa de agora
# aparece na busca seguinte, e a desta tecla não espera o glob de todas as contas.
_STALE_ON_SEARCH = 5.0
# Mensagens casadas mais novas consideradas antes do teto por arquivo: termo comum não ordena tudo.
# ponytail: conversa com milhares de ocorrências ocupa o lote todo e sobram poucas conversas.
_CANDIDATES = 2000

_SCHEMA = """
CREATE TABLE files(
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    project TEXT NOT NULL,
    session_id TEXT NOT NULL,
    size INTEGER NOT NULL DEFAULT 0,
    mtime_ns INTEGER NOT NULL DEFAULT 0,
    ino INTEGER NOT NULL DEFAULT 0,
    offset INTEGER NOT NULL DEFAULT 0,
    lines INTEGER NOT NULL DEFAULT 0,
    cwd TEXT,
    preview TEXT NOT NULL DEFAULT '',
    head_done INTEGER NOT NULL DEFAULT 0,
    internal INTEGER
);
CREATE TABLE msg(
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    role TEXT NOT NULL,
    ts REAL,
    text TEXT NOT NULL
);
CREATE INDEX msg_file ON msg(file_id);
CREATE VIRTUAL TABLE msg_fts USING fts5(
    text, content='msg', content_rowid='id', tokenize='trigram remove_diacritics 1');
CREATE TRIGGER msg_ai AFTER INSERT ON msg BEGIN
    INSERT INTO msg_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER msg_ad AFTER DELETE ON msg BEGIN
    INSERT INTO msg_fts(msg_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT);
"""


def default_path() -> Optional[Path]:
    # Sob pytest nunca o índice real: a suíte não pode varrer nem gravar os transcripts da máquina.
    if "PYTEST_CURRENT_TEST" in os.environ:
        return None
    return log_paths.base().parent / "transcript-index.sqlite3"


def fts_query(terms: list[str]) -> Optional[str]:
    """Cada termo vira uma frase (`"rror"`), que no trigram casa como substring, igual ao rg: o
    texto do usuário nunca é sintaxe do FTS5. Termo com menos de 3 caracteres não forma trigrama;
    aí None e quem chama usa o rg."""
    if not terms or any(len(t) < 3 for t in terms):
        return None
    return " AND ".join('"' + t.replace('"', '""') + '"' for t in terms)


def _ino(st: os.stat_result) -> int:
    # No Windows o file ID passa de 64 bits (ReFS) ou usa o bit alto (NTFS); o INTEGER do
    # SQLite é com sinal e estouraria.
    return st.st_ino & 0x7FFF_FFFF_FFFF_FFFF


def _connect(path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(str(path), timeout=10, check_same_thread=False)
    try:
        conn.execute("PRAGMA busy_timeout=10000")
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA synchronous=NORMAL")
        # Sem teto, o WAL fica do tamanho do maior lote de escrita mesmo após o checkpoint.
        conn.execute("PRAGMA journal_size_limit=8388608")
    except BaseException:
        conn.close()
        raise
    return conn


def _open(path: Path) -> sqlite3.Connection:
    path.parent.mkdir(parents=True, exist_ok=True)
    for tentativa in (1, 2):
        conn = None
        try:
            conn = _connect(path)
            if conn.execute("PRAGMA user_version").fetchone()[0] != SCHEMA_VERSION:
                for (tipo, nome) in conn.execute(
                        "SELECT type, name FROM sqlite_master WHERE type IN ('table','trigger') "
                        "AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'msg_fts_%'").fetchall():
                    conn.execute(f'DROP {tipo.upper()} IF EXISTS "{nome}"')
                conn.executescript(_SCHEMA)
                conn.execute(f"PRAGMA user_version={SCHEMA_VERSION}")
                conn.commit()
            return conn
        except sqlite3.DatabaseError:
            # Conexão aberta segura o arquivo no Windows e o unlink falharia (WinError 32).
            if conn is not None:
                conn.close()
            # Índice é cache: arquivo corrompido se apaga e se reconstrói.
            if tentativa == 2:
                raise
            _log.warning("índice de transcripts ilegível, reconstruindo %s", path, exc_info=True)
            for suf in ("", "-wal", "-shm"):
                try:
                    os.unlink(f"{path}{suf}")
                except OSError:
                    pass
    raise AssertionError("inalcançável")


class Index:
    def __init__(self, path: Path):
        self.path = path
        self._lock = threading.Lock()
        self._conn = _open(path)
        row = self._conn.execute("SELECT v FROM meta WHERE k='built'").fetchone()
        self.ready = row is not None
        self.last_pass = 0.0
        # Cópia da última varredura de Pi/Kimi/Codex (archive_providers.conversas()).
        self.providers: Optional[list] = None

    # ── escrita ──────────────────────────────────────────────────────────────────
    def update(self, providers: bool = True) -> None:
        with self._lock:
            self._update(providers)

    def update_if_stale(self) -> None:
        """Nunca bloqueia quem busca: dispara uma passada numa thread e volta na hora. Uma por vez:
        a thread que não pega a trava sai sem fazer nada."""
        if time.monotonic() - self.last_pass < _STALE_ON_SEARCH or self._lock.locked():
            return
        threading.Thread(target=self._stale_pass, name="transcript-index-search", daemon=True).start()

    def _stale_pass(self) -> None:
        if not self._lock.acquire(blocking=False):
            return
        try:
            self._update(providers=False)
        except Exception:
            _log.warning("passada do índice na busca falhou", exc_info=True)
        finally:
            self._lock.release()

    def _update(self, providers: bool) -> None:
        if providers:
            from app import archive_providers
            self.providers = archive_providers.conversas()
        from app.archive import _contas, conversation_files
        atuais: dict[str, tuple[str, str, os.stat_result]] = {}
        for _cfg, _rot, base in _contas():
            try:
                projetos = [d for d in base.iterdir() if d.is_dir()]
            except OSError:
                continue
            for proj in projetos:
                for f in conversation_files(proj):
                    try:
                        atuais[str(f)] = (proj.name, f.stem, f.stat())
                    except OSError:
                        continue
        conn = self._conn
        conhecidos = {p: (fid, size, mtime, ino, off) for fid, p, size, mtime, ino, off in conn.execute(
            "SELECT id, path, size, mtime_ns, ino, offset FROM files")}
        for p in conhecidos.keys() - atuais.keys():
            fid = conhecidos[p][0]
            conn.execute("DELETE FROM msg WHERE file_id=?", (fid,))
            conn.execute("DELETE FROM files WHERE id=?", (fid,))
        conn.commit()
        prefixos = _internal_prefixes()
        # Mais velho primeiro: o id da mensagem acompanha a recência e a busca pega as mais novas
        # pelo rowid, sem ordenar todas as casadas. Até a construção terminar a busca usa o rg.
        for p, (proj, sid, st) in sorted(atuais.items(), key=lambda kv: kv[1][2].st_mtime_ns):
            antigo = conhecidos.get(p)
            if antigo and (antigo[1], antigo[2], antigo[3]) == (st.st_size, st.st_mtime_ns, _ino(st)):
                continue
            # Um arquivo = uma transação: linhas e offset entram juntos ou nenhum entra; sobra
            # parcial com offset velho viraria mensagem duplicada na passada seguinte.
            try:
                with conn:
                    self._ingest(p, proj, sid, st, antigo, prefixos)
            except OSError:
                _log.debug("índice: não consegui ler %s", p, exc_info=True)
            except Exception:
                _log.warning("índice: ingestão de %s falhou; fica para a próxima passada", p,
                             exc_info=True)
        if not self.ready:
            conn.execute("INSERT OR REPLACE INTO meta(k, v) VALUES ('built', ?)", (str(time.time()),))
            conn.commit()
            self.ready = True
        self.last_pass = time.monotonic()

    def _ingest(self, path: str, project: str, sid: str, st: os.stat_result,
                antigo: Optional[tuple], prefixos: tuple[str, ...]) -> None:
        conn = self._conn
        if antigo is None:
            fid = conn.execute("INSERT INTO files(path, project, session_id, ino) VALUES (?,?,?,?)",
                               (path, project, sid, _ino(st))).lastrowid
        else:
            fid = antigo[0]
            if antigo[3] != _ino(st) or st.st_size < antigo[4]:
                conn.execute("DELETE FROM msg WHERE file_id=?", (fid,))
                conn.execute("UPDATE files SET offset=0, lines=0, cwd=NULL, preview='', head_done=0, "
                             "internal=NULL, ino=? WHERE id=?", (_ino(st), fid))
        offset, lines, cwd, preview, head_done, internal = conn.execute(
            "SELECT offset, lines, cwd, preview, head_done, internal FROM files WHERE id=?", (fid,)).fetchone()
        from app.archive import _cortar, _texto_simples
        linhas: list[tuple] = []
        ilegiveis = 0
        with open(path, "rb") as fh:
            fh.seek(offset)
            for raw in fh:
                if not raw.endswith(b"\n"):
                    break   # linha ainda sendo escrita: entra na próxima passada
                offset += len(raw)
                lines += 1
                na_cabeca = not head_done or (internal is None and lines <= _INTERNAL_LINES)
                if len(raw) > _MAX_LINE_BYTES and not na_cabeca:
                    continue
                try:
                    obj = json.loads(raw)
                    evs = parse_obj(obj) if isinstance(obj, dict) else []
                except Exception:
                    # Linha que o parser não entende não pode travar o arquivo inteiro para sempre.
                    evs, obj = [], None
                    ilegiveis += 1
                    if ilegiveis == 1:
                        # Parser que regrediu indexaria a sessão como vazia e a busca diria "nada".
                        _log.warning("índice: linha ilegível em %s (byte %d); pulando",
                                     path, offset - len(raw), exc_info=True)
                msgs = [ev for ev in evs if ev.kind in ("user_msg", "assistant_msg") and ev.text]
                if not head_done:
                    c = obj.get("cwd") if isinstance(obj, dict) else None
                    if cwd is None and isinstance(c, str) and c:
                        cwd = c
                    if not preview:
                        u = next((ev for ev in msgs if ev.kind == "user_msg"), None)
                        if u is not None:
                            preview = _cortar(_texto_simples(u.text or ""))
                    head_done = int(bool(cwd and preview) or lines >= _HEAD_LINES)
                if internal is None:
                    u = next((ev for ev in msgs if ev.kind == "user_msg"), None)
                    if u is not None:
                        internal = int((u.text or "").lstrip().startswith(prefixos))
                    elif lines >= _INTERNAL_LINES:
                        internal = 0
                if len(raw) <= _MAX_LINE_BYTES:
                    linhas += [(fid, ev.id, "user" if ev.kind == "user_msg" else "assistant", ev.ts, ev.text)
                               for ev in msgs]
                if len(linhas) >= 5000:
                    conn.executemany("INSERT INTO msg(file_id, event_id, role, ts, text) VALUES (?,?,?,?,?)",
                                     linhas)
                    linhas = []
        conn.executemany("INSERT INTO msg(file_id, event_id, role, ts, text) VALUES (?,?,?,?,?)", linhas)
        # size/mtime da stat de antes da leitura: se o arquivo cresceu no meio, a próxima passada relê.
        conn.execute("UPDATE files SET size=?, mtime_ns=?, offset=?, lines=?, cwd=?, preview=?, "
                     "head_done=?, internal=? WHERE id=?",
                     (st.st_size, st.st_mtime_ns, offset, lines, cwd, preview, head_done, internal, fid))
        time.sleep(0)   # cede o GIL entre arquivos durante a construção

    # ── leitura ──────────────────────────────────────────────────────────────────
    def _reader(self) -> sqlite3.Connection:
        conn = sqlite3.connect(str(self.path), timeout=10)
        conn.execute("PRAGMA busy_timeout=10000")
        return conn

    def search(self, terms: list[str], limit: int) -> Optional[list[tuple]]:
        """(path, project, session_id, cwd, text, role, event_id, ts), arquivo mais recente primeiro,
        até 3 por arquivo. None = consulta não expressável no FTS (quem chama usa o rg)."""
        q = fts_query(terms)
        if q is None:
            return None
        self.update_if_stale()
        conn = self._reader()
        try:
            # O lote `c` sai do FTS em rowid decrescente e para no LIMIT: o teto por arquivo e a
            # ordem final rodam sobre ele, não sobre todas as mensagens que casam.
            return conn.execute("""
                WITH c AS (
                    SELECT m.id, m.file_id, f.mtime_ns
                    FROM msg_fts JOIN msg m ON m.id = msg_fts.rowid JOIN files f ON f.id = m.file_id
                    WHERE msg_fts MATCH ? AND COALESCE(f.internal, 0) = 0
                    ORDER BY msg_fts.rowid DESC LIMIT ?),
                h AS (
                    SELECT id, mtime_ns, ROW_NUMBER() OVER (PARTITION BY file_id ORDER BY id) AS rn
                    FROM c)
                SELECT f.path, f.project, f.session_id, f.cwd, m.text, m.role, m.event_id, m.ts
                FROM h JOIN msg m ON m.id = h.id JOIN files f ON f.id = m.file_id
                WHERE h.rn <= ? ORDER BY h.mtime_ns DESC, m.id LIMIT ?""",
                (q, _CANDIDATES, _PER_FILE, limit)).fetchall()
        finally:
            conn.close()

    def heads(self) -> dict[str, tuple[str, Optional[str]]]:
        """path -> (preview, cwd) dos arquivos cujo cabeçalho já está completo no índice."""
        conn = self._reader()
        try:
            return {p: (pv, cwd) for p, pv, cwd in
                    conn.execute("SELECT path, preview, cwd FROM files WHERE head_done=1")}
        finally:
            conn.close()


_current: Optional[Index] = None


def current() -> Optional[Index]:
    return _current


def _internal_prefixes() -> tuple[str, ...]:
    from app.search import _prefixos_internos
    return _prefixos_internos()


def _loop(idx: Index) -> None:
    time.sleep(_START_DELAY)
    while True:
        inicio = time.monotonic()
        try:
            idx.update()
            gasto = time.monotonic() - inicio
            # Passada normal leva milissegundos a cada 45 s: só a lenta merece linha no journal.
            (_log.info if gasto > 1.0 else _log.debug)("índice de transcripts atualizado em %.1fs", gasto)
        except Exception:
            _log.warning("passada do índice de transcripts falhou", exc_info=True)
        time.sleep(_INTERVAL)


def start_background() -> None:
    """Thread daemon, não task: a primeira construção leva quase um minuto e não pode segurar o
    encerramento do backend nem o loop de eventos."""
    global _current
    path = default_path()
    if path is None or _current is not None:
        return
    try:
        _current = Index(path)
    except Exception:
        _log.warning("índice de transcripts indisponível; a busca segue no rg", exc_info=True)
        # Sem índice a busca volta a levar segundos: fica no diário exportável, não só no journal.
        from app import diag
        diag.registrar("busca.indice_indisponivel")
        return
    threading.Thread(target=_loop, args=(_current,), name="transcript-index", daemon=True).start()
