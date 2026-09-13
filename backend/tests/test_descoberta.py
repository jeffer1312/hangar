"""Descoberta de máquinas na tailnet: só peer online que responde como Hangar entra na lista.
Nenhum teste toca rede nem processo — `_status_tailscale` e `_bater` são trocados."""
import pytest

from app import alcance, descoberta, peers_check

STATUS = {
    "Self": {"DNSName": "eu.tail1.ts.net.", "TailscaleIPs": ["100.64.0.1"]},
    "Peer": {
        "a": {"HostName": "notebook", "DNSName": "notebook.tail1.ts.net.", "Online": True,
              "TailscaleIPs": ["100.64.0.2", "fd7a::2"]},
        "b": {"HostName": "desligado", "DNSName": "desligado.tail1.ts.net.", "Online": False,
              "TailscaleIPs": ["100.64.0.3"]},
        "c": {"HostName": "nas", "DNSName": "nas.tail1.ts.net.", "Online": True,
              "TailscaleIPs": ["100.64.0.4"]},
        "d": {"HostName": "vps", "DNSName": "vps.tail1.ts.net.", "Online": True,
              "TailscaleIPs": ["100.64.0.5"]},
        "e": {"HostName": "antigo", "DNSName": "antigo.tail1.ts.net.", "Online": True,
              "TailscaleIPs": ["100.64.0.6"]},
        "f": {"HostName": "podre", "DNSName": "", "Online": True, "TailscaleIPs": None},
    },
}
NAO_AUTORIZADO = {"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}}


def test_so_hangar_online_entra(monkeypatch):
    monkeypatch.setattr(alcance, "_status_tailscale", lambda: STATUS)
    caminhos = set()

    def bater(url, path="/api/peers/identificador", token=None):
        caminhos.add(path)
        if url.startswith("http://100.64.0.2:"):
            return 200, {"hangar": True}
        if url == "https://vps.tail1.ts.net":       # só pelo tailscale serve; a porta crua não abre
            return 200, {"hangar": True}
        if url.startswith("http://100.64.0.6:"):    # Hangar sem /ping ainda: o 401 nomeado identifica
            return 401, NAO_AUTORIZADO
        if url.startswith("http://100.64.0.4:"):
            return 401, {"error": "nginx"}          # outra coisa respondendo 401 não é Hangar
        raise ConnectionRefusedError

    monkeypatch.setattr(peers_check, "_bater", bater)
    assert descoberta.descobrir() == [
        {"nome": "antigo", "base_url": "http://100.64.0.6:8765", "hosts": ["100.64.0.6", "antigo.tail1.ts.net"]},
        {"nome": "notebook", "base_url": "http://100.64.0.2:8765", "hosts": ["100.64.0.2", "notebook.tail1.ts.net"]},
        {"nome": "vps", "base_url": "https://vps.tail1.ts.net", "hosts": ["100.64.0.5", "vps.tail1.ts.net"]},
    ]
    assert caminhos == {"/api/peers/ping"}   # nunca a rota autenticada: sem token ela conta tentativa errada lá


def test_sem_tailscale_e_erro_nomeado_nao_lista_vazia(monkeypatch):
    monkeypatch.setattr(alcance, "_status_tailscale", lambda: {})
    monkeypatch.setattr(peers_check, "_bater", lambda *a, **k: (_ for _ in ()).throw(AssertionError("não devia bater")))
    with pytest.raises(descoberta.SemTailscale):
        descoberta.descobrir()


def test_nome_tailscale_continua_lendo_o_self(monkeypatch):
    monkeypatch.setattr(alcance, "_status_tailscale", lambda: STATUS)
    assert alcance._nome_tailscale() == "eu.tail1.ts.net"
