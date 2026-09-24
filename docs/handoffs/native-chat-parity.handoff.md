---
branch: native-chat-parity
plan: docs/superpowers/plans/2026-09-23-native-chat-parity.md
status: in_progress
saved_commit: 55e0d90911b7d0ee4208445045949005331c5131
---

## TL;DR
**ENCERRADO 2026-09-24 ~04:55:** Tasks1–5 aprovadas em 2 rodadas cada (pareceres em ~/.hangar/orq/2026-09-23-native-parity/pareceres/). Demo final: unidade hangar-native-parity-task5-final-20260924 (PID 1006328, binário bin/hangar-native-task5-r2b, ws12); demo antiga 4190872 preservada no ws9. Nenhum agente/fixture/vigia/monitor ativo. Árvore suja preservada (M messages/{pt,en}.json, ?? desktop-native/), sem stage/commit/push. Pendentes na matriz desktop-native/docs/chat-parity.md.
**Sucessão feita 2026-09-24T02:07:** árbitra é native-parity-arbiter (claude-opus-5-5/xhigh, ~/.claude-claude-200-5); Codex cessado, monitor de cota antigo parado, hangar fora do grupo e não deve receber mensagens. Vigia `vigia-bf7d08ed`. Itens abaixo sobre "aos50%" são históricos.
Tasks1–3 da paridade Rust ENCERRADAS em2rodadas cada. Task4 compositor/anexos/comandos LIBERADA ao Opus5.5/medium200-3; revisor Opus5.5/highJefferson aguarda entrega. Task5 barra direita/status/controles da sessão ainda pendente.
PRIORIDADE: monitor já avisou45% do Codex pessoal; última leitura47%, RELER. Aos50%, nova árbitra Claude200-5/claude-opus-5-5/xhigh, confirmar handoff e cessar Codex. Não depender da conversa.

## Task atual
Task4 Steps10–12 em execução. Escritor native-parity-task4-exec, config_dir ~/.claude-claude-200-3, modelo claude-opus-5-5/medium. Revisor native-parity-task4-review, ~/.claude-jefferson, claude-opus-5-5/high, código/.git somente leitura comprovados. Contas/argv/HEAD conferidos. Base congelada baseline-task4/ + baseline-task4.json no diretório operacional.
Task4 foi liberada explicitamente; não espera outra autorização. Roteiro task4.md inclui compositor ampliado (anexos, comandos, rascunho, atalhos, orientar turno e interrupção com confirmação/devolução de pendente). Novo grupo é grupo-bf7d08ed; antigo grupo-dc6e47d1 dissolveu. Todos os agentes anteriores e suas fixtures/cxs/janelas de teste foram encerrados.
Task3 terminou após correções de fila, Pi/omp/Kimi, scrollbars e confirmação de pergunta por SessionKey+toolID em segundo plano. Parecer final task3-r2-astra.md aprovado,15 hashes de hashes-task-3-r2b.txt conferidos. Não reabrir código inalterado dasTasks1–3.

## Concluído nesta sessão
- Fase1 preservada em HEAD55e0d909 destacado, sem stage/stash/commit/push; main não editada.
- Task1: prévia estável, revelação gradual de blocos vivo=false, vivo=true direto; prova Codex final e fixture2blocos, fonte/hashes conferidos.
- Task2: ferramentas/raciocínio recolhíveis, agrupamento/pareamento, cópia completa, coluna de leitura; corrigidos replay/histórico que apagavam item em voo e resultado ligado a chamada futura.
- Task3: perguntas/opções/planos/fila, sem auto-envio/retry, erro/incerteza visíveis; suporte Pi/omp/Kimi derivado dos eventos; confirmação sobrevive à seleção de outra sessão.
- Build e cargo test --no-run passaram em cada entrega. Nenhum teste automatizado executado. Provas de UI/sintéticas estão separadas de provas reais nos relatórios.
- Usuário ampliou o escopo para compositor E barra direita: plano tem5Tasks, inventário composer-rightbar-inventory.md incorporado.
- Monitor independente de cota instalado em unidade transitória; teste de entrega e aviso real45% chegaram à sessão. Regras persistidas no plano/contrato/memória e neste handoff.

## Decisões
- Origem usuário23/09: manter visual nativo próprio, não copiar Electron. Python/backend existentes intactos.
- Origem usuário23/09: ao Codex pessoal (~/.codex-jefferson-felizardo) atingir50% de uso em janela aplicável, criar NOVA árbitra native-parity-arbiter naClaude200-5 (~/.claude-claude-200-5), modelo EXATO claude-opus-5-5, esforço xhigh. Confirmar conta/modelo/aceite e cessar Codex; não gastar quota para atingir limite se o escopo terminar antes.
- Origem usuário: SOMENTE árbitra na200-5/xhigh. Executores Opus5.5/medium e revisores Opus5.5/high, alternar Jefferson/200-3 por folga. Nenhuma novaTask Codex. Conta200-5 reservada e ainda não aberta como árbitra.
- Origem usuário: até2revisões porTask incluindo finalárbitra, nenhuma revisão da revisão. Não invocar autorevisores/subagente visual/retrospectiva extra, mesmo que skill genérica mande. Bar nativo aprovado já está no plano/handoffs.
- Origem usuário: continuar autonomamente enquanto dorme, fechar agentes ao terminar participação, preservar demo independente. Não pedir novamente autorização para o escopo aprovado.
- Sem trocar branch, tocar main/backend/serviços existentes, stage/stash/commit/push. Testes só sob pedido; build/--no-run/uso real autorizados. Provas mutáveis só fixture/cx própria, nunca sessões do usuário.
- Config dirs corretos têm prefixo duplicado em200-3/200-5: ~/.claude-claude-200-3 e ~/.claude-claude-200-5. Verificar retorno da criação e argv, não deduzir pelo apelido.

## Limitações conhecidas
- Task4/5 ainda não concluídas; não declarar paridade global.
- Task5 tem casos explícitos remanescentes: hooksCodex antes de jsonl/tracked=true; implementar planoClaudeheadless com mudança de modo/envio e falha parcial; /plan-preview Claude terminal. Controles por sessão de modelo/esforço/permissão incluídos, administração global não.
- Task3: Tab chega a opção fora da área mas não a rola automaticamente; mouse/barra alcançam. Provas de interações são de fixture, não servidor real.
- Task2: grupo pode perder expansão ao agregar histórico antigo; detalhe longo mostra20mil caracteres com cópia completa. Relatórios descrevem demais limites.
- Windows/macOS e benchmark permanecem sem validação; não inventar ganho.
- Demo atual independente hangar-native-parity-demo-20260923.service, último PID4190872, backend real, AINDA binárioTask1. Tasks2/3 validadas em janela própria. Atualizar demo ao final sem perder rascunhos e conferindo PID/cgroup.
- API de cota usa cache normalmente até5min. Monitor de60s não depende de memória do modelo;45% prepara,50% exige transferência. Consulta indisponível/antiga não prova folga.

## Erros / armadilhas
- IncidenteT3NATIVE RESOLVIDO: saída virtual fez displaylayout.sh escrever monitors.lua e3cliques atingiram quickshell. Saída/linha removidas; root conferiu3saídas físicas/posições preservadas/sidebar fechada. PROIBIDO criar saída virtual, editar monitores/Hyprland ou reload global. Testes só em janela própria numa saída física existente com PID/foco/geometria/entrega conferidos.
- Dois autorevisoresECC foram iniciados indevidamente naTask3, interrompidos antes de concluir, retornos não usados. Não reabrir esses agentes/resultados. Revisão só nomeada+finalárbitra.
- Literal overflow_y_scrollbar com max_h falhou; solução aprovada local: overflow_y_scroll + ScrollHandle persistente + Scrollbar Always, sem tema global.
- CLI/MCP new_session Codex não seleciona codex_account como esperado; não há novaTaskCodex agora, mas não usar esses caminhos por engano.
- Handoff save prefere main quando cwd é worktree. Usar HANDOFF_DIR=../hangar-native-desktop/docs/handoffs para gravar nesta worktree; não criar arquivo em main.

## Arquivos críticos
- (N) docs/superpowers/plans/2026-09-23-native-chat-parity.md, fonte do escopo/progresso.
- (R) desktop-native/src/{app,chat,conversation,interaction}.rs e api/*; Tasks1–3 encerradas, novas mudanças sãoTask4.
- (N) desktop-native/docs/parity-task{1,2,3}-report.md, provas/limites.
- Diretório operacional: $HOME/.hangar/orq/2026-09-23-native-parity/ — succession.md (LER PRIMEIRO), regras.md, registro.md, arbiter-kickoff.md.
- Mesmo diretório: task4.md/task5.md, api-chat-contracts.md, composer-rightbar-inventory.md, baseline-task4/ e baseline-task4.json.
- Mesmo diretório: pareceres/task3-r2-astra.md, hashes-task-3-r2b.txt, diff-task-3-r2b.txt.
- Mesmo diretório: check-quota.py, quota-watch.py, quota-watch-state.json; unidade hangar-native-quota-watch-20260923.service.
- Grupo atual: $HOME/.claude/.hangar-pair/grupo-bf7d08ed.md. Pode sumir ao dissolver; contrato durável é regras.md.
- Notas de memória solicitadas pelo usuário em $HOME/.codex-jefferson-felizardo/memories/extensions/ad_hoc/notes/2026-09-24T02-34-51-native-parity-account-succession.md e 2026-09-24T02-45-00-native-parity-role-efforts-monitor.md.

## Próximo passo
1. Consultar quota/monitor antes de continuar:
```bash
python "$HOME/.hangar/orq/2026-09-23-native-parity/check-quota.py"
systemctl --user show hangar-native-quota-watch-20260923.service -p ActiveState -p MainPID
cat "$HOME/.hangar/orq/2026-09-23-native-parity/quota-watch-state.json"
```
2. Se personal_codex_used_pct>=50 (saída5), executar succession.md: nova sessão native-parity-arbiter, Claude200-5/claude-opus-5-5/xhigh, kickoff arbiter-kickoff.md, confirmar aceite e avisar os dois agentes ativos. Atualizar árbitra no contrato, parar monitor antigo e retirar hangar do grupo para não reativar Codex por avisos. Não fechar a sessão do usuário hangar. Cessar trabalho nesta conta.
3. Abaixo do limite: acompanhar Task4 já liberada e entrega direta executor→reviewer. Só rodada1 dele+finalárbitra. Após gate, liberarTask5 com cotas relidas, mesmo padrão de modelos/contas. Não reabrir Tasks1–3.
4. Ao concluir todo escopo: demo final independente, agentes/cxs/fixtures e monitor próprios encerrados, índices vazios, main preservada, relatório/matriz/captura final e limitações honestas; sem commit/push/testes sem pedido.
