"""Paleta Material You que o desktop (quickshell/end-4) gera do papel de parede."""
import logging
import os

import pytest

from app import desktop_palette


@pytest.fixture(autouse=True)
def _reset_ultimo_aviso():
    # `_ultimo_aviso` (dedupe de WARNING entre chamadas, Fix 2 da revisao) e estado de MODULO —
    # sem reset, o aviso "gasto" por um teste anterior (mesma mensagem) silenciaria o assert de
    # outro teste. Mesmo padrao dos resets ja em conftest.py (auth backoff, ws warn flags).
    desktop_palette._ultimo_aviso = None
    yield
    desktop_palette._ultimo_aviso = None


def test_le_a_variante_ativa_e_todos_os_tokens(paleta_azul):
    p = desktop_palette.parse(paleta_azul)
    assert p["escuro"] is True
    assert p["cores"]["background"] == "#111318"
    assert p["cores"]["surfaceContainer"] == "#1D2024"
    assert p["cores"]["primary"] == "#AAC7FF"
    assert set(desktop_palette.TOKENS) <= set(p["cores"])


def test_neutros_seguem_a_foto(paleta_azul, paleta_vermelha):
    frio = desktop_palette.parse(paleta_azul)["cores"]
    quente = desktop_palette.parse(paleta_vermelha)["cores"]
    assert quente["background"] == "#1C110D"
    assert frio["background"] != quente["background"]
    # E o destaque NAO muda no esquema "Auto" — medido, e por isso esta no desenho.
    assert frio["primary"] == quente["primary"]


def test_darkmode_false_vira_claro(paleta_azul):
    texto = paleta_azul.replace("$darkmode: True;", "$darkmode: False;")
    assert desktop_palette.parse(texto)["escuro"] is False


def test_arquivo_incompleto_devolve_nada():
    # Meia paleta e pior que paleta nenhuma: pintaria o fundo novo e deixaria o texto no valor velho.
    assert desktop_palette.parse("$darkmode: True;\n$background: #111318;\n") is None


def test_lixo_nao_levanta():
    assert desktop_palette.parse("") is None
    assert desktop_palette.parse("nao sou scss\n\x00\x01") is None
    assert desktop_palette.parse("$background: verde;\n") is None


def test_ler_arquivo_ausente_devolve_nada(tmp_path, monkeypatch):
    monkeypatch.setattr(desktop_palette, "_caminho", lambda: tmp_path / "nao-existe.scss")
    assert desktop_palette.ler() is None


def test_ler_arquivo_bom(tmp_path, monkeypatch, paleta_azul):
    f = tmp_path / "material_colors.scss"
    f.write_text(paleta_azul, encoding="utf-8")
    monkeypatch.setattr(desktop_palette, "_caminho", lambda: f)
    assert desktop_palette.ler()["cores"]["background"] == "#111318"


def test_nunca_le_colors_json(tmp_path, monkeypatch, paleta_azul):
    """Regressao: colors.json fica NA MESMA PASTA, com a mesma hora de escrita, e carrega a
    variante CLARA (background #f9f9ff) enquanto o .scss diz darkmode True. Quem trocar a fonte por
    ele pinta o app de branco num desktop escuro."""
    (tmp_path / "colors.json").write_text('{"background": "#f9f9ff"}', encoding="utf-8")
    f = tmp_path / "material_colors.scss"
    f.write_text(paleta_azul, encoding="utf-8")
    monkeypatch.setattr(desktop_palette, "_caminho", lambda: f)
    p = desktop_palette.ler()
    assert p["cores"]["background"] == "#111318"
    assert "f9f9ff" not in str(p)


def test_caminho_que_e_diretorio_nao_levanta(tmp_path, monkeypatch):
    # IsADirectoryError e OSError, mas so um `except OSError` generico o pega. Um `except
    # FileNotFoundError` — que parece mais preciso e alguem vai querer escrever um dia — deixaria
    # esta passar direto pro chamador, que e uma rota HTTP.
    d = tmp_path / "material_colors.scss"
    d.mkdir()
    monkeypatch.setattr(desktop_palette, "_caminho", lambda: d)
    assert desktop_palette.ler() is None


def test_bytes_invalidos_nao_levantam(tmp_path, monkeypatch):
    # O arquivo e reescrito inteiro pelo rice a cada troca de papel de parede; ler no meio da
    # escrita traz metade de um caractere. Sem `errors="replace"` isso e UnicodeDecodeError.
    f = tmp_path / "material_colors.scss"
    f.write_bytes(b"$darkmode: True;\n$background: #11\xff\xfe1318;\n")
    monkeypatch.setattr(desktop_palette, "_caminho", lambda: f)
    assert desktop_palette.ler() is None


def test_bom_no_inicio_nao_perde_a_primeira_declaracao(paleta_azul):
    # BOM (﻿), nao "arquivo bom" — nome deliberadamente diferente de test_ler_arquivo_bom (essa e
    # "arquivo BOM" em portugues, ja testa outra coisa: ler do disco sem monkeypatch de conteudo).
    #
    # A primeira versao deste teste punha o BOM na frente de `$darkmode: True;` — a PRIMEIRA linha
    # do fixture, mas nao um token de TOKENS. Perder essa linha so muda "escuro" pro default (que ja
    # e True) e todo o resto do arquivo continua casando normalmente: o teste passava com ou sem o
    # strip do BOM, ele nao provava nada. Aqui o token na linha 1 e `$background`, que ESTA em
    # TOKENS — perder a linha 1 tira um token obrigatorio e `parse()` tem que devolver None.
    resto = "\n".join(l for l in paleta_azul.splitlines() if not l.startswith("$background:"))
    texto = "﻿$background: #111318;\n" + resto + "\n"
    p = desktop_palette.parse(texto)
    assert p is not None
    assert p["cores"]["background"] == "#111318"


def test_home_sem_env_vira_none_sem_levantar(monkeypatch, caplog):
    # `_caminho()` (chamada dentro do try de `ler()`) usa Path.home(), que levanta RuntimeError (nao
    # OSError) quando $HOME nao esta setado e o passwd nao tem home dir — cenario plausivel numa unit
    # systemd minima ou container. O contrato do modulo e "nunca levanta"; sem o `except
    # RuntimeError`, isso vazava pra rota HTTP como 500. Monkeypatch em `_caminho`, nao em
    # `Path.home` de verdade, pra nao vazar pros outros testes do processo.
    def _sem_home():
        raise RuntimeError("Could not determine home directory")
    monkeypatch.setattr(desktop_palette, "_caminho", _sem_home)
    with caplog.at_level(logging.WARNING, logger="app.desktop_palette"):
        assert desktop_palette.ler() is None
    assert "ilegivel" in caplog.text


def test_falha_repetida_com_o_mesmo_motivo_avisa_uma_vez_so(monkeypatch, caplog):
    # Fix 2 da revisao: o front rele o arquivo a cada foco da janela. Numa maquina cujo rice ficou
    # permanentemente ilegivel, duas chamadas seguidas com o MESMO motivo nao podem virar dois
    # WARNINGs — so a MUDANCA de estado interessa, senao o log rola pra sempre e ninguem le.
    def _sem_home():
        raise RuntimeError("Could not determine home directory")
    monkeypatch.setattr(desktop_palette, "_caminho", _sem_home)
    with caplog.at_level(logging.WARNING, logger="app.desktop_palette"):
        assert desktop_palette.ler() is None
        assert desktop_palette.ler() is None
    avisos = [r for r in caplog.records if "ilegivel" in r.message]
    assert len(avisos) == 1


@pytest.mark.skipif(os.geteuid() == 0, reason="root ignora permissoes de diretorio")
def test_permissao_negada_loga_diferente_de_arquivo_ausente(tmp_path, monkeypatch, caplog):
    # O comentario velho tratava "nao existe" e "sem permissao" como o mesmo silencio. Sao
    # diferentes: o primeiro e a maioria das maquinas (sem rice), o segundo e sempre uma pista.
    d = tmp_path / "sem-acesso"
    d.mkdir(mode=0o000)
    f = d / "material_colors.scss"
    monkeypatch.setattr(desktop_palette, "_caminho", lambda: f)
    try:
        with caplog.at_level(logging.WARNING, logger="app.desktop_palette"):
            assert desktop_palette.ler() is None
        assert "ilegivel" in caplog.text
    finally:
        d.chmod(0o700)   # senao o tmp_path nao e limpo no teardown
