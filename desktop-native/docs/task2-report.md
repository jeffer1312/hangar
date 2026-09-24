# Task 2: lista e histórico reais

HEAD destacado: `55e0d909`. Executor: `gpt-6-sol`, esforço `high`, conta `jefferson-felizardo` confirmada em `turn_context` e `CODEX_HOME`. Fontes congeladas após esta entrega, sem stage, stash, commit ou push.

## Implementação

- Cliente existente preservado com URL base, Bearer, timeout nos GETs e cancelamento das cargas por geração. DTO de sessão aceita `cwd: null` e inclui `headless`, `options` e `problema` sem copiar o modelo inteiro da API.
- Erro 401 reabre Conexão com motivo visível. Falha de rede mantém a janela responsiva e oferece Tentar novamente. Um 404 do histórico atualiza a lista para distinguir sessão removida de transcript ainda indisponível.
- Histórico inicia com limite de 400 e Carregar anteriores pede 800, 1200 etc. Pedidos adicionais não se sobrepõem; eventos são costurados por `id`. Corrigida a ordem quando histórico e eventos correntes não compartilham um ID.
- Lista GPUI virtualizada mantém a âncora ao inserir eventos anteriores. `TextView` renderiza Markdown básico e permite seleção; Copiar preserva o texto integral. Ferramentas mostram resumo com aviso de erro. HTML bruto e imagens em Markdown aparecem como texto; anexos e conteúdo não suportado recebem indicação para abrir no Electron.
- Troca de sessão cancela cargas anteriores e descarta respostas de outra geração. Se `jsonl` ou `tracked` mudar para o mesmo nome, a seleção limpa conversa, cache de texto e posição antes da nova carga.

## Conferência no uso real

O binário `cargo build --locked` compilou no Linux. Restaram apenas avisos de campos DTO reservados para as próximas Tasks. Nenhum teste automatizado foi executado, conforme o plano.

Com o backend existente em `127.0.0.1:8765`, o token foi lido somente do `backend/.env` em memória e digitado por stdin na entrada protegida; não entrou em argumento, captura ou log. A janela mostrou sessões reais. Na sessão A, abriu histórico de 350 eventos; na sessão B, mostrou os últimos 400. Carregar anteriores elevou o limite a 800 sem mover a mensagem visível. A API retornou 400 IDs adicionais e a ordem dos dois trechos vistos na tela correspondeu à resposta normalizada. A rolagem alcançou mensagens anteriores. A troca rápida B → A → B terminou com conteúdo só da sessão selecionada.

Na própria sessão do executor, a posição e uma seleção de texto permaneceram visíveis depois de novos eventos. Token inválido mostrou “Token recusado” dentro do diálogo; endereço local indisponível mostrou “Sem resposta do servidor” e Tentar novamente. A conexão válida foi restaurada. A janela final permanece aberta no workspace 9, PID `2954064` no momento da conferência. A geometria deve ser relida antes de nova captura.

Capturas: `artifacts/task2-connected.png`, `task2-history.png`, `task2-older.png`, `task2-older-visible.png`, `task2-switch.png`, `task2-selection.png`, `task2-selection-after.png`, `task2-invalid-token.png`, `task2-unavailable.png`, `task2-final-connected.png`.

## Limite da conferência

A recriação real de uma sessão com o mesmo nome não foi provocada: esta Task permite apenas leitura das sessões existentes. A proteção foi conferida no caminho de código `replace_sessions` → `select`, que compara `jsonl` e `tracked` e invalida a geração anterior. O revisor deve considerar esse limite; não foi criada sessão de teste nem alterado o backend.

Steps 4 e 5 foram marcados como conferidos. O Step 6 fica aberto apenas para a verificação manual de recriação na sessão descartável da Task 5, conforme a coordenação.

## SHA-256 das fontes congeladas

```text
3c4121d7d0d1bd2359541759ef2a187e74c2ae09995a996b455fcc58689c476f  desktop-native/src/app.rs
0a0485b2227e7624162c2cc75bc2ddd40be596762efb538899f349c812bba541  desktop-native/src/chat.rs
984fbedd925b73d7182e5b87fe6e6c3f0dee478946d90f1d2d88440f9218b9e0  desktop-native/src/api/dto.rs
d08b4acdde3bf47dd1512a9d411ffd9062bb48a368620713ccc524e6c494cbb2  desktop-native/src/api/mod.rs
d334a8508b8aabfd2433f45b1e4c79890804e45c6e7c965536af9fe9edd235bb  messages/pt.json
fc86a30baa99c10c9d6352ec4f43ec2b81b6e1139e8acff61c50fc981622b3b2  messages/en.json
e6feef72b1b349cdeb0e0d0169f0d0e9e41df85a0e23a9502b2378590af50489  docs/superpowers/plans/2026-09-23-native-desktop-chat.md
```
