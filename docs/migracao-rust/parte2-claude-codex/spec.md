# Parte 2 — controle de sessões Claude/Codex, com e sem terminal

**Data:** 02/10/2026, America/Sao_Paulo. **Status:** proposta, aguardando aprovação; não implementar.
**Base:** worktree/branch `hangar-server-parte2`, `f7f797bb`; upstream parte1 `aa8eec36` (CI/teste,
sem mudança dos módulos levantados). Incorporar consertos na execução aprovada, sem trocar branch.
**Análise:** [escopo, medidas e evidências](../../analise-hangar-server-parte2-2026-10-02-claude-codex.md).

## O que o dono pediu

Levar para Rust o restante do caminho das sessões Claude e Codex, com e sem terminal. A leitura
de histórico/transcript já portada na parte1 permanece como base. Analisar envio/fila/estado,
prévia, permissões/perguntas e recuperação; avaliar controle tmux e emulador; mostrar ganhos
menores que possam ser corrigidos no Python. Dividir o trabalho se um plano único ficar grande.

Ficam fora Pi/Kimi/omp/orq, lista, quadro/canvas, custos/contas/administração, terminal PTY como
recurso novo, UI nova, instaladores e retirada completa do Python. Código compartilhado que
atende outros providers mantém o comportamento deles. Documentos anteriores ficam preservados.

## Decisão proposta: quatro subpartes, um responsável por sessão

| Ordem | Alcance e resultado | Pronto quando |
|---|---|---|
| 2A | Correção pequena Python: acumulação e publicação dos deltas Claude headless e prévia Codex nos dois modos | pedaços da mesma janela são agrupados; primeira/última atualização e limpeza corretas; nenhum controle/fila atrasado pelo timer |
| 2B | Runtime headless Rust para Claude/Codex: cano, eventos, estado/pendências/controles, fila/confirm e retomada, como unidade | só um dono lê/escreve; snapshot/reconciliação completam antes de liberar envio; fallback não duplica prompt nem cria CLI |
| 2C | Observação terminal Rust por tmux-C; reducer de estado/prévia/statusline com fontes nativas; Codex WS usa reducer da2B | mesma captura/estado/época; um observador por sessão; não altera pessoa presente/tamanho; Python reserva nas plataformas sem prova |
| 2D | Entrada Claude terminal Rust: socket nativo, plugin, driver/reserva tmux, fila, opções/teclas e prova de entrega | todos os gatilhos da mesma sessão usam a esteira; incerto/partial não geram envio duplicado; Windows comprovado |

O plano com código completo que acompanha esta spec cobre **2A**. O resto tem desenho e critérios
abaixo, mas não está liberado por esse plano: cada subparte terá spec/plano próprios antes de
execução. Aprovar a divisão não autoriza iniciar todas elas nem considerar o porte completo.
2A é melhoria preparatória Python; o controle passa para Rust na2B em diante. Não chamar uma
ponte de rota ou buffer Python de runtime portado.

## Linha de base e escolha técnica

Números/metodologia completos na análise: duas sessões vivas headless (Claude/Codex), canos
Python com seis threads, RSS10.992/21.780kB; histories mediana de cinco GETs17,848/33,423ms.
Não medimos envio real nem sessão com terminal real. CPU agregada em700ms não permite atribuir
custo por sessão. Benchmark parser sintético Write200KiB/128B mediana91,206ms para o lote;
campo-alvo prompt200KiB/32B acumulou21.532,751ms. Decode somente final é uma referência de
custo, **não** preserva input parcial e não é a implementação.

Transporte tmux Linux isolado:100 capturas por método, mesmo comando-S -200,4.490bytes e corpos
iguais; mediana1,798850ms subprocesso versus0,151227ms controle. Ganho de transporte11,895×,
não do backend completo. Controle persistente será medido novamente com a integração Rust/Python.

Emulador não é necessário para tirar o fork de cada poll. `alacritty_terminal0.26.0` já existe
no nativo, mas checkpoint inicial/reconexão não estão provados. Adicionar emulação somente se
captura via-C ainda tiver custo relevante **e** a tela reconstruída for comprovada. Observador
nunca manda PtyWrite para a CLI.

## 2A — desenho executável

### Acumulação e cadência

Criar `backend/app/adapters/stream_buffer.py` com um helper stdlib usado por dois adapters:
`StreamBuffer(publish, on_error=None, interval=0.15)`. Acumula com StringIO; usa geração, revisão,
uma tarefa de publicação pendente e trava somente da publicação. Não é fila de entrada ou actor.

- `append(piece)`: primeira publicação da geração imediata; depois acumula e publica a cada150ms
  enquanto houver mudança. Timer publica mesmo se os deltas pararem. Parte vazia não cria trabalho.
- `flush()`: publica último valor sujo imediatamente, sem duplicar valor já publicado.
- `discard()/reset()`: invalidam geração, cancelam/aguardam publicação pendente e em voo.
  Não publicam vazio: o adapter aplica a limpeza autoritativa depois de impedir callback velho.
- `invalidate()` é síncrono no loop para fechamento/rename; chamada de thread passa pelo loop.
  Callbacks ainda conferem sessão/vida; não converter isso em trava entre processos.
- `value`: compatibilidade de string, materializada só na leitura/publicação. Buffer iniciado
  com valor prévio posiciona cursor no fim; append não sobrescreve esse valor.

Callback resolve a fonte PushPreviewSource na hora: último subscriber pode remover a instância.
Não guardar uma fonte descartada como destino permanente. Uma sessão/buffer não afeta outra.
Falha de publicação aparece no diário/log; Codex também propaga erro aos ouvintes existentes.
Não inventar erro do turno ou faixa nova de UI para falha de prévia, nem incluir conteúdo na
mensagem de erro. Controles, perguntas, permissões, estado nativo e drain seguem imediatos.

### Claude headless

Substituir concatenação por delta de previa/pensamento/tool_json por buffers; conservar getters
de string que código/testes leem. `_input_parcial` continua a função atual, chamada na publicação
coalescida de tool, sem chamada incondicional por pedaço. Se cada pedaço chegar após150ms,
a publicação pode ocorrer em cada chegada; o limite é a cadência, não omitir atualizações lentas.
Conservar nome, JSON parcial, rótulo do alvo e inferência
atual de escapes/campos; não adicionar lexer JSON novo nem regra de truncamento.

Tool start publica `{nome,input:{}}`; primeiro delta publica imediatamente seu parcial. Deltas
intermediários podem esperar até150ms. Block stop faz flush antes de zerar buffer/nome e conserva
o input visível até o evento assistant que comprova a chamada no transcript. Texto/pensamento
usam suas fontes próprias. Assistant/result/interrupção/close/rebind invalidam timers antes de
limpar. Não reaparecer preview velha depois de result ou /clear. Tokens/usage continuam contando
todos os pedaços, independentemente de quantos frames foram publicados.

### Codex com e sem terminal

Reutilizar o mesmo helper no buffer local da bomba de notificações `_consumir`, eliminando
`buf +=` seguido de push por delta. Callback confere client/thread/sessão correntes; resolve
fonte no momento de publicar. Separação por item agentMessage/turno permanece: item start/end,
turn-completed e close limpam buffer+fonte, sem juntar preâmbulo à resposta seguinte.

State/usage/settings/approval/requestUserInput/async questions, error/safety buffering e
turn-completed/drain passam pelo fluxo atual sem timer. Não mudar transportes, turn/start,
thread/resume, fila, envio pela TUI, escolha de conta ou propriedade do cano. Teste deve exercer
o consumidor real e confirmar que estado/question não espera a janela da prévia.

### Limite do ganho e compatibilidade

Contrato público continua full-replace, mesmos nomes/campos/ids; não há mudança nos clientes
nem protocol interno na2A (continua1 nesta base). Produzir cada texto completo ainda custa seu
tamanho, e assinantes lentos continuam podendo perder frames intermediários. Não afirmar
linearidade do pipeline inteiro nem ganho de benchmark da variante final-only como ganho do app.

2A reduz trabalho repetido por delta e não exige credencial, configuração nova, instalação ou
restart. Todas as quatro combinações provider/modo seguem funcionando; Claude terminal não
ganha novo driver ou alteração de estado nesta subparte.

## 2B — contrato que precisa preceder o runtime Rust

Uma vida é identificada por session_key, sid/thread_id, provider/transport e runtime_generation;
runtime_owner diz qual processo pode controlar. Nome reutilizável não é identidade suficiente.
Não tentar usar a geração privada de conexão do cano como autorização de dois escritores.

Transferência: suspender novas operações; terminar/registrar operação em voo; parar bombas,
drains/reconectores/claims Python daquela vida; fechar cliente antigo; Rust conectar ao cano
**existente**, aplicar snapshot e reconciliar fila/pending; só então confirmar ACK e ficar
entregável. Python continua criando/resolvendo conta/argv/env/config e encaminha todas as
ações para o dono. Portar stdout headless sem impedir warm_sessions/watch_sessions não serve.

Interfaces privadas candidatas, a fixar no plano2B após aprovação da transferência:

- `RuntimeAttachment {key,generation,provider,name,sid/thread_id,cano,choices,configuration_stamp}`:
  o cano leva pid/escuta/token somente na conexão interna; nada disso vai ao front/log.
- `RuntimeCommand {key,generation,operation_id,kind,payload}`: submit/steer/interrupt/answer/select/
  set_model/set_effort/set_permission_mode/compact/read_settings/detach, validado por provider.
- `RuntimeReply {operation_id,disposition,request_id/turn_id,error_code}`: accepted/deferred/rejected/
  unknown. Unknown conserva a ambiguidade depois de write; não transforma isso em rejeição segura.
- `RuntimeEvent {key,generation,revision,event,data}`: StateEvent, preview/pensamento/ferramenta,
  ask_question e fila existentes, sem CLI raw no cliente. Snapshot cobre estado/pending/choices
  para nova anexação, sem prometer reconstrução de texto que canov1 não guarda.

Runtime Rust reúne reducer, pending por request id/tipo, initialized, turno/state, buffers,
uso/rate, request futures, idle/reload/retry limites e erro explícito. CLI desconhecida registra
tipo sem texto; pedidos desconhecidos mantêm recusa/aviso próprios de cada codec. Claude stream-json e
Codex JSON-RPC têm codecs separados; stdio e WS Codex usam o mesmo reducer e identidade de thread.
Model/settings vindos da TUI não são substituídos pelos defaults da criação. Pedidos desconhecidos
preservam o tratamento próprio: Codex responde−32601 e aviso; Claude mantém sua resposta de
compatibilidade atual e nota. Não homogeneizar os dois sem uma correção específica aprovada.

Fila: append/claim/confirm/reconcile/desistir e todos os gatilhos passam pelo mesmo dono.
Arquivo atômico+trava Python não coordena Rust. Transição deve congelar também escritos por
HTTP Python (incluindo convidado), hook, grupo/par, loop, SSE e fim de turno. Duas mensagens
iguais mantêm IDs e ocorrências distintas. Confirmar por transcript, não por ausência/recibo RPC.

Reserva após morte Rust: suspender envio, confirmar antigo processo/client encerrado, devolver
propriedade ao Python, aplicar snapshot do cano original e reconciliar antes de drenar. Filho
Rust ainda vivo mas sem resposta não permite writer Python concorrente. Operação já despachada
com resultado incerto fica visível pendente/erro para reconciliação; não repetir POST ao Python
por timeout. Não prometer entrega exatamente uma vez sem prova de idempotência fim a fim.

Snapshotv1 do cano restaura controle, mas não todos os buffers em voo. A2B decidirá se estende
snapshot ou aceita prévia indisponível durante recuperação com diagnóstico claro. Não inventar
reconstrução de texto parcial a partir de last_result. Mudança de snapshot sobe VERSAO e versão
Rust juntos, preservando fallback dos canos já abertos conforme regra da parte1.

## 2C — terminal e fontes estruturadas

Rust mantém controle por sessão observada, refcount por consumidor, nunca por aparelho.
Usar o mesmo capture-pane com pane resolvido pelo mecanismo atual, histórico/linhas vazias,
ansi/join conforme chamada. Entrada HTTP interna é um pedido tipado de captura, não comando
tmux livre. Erro de parser/frame/capability não vira texto vazio; volta ao leitor Python.
Contrato carrega binding/geração e início da leitura para descartar dados anteriores a /clear.

`tmux -C` observador ignora tamanho/saída assíncrona quando só captura, attach-E conserva env;
não envia tecla, resize ou respostas ANSI. Filtrar client_control_mode dos guards de pessoa
presente antes de habilitar, mantendo decisão conservadora em erro. -C fora de uma sessão não
recebe automaticamente saída de todas; uma conexão por sessão simplifica recuperação.

Portar reducer com sequência de fatos/frames e precedência completa: Claude registro nativo
válido → plugin/hook → pane/fallback; Codex snapshot assinado/saudável da thread → fallback.
Não portar só classify esquecendo debounce/frozen spinner/overlay/login/limite/pergunta/Shell.
Prévia mantém sidecar antes de pane, None versus vazio e sua época; statusline mantém texto
inteiro. O estado público ainda é formado uma vez por sessão e repartido aos aparelhos.

Upstream psmux documenta suporte-C/-CC, mas versão/protocolo instalado não foram provados:
consulta SSH falhou por chave não validada. Windows fica no caminho existente até ensaio
isolado comprovar byte/frame/charset/timeout/alvo/dimensões/presença. Não contornar essa
verificação nem escolher -C porque comando rc0 alegou sucesso.

## 2D — envio Claude terminal

Portar a esteira, não um send-keys isolado: socket nativo só recado, plugin com prova de entrega,
tmux automático só por ausência/resultado seguro. Composer, overlay, pane interativo, paste,
Enter, limpeza, partial, multiline/clipboard e seleção são parte do contrato. INCERTO impede
retry que digita por cima. Slash/clear/steer seguem suas regras próprias; palavra “steer” não
autoriza Ctrl+x Ctrl+s automaticamente.

Codex terminal já escreve pelo RPC compartilhado da2B/2C; nunca encaixá-lo no driver Claude
porque tem tmux. Todas as ações internas/HTTP/recados apontam ao mesmo dono, com geração
conferida depois de esperar lock e antes de despachar. Políticas de guest/share/pair e allowlists
continuam iguais; ações não portadas são repassadas, e fallback pós-despacho não repete o efeito.

## Contrato, reserva e distribuição

Manter porteiro Rust/porta8765 e Python atrás, segredo só em memória/filho, mesmos headers/gates,
convidados no repasse, portas8766/8768 Python. Reserva HTTP usa mesmo processo/lifespan;
serviço/instalador/toolchain1.98.1/glibc2.28 não mudam nesta etapa.

Protocol atual1:2A não muda. Cada futura entrega que mudar rotas internas, side-events ou env
sobe RUST_SERVER_PROTOCOL/INTERNAL_PROTOCOL juntos no mesmo commit. Não fixar hoje o número
de todos os commits futuros nem herdar o protocol3 do plano anterior de escopo descartado.
Código público/ids de conversa mudam nos dois lados quando necessário, sem esquecer clientes.

## Como provar e quando fica pronto

2A: testes do helper e consumidores reais primeiro; conteúdo exato/primeiro/periódico/último,
JSON parcial/label, reset/clear/close/thread change, erro não engolido, tarefas encerradas,
permissões/estado/drain sem atraso. Rodar somente quando o dono pedir; medir publicações/decode,
CPU e loop no cenário definido. Uso real depois com Claude headless e Codex nos dois modos;
Claude terminal apenas conferência de regressão, sem dirigir sessão de trabalho.

2B–2D: golden sintético com codecs reais, fake CLI/cano/WS e crash nos pontos de claim/write/ACK;
recuperação sem reenvio cego, identidade/requests/pending, fim de turno sem SSE, dois aparelhos,
mesmo nome recriado, troca de modo/thread, mensagens iguais/anexo, composer ocupado/plugin
incerto, convidados e Windows. Gates e reserva precisam de prova em uso real autorizado.

**Parte 2 inteira pronta** só quando os quatro caminhos definidos forem portados e aprovados,
com cada fronteira remanescente Python documentada, sem disputa pelo cano/fila, perda de pergunta
ou duplicação de prompt. **2A pronta** significa somente buffers/cadência conferidos; nunca
marcar toda a migração concluída por esse primeiro plano. Não implementar antes de aprovar a divisão.
