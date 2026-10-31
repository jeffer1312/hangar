"""Escritas da tela única de credenciais (contas do Claude e chaves de API na mesma tabela).

A lista (`GET /api/credenciais`) é do Rust (`crates/hangar-server/src/accounts/credentials.rs`).
Aqui ficam o apelido, o cookie do OpenCode, a sincronização nos outros agentes e a remoção do
provedor que o app gravou no Kimi.

Criar, apagar ou editar a conta ou a chave em si NÃO passa por aqui: `POST/DELETE
/api/claude-configs` pra conta do Claude, `PUT/DELETE /api/engines/{nome}` pra chave. Duplicar o
caminho de escrita pra "unificar" a tela criaria dois donos do mesmo dado. O APELIDO não é de
nenhum dos dois lados: é do app.
"""
import logging

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field

from app import opencode_cota
from app.auth import require_auth
from app.mensagens import erro

_log = logging.getLogger("hangar.credenciais")

credenciais_router = APIRouter(prefix="/api/credenciais")






class CookieBody(BaseModel):
    """Cookie de sessão do painel do OpenCode. Os dois vazios APAGAM a configuração.

    Não é chave de API: é a sessão do navegador na conta, e é por isso que ela não entra no
    engines.json junto da key — arquivo à parte, 0600, e o valor nunca volta pra tela.
    """

    id: str = Field(min_length=1, max_length=512)
    workspace_id: str = Field(default="", max_length=200)
    auth_cookie: str = Field(default="", max_length=4096)


@credenciais_router.put("/cookie", dependencies=[Depends(require_auth)])
def definir_cookie(body: CookieBody) -> dict:
    """Guarda (ou apaga) o cookie do painel do OpenCode desta credencial e devolve se ficou algum.

    Devolve só o booleano de propósito: confirmar gravação não precisa do valor, e ecoá-lo poria
    a sessão do navegador no corpo de uma resposta HTTP que a tela guarda em memória.
    """
    opencode_cota.definir_config(body.id, body.workspace_id, body.auth_cookie)
    # Invalida a leitura em cache: sem isto o cookie novo só valeria no próximo ciclo de 5 min, e
    # a pessoa acabou de colar justamente pra ver o número aparecer. O cache é de quem lê a cota.
    # Extra, não a operação: com o cookie já gravado, recusar faria a pessoa colar de novo.
    from app import account_bridge
    try:
        account_bridge.request_quotas(invalidate=body.id)
    except HTTPException as error:
        _log.warning("cookie gravado, mas a cota em cache não foi invalidada: %s", error.detail)
    return {"id": body.id, "cookie_definido": body.id in opencode_cota.ler_configs()}
