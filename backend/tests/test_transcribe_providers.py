"""Lista de serviços de transcrição: ordem, espera por cota em disco e aviso de quem transcreveu.
Servidores falsos pelo `urlopen`, como em test_transcribe.py."""
import email.message
import io
import json
import time
import urllib.error
from types import SimpleNamespace

import pytest

from app import transcribe as mod
from app.transcribe import TranscribeError, Transcription, transcribe_with_provider

EL = {"id": "el", "kind": "elevenlabs", "name": "", "base_url": "", "api_key": "sk_el", "model": ""}
TURBO = {"id": "turbo", "kind": "openai", "name": "", "base_url": "https://api.groq.com/openai/v1",
         "api_key": "gsk_a", "model": "whisper-large-v3-turbo"}
LARGE = {"id": "large", "kind": "openai", "name": "Groq grande", "base_url": "https://large.exemplo/v1",
         "api_key": "gsk_b", "model": ""}
TURBO_NOME = "api.groq.com · whisper-large-v3-turbo"


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(mod, "_state_path", lambda: tmp_path / "transcription-wait.json")


def _config(monkeypatch, providers, **extra):
    cfg = {"transcription_providers": providers, "ditado_vocabulario": "", **extra}
    monkeypatch.setattr(mod.runtime_config, "get", cfg.get)


class _Resp:
    def __init__(self, body: bytes):
        self.body = body

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False

    def read(self, *a):
        return self.body


def _http_error(code, body=b"", headers=None):
    msg = email.message.Message()
    for k, v in (headers or {}).items():
        msg[k] = v
    return urllib.error.HTTPError("https://servico.exemplo", code, "erro", msg, io.BytesIO(body))


def _servidores(monkeypatch, respostas):
    """`respostas`: trecho da URL -> bytes (200) ou exceção. Devolve as chamadas, em ordem."""
    chamadas = []

    def urlopen(req, timeout=None):
        chamadas.append({"url": req.full_url, "host": req.full_url.split("/")[2], "timeout": timeout,
                         "headers": dict(req.header_items()), "body": req.data})
        for trecho, resposta in respostas.items():
            if trecho in req.full_url:
                if isinstance(resposta, BaseException):
                    raise resposta
                return _Resp(resposta)
        raise AssertionError(f"URL inesperada: {req.full_url}")

    monkeypatch.setattr("app.transcribe.urllib.request.urlopen", urlopen)
    return chamadas


def test_openai_compatible_json_response_without_authorization(monkeypatch):
    local = {**TURBO, "api_key": "", "base_url": "http://127.0.0.1:8000/v1"}
    _config(monkeypatch, [local])
    captured = []

    def endpoint(req, timeout=None):
        captured.append(req)
        assert b'name="response_format"\r\n\r\njson' in req.data
        assert not req.has_header("Authorization")
        return _Resp('{"text":"Transcrição em português."}'.encode())

    monkeypatch.setattr(mod.urllib.request, "urlopen", endpoint)
    result = transcribe_with_provider(b"audio", "fala.wav")
    assert result.text == "Transcrição em português."
    assert len(captured) == 1


def test_openai_json_is_not_returned_as_transcript(monkeypatch):
    _config(monkeypatch, [TURBO])
    _servidores(monkeypatch, {"groq": '{"text":"Olá, mundo."}'.encode()})
    assert transcribe_with_provider(b"audio", "fala.webm").text == "Olá, mundo."


def _grava_espera(waits):
    mod._state_path().write_text(json.dumps(waits), encoding="utf-8")


def test_lista_vazia_e_o_servico_unico_de_sempre(monkeypatch):
    _config(monkeypatch, [], groq_api_key="k", transcription_base_url="https://fala.exemplo/v1/",
            transcription_model="whisper-x")
    chamadas = _servidores(monkeypatch, {"fala.exemplo": b'{"text":"  texto \\n transcrito "}'})
    assert transcribe_with_provider(b"audio", "a.webm") == Transcription(
        "texto transcrito", "fala.exemplo · whisper-x", None)
    assert [c["url"] for c in chamadas] == ["https://fala.exemplo/v1/audio/transcriptions"]
    assert chamadas[0]["timeout"] == 120
    assert chamadas[0]["headers"]["Authorization"] == "Bearer k"
    assert chamadas[0]["headers"]["User-agent"] == "hangar/1.0"
    assert not mod._state_path().exists()


def test_lista_vazia_sem_chave_continua_503(monkeypatch):
    _config(monkeypatch, [], groq_api_key="")
    with pytest.raises(TranscribeError) as ei:
        transcribe_with_provider(b"audio", "a.webm")
    assert ei.value.status == 503


def test_elevenlabs_recebe_modelo_idioma_e_vocabulario_como_keyterms(monkeypatch):
    _config(monkeypatch, [EL], ditado_vocabulario="Acme, projeto-x")
    chamadas = _servidores(monkeypatch, {"api.elevenlabs.io": json.dumps({"text": "olá  mundo"}).encode()})
    assert transcribe_with_provider(b"audio", "a.webm") == Transcription("olá mundo", "ElevenLabs", None)
    c = chamadas[0]
    assert c["url"] == mod.ELEVENLABS_URL
    assert c["headers"]["Xi-api-key"] == "sk_el"
    assert "Authorization" not in c["headers"]
    assert c["timeout"] == mod.DICTATION_LIMITS[0]
    body = c["body"]
    for campo, valor in (("model_id", b"scribe_v2"), ("language_code", b"pt"),
                         ("tag_audio_events", b"false")):
        assert f'name="{campo}"'.encode() + b"\r\n\r\n" + valor in body
    assert body.count(b'name="keyterms"') == len(mod._keyterms(mod.vocabulario()))
    assert b'name="keyterms"\r\n\r\nAcme\r\n' in body
    assert b'name="keyterms"\r\n\r\nhangar-send\r\n' in body
    assert b'filename="audio.webm"' in body


def test_keyterms_descarta_o_que_o_elevenlabs_recusa():
    vocab = "ok, [x], " + "a" * 50 + ", um dois tres quatro cinco seis, ok"
    assert mod._keyterms(vocab) == ["ok"]


def test_chave_recusada_passa_ao_proximo_e_avisa_sem_por_em_espera(monkeypatch):
    _config(monkeypatch, [EL, TURBO, LARGE])
    corpo = json.dumps({"detail": {"code": "invalid_api_key", "message": "x"}}).encode()
    chamadas = _servidores(monkeypatch, {"api.elevenlabs.io": _http_error(401, corpo),
                                         "api.groq.com": b'{"text":"texto"}'})
    r = transcribe_with_provider(b"audio", "a.webm")
    assert [c["host"] for c in chamadas] == ["api.elevenlabs.io", "api.groq.com"]
    assert r.text == "texto" and r.provider == TURBO_NOME
    assert r.aviso == f"Transcrito pelo {TURBO_NOME}: ElevenLabs recusou a chave (401); confira a chave desse serviço"
    assert mod._load_waits() == {}


@pytest.mark.parametrize("falha", [
    urllib.error.URLError("sem rede"),
    TimeoutError("timed out"),
    _http_error(503, b"fora do ar"),
])
def test_rede_prazo_e_5xx_passam_ao_proximo_sem_espera(monkeypatch, falha):
    _config(monkeypatch, [EL, TURBO])
    _servidores(monkeypatch, {"api.elevenlabs.io": falha, "api.groq.com": b'{"text":"texto"}'})
    r = transcribe_with_provider(b"audio", "a.webm")
    assert r.text == "texto" and r.aviso.startswith(f"Transcrito pelo {TURBO_NOME}: ElevenLabs")
    assert mod._load_waits() == {}


def test_cota_do_elevenlabs_poe_em_espera_e_os_seguintes_vao_direto_ao_proximo(monkeypatch):
    _config(monkeypatch, [EL, TURBO])
    corpo = json.dumps({"detail": {"status": "quota_exceeded", "message": "sem creditos"}}).encode()
    chamadas = _servidores(monkeypatch, {"api.elevenlabs.io": _http_error(401, corpo),
                                         "api.groq.com": b'{"text":"texto"}'})
    antes = time.time()
    primeira = transcribe_with_provider(b"audio", "a.webm")
    assert "ElevenLabs sem cota (401), em espera" in primeira.aviso
    salvo = json.loads(mod._state_path().read_text(encoding="utf-8"))
    assert salvo["el"]["until"] >= antes + mod.QUOTA_WAIT_402 - 1
    assert salvo["el"]["reason"] == "sem cota (401)"
    chamadas.clear()
    segunda = transcribe_with_provider(b"audio", "a.webm")
    assert [c["host"] for c in chamadas] == ["api.groq.com"]
    # Pular quem está em espera não é falha desta requisição: sem aviso, o mãos-livres segue.
    assert segunda == Transcription("texto", TURBO_NOME, None)


def test_espera_gravada_em_disco_vale_depois_de_reiniciar(monkeypatch):
    # O módulo não guarda espera em memória: o arquivo é tudo que um backend novo vê.
    _grava_espera({"el": {"until": time.time() + 600, "reason": "sem cota (429)"}})
    _config(monkeypatch, [EL, TURBO])
    chamadas = _servidores(monkeypatch, {"api.groq.com": b'{"text":"texto"}'})
    transcribe_with_provider(b"audio", "a.webm")
    assert [c["host"] for c in chamadas] == ["api.groq.com"]


def test_espera_vencida_volta_a_tentar_o_primeiro(monkeypatch):
    _grava_espera({"el": {"until": time.time() - 1, "reason": "sem cota (429)"}})
    _config(monkeypatch, [EL, TURBO])
    chamadas = _servidores(monkeypatch, {"api.elevenlabs.io": json.dumps({"text": "ok"}).encode()})
    assert transcribe_with_provider(b"audio", "a.webm").provider == "ElevenLabs"
    assert [c["host"] for c in chamadas] == ["api.elevenlabs.io"]
    assert mod._load_waits() == {}


@pytest.mark.parametrize("erro,espera", [
    (lambda: _http_error(429, b"", {"Retry-After": "120"}), 120),
    (lambda: _http_error(429, b"limite"), mod.QUOTA_WAIT_429),
    (lambda: _http_error(402, b"pague"), mod.QUOTA_WAIT_402),
    (lambda: _http_error(429, b"", {"Retry-After": "²"}), mod.QUOTA_WAIT_429),
])
def test_tempo_de_espera_usa_a_data_do_servico_ou_o_padrao(monkeypatch, erro, espera):
    _config(monkeypatch, [TURBO, LARGE])
    _servidores(monkeypatch, {"api.groq.com": erro(), "large.exemplo": b'{"text":"texto"}'})
    antes = time.time()
    assert transcribe_with_provider(b"a", "a.webm").provider == "Groq grande"
    until = mod._load_waits()["turbo"]["until"]
    assert antes + espera - 1 <= until <= time.time() + espera + 1


def test_429_de_concorrencia_do_elevenlabs_nao_e_cota(monkeypatch):
    _config(monkeypatch, [EL, TURBO])
    corpo = json.dumps({"detail": {"code": "concurrent_limit_exceeded"}}).encode()
    _servidores(monkeypatch, {"api.elevenlabs.io": _http_error(429, corpo), "api.groq.com": b'{"text":"texto"}'})
    assert transcribe_with_provider(b"a", "a.webm").text == "texto"
    assert mod._load_waits() == {}


def test_todos_em_espera_tenta_a_lista_e_tira_da_espera_quem_respondeu(monkeypatch):
    futuro = time.time() + 600
    _grava_espera({"el": {"until": futuro, "reason": "sem cota (402)"},
                   "turbo": {"until": futuro, "reason": "sem cota (429)"}})
    _config(monkeypatch, [EL, TURBO])
    _servidores(monkeypatch, {"api.elevenlabs.io": json.dumps({"text": "voltou"}).encode()})
    assert transcribe_with_provider(b"a", "a.webm") == Transcription("voltou", "ElevenLabs", None)
    waits = mod._load_waits()
    assert "el" not in waits and "turbo" in waits


def test_todos_falhando_sobe_o_erro_do_primeiro_com_o_motivo_dos_outros(monkeypatch):
    _config(monkeypatch, [EL, TURBO, LARGE])
    _servidores(monkeypatch, {"api.elevenlabs.io": _http_error(500, b"quebrou"),
                              "api.groq.com": _http_error(401, b"chave"),
                              "large.exemplo": urllib.error.URLError("sem rede")})
    with pytest.raises(TranscribeError) as ei:
        transcribe_with_provider(b"a", "a.webm")
    assert ei.value.status == 502
    assert ei.value.detail == (
        "ElevenLabs: servico de transcricao 500: quebrou (depois: "
        f"{TURBO_NOME} recusou a chave (401); confira a chave desse serviço; Groq grande não respondeu)")
    assert "gsk_" not in ei.value.detail


def test_so_um_servico_falhando_sobe_o_erro_dele_sem_acrescimo(monkeypatch):
    _config(monkeypatch, [EL])
    _servidores(monkeypatch, {"api.elevenlabs.io": _http_error(500, b"quebrou")})
    with pytest.raises(TranscribeError) as ei:
        transcribe_with_provider(b"a", "a.webm")
    assert ei.value.detail == "ElevenLabs: servico de transcricao 500: quebrou"


def test_arquivo_de_espera_ilegivel_deixa_aviso_no_log(caplog):
    mod._state_path().write_text("{quebrado", encoding="utf-8")
    with caplog.at_level("WARNING", logger=mod.logger.name):
        assert mod._load_waits() == {}
    assert "ilegivel" in caplog.text


def test_teto_do_conjunto_corta_a_fila(monkeypatch):
    # monotonic: prazo calculado em 0; 1ª tentativa em 0; 2ª já perto do teto.
    marcas = iter([0.0, 0.0, mod.DICTATION_LIMITS[1] - 1])
    monkeypatch.setattr(mod, "time", SimpleNamespace(time=time.time, monotonic=lambda: next(marcas)))
    _config(monkeypatch, [EL, TURBO])
    chamadas = _servidores(monkeypatch, {"api.elevenlabs.io": TimeoutError("lento")})
    with pytest.raises(TranscribeError) as ei:
        transcribe_with_provider(b"a", "a.webm")
    assert [c["host"] for c in chamadas] == ["api.elevenlabs.io"]
    assert ei.value.detail.startswith("ElevenLabs: falha ao contatar")


def test_transcribe_do_video_devolve_so_o_texto_dentro_do_prazo_do_upload(monkeypatch):
    # A fala do vídeo roda dentro do /upload: o teto cabe antes de o cliente desistir.
    _config(monkeypatch, [TURBO])
    chamadas = _servidores(monkeypatch, {"api.groq.com": b'{"text":"so texto"}'})
    assert mod.transcribe(b"a", "a.webm") == "so texto"
    assert chamadas[0]["timeout"] == mod.VIDEO_LIMITS[0]
    assert mod.VIDEO_LIMITS[1] < 180


def test_elevenlabs_sem_texto_tem_motivo_proprio(monkeypatch):
    _config(monkeypatch, [EL, TURBO])
    _servidores(monkeypatch, {"api.elevenlabs.io": json.dumps({"language_code": "pt"}).encode(),
                              "api.groq.com": b'{"text":"texto"}'})
    r = transcribe_with_provider(b"a", "a.webm")
    assert r.aviso == f"Transcrito pelo {TURBO_NOME}: ElevenLabs deu resposta sem texto"
    assert mod._load_waits() == {}


def test_status_mostra_a_espera_na_ordem_da_lista(monkeypatch):
    futuro = time.time() + 600
    _grava_espera({"el": {"until": futuro, "reason": "sem cota (402)"},
                   "turbo": {"until": time.time() - 1, "reason": "sem cota (429)"}})
    _config(monkeypatch, [EL, TURBO])
    assert mod.providers_status() == [
        {"id": "el", "name": "ElevenLabs", "kind": "elevenlabs", "waiting_until": futuro,
         "reason": "sem cota (402)"},
        {"id": "turbo", "name": TURBO_NOME, "kind": "openai", "waiting_until": None, "reason": None},
    ]


def test_status_sem_lista_e_vazio(monkeypatch):
    _config(monkeypatch, [])
    assert mod.providers_status() == []
