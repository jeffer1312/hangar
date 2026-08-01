import json
import os
import time
from pathlib import Path

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


def test_hash_sem_instrucao_nao_muda_o_de_hoje():
    # CRITICO (fase 2, narracao guiada): o caminho SEM instrucao tem que bater byte a byte com o
    # hash de antes desta fase, senao todo audio ja ouvido perde o cache.
    assert tts.hash_de("oi", "v", "elevenlabs") == tts.hash_de("oi", "v", "elevenlabs", None, "")
    assert tts.hash_de("oi", "v", "elevenlabs", {}) == tts.hash_de("oi", "v", "elevenlabs", {}, "")


def test_hash_muda_com_instrucao_diferente():
    # Mesmo texto (ja tratado ou nao) tratado por instrucoes diferentes tem que virar audio diferente.
    sem = tts.hash_de("oi", "v", "elevenlabs", {}, "")
    com_a = tts.hash_de("oi", "v", "elevenlabs", {}, "explica o código")
    com_b = tts.hash_de("oi", "v", "elevenlabs", {}, "resuma")
    assert len({sem, com_a, com_b}) == 3


def test_hash_muda_com_ajuste_de_naturalidade():
    # CRITICO: sem os ajustes na chave do cache, mexer na estabilidade e pedir pra ouvir de novo
    # devolveria o audio ANTIGO, calado — pareceria que o controle nao faz nada.
    sem_ajuste = tts.hash_de("oi", "voz1", "elevenlabs", {})
    com_ajuste = tts.hash_de("oi", "voz1", "elevenlabs", {"stability": 0.3})
    outro_ajuste = tts.hash_de("oi", "voz1", "elevenlabs", {"stability": 0.9})
    assert len({sem_ajuste, com_ajuste, outro_ajuste}) == 3


def test_ajustes_efetivos_ausente_nao_manda_chave(monkeypatch):
    # Campo AUSENTE = usuario nunca tocou o slider (runtime_config.get devolve None de verdade —
    # nao ha attr tts_stability em Settings). Tem que se comportar igual a "no padrao".
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: None)
    assert tts._ajustes_efetivos() == {}


def test_ajustes_efetivos_igual_ao_padrao_nao_manda_chave(monkeypatch):
    # O slider SEMPRE manda um numero real — nunca vazio. Arrastar de volta pro padrao da
    # ElevenLabs (50/75/0/100) tem que dar no MESMO resultado que nunca ter tocado: sem isso, o
    # simples gesto de "voltar ao padrao" fixaria voice_settings a toa no provedor.
    config = {"tts_stability": 50, "tts_similarity_boost": 75, "tts_style": 0, "tts_speed": 100}
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: config.get(campo))
    assert tts._ajustes_efetivos() == {}


def test_ajustes_efetivos_converte_inteiro_guardado_em_fracionario(monkeypatch):
    config = {
        "tts_stability": 30, "tts_similarity_boost": 90, "tts_style": 20, "tts_speed": 80,
    }
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: config.get(campo))
    assert tts._ajustes_efetivos() == {
        "stability": 0.3, "similarity_boost": 0.9, "style": 0.2, "speed": 0.8,
    }


def test_ajustes_efetivos_clampa_fora_da_faixa_da_elevenlabs(monkeypatch):
    # Numero fora da faixa (ex: chegou de uma chamada de API externa) nao pode virar um pedido que
    # a ElevenLabs so vai recusar depois de pago — clampa na faixa valida do provedor.
    config = {"tts_stability": 500, "tts_speed": 5}   # 5.0 e 0.05, ambos fora da faixa
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: config.get(campo))
    ajustes = tts._ajustes_efetivos()
    assert ajustes["stability"] == 1.0
    assert ajustes["speed"] == 0.7


def test_corpo_elevenlabs_manda_normalizacao_on():
    # "on" (nao "auto"): e o que faz "R$ 1.200" virar "mil e duzentos reais" de forma CONFIAVEL,
    # nao so quando o provedor acha que deve normalizar.
    corpo = json.loads(tts.corpo_elevenlabs("R$ 1.200", "eleven_multilingual_v2"))
    assert corpo["text"] == "R$ 1.200"
    assert corpo["model_id"] == "eleven_multilingual_v2"
    assert corpo["apply_text_normalization"] == "on"
    assert "voice_settings" not in corpo   # sem ajuste, a chave nem aparece


def test_corpo_elevenlabs_manda_voice_settings_so_com_ajuste():
    sem_ajuste = json.loads(tts.corpo_elevenlabs("oi", "eleven_multilingual_v2", {}))
    assert "voice_settings" not in sem_ajuste

    com_ajuste = json.loads(tts.corpo_elevenlabs("oi", "eleven_multilingual_v2", {"stability": 0.3}))
    assert com_ajuste["voice_settings"] == {"stability": 0.3}


def test_sintetizar_sem_chave_levanta_503(monkeypatch):
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: "")
    with pytest.raises(tts.TtsError) as e:
        tts.sintetizar("oi", "voz", "elevenlabs")
    assert e.value.status == 503
    assert "elevenlabs" in e.value.detail.lower()


def test_sintetizar_usa_cache_e_nao_chama_o_provedor(monkeypatch, tmp_path):
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    chamadas = []
    monkeypatch.setattr(tts, "_baixar_elevenlabs", lambda t, v, a: chamadas.append(1) or b"MP3")
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: "chave" if campo == "elevenlabs_api_key" else "")

    h1, cache1, _ = tts.sintetizar("oi", "voz", "elevenlabs")
    h2, cache2, _ = tts.sintetizar("oi", "voz", "elevenlabs")

    assert h1 == h2
    assert cache1 is False and cache2 is True
    assert len(chamadas) == 1
    assert (tmp_path / f"{h1}.mp3").read_bytes() == b"MP3"


def test_cache_hit_renova_mtime(monkeypatch, tmp_path):
    # Sem isto, um trecho ouvido toda semana era apagado no 31o dia (contado da GRAVACAO, nao do
    # ultimo acesso) por _limpar_antigos e repago do provedor.
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts, "_baixar_elevenlabs", lambda t, v, a: b"MP3")
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: "chave" if campo == "elevenlabs_api_key" else "")

    h, _, _ = tts.sintetizar("oi", "voz", "elevenlabs")
    arquivo = tmp_path / f"{h}.mp3"
    velho = time.time() - 20 * 86400
    os.utime(arquivo, (velho, velho))

    tts.sintetizar("oi", "voz", "elevenlabs")   # cache hit

    assert arquivo.stat().st_mtime > velho


def test_cache_nao_deixa_arquivo_truncado(monkeypatch, tmp_path):
    # tmp+rename: rede caindo no meio nao pode deixar mp3 parcial em cache pra sempre.
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: "chave" if campo == "elevenlabs_api_key" else "")

    def explode(texto, voz, ajustes):
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


def test_hash_usa_voz_efetiva_nao_a_crua(monkeypatch, tmp_path):
    # voz="" com elevenlabs_voice_id X e voz="" com elevenlabs_voice_id Y tem que gerar hashes
    # DIFERENTES — senao trocar a voz configurada continua servindo audio da voz antiga do cache.
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts, "_baixar_elevenlabs", lambda t, v, a: b"MP3")

    config = {"elevenlabs_api_key": "chave", "elevenlabs_voice_id": "voz_x"}
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: config.get(campo, ""))
    h1, _, _ = tts.sintetizar("oi", "", "elevenlabs")

    config["elevenlabs_voice_id"] = "voz_y"
    h2, _, _ = tts.sintetizar("oi", "", "elevenlabs")

    assert h1 != h2


def test_extensao_de_detecta_wav_pelo_conteudo():
    assert tts.extensao_de(b"RIFF....WAVEfmt ") == "wav"
    assert tts.extensao_de(b"ID3\x03mp3 bytes") == "mp3"


def test_sintetizar_com_motor_local_devolvendo_wav_grava_wav(monkeypatch, tmp_path):
    # O comando local pode devolver WAV de verdade (nao so o mock generico dos outros testes) —
    # o cache tem que gravar sob .wav, e caminho_do_cache tem que achar o arquivo depois.
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts, "_baixar_local", lambda t: b"RIFF\x00\x00\x00\x00WAVEfmt ")
    monkeypatch.setattr(tts.runtime_config, "get",
                        lambda campo: "minha-voz" if campo == "tts_local_cmd" else "")

    h, cache, _ = tts.sintetizar("oi", "voz", "local")

    assert cache is False
    assert (tmp_path / f"{h}.wav").exists()
    assert not (tmp_path / f"{h}.mp3").exists()
    assert tts.caminho_do_cache(h) == tmp_path / f"{h}.wav"


def test_sem_chave_com_comando_local_configurado_cai_pro_local(monkeypatch, tmp_path):
    # provider=elevenlabs (o unico que o front manda) sem chave, mas com comando local configurado:
    # antes disto, o motor local era inalcancavel — a tela oferecia o campo e nada usava.
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts, "_baixar_local", lambda t: b"WAV")
    config = {"tts_local_cmd": "minha-voz"}
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: config.get(campo, ""))

    h, cache, provedor_final = tts.sintetizar("oi", "voz", "elevenlabs")

    assert cache is False
    assert (tmp_path / f"{h}.mp3").read_bytes() == b"WAV"
    # O front precisa saber que quem respondeu foi o motor local, senao troca de voz caladamente.
    assert provedor_final == "local"


def test_provedor_desconhecido_levanta_400(monkeypatch):
    monkeypatch.setattr(tts.runtime_config, "get", lambda campo: "")
    with pytest.raises(tts.TtsError) as e:
        tts.sintetizar("oi", "voz", "outro-provedor")
    assert e.value.status == 400


def test_falha_ao_gravar_cache_vira_ttserror_500(monkeypatch, tmp_path):
    # Disco cheio/permissao negada na gravacao: o usuario ja pagou a chamada ao provedor e nao
    # pode levar um 500 sem detail nenhum (a rota so captura TtsError).
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts, "_baixar_elevenlabs", lambda t, v, a: b"MP3")
    monkeypatch.setattr(tts.runtime_config, "get",
                        lambda campo: "chave" if campo == "elevenlabs_api_key" else "")
    monkeypatch.setattr(Path, "write_bytes", lambda self, data: (_ for _ in ()).throw(OSError("disco cheio")))

    with pytest.raises(tts.TtsError) as e:
        tts.sintetizar("oi", "voz", "elevenlabs")
    assert e.value.status == 500
    assert "cache" in e.value.detail.lower()


def test_comando_local_com_lixo_levanta_ttserror_e_nao_cacheia(monkeypatch, tmp_path):
    # Codigo 0, stdout nao-vazio, mas nao e wav nem mp3 (comando mal configurado imprimindo erro
    # no stdout) -> tem que falhar ANTES de escrever, senao o lixo vira <hash>.mp3 e envenena o
    # cache por 30 dias (toda tentativa seguinte bateria no arquivo podre). Comando REAL (nao
    # mock de _baixar_local): a validacao mora dentro dela, mockar a funcao inteira pularia o guard.
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts.runtime_config, "get",
                        lambda campo: "printf erro-de-configuracao" if campo == "tts_local_cmd" else "")

    with pytest.raises(tts.TtsError) as e:
        tts.sintetizar("oi", "voz", "local")
    assert e.value.status == 502
    assert list(tmp_path.glob("*.mp3")) == []
    assert list(tmp_path.glob("*.wav")) == []


def test_formato_de_audio_valido_aceita_wav_id3_e_frame_sync_recusa_lixo():
    assert tts._formato_de_audio_valido(b"RIFF....WAVEfmt ") is True
    assert tts._formato_de_audio_valido(b"ID3\x03mp3 bytes") is True
    assert tts._formato_de_audio_valido(b"\xff\xfbmp3 sem tag id3") is True
    assert tts._formato_de_audio_valido(b"erro: comando mal configurado") is False


def test_comando_local_mal_formado_levanta_ttserror(monkeypatch, tmp_path):
    monkeypatch.setattr(tts, "_base_cache", lambda: tmp_path)
    monkeypatch.setattr(tts.runtime_config, "get",
                        lambda campo: 'echo "aspas desbalanceadas' if campo == "tts_local_cmd" else "")
    with pytest.raises(tts.TtsError) as e:
        tts.sintetizar("oi", "voz", "local")
    assert e.value.status == 503
