# Parte 2B — runtime sem terminal Claude/Codex no Rust

**Data:** 03/10/2026. **Status:** proposta para aprovação; não implementar nesta etapa.
**Base de execução:** `hangar-server-parte1` em `a80a9e45`, já com a 2A. Esta worktree continua em
`f7f797bb`; os documentos não fizeram merge nem modificaram código. Incorporar a ponta acordada
e os encontros da 2C antes de executar. A 2A não será reimplementada aqui como correção Python.

**Contexto:** análise `docs/analise-hangar-server-parte2b-2026-10-03-claude-codex.md`; divisão 2A–2D
registrada na spec de 02/10. O presente desenho fecha a unidade executável 2B.

## Alcance

Rust assume a conexão do cano, reducer, requests/pendências, estado/prévia/pensamento/ferramenta,
controle, fila, recibos e confirmação e recuperação de **Claude headless e Codex headless**. Uma vida tem
um responsável. O cano continua dono do processo CLI, separado do backend; não abrir segunda CLI.
Histórico/transcript já portados na parte 1 continuam com mesmos campos/ids.

Python mantém API/autenticação/guest, descoberta/contas/configuração, escolha de argv/env/paths,
lançamento no escopo atual e serviços administrativos de dados. Esses serviços não abrem
cliente CLI, não executam reducers antigos e não escrevem fila fora do responsável atual.
Recado nativo UDS pode reutilizar o serviço Python, sempre por operação registrada/autorizada
pelo ator Rust; esse serviço não decide retry/fallback nem grava PromptQueue por fora.

Não entram Pi/Kimi/omp/orq/lista/quadro/canvas, nova UI, emulador, PTY, adaptadores terminal,
custos globais, instalação, remoção Python ou migração da voz Codex. Codex terminal segue Python
nesta unidade. A 2C observa terminais em paralelo; a 2D tratará a entrega Claude terminal.

## Dono e identidade

Usar `meta.key` durável dos sidecars, nunca só nome ou nome do transcript. Ela existe hoje em
claude_headless/sessions.py:52 e codex/sem_terminal.py:107. Nome pode mudar, sid/thread pode mudar.
Binding inclui key, generation, provider/modo, sid/thread, cano pid/escuta/token e paths resolvidos
pelo backend. Toda operação revalida binding após espera e antes do despacho.

Trava de arquivo separada por key, mantida durante toda a posse; arquivo de trava não é
substituído/deletado/renomeado. Rust usa File::try_lock da stdlib 1.98.1; Python usa flock/locking
equivalentes e reutiliza a lease já adquirida. Windows precisa de prova entre processos, não
inferência a partir de biblioteca. Nome/projeção podem mudar sem trocar essa identidade.

Fases: Python → PreparingRust → Rust → RecoveringPython → Python. Fases intermediárias bloqueiam
novas mutações com erro explícito. Não cair no caminho Legacy porque uma chamada nativa demorou.
Fila gerenciada continua gerenciada quando uma sessão vira terminal; seu responsável passa para
Python sob a mesma trava/estado, sem dois arquivos autoritativos concorrentes.

## Cano v2, prefixos em voo e confirmação

Subir as versões Python/Rust do cano juntas para 2. Preservar campos anteriores; corrigir IDs
int/string dos pedidos JSON-RPC no snapshot e separar eventos de subagente do turno do pai.
Novo cano recebe token também no Unix, mantendo 0600. Cano v1 fica no Legacy até reabertura
natural ou ação explícita, sem matar sessão ociosa só para trocar versão.

Autenticação normal `<token>\n` entrega writer; `peek <token>\n` retorna snapshot e fecha,
sem substituir writer atual nem mandar stdin. Prepare valida snapshot antes de desarmar Python;
claim lê snapshot fresco após adquirir a trava. Peek nunca é tentado em cano v1 sem essa capacidade.

Snapshot inclui inflight Claude (text/thinking/tool+rawinput/index) e Codex por thread
(text/item/turn). Coleta por pedaços; só serializa no snapshot. Prefixo completo restaura
buffer sem reapresentar deltas antigos. Se exceder orçamento, `complete:false` impede publicar
sufixo como prévia completa até a próxima fronteira segura; nunca truncar e declarar full.
Pedidos pendentes não são descartados para caber; snapshot impossível recusa takeover.

CLI raw e snapshot têm teto de 16 MiB; envelopes input/output aceitam 32 MiB + 1 KiB por causa dos escapes.
Native input: `{type:cano_input,operation_id:<wire-id>,frame:<linha JSON CLI>}`. Cano valida,
escreve/flush no filho, atualiza tracker só após sucesso e emite cano_input_ack. Prefixo sem
newline/JSON inválido nunca é encaminhado como comando. CLI stdout v2 vem em cano_output para
ACK privado não ser confundido com mensagem do provedor. Readers Legacy v2 desembrulham;
v1 permanece compatível. ACK perdido/IO parcial é Unknown; socket flush não significa stdin flush.

## Runtime e operação registrada

Ator por vida, reader/writer separados, caixas limitadas e núcleos puros Claude/Codex. Não
esperar reply RPC dentro do processamento do ator: estado, cancelamento e perguntas continuam
chegando. Relógio monotônico para timeout/publicação, epoch para quota/timestamp; não comparar
monotônico salvo de outro processo. Buffers Rust acumulam por pedaço e publicam full-replace
primeiro, a cada 150 ms e último, sem parse/clone do prefixo a cada delta. A 2A define o comportamento base.

Prepare registra intenção e input na fila antes de efeito; BeginDispatch/fsync registra tentativa
antes de bytes. Requests compostos têm tentativas `wire:<op>:<request/seq>` distintas, agregadas
na operação lógica. Contexto/permission/effort não é marcado aplicado antes da confirmação própria.
Reply tardio só resolve mesmo ID com mesmo tipo/geração; não resolve operação nova com id reciclado.

Accepted/Deferred/Rejected/Unknown são distintos. Deferred com fila persistida mantém mensagem
visível; NotWritten pode devolver claim. Unknown não volta delivered=False nem muda para outro
transporte. UUID/arquivo atômico não prometem entrega exatamente uma vez pela CLI. Recibo CLI
definitivo pode chegar antes do ACK do cano; erro posterior não rebaixa esse resultado nem repete.

Fila tem estado autoritativo privado por key contendo rows e diário; JSONL do nome é projeção
compatível com os readers existentes. Commit autoritativo e projeção precisam terminar antes
do sucesso; falha de projeção mantém recibo salvo, fica visível e é reparada com mesma operationId,
sem recomputar mutação. Falha de fsync depois do rename bloqueia novas mutações até reler
estado/reparar projeção, sem restaurar cópia antiga. Reserva Python grava o mesmo
estado/projeção sob lease, não só JSONL.
Leitura de histórico com projeção inválida falha explicitamente, não devolve história velha com 304.

Todos os métodos PromptQueue são encaminhados: append/local/claim/delivered/confirm/reconcile/
desistir/prune/remove/clear/rename e mutação privada. Follow pode permanecer Python só de leitura,
uma fonte para fila; não publicar o mesmo queued/local event por duas fontes. CAP de 1000 só poda
entradas confirmadas ou saídas locais; desistiu sem confirmação e Unknown são conservados.
Não eliminar pendência silenciosamente para aceitar novo input.

Confirmação usa ocorrência do transcript da conversa atual, com ID/offset e cursor capturado
antes do despacho; ocorrência não confirma dois prompts iguais nem confirma segunda tentativa
em chamada posterior. Linha parcial, rewrite, inode/anchor, queued_command/enqueue/dequeue,
anexos/Unicode/Windows e falta de timestamp têm casos específicos. attachment/queued_command
é prova de steer entregue; queue-operation/enqueue sozinho não comprova consumo. Ausência/arquivo ilegível
não comprova perda e não autoriza reenvio. Effort local sem confirmação recuperável fica Unknown,
sem inventar ACK ou repetir comando no restart.

## Entrada e fachadas

API/grupo/par/hook/loop/SSE/fim de turno encaminham para o mesmo responsável antes de Legacy
append/UDS/stdin/RPC. Warm/reconnect/parking/spawn revalidam posse depois de awaits. Snapshot,
picker/comandos e leituras diretas `_sessions` passam para RuntimeView cacheado; não montar um
_Sessao Python com leitor para fingir que portou runtime. Terminal e outros providers mantêm
implementações existentes. Perguntas/approval preservam schema, tipo de ID e request corrente.

ManagedRuntime seleciona apenas headless; ManagedQueue seleciona key já gerenciada inclusive
na reserva/modo terminal. Mudança de modo/thread, clear, rename, kill, reload e conta usam barreira
de lifecycle. Não remover arquivo/fila/cano quando parada falhou; não despachar para vida seguinte.

## Reserva e partida

Rust abre controle privado em 127.0.0.1:0 no mesmo processo. Supervisor recebe UMA linha JSON de
partida por stdout PIPE com instance/protocol/port, valida e guarda segredo/endereço em memória.
Não depender de que a porta pública ligada só na LAN aceite conexão 127.0.0.1. Nenhum segredo vai
ao os.environ Python, à sessão, à URL, ao log ou ao health público.

Transferência: peek válido; barrar operações; aguardar clientes/control/drains em voo; desarmar
reconectores; fechar/juntar reader e writer antigos sem matar cano; liberar lease Python;
Rust claim/importar/reparar/conectar/hidratar; ready só do mesmo instance/key/generation.
Falha requer detach confirmado e trava obtida antes de restaurar Python. Não furar a barreira.

Rust morto: suspender envio, confirmar processo/client antigo encerrado, adquirir mesma lease,
carregar estado/journal, converter tentativa em despacho em Unknown, reparar projeção, abrir
um cliente Legacy no cano original, aplicar snapshot antes de drenar. Falso health/timeout com
processo ainda vivo não libera writer. Sem Rust/desligado/incompatível, Python funciona como
reserva; sem segundo lifespan nem segunda CLI. Controles de reserva também registram antes do IO.

## Encontro com a 2C e versão interna

Base mínima da 2B: a80a9e45 com a 2A, mais a ponta integrada da 2C nos arquivos compartilhados.
Em 03/10, rust-parte2c esclareceu que d21a445b já usa protocolo **2** local com acquire/capture/
release/reduce. A correção sem push retira `reduce` e a segunda HTTP do reducer, mudando o
contrato: subir os dois lados da 2C para **3**. `terminal_address` na saúde e side-events
continuam. A 2B segue só documental e não publicou protocolo novo. Reservar o primeiro
contrato da 2B em **4**, após integrar a ponta corrigida da 2C; se outro contrato entrar antes,
escolher o próximo valor real. Revalidar a ponta antes da integração.
Preservar `terminal_address` da 2C; a regra de endereço fora da saúde vale para o IPC novo da 2B.

| Arquivo/ponto | 2B | 2C | Regra de encontro |
|---|---|---|---|
| rust_server.py / lib.rs | IPC privado/startup/posse/reserva runtime | configuração/desativação observador terminal | um Supervisor, um lifecycle; desativar ambos antes da reserva |
| Config / routes / AppState | runtime separado por key, controle em porta privada | pool/parser/rota terminal | preservar campos/routers existentes; não substituir construtor inteiro |
| side-events / adapters / sse.py | fonte headless RuntimeView | fonte terminal/capture/reducer | seleção por provider+modo+vida; nunca duas fontes públicas para o mesmo campo |
| fila/drain/Codex | headless dono Rust | terminal observação, controle ainda existente | JSON-RPC terminal não vira TUI; compartilhar codec depois sem mudar dono agora |
| protocolo/env | contrato novo 2B | contrato novo 2C | versão seguinte após integrar o que já entrou; subir Python/Rust juntos a cada mudança |

A 2C corrigida usa 3; o primeiro contrato da 2B usa 4 depois da integração, ou próximo número real.
Toda mudança posterior de formato publicado recebe novo número, com constantes dos dois lados. VERSION 2 do cano é protocolo separado. Formatos
públicos/ChatEvent/ids não mudam; mudanças internas precisam dos dois lados no mesmo commit.

## Como provar / pronto quando

Plano: testes escritos primeiro, comparação sintética com Python gerado com a80, cano/CLI/app-server falsos e processos separados
para trava. Rodar somente quando o dono pedir. Nenhum código do produto foi aplicado, compilado ou testado.
Provar retomada v2, peek sem takeover, ACK positivo, partial/EOF/timeout, prefixo completo e pending IDs,
init real, modelos/modos/plano/effort, async question, nota local, quota/usage e neutralidade desconhecida.

Falhas nos pontos prepare/claim/beforewrite/afterwrite/beforereply/projeção; writer único com
POST+hook+drain+turn-completed; fonte sem SSE e dois aparelhos; mesmos textos e source/epoch
mudados; takeover/reserva/mode-switch sem perder entrada/pergunta; Windows conforme regras.
Uso real somente no canal acordado com o dono, sem tocar serviços/sessões para fabricar prova.

A 2B fica pronta quando runtime sem terminal está nativo com fila/controle como unidade, caminhos Legacy
condicionados ao responsável e reserva comprovada, fixtures/checks autorizados e uso real registrados. Cano v1 vivo pode
continuar Legacy por compatibilidade documentada; não chamar isso de migração forçada concluída
de todas as sessões antigas. Sem ACK/controle incerto há erro/pendência, jamais sucesso falso.

**Plano executável:** `docs/superpowers/plans/2026-10-03-hangar-server-parte2b-claude-codex.md`.
