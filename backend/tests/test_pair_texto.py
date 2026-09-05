from app import pair_texto


def test_texto_grupo_cita_membros_tarefa_e_contrato():
    t = pair_texto.texto_grupo("a", ["b", "c"], "PM-1", "/x/.hangar-pair/grupo-g1.md")
    assert t.startswith("[de: hangar] GRUPO DE TRABALHO ATIVO: você ('a') trabalha junto com 'b', 'c' na tarefa: PM-1.")
    assert "hangar-send b \"sua mensagem\"" in t
    assert "/x/.hangar-pair/grupo-g1.md" in t
    assert "Confirme em uma linha." in t


def test_texto_grupo_sem_contrato_nao_cita_arquivo():
    t = pair_texto.texto_grupo("a", ["srv::b"], "", None)
    assert "Contrato/decisões" not in t
    assert " na tarefa:" not in t


def test_texto_entrada_lista_quem_entrou_e_membros_atuais():
    t = pair_texto.texto_entrada(["d"], ["a", "b", "d"], "PM-1")
    assert t == ("[de: hangar] 'd' entrou no seu grupo de trabalho na tarefa: PM-1. "
                 "Membros agora: 'a', 'b', 'd'. Mesmo protocolo de sempre (1:1 por SendMessage/hangar-send; "
                 "--group só pra marco). Não precisa responder.")


def test_texto_saida_com_e_sem_resto():
    assert pair_texto.texto_saida("a", "saiu do grupo de trabalho", ["b", "c"]) == (
        "[de: hangar] 'a' saiu do grupo de trabalho. O grupo continua entre você e 'b', 'c'.")
    assert pair_texto.texto_saida("a", "encerrou a sessão e saiu do grupo de trabalho", []) == (
        "[de: hangar] 'a' encerrou a sessão e saiu do grupo de trabalho. "
        "O grupo foi dissolvido (só restava você); volte a operar independente.")


def test_texto_tarefa_atualizada():
    assert pair_texto.texto_tarefa_atualizada("PM-2") == (
        "[de: hangar] Tarefa do grupo atualizada para: PM-2. Não precisa responder.")


def test_modulo_e_stdlib_only():
    import ast, pathlib
    src = pathlib.Path(pair_texto.__file__).read_text(encoding="utf-8")
    mods = {n.names[0].name.split(".")[0] if isinstance(n, ast.Import) else (n.module or "").split(".")[0]
            for n in ast.walk(ast.parse(src)) if isinstance(n, (ast.Import, ast.ImportFrom))}
    assert "app" not in mods
