"""Paleta Material You que o desktop (quickshell/end-4) gera do papel de parede."""
from app import desktop_palette


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
