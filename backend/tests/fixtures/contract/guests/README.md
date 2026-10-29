# Contrato de convidados e cofre

`capture.py` executa os roteadores Python com `TestClient`, sem lifespan e com diretórios
transitórios. Não abre o backend nem acessa sessões reais. `http.json` registra o commit de
origem, o comando gerador, as entradas, as respostas, os cookies e os arquivos depois de cada
operação. A única substituição de caminho é a raiz temporária por `⟦ROOT⟧`. Relógio e entropia
são fixados no Python para tornar verificáveis as assinaturas e o derivador PBKDF2.

```sh
cd backend
uv run python tests/fixtures/contract/guests/capture.py
```

A captura contém 36 operações HTTP encadeadas e 168 combinações de método/rota da política.
A matriz usa uma função de visibilidade isolada: ela prova o recorte de rotas, não a descoberta
da vida das sessões. Os testes Rust comparam o resultado HTTP e o estado persistido com essa
captura; o resultado esperado não é produzido pelo Rust.

| Casos | IDs do inventário |
| --- | --- |
| criar, pasta ausente, sem raiz | GUE-001, GUE-002, GUE-003, GUE-005 |
| editar e editar ausente | GUE-006, GUE-007 |
| apagar, apagar ausente, revogado | GUE-008, GUE-009, GUE-017 |
| dono, convidado, desconhecido, não administra | GUE-009, GUE-010, GUE-030 |
| matriz de método e rota | GUE-031, GUE-032 |
| lista do hub e recusa de administração | GUE-039 |
| nomes vazios/com barra e nome do dono | GUE-040, GUE-041 |
| remoção e cookie revogado | GUE-043 |
| cofre vazio, gravação, conflito e cofre do convidado | GUE-044 |

Esta captura inicial não fecha o inventário. Concorrência, falhas de disco, cache, vida de sessão,
SSE, WebSocket, raiz de arquivos, instalação e Windows exigem provas próprias.
