"""provider_padrao_kimi: prefixo do default_model -> id de cota, com cache por mtime."""
import time

from app import cotas


def _cfg(tmp_path, texto: str):
    cfg = tmp_path / "config.toml"
    cfg.write_text(texto, encoding="utf-8")
    return cfg


def _aponta(monkeypatch, tmp_path):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    cotas._padrao_kimi = None


PROVIDER = '[providers.apikey]\ntype = "kimi"\napi_key = "sk-x"\nbase_url = "https://x"\n'


def test_prefixo_do_default_model(monkeypatch, tmp_path):
    _aponta(monkeypatch, tmp_path)
    _cfg(tmp_path, 'default_model = "apikey/k3"\n' + PROVIDER)
    assert cotas.provider_padrao_kimi() == "apikey"


def test_provider_sem_chave_nao_tem_cota(monkeypatch, tmp_path):
    # OAuth (managed:) ou provider sem api_key: o id "kimi:<nome>" não existe no /api/cotas —
    # None, pra pílula cair no pior-geral em vez de carregar id pendente (achado da revisão).
    _aponta(monkeypatch, tmp_path)
    _cfg(tmp_path, 'default_model = "managed:kimi-code/k3"\n[providers."managed:kimi-code"]\ntype = "kimi"\n')
    assert cotas.provider_padrao_kimi() is None


def test_config_ausente_ou_quebrado(monkeypatch, tmp_path):
    _aponta(monkeypatch, tmp_path)
    assert cotas.provider_padrao_kimi() is None            # sem arquivo
    cfg = _cfg(tmp_path, "isto nao = = toml\n")
    assert cotas.provider_padrao_kimi() is None            # TOML quebrado
    assert cotas._padrao_kimi is not None                  # e a falha FICA cacheada
    cfg.write_text('default_model = "apikey/k3"\n' + PROVIDER, encoding="utf-8")
    import os
    os.utime(cfg, (time.time() + 5, time.time() + 5))      # mtime novo destrava o cache
    assert cotas.provider_padrao_kimi() == "apikey"


def test_cache_por_mtime_nao_rele(monkeypatch, tmp_path):
    _aponta(monkeypatch, tmp_path)
    cfg = _cfg(tmp_path, 'default_model = "apikey/k3"\n' + PROVIDER)
    assert cotas.provider_padrao_kimi() == "apikey"
    # Conteúdo muda com o MESMO mtime -> a resposta continua a cacheada (não relê do disco).
    mt = cfg.stat().st_mtime
    cfg.write_text('default_model = "outro/k9"\n[providers.outro]\ntype = "kimi"\napi_key = "k"\nbase_url = "b"\n', encoding="utf-8")
    import os
    os.utime(cfg, (mt, mt))
    assert cotas.provider_padrao_kimi() == "apikey"
