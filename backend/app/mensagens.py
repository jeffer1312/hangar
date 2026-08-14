"""Erro de API em forma que o front consegue traduzir.

O `detail` de um HTTPException vira um dict com tres campos, e nao uma string:

    {"code": "sessao_inexistente", "params": {...}, "msg": "sessao nao existe"}

`code` e o que o front usa pra achar a mensagem traduzida; `params` sao os valores que entram nela;
`msg` e o MESMO texto em portugues que existia antes, e ele fica de proposito. Endpoint ainda nao
migrado continua devolvendo string crua, e o front entende as duas formas — foi o que permitiu
migrar isto endpoint a endpoint em vez de num commit so. `msg` tambem e a rede quando o front for
mais velho que o backend (celular com build antigo em cache do service worker): ele mostra o
portugues em vez de mostrar um codigo.
"""


def erro(code: str, msg: str, **params) -> dict:
    return {"code": code, "params": params, "msg": msg}
