import json
import pytest
from app import tts


def test_hash_muda_com_voz_e_provedor():
    a = tts.hash_de("oi", "voz1", "elevenlabs")
    b = tts.hash_de("oi", "voz2", "elevenlabs")
    c = tts.hash_de("oi", "voz1", "local")
    assert a != b != c and a != c
    assert len(a) == 64 and all(ch in "0123456789abcdef" for ch in a)


def test_hash_estavel_para_o_mesmo_conteudo():
    assert tts.hash_de("oi", "v", "elevenlabs") == tts.hash_de("oi", "v", "elevenlabs")


def test_corpo_elevenlabs_manda_normalizacao_explicita():
    # apply_text_normalization NAO e default do provedor pro que a gente precisa: sem ele, "R$ 1.200"
    # sai soletrado. Vai explicito pra nao depender do default mudar do outro lado.
    corpo = json.loads(tts.corpo_elevenlabs("R$ 1.200", "eleven_multilingual_v2"))
    assert corpo["text"] == "R$ 1.200"
    assert corpo["model_id"] == "eleven_multilingual_v2"
    assert corpo["apply_text_normalization"] == "auto"


def test_sintetizar_sem_chave_levanta_503(monkeypatch):
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: "")
    with pytest.raises(tts.TtsError) as e:
        tts.sintetizar("oi", "voz", "elevenlabs")
    assert e.value.status == 503
    assert "elevenlabs" in e.value.detail.lower()


def test_sintetizar_usa_cache_e_nao_chama_o_provedor(monkeypatch, tmp_path):
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    chamadas = []
    monkeypatch.setattr(tts, "_baixar_elevenlabs", lambda t, v: chamadas.append(1) or b"MP3")
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: "chave" if campo == "elevenlabs_api_key" else "")

    h1, cache1 = tts.sintetizar("oi", "voz", "elevenlabs")
    h2, cache2 = tts.sintetizar("oi", "voz", "elevenlabs")

    assert h1 == h2
    assert cache1 is False and cache2 is True
    assert len(chamadas) == 1
    assert (tmp_path / f"{h1}.mp3").read_bytes() == b"MP3"


def test_cache_nao_deixa_arquivo_truncado(monkeypatch, tmp_path):
    # tmp+rename: rede caindo no meio nao pode deixar mp3 parcial em cache pra sempre.
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: "chave" if campo == "elevenlabs_api_key" else "")

    def explode(texto, voz):
        raise tts.TtsError(502, "conexao caiu")
    monkeypatch.setattr(tts, "_baixar_elevenlabs", explode)

    with pytest.raises(tts.TtsError):
        tts.sintetizar("oi", "voz", "elevenlabs")
    assert list(tmp_path.glob("*.mp3")) == []
    assert list(tmp_path.glob("*.tmp*")) == []


def test_comando_local_sem_configurar_levanta_503(monkeypatch):
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: "")
    with pytest.raises(tts.TtsError) as e:
        tts.sintetizar("oi", "voz", "local")
    assert e.value.status == 503


def test_comando_local_com_saida_vazia_levanta_502(monkeypatch, tmp_path):
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts.runtime_config, "get",
                        lambda campo: "true" if campo == "tts_local_cmd" else "")
    # `true` sai com codigo 0 e nao escreve nada -> tem que virar erro, nunca audio vazio calado.
    with pytest.raises(tts.TtsError) as e:
        tts.sintetizar("oi", "voz", "local")
    assert e.value.status == 502
