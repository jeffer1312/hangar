import os


def test_closed_names_guarda_o_fechamento_mais_novo_e_poda(tmp_path, monkeypatch):
    from app import closed_names
    monkeypatch.setattr(closed_names, "_path_override", tmp_path / "closed.json")
    monkeypatch.setattr(closed_names, "_KEEP", 2)
    closed_names.record(str(tmp_path / "a.jsonl"), "velho", now=1)
    closed_names.record(str(tmp_path / "a.jsonl"), "novo", now=2)
    closed_names.record(str(tmp_path / "b.jsonl"), "b", now=3)
    closed_names.record(str(tmp_path / "c.jsonl"), "c", now=4)

    assert closed_names.names() == {os.path.realpath(tmp_path / "b.jsonl"): "b", os.path.realpath(tmp_path / "c.jsonl"): "c"}


def test_closed_names_ilegivel_vai_para_o_lado_antes_de_gravar(tmp_path, monkeypatch):
    from app import closed_names
    arquivo = tmp_path / "closed.json"
    monkeypatch.setattr(closed_names, "_path_override", arquivo)
    (tmp_path / "closed.json.bad").write_text("anterior", encoding="utf-8")
    arquivo.write_text("{quebrado", encoding="utf-8")

    closed_names.record(str(tmp_path / "a.jsonl"), "a", now=1)

    assert (tmp_path / "closed.json.bad").read_text(encoding="utf-8") == "anterior", "o .bad anterior fica"
    assert (tmp_path / "closed.json.bad.2").read_text(encoding="utf-8") == "{quebrado"
    assert closed_names.names() == {os.path.realpath(tmp_path / "a.jsonl"): "a"}
