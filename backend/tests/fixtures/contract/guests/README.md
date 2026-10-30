# Contrato de convidados e cofre

As capturas foram obtidas executando os roteadores Python com `TestClient`, sem lifespan e com
diretórios transitórios, sem abrir o backend nem acessar sessões reais. `http.json` registra o commit de
origem, o comando gerador, as entradas, as respostas, os cookies e os arquivos depois de cada
operação. A única substituição de caminho é a raiz temporária por `⟦ROOT⟧`. Relógio e entropia
são fixados no Python para tornar verificáveis as assinaturas e o derivador PBKDF2.

Os geradores usados durante a migração foram removidos. Os comandos registrados nos JSON são
metadados históricos da captura; não são comandos disponíveis na árvore atual. Os testes Rust
consomem diretamente os dados congelados.

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

As capturas adicionais foram obtidas com o Python isolado pelo comando registrado em cada JSON:

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
| `terminal.json` | Oito combinações de credencial e Origin executadas no porteiro Python, sem criar PTY |
| `visibility.json` | 11 valores históricos de `owner_sees`, pela filtragem Python |
| `ignored-body.json` | 14 requisições com JSON malformado em rotas sem modelo de corpo |
| `claim.json` | Três sequências de descoberta de vida e seis trocas com ou sem proprietário |
| `transfer-failure.json` | Resultado e limpeza da transferência com gravação do dono bem-sucedida ou falha |
| `limit-routes.json` | 12 combinações de rota, credencial e transporte, incluindo se o corpo foi consumido |
| `push.json` | 15 decisões de aviso de espera e 18 de conclusão para Pi, omp e Kimi |
| `placeholder.json` | 18 textos na guarda de marcador pendente do atalho |
| `base64.json` | 16 formulários de cadastro e configuração, incluindo bits de preenchimento não canônicos |
| `blobs.json` | 21 combinações de preenchimento, caracteres e precedência de erro no bloco cifrado |
| `cookies.json` | 26 cookies e contextos de origem, com expiração, geração, renovação e CSRF |
| `rate.json` | Dez sequências do limitador, com relógio fracionário, retrocesso e limites extremos |
| `request-body.json` | 38 corpos e tipos de conteúdo; inclui UTF-16/32, surrogates, parser e falha posterior da resposta |
| `loop-validation.json` | 80 modelos de loop pela borda FastAPI, antes da implementação dos consumidores |
| `settings.json` | 30 opções no dotenv, incluindo inteiros grandes, entradas inválidas e binds do cookie |

Diferenças deliberadas autorizadas para esta migração:

- Configuração inválida do sync passa a recusar somente essa área com `503 sync_config_invalid`;
  antes impedia o processo Python de iniciar. O erro não expõe o valor, não escolhe padrão
  silencioso e não desliga o cadastro local de convidados.

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

`limits.json` executa somente o middleware de tamanho, com uma aplicação ASGI mínima;
`limit-routes.json` executa também o FastAPI e o porteiro reais. O limite é reduzido a oito
bytes para comparar os mesmos ramos sem arquivos grandes. Na rota real, tamanho declarado
excessivo retorna 413; excesso ao ler o modelo JSON retorna 400 porque o FastAPI captura a
exceção. Um login recusado não consome o fluxo e retorna 403. GET sem corpo declarado também
não o consome. O 413 do middleware isolado para fluxo sem tamanho não comprova a resposta da
rota pública.

As capturas por AST executam os corpos originais das funções nomeadas, com dependências e
entrega isoladas. A reprodução histórica exige recuperar tanto a implementação de origem quanto
o gerador no histórico Git; as capturas congeladas continuam sendo a referência dos testes Rust.
