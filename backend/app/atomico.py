"""`substituir(origem, destino)` — o `os.replace` que sobrevive ao Windows.

No POSIX, renomear por cima de um arquivo ABERTO sempre funciona: quem esta lendo segue no inode
antigo e a entrada de diretorio troca embaixo dele. No Windows nao — o destino precisa ter sido
aberto com `FILE_SHARE_DELETE`, e o `open()` do Python NAO pede isso.

Medido nesta VM em 22/08/2026, com outro processo segurando o destino aberto SO PRA LEITURA:

    os.replace(tmp, destino)            -> PermissionError [WinError 5] Acesso negado
    open(destino, 'w').write(...)       -> OK

A ironia e o ponto: o `tmp+rename`, que o repo adotou POR SEGURANCA (um corte no meio nao pode
deixar JSON pela metade), e justamente o que falha no Windows — e falha no instante que ele existe
pra proteger, que e o leitor concorrente. O caminho inseguro, truncar por cima, passa.

Os leitores aqui sao CLIs vivos (o `claude` lendo o `.claude.json`/`settings.json`, o `cp-engine`
lendo o `engines.json`) e o proprio app lendo sidecar. Todos abrem e fecham rapido, entao a janela
e curta: isto nao e "sempre quebra", e a falha INTERMITENTE que aparece uma vez e nao reproduz.
Retentar por meio segundo cobre a janela real sem virar espera visivel.

Duas decisoes:

  - **POSIX fica byte-identico.** La nao ha o que retentar — o `os.replace` nao falha por leitor
    aberto —, entao o ramo POSIX e uma linha e nao existe laco nenhum. Um retry "inofensivo" no
    Linux mascararia PermissionError de verdade (permissao mesmo, ou destino em outro filesystem).
  - **Stdlib-only, de proposito.** O `engines.py` importa daqui e ele nao pode puxar `app.config`:
    arrastaria pydantic e quebraria o `scripts/cp-engine`, que o shell chama com o python do
    SISTEMA (invariante ja registrada no CLAUDE.md).

Esgotadas as tentativas, o `PermissionError` original sobe. Quem trata tem de dizer "arquivo em
uso", nao "sem permissao de escrita": no Windows a permissao existe: o que falta e o arquivo estar
livre. (Era o diagnostico errado do `filetree`.)
"""
import os
import time

# ~0.42s no pior caso (0.02+0.04+...+0.12), em espera crescente. Curto porque quem chama esta num
# request: o leitor tipico e um CLI que abre e fecha o JSON em milissegundos, e alem disso o que
# resta e um processo segurando o arquivo de verdade — ai esperar mais nao resolve, so demora.
_ESPERAS = (0.02, 0.04, 0.06, 0.08, 0.10, 0.12)

_E_WINDOWS = os.name == "nt"


def substituir(origem: str | os.PathLike, destino: str | os.PathLike) -> None:
    """`os.replace(origem, destino)`, com retentativa curta no Windows.

    Levanta o `PermissionError` da ULTIMA tentativa quando nao consegue — nunca engole a falha:
    perder a gravacao em silencio e o defeito que o tmp+rename existe pra impedir.
    """
    if not _E_WINDOWS:
        os.replace(origem, destino)
        return
    for espera in _ESPERAS:
        try:
            os.replace(origem, destino)
            return
        except PermissionError:
            time.sleep(espera)
    # Ultima tentativa fora do laco: assim o erro que sobe e o da tentativa final, com o traceback
    # apontando pra ca, e nao um erro guardado de uma tentativa antiga.
    os.replace(origem, destino)


def em_uso(erro: BaseException) -> bool:
    """`PermissionError` que, no Windows, quer dizer "alguem esta com o arquivo aberto".

    Serve pra quem precisa escolher a MENSAGEM: no Windows o mesmo errno cobre "sem permissao" e
    "arquivo em uso", e mandar a pessoa conferir permissao de um arquivo que ela pode escrever e
    manda-la pro lugar errado. No POSIX devolve False — la o rename nao falha por leitor aberto,
    entao PermissionError ali e permissao de verdade.
    """
    return _E_WINDOWS and isinstance(erro, PermissionError)


def explicar(erro: BaseException) -> str:
    """O erro em texto, ja com a palavra certa pra quem so vai LER a mensagem.

    O `em_uso` responde sim/nao pra quem escolhe um CODIGO de erro (o `filetree` faz isso: 409
    `erro_arq_em_uso` contra 403 `erro_arq_sem_permissao`, cada um com sua traducao). Este aqui e
    pro outro caso, o de quem monta uma frase com o `{e}` dentro e manda pra tela: ali o
    `[WinError 5] Acesso negado` chega intacto e diz a coisa errada — o usuario TEM permissao no
    arquivo, o que falta e ele estar livre.

    Fora do Windows devolve `str(erro)` e nada muda.
    """
    if em_uso(erro):
        return f"outro programa esta com o arquivo aberto ({erro})"
    return str(erro)
