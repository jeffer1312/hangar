"""`atomico.substituir` — o `os.replace` que sobrevive a um leitor concorrente no Windows.

O defeito e de plataforma, nao do multiplexador: reproduzido com dois pythons, sem tmux nenhum no
caminho. No Windows o destino do rename precisa ter sido aberto com FILE_SHARE_DELETE, e o `open()`
do Python nao pede isso — entao `os.replace` levanta PermissionError [WinError 5] quando outro
processo esta com o destino aberto SO PRA LEITURA. No POSIX isso nunca acontece.

Os casos abaixo nao dependem do sistema de quem roda: o comportamento do Windows e SIMULADO por um
`os.replace` falso que falha as primeiras vezes. Assim eles falham contra o codigo velho no Linux
tambem, que e a regra do projeto.
"""
import os
import subprocess
import sys
import time

import pytest

from app import atomico


def _falha_n_vezes(monkeypatch, n: int):
    """`os.replace` que levanta PermissionError nas `n` primeiras chamadas e depois funciona."""
    real = os.replace
    estado = {"chamadas": 0}

    def falso(origem, destino):
        estado["chamadas"] += 1
        if estado["chamadas"] <= n:
            raise PermissionError(5, "Acesso negado")
        return real(origem, destino)

    monkeypatch.setattr(atomico.os, "replace", falso)
    monkeypatch.setattr(atomico.time, "sleep", lambda _s: None)   # sem espera real no teste
    return estado


def test_retenta_no_windows_e_a_gravacao_chega(monkeypatch, tmp_path):
    monkeypatch.setattr(atomico, "_E_WINDOWS", True)
    estado = _falha_n_vezes(monkeypatch, 3)
    alvo = tmp_path / "a.json"
    alvo.write_text("velho", encoding="utf-8")
    tmp = tmp_path / "a.tmp"
    tmp.write_text("novo", encoding="utf-8")

    atomico.substituir(tmp, alvo)

    assert alvo.read_text(encoding="utf-8") == "novo"
    assert estado["chamadas"] == 4      # 3 falhas + a que passou


def test_esgotadas_as_tentativas_o_erro_SOBE(monkeypatch, tmp_path):
    """Nunca engolir: perder a gravacao em silencio e o defeito que o tmp+rename existe pra impedir."""
    monkeypatch.setattr(atomico, "_E_WINDOWS", True)
    _falha_n_vezes(monkeypatch, 99)
    alvo = tmp_path / "a.json"
    alvo.write_text("velho", encoding="utf-8")
    tmp = tmp_path / "a.tmp"
    tmp.write_text("novo", encoding="utf-8")

    with pytest.raises(PermissionError):
        atomico.substituir(tmp, alvo)
    assert alvo.read_text(encoding="utf-8") == "velho"


def test_posix_nao_retenta_nem_uma_vez(monkeypatch, tmp_path):
    """O ramo POSIX tem que ser byte-identico ao `os.replace` de antes.

    La o rename por cima de arquivo aberto NAO falha, entao um retry "inofensivo" so mascararia
    PermissionError de verdade — permissao mesmo, ou destino noutro filesystem.
    """
    monkeypatch.setattr(atomico, "_E_WINDOWS", False)
    estado = _falha_n_vezes(monkeypatch, 1)
    with pytest.raises(PermissionError):
        atomico.substituir(tmp_path / "x", tmp_path / "y")
    assert estado["chamadas"] == 1


def test_em_uso_so_diz_sim_no_windows(monkeypatch):
    """Quem escolhe a MENSAGEM precisa distinguir 'sem permissao' de 'arquivo aberto'."""
    monkeypatch.setattr(atomico, "_E_WINDOWS", True)
    assert atomico.em_uso(PermissionError(5, "Acesso negado")) is True
    assert atomico.em_uso(OSError("outra coisa")) is False
    monkeypatch.setattr(atomico, "_E_WINDOWS", False)
    assert atomico.em_uso(PermissionError(13, "Permission denied")) is False


@pytest.mark.skipif(os.name != "nt", reason="o defeito so existe no Windows")
def test_o_defeito_e_real_neste_sistema(tmp_path):
    """Sem simulacao: outro processo com o destino aberto SO PRA LEITURA derruba o os.replace cru.

    E a prova de que o modulo nao esta defendendo de um problema imaginario. Roda so no Windows —
    no Linux este caso passaria por acidente (o rename funciona) e nao provaria nada.
    """
    alvo = tmp_path / "alvo.json"
    alvo.write_text("1", encoding="utf-8")
    leitor = subprocess.Popen(
        [sys.executable, "-c", f"import time; f=open(r'{alvo}'); f.read(); time.sleep(6)"])
    try:
        time.sleep(1.5)
        tmp = tmp_path / "alvo.tmp"
        tmp.write_text("2", encoding="utf-8")
        with pytest.raises(PermissionError):
            os.replace(tmp, alvo)          # o codigo VELHO, cru
    finally:
        leitor.wait(timeout=15)
