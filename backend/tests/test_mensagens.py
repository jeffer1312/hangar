from app.mensagens import erro


def test_erro_carrega_codigo_params_e_texto_pt():
    d = erro("sessao_inexistente", "sessao nao existe", nome="api-fix")
    assert d == {
        "code": "sessao_inexistente",
        "params": {"nome": "api-fix"},
        "msg": "sessao nao existe",
    }


def test_erro_sem_params_traz_dict_vazio_nao_none():
    # `params` ausente viraria `undefined` no front e quebraria o spread na chamada da mensagem.
    assert erro("sem_paleta", "sem paleta")["params"] == {}
