# Parte 2B — análise do runtime Claude/Codex sem terminal

**Data:** 03/10/2026. **Estado:** planejamento para aprovação, sem implementação.
**Base de execução:** `hangar-server-parte1` em `a80a9e45`, PR único #24, já com a 2A.
**Worktree desta análise:** `hangar-server-parte2`, HEAD `f7f797bb`; não houve merge/checkout.

A 2B migra o runtime sem terminal para Rust. A 2A já integrada é a base de comportamento dos
buffers; esta entrega não propõe fazer outra correção Python antes da migração. Python continua
como reserva e como API pública durante a transição. Isso exige encaminhamento e exclusão reais,
não dois runtimes tentando entregar a mesma mensagem.

## O que a parte 1 já oferece

O `hangar-server` ocupa a porta pública 8765 e atende histórico/eventos Claude/Codex com o
token do dono. O restante é repassado ao Python. Parsing de transcript, eventos públicos e
compatibilidade de convidados já têm contrato. A 2B reutiliza essa leitura, os modelos de
eventos e o processo supervisionado; não abre outro serviço para controlar sessões.

Na ponta a80, a 2A já inclui o helper de buffer, integração Claude/Codex e ajustes posteriores
de rename, EOF antigo, falhas de publicação e logs. Usar apenas o primeiro commit da 2A perderia
essas correções. O diff da ponta foi lido sem aplicá-lo nesta árvore.

## Por que cano, fila e controle precisam migrar juntos

O cano durável já mantém a CLI viva quando o backend reinicia, mas substitui o cliente quando
outra conexão assume. Claude Legacy reconecta; Codex aquece sidecars e mantém RPCs. Se o Rust
assumir só transporte, um reconector Python pode tomar o cano de volta enquanto a fila continua
sendo alterada por outro processo. Escrita atômica de JSONL não impede esse conflito.

Entradas também vêm de POST, steer, grupo/par/recado, hook, SSE, fim de turno e encadeamento.
Controles passam por modelos, esforço, permissão, perguntas, select e interrupt. Apenas trocar
o handler de input deixaria caminhos de envio e escrita Legacy ativos.

| Unidade existente | Destino da 2B |
|---|---|
| `_Sessao`, `_ler`, `_on_event`, `_on_stream`, `_ctrl` Claude | Estado, reducer, encoder e espera de controles no núcleo Rust. |
| appserver/adapter/async_questions Codex headless | Correlação RPC, thread/turno, perguntas e controles no núcleo Rust. Codex terminal fica no caminho atual. |
| PromptQueue e seus métodos privados | Uma autoridade por key, com diário e projeção compatível; reserva usa o mesmo estado. |
| warm/reconnect/parking/drain | Consultam posse após espera e antes de abrir/escrever; desarmados quando Rust assume. |
| getters diretos de `_sessions` | Cache RuntimeView por key/geração, sem criar leitor Python oculto. |
| launcher, conta/config/argv, formatação, quota, arquivos administrativos | Serviços Python delimitados, sem cliente CLI/reducer ou escrita independente de fila. |

## Decisões necessárias para a transferência

Identidade é `meta.key`, estável no sidecar. Nome e sid/thread podem mudar; a trava de arquivo
por key permanece durante cliente, fila e tarefas de persistência. Fases intermediárias barram
mutações. Timeout HTTP com Rust vivo não libera Python para tentar o envio de novo.

Cano v2 acrescenta leitura `peek` autenticada sem tomar a conexão, prefixo em voo no snapshot,
IDs de pedidos com tipo preservado e ACK depois de escrever/flush no stdin do filho. Flush do
socket do backend não prova que a CLI recebeu o comando. Linha parcial e ACK perdido exigem
resultado incerto, sem reenvio automático. Cano v1 vivo permanece Legacy até reabertura natural.

Fila passa a ter estado autoritativo por key com linhas e diário; JSONL por nome é projeção para
histórico/follow. Falha entre os arquivos não perde intenção nem permite recalcular append.
Confirmar entrega exige ocorrência posterior ao cursor capturado antes do despacho. Um echo
não pode confirmar dois prompts iguais; conjunto de texto de todo o histórico é insuficiente.

Os núcleos não fazem IO. Um ator por vida executa efeitos com registro antes dos bytes e mantém
reader/writer separados para que espera de RPC não bloqueie estado ou cancelamento. Preserva
primeiro frame, intervalo de 150 ms e limpeza da 2A. Leitores SSE não controlam a vida do ator.

## Encontro com a 2C

Outra sessão executa a 2C em `hangar-server-parte2c`. Esclarecimento recebido em 03/10:
`d21a445b` já usa protocolo 2 local com acquire/capture/release/reduce. Retirar `reduce` e a
segunda HTTP do reducer muda esse contrato; a correção sobe Python/Rust para **3**, sem push
ainda. Endereço terminal na saúde e side-events permanecem. A 2B não publicou protocolo; seu
primeiro contrato novo usará **4** após integrar a ponta corrigida da 2C, ou próximo número
real. Não reutilizar versão de um esquema anterior; reler a branch antes da integração.
Os encontros são Supervisor/configuração, campos de AppState/routers, protocolo interno e
`side-events`. A 2B fornece headless, a 2C fornece terminal; nenhuma fonte publica o mesmo campo
duas vezes. A versão interna é o próximo número após integrar o que já entrou, com constantes
Python/Rust juntas. A versão 2 do cano é independente.

## Evidência e limites

Esta análise usou leitura de código, commits e documentos; não executou produto, testes, builds,
serviços ou CLIs. Os cenários e verificações estão no plano, ainda por executar após aprovação
e solicitação dos testes. Não há medição nova de ganho da 2B. Medições anteriores estão na
análise de 02/10 e não provam desempenho do runtime nativo ainda inexistente.

Windows exige prova de lock entre processos Python/Rust, replace e encoding. A tentativa de
consulta remota anterior foi impedida por verificação da chave SSH; essa prova permanece sem
conferência, sem desativar a verificação da chave. Uso real deverá ocorrer no canal autorizado
pelo dono, preservando sessões de trabalho e serviços existentes.

**Desenho:** `docs/superpowers/specs/2026-10-03-hangar-server-parte2b-design-claude-codex.md`.
**Plano:** `docs/superpowers/plans/2026-10-03-hangar-server-parte2b-claude-codex.md`.
