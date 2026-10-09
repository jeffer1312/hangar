# Contratos de contas Claude/Codex — #115

Referência: Python da revisão `6186ce1363306ef0dacfdafac9ea8b8597f47128`, em
07/10/2026. A #115 separa contas, login, identidade e cotas da 5E/parte 6. Modelos
permanecem na 5E, integração geral na 5F, transferência na 5G; nascimento de sessão,
terminal, lançadores e troca de conta de sessão continuam nas frentes existentes.

## Rotas e consumidores

Métodos, query strings, nomes dos campos, nulos, campos omitidos, ordem e envelopes
de erro continuam públicos. Rótulo Claude pode ser apelido com acento; resolve-se
pelo catálogo, nunca concatenando esse rótulo ao caminho de conta.

| Rota e métodos | Implementação Python de referência | Consumidores principais | Dono com Rust ativo |
|---|---|---|---|
| `/api/claude-configs` GET/POST | `api.claude_configs`, `post_claude_config` | `packages/core/src/api.ts`, criação e configuração PWA/mobile, nativo `app/create/choices.rs`, `scripts/hangar-send` | Rust |
| `/api/claude-configs/{nome}` DELETE | `api.delete_claude_config` | Core, configuração PWA/mobile, nativo `app/accounts.rs` e criação | Rust |
| `/api/claude-configs/{nome}/logout` POST | `api.logout_claude_config` | Core e nativo `app/accounts/actions.rs` | Rust |
| `/api/conta-estado` GET | `conta_estado.listar_contas` | `frontend/src/lib/contaEstado.ts`, aba Contas; credenciais e configuração mobile; nativo | Rust |
| `/api/conta-estado/{label}/login` POST | `conta_estado.iniciar_login` | `frontend/src/lib/loginConta.ts`, nativo `app/accounts/actions.rs` | Rust |
| `/api/conta-estado/{label}/login/codigo` POST | `conta_estado.confirmar_login` | Cliente de login PWA e nativo | Rust |
| `/api/conta-estado/{label}/login/passo` GET | `conta_estado.passo_login` | Cliente de login PWA e nativo | Rust |
| `/api/conta-estado/{label}/login/cancelar` POST | `conta_estado.cancelar_login` | Cliente de login PWA e nativo | Rust |
| `/api/codex-contas` GET/POST | `codex_contas_api.list_codex_accounts`, `create_codex_account` | Core, `frontend/src/lib/credenciais.ts`, criação/configuração PWA/mobile, nativo, `scripts/hangar-codex` | Rust |
| `/api/codex-contas/{account_id}` DELETE | `codex_contas_api.delete_codex_account` | Core, PWA/mobile e nativo | Rust |
| `/api/codex-contas/{account_id}/prepare` GET/POST | `codex_account_preparation`, `prepare_codex_account` | Core, PWA/mobile, nativo `app/harness.rs`, `scripts/hangar-codex-tui` | Rust decide; gancho Python prepara |
| `/api/codex-contas/{account_id}/login` GET/POST/DELETE | `codex_login_status`, `start_codex_login`, `cancel_codex_login` | Core, PWA/mobile, nativo `app/accounts/codex.rs` | Rust |
| `/api/codex-contas/{account_id}/rate-limit-reset` POST | `consume_rate_limit_reset` | Core e nativo `app/accounts/codex.rs` | Rust |
| `/api/cotas` GET | `cotas.listar_cotas` | Core, feed de cotas PWA, criação e configuração mobile, nativo Contas/Orq e políticas de sessão | Rust agrega; Python fornece só outros provedores |
| `/api/cotas/sugestao` GET | `cotas.sugerir_conta` | Core e `scripts/hangar-send --conta auto` | Rust |
| `/api/credenciais/codex` GET | `credenciais.codex_estado` | `frontend/src/lib/credenciais.ts`; fluxo legado de dispositivo | Rust |
| `/api/credenciais/codex/login` GET/POST/DELETE | `codex_login_passo`, `codex_login_iniciar`, `codex_login_cancelar` | Cliente de credenciais PWA | Rust, inclusive os destinos Pi/omp |
| `/api/credenciais` GET | `credenciais.listar_credenciais` | `packages/core/src/credenciais.ts`, PWA/mobile e nativo | Python recebe retratos públicos Rust |
| `/api/sessions/{name}/conta` GET/POST | `api.py`, transferência e política de sessão | Core, seleção e troca da conta da sessão | Frente de sessões; não migra na #115 |

Cadastro Codex responde 201; preparo POST responde 202. Exclusão responde
`200 {"ok": true}`: o cliente sempre decodifica JSON. `GET prepare?cwd=…` mantém
pretrust explícito quando ready/partial; sem cwd não cria confiança. DELETE login
exige `attempt_id`; falta é 422, tentativa antiga é 409. Os erros preservam
`detail` string ou objeto conforme a rota; validação Pydantic preserva sua lista.

## Arquivos e autoria

| Estado/arquivo | Dono no modo Rust | Participação Python permitida |
|---|---|---|
| Catálogo, marcador `.hangar-conta`, marcador `.hangar-codex-conta` v1, cadastro/exclusão | Rust | Leitura pura dos formatos atuais |
| Autenticação nativa Claude/Codex | CLI chamada pelo serviço Rust | Nenhuma escrita no fluxo migrado |
| Cofre do device flow, `auth.json` Codex e OAuth Pi/omp | Rust | Escrita e leitura Pi/omp dentro da guarda da conta; o Python só conserva o fluxo sem Rust |
| `settings.json`, links/cópias, gaveta, plugins, hooks, skills, instruções e preparação | Reconciliadores Python até a frente de integração | Gancho restrito, com conta/operação validadas e lock de existência |
| `cotas-cache.json` (mistura provedores) | Rust, arquivo inteiro | Fornece parcelas de outros provedores; não grava com Rust ativo |
| `codex-reset-attempts.json` (JSONL, apesar do sufixo) | Rust | Nenhum consumo/escrita no fluxo migrado |
| Apelidos compartilhados e configuração fixa por ambiente | Contrato existente | Fornece fatos; mudança de apelido continua na rota existente |
| Sidecars e processos de sessões | Frente de sessões | Fatos completos de uso, proteção compartilhada durante criação |
| Locks de existência fora das pastas apagáveis | Protocolo compartilhado Rust/Python | Mesmo descritor/protocolo, inclusive Windows; não vale mutex interno sozinho |

Leitores de `codex_contas` encontrados por busca e GitNexus na base: adaptadores
Codex (`adapter`, `appserver`, `sem_terminal`, `transfer`), `api`, `registry`,
`list_bridge`, `archive`, `archive_providers`, `bastao`, `cliproxy_accounts`,
`conversation_history`, `conversation_transfer`, `costs_sources`, `cotas`,
`credenciais`, `orq_politica`, `config_sync`, `config_sync_paths`, `codex_appserver`,
`codex_models`, `codex_importador`, serviço/rotas de contas e reconciliadores de
sync/plugins. Os lançadores e probes também leem o formato. Esses consumidores
não autorizam escritor Python de cadastro/login com Rust ativo.

Claude também fornece config dir às sessões Pi: `registry.py` escolhe a conta e
`skill_bridge.py`/`pi_extension_installer.py` usam essa configuração. A proteção da
#115 cobre esse uso; migrar o adaptador Pi continua fora do escopo. Política de
status (`runtime_policy.quota_windows`), `/internal/quota` e custos precisam usar o
retrato de contas/cotas Rust sem chamada pública recursiva.

## Referência e prova de autoria

`python-reference.json` contém 26 pedidos executados pelas rotas Python reais:
catálogos deslogados, apelido com acento, filtro de backup na aba de contas,
ordenação, nulos/omissões, validação, conta padrão protegida, cadastro Codex,
duplicata, preparo idle, cancelamento antigo, UUID inválido, device flow idle,
cotas deslogadas e exclusão. A árvore do cadastro inclui o marcador e o
`config.toml`; nenhum arquivo de autenticação nasce nesse cadastro. A árvore
compara o conjunto exato de arquivos, o marcador como JSON e a configuração como
TOML: CRLF no Windows e LF no POSIX não mudam seus valores. Respostas HTTP mantêm
seu conteúdo completo e não recebem essa decodificação adicional.

`tests/accounts_contract.py` inicia um filho Python com HOME, USERPROFILE,
HOMEDRIVE/HOMEPATH, XDG, APPDATA, config e cache temporários **antes dos imports**.
O ambiente é uma lista permitida; não herda identidade, CP de contas, TMUX ou
proxies. O filho monta os handlers reais numa FastAPI isolada, sem o lifespan que
recupera sessões da máquina. Autenticação externa é sintética e deslogada; CLI e
conexões externas inesperadas são recusadas. O teardown recolhe o filho criado.

`HttpTransport.request(method, path, json=None)` funciona contra a porta de teste
Python ou uma instância Rust isolada. Não segue redirect nem usa proxy. A
normalização muda somente a raiz temporária declarada e seus separadores: não
remove horários, UUIDs, mensagens, campos desconhecidos ou listas fora de ordem.
Dados temporais novos deverão declarar sua normalização na Task consumidora.

`PythonReference(..., block_handlers=True)` recusa operações de conta com 503 e
`contract_python_handler_blocked`, guardando um diário separado. Uma resposta
200 entregue por proxy Python falha em `assert_rust_ownership`. Só
`bridge.prepare`, `bridge.claude_window` e `bridge.other_quotas` são
permitidas nessa conferência. As quatro rotas da ponte Pi/omp aposentada
(`device-propagate`, `/ack`, `/close` e `device-propagate-state`) entram no diário
como `bridge.propagate_device_login` e reprovam a conferência;
`/__contract__/secondary-native-only` as recusa antes do handler. Na integração, a instância Rust deve apontar para
essa referência bloqueada e produzir o contrato esperado sem chamar handlers de
contas; preparação e outros provedores têm seus próprios diários. A Task 1 prova
a sonda, não declara que o Rust já atende contas.

`account-keys.json` fixa a chave de existência nos dois runtimes: SHA-256 de
provedor, byte NUL e caminho canônico em UTF-8. No Windows, a normalização remove
o prefixo estendido, uniformiza barras e converte cada caractere para minúsculas.
Conta inexistente resolve pelo ancestral existente; os descritores ficam fora da
pasta da conta e seus arquivos nunca são apagados.

`test_account_lifecycle.py` disputa descritores com um processo Rust real e usa
`start_session_async`, `account_exists` e barreiras do harness para provar o
nascimento pela rota real, cancelamento repetido e reinício apenas do Rust.
Sidecars sem PID ainda podem representar nascimento pendente; inspeção incompleta
recusa a exclusão. `test_accounts_catalog.py` comprova o DELETE HTTP depois do retorno
do criador e antes da publicação do lançador, incluindo dois cancelamentos e reinício
somente do Rust. Os handlers públicos Python ficam bloqueados; fatos internos seguem
pela rota real com segredo e instância. Goldens completos conferem cadastro, exclusão,
catálogo Claude e envelopes de validação.

`environment.json` é compartilhado pelos testes Python e Rust: a conta padrão mantém
a identidade original e a adicional remove autenticação, provedor e diretórios herdados.
O teste nativo usa o Codex instalado em HOME descartável, com credenciais sintéticas;
confere conta deslogada e API key sem copiar autenticação. OAuth usa resposta sintética
pelo cliente JSON-RPC existente, sem afirmar login real.

Na Task 3, GET `/api/codex-contas` ainda depende do coordenador de preparo, e POST
`/api/claude-configs` ainda depende da semeadura. Essas rotas não são reivindicadas
pelo classificador até a integração consumidora completar seus ramos.

No Codex com terminal, `account-locks/births` registra conta, nome e instância do
multiplexador antes de soltar o descritor. Esse estado não é sidecar de transporte.
O registro é aposentado após publicação final com PID conferido ou fim daquela
instância; reutilizar o nome em outro pane não reativa o nascimento antigo. A
inspeção também resolve `--codex-home` enquanto o lançador não publicou o ambiente.

Relógio controlável e eventos da CLI serão acrescentados quando login/cota tiverem
consumidores concretos. Login com identidade conectada, CLI travada, preparo
ready/partial, 429 e redefinição idempotente ainda precisam das Tasks consumidoras.
`forcar=true` jamais serve para criar golden do desvio conhecido que burla 429.

Executar somente em VM Windows de teste ou CI descartável, a partir de `backend/`:

```bash
uv run pytest tests/test_accounts_contract.py -q
```

Regeneração explícita após conferir uma mudança de contrato Python:

```bash
uv run python tests/accounts_contract.py --capture tests/fixtures/accounts_contract/python-reference.json
```

Golden não é atualizado automaticamente por falha de teste. A prova real de
cadastro/login continua nas interfaces existentes e em contas descartáveis; esta
referência não usa nem altera contas da máquina principal.
