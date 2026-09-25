"""Fixture SINTÉTICA da Task 24 (Enter no Cancelar dos alertas): a da Task 14 (que já carrega a 13, a de sessão e a de
contas) com a credencial `chave:opencode-zen` já com cookie guardado, para o alerta "parar de ler a cota" aparecer. Nenhuma
sessão, conta, máquina ou cookie daqui existe de verdade; nada sai deste processo.

Os seis alertas e o que o sim pede aqui: fechar sessão (DELETE /api/sessions/<n>), sair/remover conta
(POST .../logout, DELETE /api/claude-configs/<n>), parar de ler a cota (PUT /api/credenciais/cookie), atualizar
(POST /api/atualizacao/iniciar), remover máquina (DELETE /api/peers/<id>). Sair/remover este servidor não chama rota:
apaga o connection.json do XDG_CONFIG_HOME do app.

Toda requisição fica no registro, GET /control/log; as escritas são as de método diferente de GET, e o "não" não soma nenhuma.
Porta 0 (TASK24_PORT muda); o endereço sai na primeira linha.
"""
import os
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task14_sidebar_fixture.py").read_text(encoding="utf-8")
T14NS = {"__name__": "task14_base", "__file__": str(HERE / "task14_sidebar_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task14_sidebar_fixture.py", "exec"), T14NS)

BASE = T14NS["BASE"]
BASE["accounts"].COOKIES.add("chave:opencode-zen")

server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK24_PORT", "0"))), T14NS["Handler"])
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
