# Revisão corretiva da 2C — conferência

Todos os itens 1–9 e menores corrigidos, com regressões antes da implementação, commits por item e revisão independente sem pendências. Branch `hangar-server-parte2c`, HEAD `e0c44443`, base desta correção `d21a445b`; árvore limpa, sem push/merge.

## Resultado

- Divergência transitória de `/clear` aguarda convergência; tarefa de prévia finalizada é retomada.
- Clientes de controle não contam como pessoa/janela nos scripts de painel, envio e navegador.
- Falhas persistentes têm pausa crescente por sessão; marcadores não exigem `display-message -l` do tmux 3.4.
- Espera curta e executor dedicado com quatro trabalhos máximos; circuito temporário; erro antigo não altera a geração nova. Trava global Rust não espera resposta de actor.
- Codex não abre lease/heartbeat sem consumidor. Inicialização/heartbeat tratam falhas preservando Python.
- Sem grade/alacritty, checkpoint ou outra HTTP para cálculo temporal. Uma conferência de alvo e uma captura por rodada, seis comandos com molduras e uma HTTP. O cálculo temporal usa o quadro no Python; referência Rust mantém paridade e guarda de constantes.
- Diagnóstico registra causa estática/ErrorKind, status e transições de reserva, sem conteúdo; recuperação só após quadro válido. Limpeza de cache não muda época de outra sessão.
- Contrato Python/Rust 3; combinado com `rust-parte2`: primeira 2B posterior em 4, ou próximo número real se houver outra mudança.

## Critério de custo

| Chats | Python, CPU/chat/rodada | Corrigido, CPU/chat/rodada | Redução |
|---:|---:|---:|---:|
| 1 | 2,803 ms | 1,668 ms | 40,50% |
| 4 | 2,745 ms | 1,784 ms | 35,02% |

Método: quatro lotes alternados de700 capturas/chat,20 de aquecimento fora da conta; soma de CPU dos processos Python, Rust e tmux/filhos; dividido por 700 × chats. Mesmos panes sintéticos e produtores, sem fallback. Os oito pares de lotes tiveram eventos completos iguais, conferidos também nos dados preservados. Cliente tmux persistente: zero criações recorrentes por rodada versus uma no Python, um cliente residente por chat (mais o Rust compartilhado). Startup separado: 60 ciclos, 2,000 ms CPU/ciclo. Com quatro chats, parede final 5,072 ms versus Python 4,804 ms; o critério é CPU/processos e não foi apresentado ganho de latência concorrente.

Medição versionada e método completo em `docs/decisoes/harnesses.md`, seção “Custo da observação terminal”. Dados, verificador, scripts e relatórios preservados em `.superpowers/sdd/2026-10-03-hangar-server-parte2c-revision/`.

## Verificações

Última conferência focada:291 casos Python aprovados e um skip existente;38 Rust aprovados. Scripts de painel/identidade:4 casos de dados, 13 de abertura, 18 verificações de envio e 7 de navegador, sintaxe válida. Etapas anteriores têm provas específicas, sem somar reexecuções. Não rodou suíte inteira.

Revisão do primeiro lote, revisão dos itens 4/7 e geração e revisão final aprovadas. Uma falha de abertura do programa falso em um teste Rust anterior foi registrada; duas repetições focadas passaram e a causa não foi reproduzida com instrumentação. Não foi atribuída a uma hipótese como se fosse fato.

Somente tmux privado, HTTP sintético e dados inventados. Nenhum serviço/backend/provedor/app vivo foi alterado ou validado; Windows real não executado. Sem afirmação de carga em produção.

## Commits da correção

```text
457f26f2 fix(observers): survive delayed SSE bindings after clear
6f1f5984 fix(observers): keep heartbeat failures visible and recoverable
da54d2f8 fix(panel): ignore terminal control clients in local attachment checks
41bf10e4 fix(codex): remove unused terminal observation lease
a340b931 fix(observers): preserve Python fallback when lease startup fails
b104db6c fix(observers): expire cached frames without advancing session epoch
59d7a0d2 fix(observer): back off persistent terminal control failures
4bf95103 fix(observer): bound stalled requests and release the shared pool lock
ad81ae44 fix(observer): report safe failure causes and validated recovery
84796160 test(observer): guard reducer debounce parity with Python
80f93dc2 fix(observer): ignore resolver failures from retired generations
e0c44443 perf(observer): use one capture and one HTTP per terminal round
```

Temporários da medição removidos após preservar dados/scripts; nenhum processo privado permaneceu ativo. Resumo entregue à sessão `hangar` pelo MCP tipado em 03/10/2026.
