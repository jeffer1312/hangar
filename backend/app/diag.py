"""Diário de uso: um arquivo que a pessoa manda pra quem mantém o app.

Existe porque o problema quase nunca acontece na máquina de quem programa. Os três defeitos
consertados em 25/08/2026 — o clique que marcava a opção errada, a tira de atenção que não
respondia, as sessões sumindo — foram todos relatados de uma máquina Windows, por print e vídeo, e
nenhum deixou rastro que o autor pudesse ler. O `journalctl` só existe no Linux; do lado do
navegador não havia absolutamente nada.

O que é: um JSONL por DIA, uma linha por evento, em `<config>/.hangar-diag/`, alimentado pelas duas
pontas — a tela manda o que a pessoa fez e o que aconteceu, o backend acrescenta o que só ele vê (o
tmux que não respondeu, a opção que não convergiu). Sai da máquina só quando a pessoa aperta
"Baixar diagnóstico" e manda o arquivo; nada é enviado a lugar nenhum sozinho.

**Conteúdo de conversa NUNCA entra aqui.** Nem prompt, nem resposta do agente, nem texto digitado,
nem chave de API, nem conteúdo de arquivo. O que entra é o VERBO e o DESFECHO: qual ação, em qual
tela, deu certo ou não, com qual código e em quanto tempo. Essa linha é o que separa um arquivo que
a pessoa manda sem pensar duas vezes de um que ela não deveria mandar nunca. `_limpar` é onde ela é
imposta, e é de propósito que ele DESCARTA campo desconhecido em vez de deixar passar: campo novo
entra quando alguém o escreve em `_CAMPOS`, nunca por acidente de um cliente mais novo.

Por que um arquivo por DIA e não um só que rotaciona por tamanho: a pergunta real de quem analisa
tem data ("aconteceu hoje, e tinha acontecido sábado também" — o relato que originou isto). Cortar
por tamanho embaralha justamente esse limite, e um dia movimentado apaga a semana inteira. Assim
cada dia é um arquivo, guardam-se sete, e o mais velho sai sozinho. O teto POR DIA continua
existindo como rede contra laço maluco — ele para de gravar aquele dia em vez de encher o disco.
"""
import contextvars
import json
import logging
import os
import re
import subprocess
import sys
import threading
from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any

_log = logging.getLogger("hangar.diag")

# Id do pedido HTTP em curso, posto pelo middleware a partir do cabeçalho `X-Hangar-Req` que o
# front manda. É o que LIGA a linha da tela ("mandei POST /select e voltou 409") à linha do servidor
# ("o cursor do picker não convergiu") — sem ele as duas ficam soltas no arquivo, e reconstruir o
# que causou o quê depende de adivinhar por horário, que empata quando há duas telas abertas.
# contextvar, não parâmetro: `registrar` é chamado no fundo de handlers que não têm o request.
req_atual: contextvars.ContextVar[str] = contextvars.ContextVar("hangar_req", default="")

DIAS_GUARDADOS = 7
# Teto por DIA. Não é pra economizar disco — é pra o arquivo continuar mandável por chat, que é o
# único caminho dele até quem analisa. Estourou, aquele dia para de receber (com uma última linha
# dizendo isso), e os outros seguem normais.
_TETO_DIA = 4 * 1024 * 1024
_NOME = re.compile(r"^uso-(\d{4}-\d{2}-\d{2})\.jsonl$")

# Campos aceitos numa linha vinda da tela. Ver a regra de conteúdo no topo do módulo.
_CAMPOS: dict[str, type] = {
    "ts": str,          # horário do EVENTO na tela (ISO-8601). Ver _ts_da_tela
    "evento": str,      # verbo curto: "opcao.tocar", "sessao.abrir", "api.falhou", "js.erro"
    "nivel": str,       # ok | aviso | erro  (ver _NIVEIS)
    "tela": str,        # ONDE foi usado: chat, quadro, canvas, tira, config, terminal, arquivos…
    "sessao": str,      # nome da sessão, quando o evento é de uma
    "provider": str,    # claude / codex / pi / kimi
    "codigo": str,      # código do erro do backend (o `code` de mensagens.erro) ou o status HTTP
    "detalhe": str,     # texto CURTO de diagnóstico — nunca conteúdo de conversa
    "ms": int,          # quanto demorou
    "cli": str,         # id curto da aba/janela: amarra a linha ao evento app.abriu dela
    "req": str,         # id do pedido HTTP: amarra a linha da tela à do servidor (ver req_atual)
    "seq": int,         # contador da aba: ordem inequívoca quando duas linhas caem no mesmo ms
    "pilha": str,       # primeiras molduras do stack de um erro de JS
    # Plataforma. Vem UMA vez por carga de página, no evento `app.abriu`, e não em toda linha (que
    # multiplicaria o arquivo por nada) — o `cli` acima é o que liga as duas pontas.
    "so": str,          # Windows / macOS / Linux / Android / iOS
    "navegador": str,   # Chrome 141, Safari 18, Electron 33…
    "versao": str,      # versão do app (__HANGAR_VERSION__)
    "vista": str,       # desktop | celular — os dois caminhos de UI que costumam divergir
    "tela_px": str,     # 1920x1032 — quase todo defeito de layout precisa disto
}

def _git_describe() -> str:
    try:
        return subprocess.run(
            ["git", "describe", "--tags", "--always", "--dirty"],
            cwd=Path(__file__).resolve().parents[2], capture_output=True,
            text=True, timeout=5, encoding="utf-8", errors="replace").stdout.strip()
    except Exception:                                # noqa: BLE001 — versão nunca derruba nada
        return ""


# Commit do backend EM EXECUÇÃO, resolvido na importação (ou seja, quando o processo subiu) e nunca
# mais. Parece detalhe e não é: entre o `git pull` e o restart do serviço, o checkout já está no
# commit novo e o processo ainda é o velho. Lendo `git describe` na hora do download, o diário
# afirmaria estar rodando código que ninguém carregou — e a primeira pergunta de toda análise ("de
# qual versão veio isto?") sairia com a resposta errada, justamente na janela em que a máquina está
# meio atualizada, que é quando o defeito estranho aparece.
VERSAO_EM_EXECUCAO = _git_describe()

_NIVEIS = ("ok", "aviso", "erro")
_TETO_DETALHE = 300
_TETO_LOTE = 50        # linhas por POST: acima disso é ruído ou cliente com defeito

# Serializa o append: duas telas (celular e desktop) mandam lote ao mesmo tempo e o arquivo é um só.
_LOCK = threading.Lock()


def _base() -> Path:
    # Pasta NOVA — nunca existiu com o nome antigo, então não passa pela ponte de compatibilidade
    # do `migracao_sidecars`. O diário é do APARELHO, não de uma conta: fica no config dir em uso,
    # como os outros marcadores (`.hangar-status`, `.hangar-preview`).
    raiz = Path(os.environ.get("CLAUDE_CONFIG_DIR") or (Path.home() / ".claude"))
    return raiz / ".hangar-diag"


def caminho_do_dia(quando: date | None = None) -> Path:
    return _base() / f"uso-{(quando or date.today()).isoformat()}.jsonl"


def arquivos() -> list[Path]:
    """Os diários existentes, do mais ANTIGO pro mais novo."""
    try:
        achados = [(m.group(1), p) for p in _base().iterdir()
                   if (m := _NOME.match(p.name))]
    except OSError:
        return []
    return [p for _, p in sorted(achados)]


def _podar() -> None:
    # Guarda os N dias mais recentes. Roda no append, não num temporizador: o arquivo só cresce
    # quando há uso, e um temporizador seria uma tarefa viva pra fazer o que uma linha faz aqui.
    corte = (date.today() - timedelta(days=DIAS_GUARDADOS - 1)).isoformat()
    for p in arquivos():
        m = _NOME.match(p.name)
        if m and m.group(1) < corte:
            try:
                p.unlink()
            except OSError:
                _log.debug("diag: nao deu pra apagar %s", p.name, exc_info=True)


def _ts_da_tela(bruto: str) -> str:
    """Horário mandado pela tela, no fuso e no formato do arquivo. Vazio = não serve, usa o do envio.

    A tela agrupa o lote por até 4s antes de mandar, então carimbar tudo na chegada colava eventos
    distintos no mesmo instante e apagava a ordem — foi isso que impediu de reconstruir uma corrida
    entre remontagem e recarga em 26/08/2026.

    Só aceita horário do MESMO dia local do servidor: o arquivo é um por dia (`caminho_do_dia`), e
    um relógio errado no aparelho gravaria linha datada de outro dia dentro dele. Nesse caso o
    horário do envio é o menos errado dos dois, e o `seq` da aba continua ordenando as linhas.
    """
    try:
        quando = datetime.fromisoformat(bruto).astimezone()
    except ValueError:
        return ""
    agora = datetime.now().astimezone()
    if quando.date() != agora.date():
        return ""
    return quando.isoformat(timespec="milliseconds")


def _limpar(bruto: Any) -> dict[str, Any] | None:
    """Uma linha da tela virando uma linha do arquivo. Campo fora de `_CAMPOS` é DESCARTADO."""
    if not isinstance(bruto, dict):
        return None
    evento = bruto.get("evento")
    if not isinstance(evento, str) or not evento.strip():
        return None   # sem verbo não é evento
    fora: dict[str, Any] = {}
    for campo, tipo in _CAMPOS.items():
        valor = bruto.get(campo)
        # `bool` antes de `int`: em Python `True` É um int, e sem esta checagem um booleano mandado
        # no campo `ms` passaria como número.
        if isinstance(valor, bool) != (tipo is bool):
            continue
        if not isinstance(valor, tipo):
            continue
        if tipo is str:
            valor = valor.strip()[:_TETO_DETALHE]
            if not valor:
                continue
        fora[campo] = valor
    if "ts" in fora:
        fora["ts"] = _ts_da_tela(fora["ts"])
        if not fora["ts"]:
            del fora["ts"]
    if fora.get("nivel") not in _NIVEIS:
        # Nível ausente ou inventado vira "ok". Recusar a linha inteira por causa dele perderia o
        # evento, que é o dado; e deixar passar um valor livre quebraria qualquer contagem por nível.
        fora["nivel"] = "ok"
    return fora if fora.get("evento") else None


def registrar(evento: str, nivel: str = "ok", **campos: Any) -> None:
    """Um evento do PRÓPRIO backend, no mesmo arquivo da tela.

    Mesma regra do lado de cá: entra o que o backend viu acontecer, nunca o que a pessoa escreveu.
    Nunca levanta — um diário que derruba o pedido que ele deveria estar descrevendo é pior que
    diário nenhum.
    """
    try:
        linha = _limpar({**campos, "evento": evento, "nivel": nivel,
                         "req": req_atual.get()})
        if linha:
            linha["origem"] = "servidor"
            _escrever([linha])
    except Exception:                                # noqa: BLE001 — ver docstring
        _log.debug("diag: falhou ao registrar %r", evento, exc_info=True)


def _escrever(linhas: list[dict[str, Any]]) -> int:
    if not linhas:
        return 0
    arq = caminho_do_dia()
    arq.parent.mkdir(parents=True, exist_ok=True)
    # `**linha` DEPOIS do ts: a linha que trouxe horário próprio (o da tela, ver _ts_da_tela) fica
    # com ele; quem não trouxe — os eventos do próprio backend — herda o do envio, que pra eles é o
    # mesmo instante.
    agora = datetime.now().astimezone().isoformat(timespec="milliseconds")
    texto = "".join(
        json.dumps({"ts": agora, **linha}, ensure_ascii=False) + "\n" for linha in linhas
    )
    with _LOCK:
        try:
            tamanho = arq.stat().st_size if arq.exists() else 0
        except OSError:
            tamanho = 0
        if tamanho > _TETO_DIA:
            return 0
        if tamanho + len(texto.encode()) > _TETO_DIA:
            # Última linha do dia diz que parou — um arquivo que simplesmente cessa parece máquina
            # desligada, e é a leitura errada.
            texto += json.dumps({"ts": agora, "evento": "diag.teto", "nivel": "aviso",
                                 "origem": "servidor",
                                 "detalhe": f"teto de {_TETO_DIA} bytes no dia"}) + "\n"
        # Append direto, sem tmp+rename: a escrita é SEMPRE no fim e nunca reescreve o que já está
        # lá, então o padrão da casa (que protege quem substitui o arquivo inteiro) só faria
        # reescrever megabytes a cada evento. Uma linha cabe folgada no buffer do sistema.
        try:
            with arq.open("a", encoding="utf-8") as f:
                f.write(texto)
        except OSError:
            _log.debug("diag: nao deu pra gravar", exc_info=True)
            return 0
        _podar()
    return len(linhas)


def anotar_da_tela(lote: Any) -> int:
    """Grava um lote vindo do navegador. Devolve quantas linhas entraram."""
    if not isinstance(lote, list):
        return 0
    limpas = [d for d in (_limpar(x) for x in lote[:_TETO_LOTE]) if d]
    for d in limpas:
        d["origem"] = "tela"
    return _escrever(limpas)


def _cabecalho() -> str:
    """Primeira linha do download: o que o REPOSITÓRIO não pode contar.

    Quem analisa tem o repo, então o dicionário de campos não precisa vir aqui — `_CAMPOS`, logo
    acima, já é essa documentação, e repeti-la no arquivo só criaria uma segunda cópia pra
    envelhecer. O que o repo NÃO diz é o estado da máquina de onde o diário veio: em qual commit o
    backend está (o front manda o dele no `app.abriu`), qual sistema, e quantos dias o arquivo
    guarda. Sem o commit não dá pra saber se o defeito relatado já foi corrigido, e é a primeira
    pergunta de qualquer análise. JSON válido numa linha: não quebra parser de JSONL.
    """
    v = VERSAO_EM_EXECUCAO
    return json.dumps({
        "evento": "diag.formato",
        "ts": datetime.now().astimezone().isoformat(timespec="milliseconds"),
        "backend": v,
        "so_servidor": f"{os.name}/{sys.platform}",
        "dias_guardados": DIAS_GUARDADOS,
        "ordem": "mais antigo primeiro",
        "campos_documentados_em": "backend/app/diag.py (_CAMPOS) e frontend/src/lib/diag.ts",
        "nao_contem": "conversa, prompt, resposta do agente, chave de API, conteúdo de arquivo",
    }, ensure_ascii=False)


def ler_tudo() -> str:
    """Os sete dias concatenados, mais antigo primeiro — o corpo do download."""
    partes = [_cabecalho() + "\n"]
    for p in arquivos():
        try:
            partes.append(p.read_text(encoding="utf-8", errors="replace"))
        except OSError:
            continue
    return "".join(partes)


def ultimas(n: int = 60) -> list[dict[str, Any]]:
    """As N linhas mais RECENTES, já parseadas, mais novas primeiro.

    É o que a tela mostra pra pessoa conferir que o diário está mesmo gravando — sem isso o botão
    de baixar é fé: ela envia o arquivo e só quem recebe descobre que veio vazio. Lê de trás pra
    frente, arquivo por arquivo, e para assim que junta N: numa semana movimentada, parsear os sete
    dias inteiros pra mostrar sessenta linhas é trabalho jogado fora.
    """
    fora: list[dict[str, Any]] = []
    for p in reversed(arquivos()):
        try:
            linhas = p.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        for bruto in reversed(linhas):
            if not bruto.strip():
                continue
            try:
                obj = json.loads(bruto)
            except (json.JSONDecodeError, ValueError):
                continue   # linha truncada por queda no meio do append: pula, não derruba a tela
            if isinstance(obj, dict):
                fora.append(obj)
            if len(fora) >= n:
                return fora
    return fora


def resumo() -> dict[str, Any]:
    """Números pra tela decidir o que mostrar no botão (sem baixar o arquivo inteiro)."""
    total = 0
    for p in arquivos():
        try:
            total += p.stat().st_size
        except OSError:
            continue
    nomes = [p.name for p in arquivos()]
    return {"dias": len(nomes), "bytes": total, "arquivos": nomes,
            "dias_guardados": DIAS_GUARDADOS}
