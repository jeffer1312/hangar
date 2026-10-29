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

As capturas adicionais executam o Python isolado pelo comando registrado em cada JSON:

| Arquivo | Entradas e comparação |
| --- | --- |
| `validation.json` | 98 entradas de sete modelos; 90 erros comparados integralmente, inclusive ordem e localização dos campos |
| `revision.json` | Oito gravações do cofre, incluindo inteiros acima de 64 bits e coerções; resposta e arquivo |
| `fs.json` | Sete consultas de raízes/pastas, incluindo escape e symlink; status e corpo |
| `storage.json` | Nove arquivos legados; padrões, IDs repetidos, pares de sessão e recusa de corrupção |
| `store.json` | Sete estados de vida, matriz de três observadores e falha de adoção persistida; propriedade e arquivo |
| `setup.json` | 17 estados de ativação/desativação; status, corpo, cookie e arquivos |
| `credentials.json` | 13 combinações de Bearer, query e cookies HTTP/HTTPS |
| `identity-types.json` | 24 combinações de campo estrutural e tipo, com leitura autenticada e tentativa de escrita |

Diferenças deliberadas autorizadas para esta migração:

- Falhas de leitura do cofre que retornavam `500 Internal Server Error` passam a retornar
  `500` com `detail.code=sync_vault_unreadable`, sem conteúdo do arquivo. Os testes conservam a
  igualdade do status e dos efeitos; só esse corpo tem uma expectativa explícita diferente.
- `id`, `name`, `root` e `token_hash` de `guests.json` precisam ser strings. `null`, inteiro,
  número fracionário, booleano, lista e objeto invalidam o arquivo inteiro: autenticação recusada
  e escrita impedida. O arquivo fica intacto. O último snapshot válido serve somente à
  visibilidade da lista, nunca à autenticação nem à gravação. Os demais padrões históricos
  continuam valendo.

O teste do SSE usa a matriz Python e um multiplexador falso: verifica a lista publicada e o
encerramento após revogação, sem sessões reais. As entradas HTTP e os arquivos também passam
por `proptest`. Essas provas não substituem a conferência de Windows e do uso real.
