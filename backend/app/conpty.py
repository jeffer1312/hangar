"""ConPTY do Windows por `ctypes` — o pty do motor de terminal daquele lado.

E stdlib pura, de proposito. O `pywinpty` chegou a entrar como dependencia condicional e SAIU
depois de medido (22/08/2026), por tres motivos independentes:

  1. ele nao usa o ConPTY do sistema: embarca `conpty.dll` + `OpenConsole.exe` proprios (do
     Windows Terminal, ~1,2 MB), entao nao havia handle aproveitavel pra entregar ao asyncio —
     e o handle e o ponto inteiro do Caminho A;
  2. `PTY.read()` devolve **`str`, nao `bytes`**, com laco de recompletar UTF-8 e um sentinela
     `'0011Ignore'` dentro do `PtyProcess`. Daqui ate o xterm.js isto e um cano de bytes CRUS
     (mesma escolha do `adapters/codex`): decodificar no meio e corromper o que nao for texto;
  3. o `PtyProcess` abre um `socket.bind(("127.0.0.1", 0))` **escutando** por sessao — superficie
     de rede que ninguem pediu num backend que ja e LAN/VPN-only.

O import deste modulo NAO carrega DLL nenhuma: `wintypes` so define tipos, e o `WinDLL` fica
preguicoso no `_k32()`. Isso e regra, nao estilo — o import do `winpty` tinha efeito colateral
(carregava a DLL do ConPTY) e derrubava `test_sessao_escondida_nao_muda_o_custo_da_listagem`
quando rodava junto com o test_api. Quem abre pty paga por ele; o resto do backend nao.
"""
import logging
import os
import sys
from typing import Optional

_log = logging.getLogger(__name__)

# `ctypes.wintypes` NAO importa fora do Windows (ele depende de `ctypes.HRESULT`, que so existe
# la). Mesmo motivo do `pty`/`termios` no termsock, so que ao contrario — por isso o guarda e de
# plataforma e nao um try/except: aqui a pergunta "existe ConPTY?" ja implica Windows.
if sys.platform == "win32":
    import ctypes
    from ctypes import wintypes
    import _winapi
    from asyncio import windows_utils

    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = 0x00020016
    EXTENDED_STARTUPINFO_PRESENT = 0x00080000
    CREATE_UNICODE_ENVIRONMENT = 0x00000400
    STARTF_USESTDHANDLES = 0x00000100

    class COORD(ctypes.Structure):
        _fields_ = [("X", wintypes.SHORT), ("Y", wintypes.SHORT)]

    class STARTUPINFOW(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD), ("lpReserved", wintypes.LPWSTR),
            ("lpDesktop", wintypes.LPWSTR), ("lpTitle", wintypes.LPWSTR),
            ("dwX", wintypes.DWORD), ("dwY", wintypes.DWORD),
            ("dwXSize", wintypes.DWORD), ("dwYSize", wintypes.DWORD),
            ("dwXCountChars", wintypes.DWORD), ("dwYCountChars", wintypes.DWORD),
            ("dwFillAttribute", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
            ("wShowWindow", wintypes.WORD), ("cbReserved2", wintypes.WORD),
            ("lpReserved2", ctypes.POINTER(wintypes.BYTE)),
            ("hStdInput", wintypes.HANDLE), ("hStdOutput", wintypes.HANDLE),
            ("hStdError", wintypes.HANDLE),
        ]

    class STARTUPINFOEXW(ctypes.Structure):
        _fields_ = [("StartupInfo", STARTUPINFOW), ("lpAttributeList", ctypes.c_void_p)]

    class PROCESS_INFORMATION(ctypes.Structure):
        _fields_ = [("hProcess", wintypes.HANDLE), ("hThread", wintypes.HANDLE),
                    ("dwProcessId", wintypes.DWORD), ("dwThreadId", wintypes.DWORD)]

    HPCON = wintypes.HANDLE

_k32_cache = None


def _k32():
    """kernel32 com as assinaturas ja declaradas. Preguicoso e cacheado (ver docstring do modulo)."""
    global _k32_cache
    if _k32_cache is not None:
        return _k32_cache
    k = ctypes.WinDLL("kernel32", use_last_error=True)
    # `restype = HRESULT` faz o ctypes LEVANTAR OSError sozinho quando o HRESULT e de falha — sem
    # isto um CreatePseudoConsole que falha devolve um HPCON nulo que so estoura muito depois.
    k.CreatePseudoConsole.argtypes = [COORD, wintypes.HANDLE, wintypes.HANDLE, wintypes.DWORD,
                                      ctypes.POINTER(HPCON)]
    k.CreatePseudoConsole.restype = ctypes.HRESULT
    k.ResizePseudoConsole.argtypes = [HPCON, COORD]
    k.ResizePseudoConsole.restype = ctypes.HRESULT
    k.ClosePseudoConsole.argtypes = [HPCON]
    k.ClosePseudoConsole.restype = None
    k.InitializeProcThreadAttributeList.argtypes = [
        ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, ctypes.POINTER(ctypes.c_size_t)]
    k.InitializeProcThreadAttributeList.restype = wintypes.BOOL
    k.UpdateProcThreadAttribute.argtypes = [
        ctypes.c_void_p, wintypes.DWORD, ctypes.c_size_t, ctypes.c_void_p,
        ctypes.c_size_t, ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t)]
    k.UpdateProcThreadAttribute.restype = wintypes.BOOL
    k.DeleteProcThreadAttributeList.argtypes = [ctypes.c_void_p]
    k.DeleteProcThreadAttributeList.restype = None
    k.CreateProcessW.argtypes = [
        wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p, ctypes.c_void_p,
        wintypes.BOOL, wintypes.DWORD, ctypes.c_void_p, wintypes.LPCWSTR,
        ctypes.c_void_p, ctypes.POINTER(PROCESS_INFORMATION)]
    k.CreateProcessW.restype = wintypes.BOOL
    k.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
    k.TerminateProcess.restype = wintypes.BOOL
    _k32_cache = k
    return k


_disponivel: Optional[bool] = None


def disponivel() -> bool:
    """Da pra abrir um ConPTY nesta maquina? Responde uma vez e guarda.

    `CreatePseudoConsole` so existe a partir do Windows 10 1809 — perguntar pelo simbolo e a
    pergunta de CAPACIDADE certa, do mesmo jeito que o lado POSIX pergunta se `import pty` carrega.
    """
    global _disponivel
    if _disponivel is None:
        if sys.platform != "win32":
            _disponivel = False
        else:
            try:
                _disponivel = hasattr(_k32(), "CreatePseudoConsole")
            except Exception:                    # noqa: BLE001 — qualquer falha e "nao da"
                _disponivel = False
    return _disponivel


def _bloco_env(env: dict) -> "ctypes.Array":
    """Bloco de ambiente do CreateProcessW: `K=V\\0K=V\\0...\\0` em UTF-16."""
    return ctypes.create_unicode_buffer("".join(f"{k}={v}\0" for k, v in env.items()) + "\0")


class ConPty:
    """Um pseudoconsole com um filho dentro. Cano de bytes, nada mais.

    As duas pontas de pipe do NOSSO lado saem daqui como handles crus e a posse delas passa pros
    transportes do asyncio (`PipeHandle` fecha o handle no `close()`): por isso `encerrar()` cuida
    so do filho e do HPCON — fechar aqui de novo seria double-close num handle que o processo ja
    pode ter reusado.
    """

    def __init__(self, hpc, pi, saida: int, entrada: int):
        self._hpc, self._pi = hpc, pi
        self.saida, self.entrada = saida, entrada    # nossos: ler / escrever
        self.pid = int(pi.dwProcessId)
        self._encerrado = False

    def redimensionar(self, cols: int, rows: int) -> None:
        if self._encerrado:
            return
        _k32().ResizePseudoConsole(self._hpc, COORD(cols, rows))

    def encerrar(self) -> None:
        """Mata o filho ANTES de fechar o pseudoconsole. Idempotente.

        A ordem nao e preferencia: `ClosePseudoConsole` pode TRAVAR esperando o cliente sair
        (microsoft/terminal#17716). E matar o nosso proprio processo de attach e justamente o
        desmonte que foi medido: o servidor solta AQUELE cliente, o cliente de outra sessao
        continua vivo e a sessao segue rodando. E o unico desmonte possivel no psmux, que nao
        tem identidade de cliente (`list-clients -F` e ignorado e todo cliente aparece com o
        mesmo tty ficticio) — `detach-client -s <sessao>` existiria, mas derruba TODOS os
        clientes daquela sessao, inclusive o `tmux attach` nativo do dono.
        """
        if self._encerrado:
            return
        self._encerrado = True
        k = _k32()
        # `TerminateProcess` e `restype = BOOL`: falha volta como 0, nunca como excecao. Um
        # `except OSError` aqui seria codigo morto, e a falha passaria batida ate o
        # `ClosePseudoConsole` — que trava esperando um cliente que continua vivo.
        erro_matar = None
        if not k.TerminateProcess(self._pi.hProcess, 1):
            erro_matar = ctypes.WinError(ctypes.get_last_error())
        saiu = False
        try:
            saiu = _winapi.WaitForSingleObject(self._pi.hProcess, 3000) == _winapi.WAIT_OBJECT_0
        except OSError:
            pass
        if saiu:
            k.ClosePseudoConsole(self._hpc)
        else:
            # Quem avisa e o WAIT, nao o TerminateProcess: com o filho ja saido sozinho (o usuario
            # digitou `exit`) ele devolve 0 com ERROR_ACCESS_DENIED — medido 3 de 3 no Windows —, e
            # avisar ali punha um WARNING em TODO fechamento de painel, afogando a falha de verdade.
            # Vazar o pseudoconsole (um `conhost.exe`) e o mal menor: fechar com o filho vivo
            # pendura esta thread pra sempre, e ela e um worker do `to_thread` do backend inteiro.
            _log.warning("conpty: filho pid=%s nao saiu em 3s; pseudoconsole nao foi fechado"
                         " (TerminateProcess: %s)", self.pid, erro_matar or "ok")
        for h in (self._pi.hProcess, self._pi.hThread):
            try:
                _winapi.CloseHandle(h)
            except OSError:
                pass


def abrir(cmdline: str, cols: int, rows: int, env: dict) -> ConPty:
    """Sobe `cmdline` dentro de um pseudoconsole novo. Bloqueante — chame em `to_thread`."""
    k = _k32()
    # SAIDA: o ConPTY escreve, a gente le. A NOSSA ponta e overlapped (e o que o
    # `connect_read_pipe` do Proactor exige); a ponta DELE e sincrona. A assimetria e o Caminho A
    # inteiro: o ConPTY nao aceita overlapped nos handles que RECEBE, mas a nossa ponta e nossa.
    saida_nossa, saida_conpty = windows_utils.pipe(overlapped=(True, False))
    # ENTRADA: `duplex=True` nao e capricho — o `_ProactorWritePipeTransport` dispara um `ReadFile`
    # de 16 bytes na PROPRIA ponta de escrita so pra detectar o pipe fechando, e com GENERIC_WRITE
    # puro isso volta WinError 5 (medido). E o mesmo par que o `windows_utils.Popen` usa pro stdin.
    entrada_conpty, entrada_nossa = windows_utils.pipe(overlapped=(False, True), duplex=True)

    hpc = HPCON()
    try:
        k.CreatePseudoConsole(COORD(cols, rows), entrada_conpty, saida_conpty, 0,
                              ctypes.byref(hpc))
    except OSError:
        for h in (saida_nossa, saida_conpty, entrada_conpty, entrada_nossa):
            try:
                _winapi.CloseHandle(h)
            except OSError:
                pass
        raise
    # As pontas DELE ja foram duplicadas pro conhost: fechar as nossas copias e o que faz o EOF
    # chegar aqui quando o filho morrer.
    _winapi.CloseHandle(saida_conpty)
    _winapi.CloseHandle(entrada_conpty)

    def _falhou(erro):
        # O `erro` chega pronto do chamador: `ClosePseudoConsole`/`CloseHandle` sobrescrevem o
        # last-error da thread, e o que sobraria pra mensagem seria "operacao concluida".
        k.ClosePseudoConsole(hpc)
        for h in (saida_nossa, entrada_nossa):
            try:
                _winapi.CloseHandle(h)
            except OSError:
                pass
        return erro

    tam = ctypes.c_size_t(0)
    k.InitializeProcThreadAttributeList(None, 1, 0, ctypes.byref(tam))   # 1a chamada: so o tamanho
    lista = ctypes.create_string_buffer(tam.value)
    if not k.InitializeProcThreadAttributeList(lista, 1, 0, ctypes.byref(tam)):
        raise _falhou(ctypes.WinError(ctypes.get_last_error()))   # lista ainda nao vale um Delete
    # `lpValue` e o HPCON POR VALOR, nao um ponteiro pra ele (medido: com `byref` o filho nasce
    # fora do pseudoconsole e o conhost morre sem emitir um byte).
    if not k.UpdateProcThreadAttribute(lista, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, hpc,
                                       ctypes.sizeof(HPCON), None, None):
        erro = ctypes.WinError(ctypes.get_last_error())
        k.DeleteProcThreadAttributeList(lista)
        raise _falhou(erro)

    si = STARTUPINFOEXW()
    si.StartupInfo.cb = ctypes.sizeof(STARTUPINFOEXW)
    si.lpAttributeList = ctypes.cast(lista, ctypes.c_void_p)
    # ---------------------------------------------------------------------------------------
    # A ARMADILHA de quem segue o exemplo oficial da Microsoft, medida em 22/08/2026.
    # Sem `STARTF_USESTDHANDLES` o CreateProcess propaga os std handles do PAI pro filho. Num
    # backend rodando como servico, com stdout indo pro log, o resultado e: o filho escreve NO
    # LOG e o pseudoconsole renderiza tela VAZIA. E o sintoma aponta pro lugar errado — dentro do
    # filho o `mode con` ja diz 120x30, ou seja, o attach ao pseudoconsole estava CERTO o tempo
    # todo; o que vazava era so o stdio. Limpar `HANDLE_FLAG_INHERIT` dos nossos std handles NAO
    # resolve (medido). O que resolve e ligar o flag com os TRES handles NULOS: ai o filho cai no
    # CONIN$/CONOUT$ do proprio pseudoconsole. O exemplo da MS "funciona" porque o pai dele e um
    # app de console cujos std handles JA sao de console — servico com stdout em arquivo e
    # exatamente o caso que quebra, e e o nosso.
    # ---------------------------------------------------------------------------------------
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES
    si.StartupInfo.hStdInput = None
    si.StartupInfo.hStdOutput = None
    si.StartupInfo.hStdError = None

    pi = PROCESS_INFORMATION()
    linha = ctypes.create_unicode_buffer(cmdline)   # CreateProcessW pode ESCREVER neste buffer
    ok = k.CreateProcessW(
        None, linha, None, None, False,
        EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
        _bloco_env(env), None, ctypes.byref(si), ctypes.byref(pi))
    err = ctypes.get_last_error()
    k.DeleteProcThreadAttributeList(lista)
    if not ok:
        k.ClosePseudoConsole(hpc)
        for h in (saida_nossa, entrada_nossa):
            try:
                _winapi.CloseHandle(h)
            except OSError:
                pass
        raise ctypes.WinError(err)
    return ConPty(hpc, pi, saida_nossa, entrada_nossa)
