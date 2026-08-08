"""paste_text: caminho normal (tmux) e o plano B de quem nao tem paste-buffer (psmux/Windows)."""
import subprocess
import time

from app import tmux


def _grava(monkeypatch, falha_paste: bool, trunca: bool = False):
    """Troca o RUN do tmux.py por um espiao. `falha_paste` simula o psmux VELHO, que devolve codigo
    != 0 no paste-buffer. `trunca` simula o que foi MEDIDO no psmux 3.3.7: rc=0 mentiroso, com o
    buffer cortado na primeira quebra de linha."""
    chamadas: list[list[str]] = []

    def fake(args, **kw):
        chamadas.append(args)
        rc = 1 if (falha_paste and "paste-buffer" in args) else 0
        # show-buffer do probe: `trunca` devolve so o que vem ANTES do \n, como o psmux faz.
        out = ("A\n" if trunca else "A\nB\n") if "show-buffer" in args else ""
        if kw.get("input") is not None:
            # load-buffer -: sem text=True, RUN devolve SEMPRE bytes e _run decodifica — o espiao
            # imita isso, senao o `.decode()` real quebraria contra um mock devolvendo str.
            return subprocess.CompletedProcess(args, rc, stdout=out.encode(), stderr=b"")
        return subprocess.CompletedProcess(args, rc, stdout=out, stderr="")

    monkeypatch.setattr(tmux, "RUN", fake)
    monkeypatch.setattr(tmux, "_TRUNCA_BUFFER", None)   # probe roda por teste, sem cache vazado
    return chamadas


def test_linux_usa_paste_buffer_e_nao_cai_no_plano_b(monkeypatch):
    # A garantia de nao-regressao: onde o paste-buffer funciona, NADA muda — duas chamadas (fora a
    # resolucao de pane do agentpane, Task 1), e nenhum send-keys. Se este teste comecar a ver
    # send-keys, o Linux regrediu.
    chamadas = _grava(monkeypatch, falha_paste=False)
    tmux.paste_text("s", "uma\nduas\ntres")
    verbos = [c[1] for c in chamadas
              if c[1] not in ("show-buffer", "delete-buffer", "list-panes")][1:]
    assert verbos == ["load-buffer", "paste-buffer"]   # [1:] tira o set-buffer do probe


def test_sem_paste_buffer_manda_linha_a_linha_com_cj(monkeypatch):
    chamadas = _grava(monkeypatch, falha_paste=True)
    tmux.paste_text("s", "uma\nduas\ntres")
    # depois do set-buffer + paste-buffer falho: linha, C-j, linha, C-j, linha
    depois = [c for c in chamadas if c[1] == "send-keys"]
    assert [c[-1] for c in depois] == ["uma", "C-j", "duas", "C-j", "tres"]
    # As linhas vao LITERAIS (-l --), senao um texto começando com '-' viraria flag do send-keys.
    assert all("-l" in c and "--" in c for c in depois if c[-1] != "C-j")


def test_nenhum_argumento_carrega_quebra_de_linha(monkeypatch):
    # O achado que motiva o plano B: com \n dentro do argumento o psmux engole tudo depois dele.
    chamadas = _grava(monkeypatch, falha_paste=True)
    tmux.paste_text("s", "uma\nduas")
    for c in chamadas:
        if c[1] == "send-keys":
            assert "\n" not in c[-1]


def test_linha_vazia_no_meio_vira_so_um_cj(monkeypatch):
    # "a\n\nb" tem uma linha vazia: ela nao pode virar um send-keys -l com string vazia (o tmux
    # trataria como argumento faltando), so a quebra.
    chamadas = _grava(monkeypatch, falha_paste=True)
    tmux.paste_text("s", "a\n\nb")
    assert [c[-1] for c in chamadas if c[1] == "send-keys"] == ["a", "C-j", "C-j", "b"]


def test_probe_detecta_multiplexador_que_trunca_e_vai_direto_pro_plano_b(monkeypatch):
    # O achado da sessao-irma no Windows: no psmux 3.3.7 o set-buffer devolve rc=0 e grava so ate a
    # primeira quebra ("ABC\nDEF\nGHI" vira "ABC"), e o paste-buffer TAMBEM devolve rc=0 entregando
    # nada. Confiar no rc mantinha o plano B — que funciona — desligado. Agora o probe pergunta ao
    # multiplexador o que ele faz e, se ele trunca, o paste-buffer nem e tentado.
    chamadas = _grava(monkeypatch, falha_paste=False, trunca=True)
    tmux.paste_text("s", "uma\nduas\ntres")
    assert not any(c[1] == "paste-buffer" for c in chamadas)   # nem tentou
    assert [c[-1] for c in chamadas if c[1] == "send-keys"] == ["uma", "C-j", "duas", "C-j", "tres"]


def test_falha_confirmada_no_meio_da_2a_linha_devolve_false_e_para(monkeypatch):
    # Achado CRITICO da review 02/08/2026: o retorno de _send_literal era descartado aqui e em
    # paste_text -- send_prompt so tinha a leitura da tela como prova, e com ela aceitando o COMECO
    # como evidencia (terminal_input._RESIDUO_INICIO) um texto que parasse no MEIO ainda parecia
    # "entregue". Agora a falha CONFIRMADA (rc != 0) tem que propagar: False, e PARAR (nao tentar a
    # 3a linha, que deixaria um buraco no meio do texto).
    chamadas = _grava(monkeypatch, falha_paste=True)   # cai no plano B (linha a linha)
    base_fake = tmux.RUN

    def fake_com_falha_na_segunda(args, **kw):
        if args[1] == "send-keys" and args[-1] == "duas":
            return subprocess.CompletedProcess(args, 1, stdout="", stderr="deu ruim")
        return base_fake(args, **kw)

    monkeypatch.setattr(tmux, "RUN", fake_com_falha_na_segunda)
    ok = tmux.paste_text("s", "uma\nduas\ntres")
    assert ok is False


def test_probe_e_por_capacidade_e_fica_em_cache(monkeypatch):
    # Uma vez por processo: o comportamento do multiplexador nao muda no meio da vida do backend, e o
    # probe custa 3 chamadas. E e por CAPACIDADE, nao por nome de SO — um tmux que passe a truncar
    # (ou um psmux que conserte) e tratado certo sem tocar no codigo.
    chamadas = _grava(monkeypatch, falha_paste=False, trunca=True)
    tmux.paste_text("s", "a\nb")
    tmux.paste_text("s", "c\nd")
    assert len([c for c in chamadas if c[1] == "show-buffer"]) == 1


# --- clipboard do Windows (Alt+V) -------------------------------------------------------------
#
# Medido na winboat em 08/08/2026 (psmux 3.3.7): o clipboard entrega as 600 linhas inteiras, com
# quebra de verdade, enquanto o caminho linha-a-linha mede 309 de 600 e devolve True. Ver
# docs/medicoes-2026-08-08-windows.md.


def _ok(stdout: bytes | str = b"") -> subprocess.CompletedProcess:
    return subprocess.CompletedProcess(["x"], 0, stdout=stdout, stderr=b"" if isinstance(stdout, bytes) else "")


def _falha(err: str = "deu ruim") -> subprocess.CompletedProcess:
    return subprocess.CompletedProcess(["x"], 1, stdout=b"", stderr=err.encode())


def test_clipboard_no_posix_e_no_op(monkeypatch):
    # O ramo inteiro e Windows-only. No Linux nao pode nem tentar escrever clipboard: o load-buffer -
    # ja entrega tudo, e o clipboard e da MAQUINA (sobrescreve o que o usuario copiou).
    chamadas = []
    monkeypatch.setattr(tmux, "RUN", lambda *a, **k: chamadas.append(a) or _ok())
    monkeypatch.setattr(tmux.os, "name", "posix")
    assert tmux.paste_via_clipboard("cc", "linha 1\nlinha 2") is False
    assert chamadas == []


def test_clipboard_falhando_nao_manda_a_tecla(monkeypatch):
    # Se o clipboard nao foi escrito, mandar M-v colaria a mensagem ANTERIOR — conteudo de outra
    # mensagem submetido como se fosse esta. Nada de tecla sem clipboard confirmado.
    monkeypatch.setattr(tmux.os, "name", "nt")
    monkeypatch.setattr(tmux, "RUN", lambda args, **kw: _falha())
    enviadas = []
    monkeypatch.setattr(tmux, "send_keys", lambda n, k, **kw: enviadas.append(k) or True)
    assert tmux.paste_via_clipboard("cc", "linha 1\nlinha 2") is False
    assert enviadas == []


def test_clipboard_manda_o_texto_por_stdin_e_uma_tecla_so(monkeypatch):
    # Duas garantias num teste: (1) a mensagem do usuario vai por STDIN, nunca na linha de comando —
    # o argv e world-readable em /proc e o quoting ja provou mutilar texto no caminho pro Windows;
    # (2) UMA tecla so. Medido: o rodape vira "paste again to expand", entao um segundo M-v EXPANDE
    # em vez de recolar, e o codigo nunca pode mandar dois achando que reforca.
    monkeypatch.setattr(tmux.os, "name", "nt")
    vistas = []
    monkeypatch.setattr(tmux, "RUN", lambda args, **kw: vistas.append((args, kw.get("input"))) or _ok())
    teclas = []
    monkeypatch.setattr(tmux, "send_keys", lambda n, k, **kw: teclas.append(k) or True)
    texto = "linha 1\nlinha 2 com acento ção e emoji 🚀"
    assert tmux.paste_via_clipboard("cc", texto) is True
    (args, entrada), = vistas
    assert entrada is not None and texto.encode("utf-8") in entrada
    assert texto not in " ".join(args)
    assert teclas == ["M-v"]


def test_clipboard_serializa_entre_sessoes(monkeypatch):
    # O clipboard e recurso GLOBAL da maquina. Sem lock de modulo, A escreve, B sobrescreve, e o
    # M-v de A cola o texto de B — com o placeholder novo aparecendo do mesmo jeito, entao a prova
    # nao ve. Este teste falha se alguem trocar o lock global por um lock por sessao.
    import threading
    monkeypatch.setattr(tmux.os, "name", "nt")
    ordem = []

    def fake(args, **kw):
        ordem.append(("clip", threading.current_thread().name))
        time.sleep(0.05)
        return _ok()

    monkeypatch.setattr(tmux, "RUN", fake)
    monkeypatch.setattr(tmux, "send_keys",
                        lambda n, k, **kw: ordem.append(("tecla", threading.current_thread().name)) or True)
    ts = [threading.Thread(target=tmux.paste_via_clipboard, args=(f"s{i}", "a\nb"), name=f"t{i}")
          for i in range(2)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    # Cada par (clip, tecla) tem que ser da MESMA thread e vir junto: sem intercalacao.
    pares = [ordem[i:i + 2] for i in range(0, len(ordem), 2)]
    assert all(p[0][1] == p[1][1] for p in pares), f"intercalou: {ordem}"


def test_clipboard_recusa_texto_vazio(monkeypatch):
    # Medido: `Set-Clipboard` com string vazia devolve rc=1 (o PowerShell casa o parametro como null)
    # e o clipboard fica com o conteudo ANTERIOR. Recusar antes de chamar, pra nao gastar 250ms de
    # PowerShell num caminho que so pode falhar.
    monkeypatch.setattr(tmux.os, "name", "nt")
    chamadas = []
    monkeypatch.setattr(tmux, "RUN", lambda args, **kw: chamadas.append(args) or _ok())
    teclas = []
    monkeypatch.setattr(tmux, "send_keys", lambda n, k, **kw: teclas.append(k) or True)
    assert tmux.paste_via_clipboard("cc", "") is False
    assert chamadas == [] and teclas == []
