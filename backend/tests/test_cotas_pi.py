"""conta_de_provider_pi: provider do Pi -> id de conta, casado pela CHAVE (não pelo nome).

O caso real: o Pi chama de "kimi-coding" a mesma credencial que o Kimi Code chama de "apikey".
"""
import json
import os
import time

from app import cotas


def _monta(monkeypatch, tmp_path, *, auth: dict, models: dict | None = None,
           kimi: str = "", engines: dict | None = None):
    pi = tmp_path / "pi"
    pi.mkdir(exist_ok=True)
    (pi / "auth.json").write_text(json.dumps(auth), encoding="utf-8")
    if models is not None:
        (pi / "models.json").write_text(json.dumps(models), encoding="utf-8")
    (tmp_path / "config.toml").write_text(kimi, encoding="utf-8")
    eng = tmp_path / "engines.json"
    eng.write_text(json.dumps(engines or {}), encoding="utf-8")
    monkeypatch.setattr(cotas, "_PI_AGENT", pi)
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    monkeypatch.setenv("CP_ENGINES_FILE", str(eng))
    cotas._mapa_pi_cache = None


KIMI = '[providers.apikey]\ntype = "kimi"\napi_key = "sk-kimi-1"\nbase_url = "https://x"\n'


def test_mesma_chave_com_outro_nome_e_a_mesma_conta(monkeypatch, tmp_path):
    _monta(monkeypatch, tmp_path,
           auth={"kimi-coding": {"type": "api_key", "key": "sk-kimi-1"}}, kimi=KIMI)
    assert cotas.conta_de_provider_pi("kimi-coding") == "kimi:apikey"


def test_provider_de_motor_casa_com_chave(monkeypatch, tmp_path):
    _monta(monkeypatch, tmp_path,
           auth={"zen": {"type": "api_key", "key": "sk-z"}},
           engines={"deepseek": {"api_key": "sk-z", "base_url": "https://z"}})
    assert cotas.conta_de_provider_pi("zen") == "chave:deepseek"


def test_chave_desconhecida_e_oauth_ficam_sem_conta(monkeypatch, tmp_path):
    # OAuth não tem `key`, e chave que não é de nenhuma fonte não vira id: None faz a pílula cair
    # no pior-geral, que é o comportamento de antes — id inventado não casaria com nada.
    _monta(monkeypatch, tmp_path,
           auth={"codex": {"type": "oauth", "access": "j.w.t"},
                 "outro": {"type": "api_key", "key": "sk-nao-conhecida"}}, kimi=KIMI)
    assert cotas.conta_de_provider_pi("codex") is None
    assert cotas.conta_de_provider_pi("outro") is None
    assert cotas.conta_de_provider_pi(None) is None


def test_provider_manual_do_models_json(monkeypatch, tmp_path):
    _monta(monkeypatch, tmp_path, auth={},
           models={"providers": {"hcn": {"apiKey": "sk-kimi-1"}}}, kimi=KIMI)
    assert cotas.conta_de_provider_pi("hcn") == "kimi:apikey"


def test_cache_por_mtime(monkeypatch, tmp_path):
    _monta(monkeypatch, tmp_path,
           auth={"kimi-coding": {"type": "api_key", "key": "sk-kimi-1"}}, kimi=KIMI)
    assert cotas.conta_de_provider_pi("kimi-coding") == "kimi:apikey"
    auth = tmp_path / "pi" / "auth.json"
    mt = auth.stat().st_mtime
    auth.write_text(json.dumps({"kimi-coding": {"key": "sk-outra"}}), encoding="utf-8")
    os.utime(auth, (mt, mt))                                  # mesmo mtime -> resposta cacheada
    assert cotas.conta_de_provider_pi("kimi-coding") == "kimi:apikey"
    os.utime(auth, (time.time() + 5, time.time() + 5))        # mtime novo destrava
    assert cotas.conta_de_provider_pi("kimi-coding") is None
