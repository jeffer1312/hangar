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


def test_quebra_de_bloco_nao_duplica_pontuacao_de_frase_ja_fechada():
    # Caso real de um plano: titulo e item de lista SEM pontuacao final ganham a pausa (".");
    # a frase que ja termina em "." NAO pode virar "..".
    entrada = "Passo 1\nFazer X no arquivo.\nitem um\nitem dois"
    assert preparar(entrada) == "Passo 1. Fazer X no arquivo. item um. item dois"
