#!/usr/bin/env python3
# UserPromptSubmit: diz ao Claude se esta sessão tem (ou pode ter) navegador embutido. Skill só
# dispara quando o pedido casa com a descrição, e "testa o login" não cita preview — sem este
# aviso a sessão vai de agent-browser sem saber que há um navegador dela na tela do usuário.
# Electron no ar = pid do ~/.hangar/nav/_srv.json vivo (o arquivo sobrevive a crash, só o pid
# decide, mesmo teste do CLI). Sem Electron, sem saída. Falha em silêncio.
import json
import os
import subprocess
import sys


def _nav_dir() -> str:
    return os.path.join(os.path.expanduser("~"), ".hangar", "nav")


def _pid_vivo(pid: object) -> bool:
    if not isinstance(pid, int) or pid <= 0:
        return False
    # No Windows `os.kill(pid, 0)` NÃO é sondagem: é TerminateProcess com código 0 — o hook
    # mataria o app a cada prompt. Lá a pergunta é feita ao kernel32.
    if os.name == "nt":
        import ctypes
        k32 = ctypes.windll.kernel32
        h = k32.OpenProcess(0x1000, False, pid)   # PROCESS_QUERY_LIMITED_INFORMATION
        if not h:
            return False
        try:
            codigo = ctypes.c_ulong()
            return bool(k32.GetExitCodeProcess(h, ctypes.byref(codigo))) and codigo.value == 259  # STILL_ACTIVE
        finally:
            k32.CloseHandle(h)
    try:
        os.kill(pid, 0)
        return True
    except PermissionError:
        return True
    except OSError:
        return False


def _nome_da_sessao() -> str | None:
    pane = os.environ.get("TMUX_PANE")
    if not pane:
        return None
    try:
        saida = subprocess.run(["tmux", "list-panes", "-a", "-F", "#{pane_id}\t#{session_name}"],
                               capture_output=True, text=True, timeout=1).stdout
    except Exception:
        return None
    achados = [l.split("\t", 1)[1] for l in saida.splitlines()
               if "\t" in l and l.split("\t", 1)[0] == pane]
    return achados[0] if len(achados) == 1 else None


def _url_do_navegador(nav_dir: str, nome: str) -> str | None:
    for arq in os.listdir(nav_dir):
        if not arq.endswith(".json") or arq.startswith(("_", ".")):
            continue
        try:
            with open(os.path.join(nav_dir, arq), encoding="utf-8") as fh:
                d = json.load(fh)
        except Exception:
            continue
        chave = d.get("chave") if isinstance(d, dict) else None
        if chave == nome or (isinstance(chave, str) and chave.endswith(f"::{nome}")):
            return str(d.get("url") or "")
    return None


def texto(url: str | None) -> str:
    if url:
        return (f"[hangar] Esta sessão tem navegador embutido aberto em {url}. Pra ver, ler, clicar, "
                "testar ou tirar print dessa página use `hangar-preview` (skill hangar-preview), "
                "não agent-browser nem ver-front.")
    return ("[hangar] O app desktop do Hangar está aberto: esta sessão pode abrir um navegador "
            "embutido com `hangar-preview open <url>` (skill hangar-preview). Pra ver ou testar "
            "página local prefira-o a agent-browser/ver-front — e avise o usuário, a janela dele muda.")


def main() -> None:
    nav_dir = _nav_dir()
    try:
        with open(os.path.join(nav_dir, "_srv.json"), encoding="utf-8") as fh:
            srv = json.load(fh)
    except Exception:
        return   # sem arquivo = sem Electron, o caso normal de quem usa só o celular
    if not isinstance(srv, dict) or not _pid_vivo(srv.get("pid")):
        return
    nome = _nome_da_sessao()
    url = _url_do_navegador(nav_dir, nome) if nome else None
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit",
                                             "additionalContext": texto(url)}}))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        pass
    sys.exit(0)
