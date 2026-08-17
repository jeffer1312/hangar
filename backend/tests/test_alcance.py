"""Primitiva de alcance — bater num endereço e dizer se respondeu, em quanto tempo
e por que não. A Task 8 (peers) reusa `testar_endereco` sem reescrever: teto de
espera e motivos nomeados nascem aqui, uma vez.

Padrão da casa (precedente: engine_probe._buscar): toda chamada externa vive numa
função privada de I/O trocada no teste (`_bater`, `_detectar_lan`, `_nome_tailscale`).
Nenhum teste toca rede nem processo real.
"""
import pytest

from app import alcance
from app.config import Settings


def test_respondeu_com_tempo_medido(monkeypatch):
    monkeypatch.setattr(alcance, "_bater", lambda url: 12.0)
    assert alcance.testar_endereco("http://192.168.0.42:5173") == {
        "estado": "ok",
        "tempo_ms": 12,
        "motivo": "",
    }


def test_recusou_conexao(monkeypatch):
    def recusa(url):
        raise ConnectionRefusedError

    monkeypatch.setattr(alcance, "_bater", recusa)
    r = alcance.testar_endereco("http://192.168.0.42:5173")
    assert r["estado"] == "falhou"
    assert r["motivo"] == "recusou"


def test_estourou_o_teto_de_espera(monkeypatch):
    def demora(url):
        raise TimeoutError

    monkeypatch.setattr(alcance, "_bater", demora)
    r = alcance.testar_endereco("http://192.168.0.42:5173")
    assert r["estado"] == "falhou"
    assert r["motivo"] == "timeout"


def test_motivo_desconhecido_nao_engole(monkeypatch):
    def estranho(url):
        raise RuntimeError("dns quebrou de um jeito novo")

    monkeypatch.setattr(alcance, "_bater", estranho)
    r = alcance.testar_endereco("http://nada-existe.invalid")
    assert r["estado"] == "falhou"
    assert r["motivo"] == "erro"


def test_endereco_vazio_nunca_e_testado(monkeypatch):
    chamadas = []

    def registra(url):
        chamadas.append(url)
        return 1.0

    monkeypatch.setattr(alcance, "_bater", registra)
    assert alcance.testar_endereco("") == {
        "estado": "nao_configurado",
        "tempo_ms": None,
        "motivo": "",
    }
    assert chamadas == []  # vazio não é testado — não configurado não é defeito


def _ambiente(monkeypatch):
    monkeypatch.setattr(alcance, "_bater", lambda url: 12.0)
    monkeypatch.setattr(alcance, "_detectar_lan", lambda: "192.168.0.42")
    monkeypatch.setattr(alcance, "_nome_tailscale", lambda: "hangar.tail9c2f.ts.net")


def test_loopback_traz_alerta_e_a_linha_desta_maquina(monkeypatch):
    _ambiente(monkeypatch)
    r = alcance.levantar_estados(Settings(lan_bind_ip="127.0.0.1", front_port=5173, public_url=""))
    assert r["loopback"] is True
    assert r["bind"] == "127.0.0.1"
    assert [e["tipo"] for e in r["enderecos"]] == [
        "nesta_maquina", "rede_local", "tailscale", "publico",
    ]
    assert r["enderecos"][0]["url"] == "http://127.0.0.1:5173"


def test_bind_nao_loopback_nao_tem_linha_nesta_maquina(monkeypatch):
    _ambiente(monkeypatch)
    r = alcance.levantar_estados(Settings(lan_bind_ip="192.168.0.42", front_port=5173, public_url=""))
    assert r["loopback"] is False
    assert [e["tipo"] for e in r["enderecos"]] == [
        "rede_local", "tailscale", "publico",
    ]


def test_sem_public_url_e_sem_tailscale_volta_nao_configurado_sem_probar(monkeypatch):
    chamadas = []

    def registra(url):
        chamadas.append(url)
        return 1.0

    monkeypatch.setattr(alcance, "_bater", registra)
    monkeypatch.setattr(alcance, "_detectar_lan", lambda: "192.168.0.42")
    monkeypatch.setattr(alcance, "_nome_tailscale", lambda: "")
    r = alcance.levantar_estados(Settings(lan_bind_ip="192.168.0.42", front_port=5173, public_url=""))
    assert [e["tipo"] for e in r["enderecos"]] == ["rede_local", "publico"]
    assert r["enderecos"][1] == {
        "tipo": "publico",
        "url": "",
        "estado": "nao_configurado",
        "tempo_ms": None,
        "motivo": "",
    }
    # Só o rede_local foi testado: o público vazio NUNCA é probado.
    assert chamadas == ["http://192.168.0.42:5173"]


def test_public_url_entra_e_e_testada(monkeypatch):
    _ambiente(monkeypatch)
    r = alcance.levantar_estados(
        Settings(lan_bind_ip="192.168.0.42", front_port=5173, public_url="https://hangar.example.com/")
    )
    pub = r["enderecos"][2]
    assert pub["tipo"] == "publico"
    assert pub["url"] == "https://hangar.example.com"  # sem a barra final
    assert pub["estado"] == "ok"