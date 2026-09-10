# Tokens e estimativa de custos do Codex

Conferido em 10/09/2026 com Codex CLI 0.154.0, arquivos locais e fontes oficiais.
O JSONL informa quantidades de tokens. A estimativa em dinheiro do Hangar aplica
tarifas de API; não representa a fatura nem a cota de uma assinatura ChatGPT.

## O que existe no arquivo

O Codex grava o histórico em `CODEX_HOME/sessions/**/rollout-*.jsonl`; conversas
arquivadas ficam em `archived_sessions`. O gravador serializa cada registro JSON
seguido de uma quebra de linha. [Código do gravador](https://github.com/openai/codex/blob/818f1cca8ccf8899f0f4d59336baebaccf358eed/codex-rs/rollout/src/recorder.rs).

`token_usage_record.payload.usage` contém o uso de uma resposta concluída.
`response_id` identifica a resposta; `thread_id` identifica a conversa dona do uso.
`turn_token_usage` e `thread_token_usage` são acumulados, não parcelas adicionais.
O evento legado `event_msg` com `payload.type = "token_count"` traz
`info.total_token_usage` e `info.last_token_usage`. O código acrescenta o último
uso ao total: somar os retratos do total cobra o histórico repetidamente.
Nenhuma dessas estruturas contém preço em dólares.
[Protocolo oficial](https://github.com/openai/codex/blob/818f1cca8ccf8899f0f4d59336baebaccf358eed/codex-rs/protocol/src/protocol.rs).

O leitor prefere registros por resposta, elimina respostas repetidas e uso
herdado de outra conversa. Quando faltam registros por resposta em um turno,
usa diferenças dos acumulados legados. Modelo vem do `turn_context` vigente;
dia vem do instante do uso, não da criação do arquivo. Contas usam a origem
`CODEX_HOME`, sem reunir identidades de contas diferentes pelo método de login.

## Como contar e precificar

| Campo Codex | Tratamento |
| --- | --- |
| `input_tokens` | Toda entrada, incluindo leitura e escrita de cache |
| `cached_input_tokens` | Parte da entrada reutilizada do cache |
| `cache_write_input_tokens` | Parte da entrada escrita no cache; ausente vira zero |
| `output_tokens` | Saída total, já incluindo raciocínio |
| `reasoning_output_tokens` | Detalhamento da saída; não somar novamente |
| `total_tokens` | Total informado, não uma parcela adicional |

O parser oficial converte os detalhes de cache e raciocínio da Responses API
para esses campos. [Conversão de uso](https://github.com/openai/codex/blob/818f1cca8ccf8899f0f4d59336baebaccf358eed/codex-rs/codex-api/src/sse/responses.rs).

Entrada comum = entrada total − leitura de cache − escrita de cache.
Essa é a fórmula publicada no [guia de cache](https://developers.openai.com/api/docs/guides/prompt-caching).
Raciocínio é cobrado como saída, conforme o [guia de reasoning](https://developers.openai.com/api/docs/guides/reasoning).
Cada parcela é multiplicada por sua tarifa por milhão; as quatro parcelas
somadas produzem a estimativa. Somar cache novamente à entrada bruta ou raciocínio
novamente à saída aumenta indevidamente tokens e valor.

Tarifas Standard conferidas, em USD por milhão de tokens:

| Modelo | Entrada comum | Leitura de cache | Escrita de cache | Saída |
| --- | ---: | ---: | ---: | ---: |
| GPT-6 Astra | 10,00 | 1,00 | 12,50 | 50,00 |
| GPT-5.6 Sol | 4,00 | 0,40 | 5,00 | 20,00 |
| GPT-5.6 Terra | 2,00 | 0,20 | 2,50 | 12,00 |
| GPT-5.6 Luna | 0,20 | 0,02 | 0,25 | 1,20 |

Para esses modelos, uma resposta com mais de 272.000 tokens de entrada total
usa 2× as tarifas de entrada/cache e 1,5× a de saída. O limite é aplicado antes
de juntar respostas, dias ou modelos. O total de uma sessão longa não basta
para ativá-lo. [Tabela oficial de preços](https://developers.openai.com/api/docs/pricing).

O cache local de preços conferido coincidia com essas tarifas. O snapshot do
repositório ainda não tinha Astra e mantinha Sol em 5/0,50/6,25/30; essas entradas
foram corrigidas. O relatório continua usando o catálogo disponível no momento,
sem reconstruir preços históricos. Tarifas manuais permanecem como definidas.

## Limites da estimativa

Os registros locais inspecionados tinham escrita de cache igual a zero. Isso
não prova que a API nunca cobra escrita: o campo existe e é considerado quando
positivo. Ausência de tarifa significa preço desconhecido, não uso gratuito.

Os `token_usage_record` e `turn_context` locais não informavam `service_tier`,
modo Fast ou região de processamento. Não é possível reconstruir seus adicionais
de cobrança desses arquivos; a estimativa usa Standard. A tabela oficial publica
preços diferentes para Fast, Batch e Flex e adicionais regionais.
[Modalidades de preço](https://developers.openai.com/api/docs/pricing).

O Codex descreve o registro por resposta como uma medição de melhor esforço.
Arquivo incompleto ou uso não registrado não pode virar promessa de uma fatura
exata. A ocupação da janela de contexto e o percentual de cota da assinatura
também não são o total de tokens processados pelo período.

## Conferência das outras fontes e desempenho

O Claude também passa a separar uso por dia/modelo e a deduplicar blocos pela
identidade da resposta (`message.id` + `requestId`). Escrita de cache de 1 hora
usa 2× a tarifa de entrada, enquanto 5 minutos usa 1,25×, conforme o
[guia oficial da Anthropic](https://platform.claude.com/docs/en/build-with-claude/prompt-caching).
Pi, oh-my-pi e Kimi conservam a agregação por sessão do leitor anterior;
seus cortes por dia/modelo não têm a mesma precisão dos leitores corrigidos.

O cache Codex passa a invalidar por arquivo, conta, tamanho e instante de
modificação. Uma sessão viva não obriga a reler todos os rollouts da conta.
O cache Claude muda para versão 2: há uma reconstrução inicial do histórico,
seguida de reutilização por arquivo. A interface publica cada máquina ao
receber sua resposta e sinaliza total provisório enquanto outras estão pendentes.
