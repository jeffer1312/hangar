from app import pair_texto


def test_texto_grupo_cita_membros_tarefa_e_contrato():
    t = pair_texto.texto_grupo("a", ["b", "c"], "ABC-1", "/x/.hangar-pair/grupo-g1.md")
    assert t.startswith("[painel: grupo de trabalho] GRUPO DE TRABALHO ATIVO: você, 'a', trabalha junto com 'b', 'c' na tarefa: ABC-1.")
    assert "hangar-send b \"msg\"" in t
    assert "/x/.hangar-pair/grupo-g1.md" in t
    assert "confirme em uma linha aqui mesmo, sem hangar-send." in t


def test_remetente_do_aviso_nunca_e_nome_de_sessao():
    from app.names import sanitize_session_name
    from app.uds_messaging import separar_prefixo
    remetente, _ = separar_prefixo(pair_texto.texto_tarefa_atualizada("PM-1"))
    assert remetente and sanitize_session_name(remetente) != remetente


def test_texto_grupo_rotula_harness_e_escolhe_como_mandar_pelo_destinatario():
    harness = {"a": "claude", "b": "codex", "c": "pi"}
    claude = pair_texto.texto_grupo("a", ["b", "c"], "", None, harness)
    assert "você, 'a' (Claude Code), trabalha junto com 'b' (Codex), 'c' (Pi)" in claude
    assert "tool `send`" in claude and "Não use SendMessage" in claude
    codex = pair_texto.texto_grupo("b", ["a", "c"], "", None, harness)
    assert "tool `send`" in codex and "SendMessage" not in codex
    pi = pair_texto.texto_grupo("c", ["a", "b"], "", None, harness)
    assert "tool `send`" not in pi and "SendMessage" not in pi
    assert "hangar-send a \"msg\"" in pi


def test_texto_grupo_sem_contrato_nao_cita_arquivo():
    t = pair_texto.texto_grupo("a", ["srv::b"], "", None)
    assert "CONTRATO:" not in t
    assert " na tarefa:" not in t


def test_texto_entrada_lista_quem_entrou_e_membros_atuais():
    t = pair_texto.texto_entrada(["d"], ["a", "b", "d"], "ABC-1", {"d": "kimi"})
    assert t == ("[painel: grupo de trabalho] 'd' (Kimi Code) entrou no seu grupo de trabalho na tarefa: ABC-1. "
                 "Membros agora: 'a', 'b', 'd' (Kimi Code). Mesmo protocolo de sempre (1:1; aviso de grupo "
                 "só pra marco). Não precisa responder.")


def test_texto_saida_com_e_sem_resto():
    assert pair_texto.texto_saida("a", "saiu do grupo de trabalho", ["b", "c"]) == (
        "[painel: grupo de trabalho] 'a' saiu do grupo de trabalho. O grupo continua entre você e 'b', 'c'.")
    assert pair_texto.texto_saida("a", "encerrou a sessão e saiu do grupo de trabalho", []) == (
        "[painel: grupo de trabalho] 'a' encerrou a sessão e saiu do grupo de trabalho. "
        "O grupo foi dissolvido (só restava você); volte a operar independente.")


def test_texto_tarefa_atualizada():
    assert pair_texto.texto_tarefa_atualizada("PM-2") == (
        "[painel: grupo de trabalho] Tarefa do grupo atualizada para: PM-2. Não precisa responder.")


def test_modulo_e_stdlib_only():
    import ast, pathlib
    src = pathlib.Path(pair_texto.__file__).read_text(encoding="utf-8")
    mods = {n.names[0].name.split(".")[0] if isinstance(n, ast.Import) else (n.module or "").split(".")[0]
            for n in ast.walk(ast.parse(src)) if isinstance(n, (ast.Import, ast.ImportFrom))}
    assert "app" not in mods
