from app.tts_text import preparar


def test_encurta_caminho_de_arquivo():
    assert preparar("veja frontend/src/lib/api.ts agora") == "veja api ponto ts agora"


def test_underline_vira_espaco():
    assert preparar("o ms_mensageiro caiu") == "o ms mensageiro caiu"


def test_tira_emoji_e_seta():
    # A voz lia "marca de selecao branca" e "seta pra direita" no meio da frase.
    assert preparar("pronto ✅ segue → depois") == "pronto segue depois"


def test_normaliza_espaco_e_quebra():
    assert preparar("linha um\n\n\nlinha dois   fim") == "linha um. linha dois fim"


def test_texto_so_de_simbolo_vira_vazio():
    # Quem chama usa isto pra devolver 400 em vez de sintetizar silencio pago.
    assert preparar("✅ → •") == ""
