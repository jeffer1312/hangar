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


def test_hash_de_commit_nao_e_soletrado():
    # "8f94525..2e70e70" letra a letra e a coisa mais inutil que a voz pode fazer: ninguem decora
    # hash de ouvido. O resto da frase continua de pe sem ele.
    assert preparar("No ar: 8f94525..2e70e70. Recarrega e testa.") == "No ar: . Recarrega e testa."
    assert preparar("commit d47dd19 na main") == "commit na main"


def test_hash_nao_come_numero_nem_palavra_hexadecimal():
    # Dois falsos positivos medidos, e a razao de a regra exigir digito E letra a-f ao mesmo tempo:
    # so digito pegaria um numero de verdade; so letra pegaria palavra composta de letras hex.
    assert preparar("o numero 1234567 continua") == "o numero 1234567 continua"
    assert preparar("ele defaced o cafe e acceded") == "ele defaced o cafe e acceded"
    assert preparar("a versao 2.1.220 fica") == "a versao 2.1.220 fica"
