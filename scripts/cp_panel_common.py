"""Resolução de servidor/token compartilhada pelos scripts do painel (cp-panel-data,
cp-panel-action). Módulo em vez de copiar: é o ponto que lê CREDENCIAL e monta a URL — duas
cópias divergindo aqui viraria bug de auth silencioso."""
import fcntl
import json
import os
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

BACKEND = Path(__file__).resolve().parent.parent / "backend"
TIMEOUT = 8
# Operação de ESCRITA (commit/push) precisa de folga MAIOR que o timeout do subprocesso git do
# backend (git_ops._TIMEOUT = 20). Com o timeout curto, um push lento (link ruim, diff grande)
# estourava aqui e virava "servidor inacessível" — mentira: o push seguia rodando lá e podia
# até completar, deixando o usuário sem saber se caiu ou não.
WRITE_TIMEOUT = 30


class PanelError(Exception):
    """Erro já legível pro usuário final (sem traceback)."""


def env(key: str) -> str:
    try:
        for line in (BACKEND / ".env").read_text(encoding="utf-8").splitlines():
            if line.startswith(f"{key}="):
                return line.split("=", 1)[1].strip()
    except OSError:
        pass
    return ""


def peer_fields(cfg: object) -> tuple[str, str, str]:
    """(base_url, token, web_url) de UMA entrada do peers.json, ou strings vazias se a entrada
    estiver malformada. Valida TIPO, não só presença: `"srv": "oops"` (não-dict) ou
    `"base_url": 8080` (sem aspas) passavam por checagem de verdade-lógica e só estouravam
    AttributeError lá no rstrip — fora de qualquer try, derrubando a coleta inteira."""
    if not isinstance(cfg, dict):
        return "", "", ""

    def s(key: str) -> str:
        v = cfg.get(key)
        return v.strip() if isinstance(v, str) else ""

    return s("base_url"), s("token"), s("web_url")


def enabled(cfg: object) -> bool:
    """Peer LIGADO pra varredura. `"enabled": false` no peers.json marca máquina que o dono sabe
    que está desligada: cada varredura pagava o timeout inteiro (4s) esperando host que não vai
    responder. Ausente = ligado, então peers.json antigo segue idêntico.

    Vale só pra ENUMERAÇÃO (painel, cp-send --list). Endereçar explicitamente
    (`cp-send pc::sessao`) continua resolvendo — senão "desativar" viraria "sumiu", e o usuário
    ia levar um "servidor desconhecido" sem entender que foi ele mesmo que desligou."""
    return not (isinstance(cfg, dict) and cfg.get("enabled") is False)


def peers() -> dict:
    """Mapa id -> {base_url, token, web_url?}. Ausente = máquina só-local (não é erro)."""
    try:
        data = json.loads((BACKEND / "peers.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as e:
        raise PanelError(f"peers.json inválido: {e}") from e
    # Raiz precisa ser objeto: JSON válido mas não-dict (um `[` sobrando vira lista) passava
    # direto e estourava AttributeError no .get/.items de quem chama — fora de qualquer try.
    if not isinstance(data, dict):
        raise PanelError("peers.json inválido: raiz deve ser um objeto {id: {...}}")
    return data


def set_peer_enabled(pid: str, on: bool, path: Path | None = None) -> None:
    """Grava o "enabled" de UM peer no peers.json (o toggle de servidor do painel).

    Escrita ATÔMICA (tmp + os.replace) sob lock exclusivo, preservando o modo do arquivo: o
    peers.json guarda os TOKENS da malha inteira, e um write cortado no meio deixaria todo
    servidor remoto inalcançável — estrago muito maior que os 4s de timeout que o toggle evita.
    Mora aqui, e não no cp-panel-action, porque este é o módulo que já lê o arquivo — e é
    importável, então dá pra testar a gravação sem mexer no peers.json de verdade."""
    path = path or (BACKEND / "peers.json")
    # Lock EXCLUSIVO cobrindo leitura+escrita: isto é read-modify-write do arquivo inteiro, e o
    # painel (Quickshell) e a CLI podem chamar ao mesmo tempo. Sem lock, quem grava por último
    # apaga a mudança do outro — sem erro, sem log. Sidecar e não o próprio peers.json porque o
    # os.replace troca o INODE: quem travasse o arquivo antigo seguraria um inode órfão.
    lock_path = path.with_name(path.name + ".lock")
    try:
        lock = open(lock_path, "w")
    except OSError as e:
        raise PanelError(f"não consegui abrir o lock do peers.json: {e}") from e

    with lock:
        fcntl.flock(lock, fcntl.LOCK_EX)

        # Sob o lock, qualquer .tmp que sobrou é órfão de um processo morto entre o write e o
        # replace (kill -9/OOM não roda except nenhum). Contém a malha inteira de tokens, então
        # não pode ficar apodrecendo no disco à espera da próxima gravação.
        for stale in path.parent.glob(path.name + ".*.tmp"):
            stale.unlink(missing_ok=True)

        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as e:
            raise PanelError(f"peers.json ilegível: {e}") from e
        if not isinstance(data, dict) or not isinstance(data.get(pid), dict):
            raise PanelError(f"servidor '{pid}' não está no peers.json")

        data[pid]["enabled"] = on
        # mkstemp e não um nome fixo, por DOIS motivos: (1) nome único — com "peers.json.tmp"
        # chumbado, dois processos abriam o MESMO caminho em modo truncate e o replace podia
        # promover conteúdo entrelaçado dos dois; (2) nasce 0600 — write_text criava com o umask
        # (medido: 0644), deixando os tokens todos legíveis por qualquer usuário local durante a
        # janela até o chmod. Aqui o arquivo nunca é mais frouxo que o destino.
        fd, tmp_name = tempfile.mkstemp(dir=path.parent, prefix=path.name + ".", suffix=".tmp")
        tmp = Path(tmp_name)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(json.dumps(data, indent=2, ensure_ascii=False) + "\n")
            # Só DEPOIS de escrito: alarga do 0600 do mkstemp pro modo real do original.
            os.chmod(tmp, path.stat().st_mode & 0o777)
            os.replace(tmp, path)
        except OSError as e:
            tmp.unlink(missing_ok=True)
            raise PanelError(f"falha ao gravar peers.json: {e}") from e


def local_base() -> str:
    return f"http://127.0.0.1:{env('CP_PORT') or '8765'}"


def resolve(address: str) -> tuple[str, str, str]:
    """'sessao' ou 'servidor::sessao' -> (base_url, token, nome_da_sessao)."""
    if "::" not in address:
        return local_base(), env("CP_AUTH_TOKEN"), address
    server, _, name = address.partition("::")
    known = peers()
    cfg = known.get(server)
    if not cfg:
        raise PanelError(f"servidor '{server}' desconhecido (conhecidos: {', '.join(known) or 'nenhum'})")
    base, token, _ = peer_fields(cfg)
    if not base or not token:
        raise PanelError(f"peers.json: entrada '{server}' sem base_url/token válidos (string)")
    return base.rstrip("/"), token, name


def api(address: str, method: str, path: str, body: dict | None = None, timeout: int = TIMEOUT):
    """Chama a API do servidor DONO da sessão. `path` usa {name} como placeholder do nome."""
    base, token, name = resolve(address)
    url = base + path.replace("{name}", urllib.parse.quote(name, safe=""))
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
                                 headers={"Authorization": f"Bearer {token}",
                                          "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode()
            try:
                return json.loads(raw) if raw else {}
            except ValueError as e:
                # 200 com corpo não-JSON (proxy devolvendo HTML, backend cuspindo texto):
                # sem isto o ValueError subia cru e quebrava o contrato "sempre imprime JSON".
                raise PanelError(f"resposta não-JSON de {base}: {e}") from e
    except urllib.error.HTTPError as e:
        # detail do FastAPI é a mensagem ÚTIL ("sessao nao encontrada"); sem extrair, o usuário
        # via só "HTTP 404" e não tinha como se corrigir.
        try:
            detail = json.loads(e.read().decode()).get("detail", "")
        except (ValueError, OSError):
            detail = ""
        raise PanelError(f"{e.code}: {detail or e.reason}") from e
    except (urllib.error.URLError, OSError) as e:
        raise PanelError(f"servidor inacessível: {e}") from e
