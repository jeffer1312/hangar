# Task 1: prévia contínua e rolagem

Base: `55e0d909` destacado. Esta entrega preserva a fase 1 e altera somente `desktop-native/`. Não houve stage, stash, commit, push nem execução de testes automatizados.

## Causa observada e correção

A fixture **sintética** `parity_stream_fixture.py` enviou, enquanto `state=working`, prévias de 72 → 20 → 72 → 0 caracteres (`md=false`, `full=false`, `vivo=false`), depois `idle` 1,2 s antes de `assistant_msg`. No binário anterior, a prévia encolheu, desapareceu e voltou; `Chat::update_state` também a removia imediatamente no `idle`. O vídeo `../artifacts/parity-pane-before.mp4` e a folha `parity-pane-before-sheet.png` mostram as lacunas. Esses números foram construídos na fixture; não são medições da sessão Claude do usuário.

`Chat::update_preview` agora ignora prefixos transitórios da mesma fonte e quadros vazios. Uma fonte com `md`, `full` ou `vivo` diferente pode substituí-la. `idle` inicia uma carência de 5 s: a mensagem canônica correspondente que chegar antes limpa a prévia no mesmo update da lista; sem mensagem final, a prévia expira. Reset, troca de sessão/conexão e fim da sessão cancelam os timers pela geração. Timestamp recente sozinho não autoriza limpar uma prévia diferente, e replay inalterado de ID conhecido também não a limpa.

Para `vivo=false`, a primeira prévia aparece inteira e os acréscimos são revelados em passos de 33 ms, a pelo menos 160 caracteres/s e com atraso alvo máximo de 1,2 s. O avanço usa caracteres Unicode completos. `vivo=true`, troca de fonte, texto não incremental e preferência por movimento reduzido mostram o novo conteúdo imediatamente. O `TextViewState` recebe acréscimos por `push_str`; uma troca completa usa `set_text`. A fonte `md=false/full=false` mostra somente as últimas dez linhas enquanto está em voo; o texto completo continua no estado e aparece na mensagem final.

## Provas de interface

| Prova | Binário | Resultado |
|---|---|---|
| `parity-pane-after.mp4` / `parity-pane-after-sheet.png` | Após guard e carência, antes da revelação gradual | Os quadros 72 → 20 → 72 → 0 não reduziram nem desmontaram a prévia; `idle` manteve o texto e `assistant_msg` o substituiu. |
| `parity-no-final-after.mp4` / `parity-no-final-sheet.png` | Após guard e carência, antes da revelação gradual | Sem mensagem final, o texto permaneceu no início do `idle` e desapareceu após a carência. A geometria da janela foi conferida antes e depois. |
| `parity-blocks-after.mp4` / `parity-blocks-after-sheet.png` | Rodada 1, antes do ajuste de correlação da rodada 2 | Blocos `md=true/full=true/vivo=false` cresceram entre quadros; acentos e `✨` ficaram íntegros. O trecho `vivo=true` apareceu de uma vez. A mensagem final substituiu a prévia sem duplicação. |
| `parity-blocks-no-final.mp4` / `parity-blocks-no-final-sheet.png` | Rodada 1, antes do ajuste de correlação da rodada 2 | A prévia com revelação gradual permaneceu após `idle` e expirou sem mensagem final. Geometria estável em toda a gravação. |
| `parity-real-scroll-hold-working.png`, `parity-real-scroll-hold-final.png`, `parity-real-scroll-return.png` | Antes da revelação gradual | Durante um segundo turno da `cx-*` própria, a leitura acima permaneceu na mesma posição; “Ir para o fim” mostrou a nova resposta. A lista/rolagem não foi modificada depois dessas capturas. |
| `parity-codex-real-final.mp4` / `parity-codex-real-final-sheet.png` | Rodada 1, antes do ajuste de correlação da rodada 2 | Codex real da conta pessoal cresceu em vários instantes e fechou sem duplicação; o modelo produziu **um** `assistant_msg` com preâmbulo e lista. |
| `parity-codex-two-late-final.mp4` / `parity-codex-two-late-final-sheet.png` | **Binário final da rodada 2**, fixture sintética | Duas mensagens: prévia B começou antes da canônica A (`ts=1001`, histórico `ts=1000`). B permaneceu visível durante 1 s sem novo delta, A virou mensagem canônica acima e B foi substituída por sua própria mensagem final. |
| `parity-codex-r2-real.mp4` / `parity-codex-r2-real-sheet.png` | **Binário final da rodada 2**, Codex real | A resposta curta cresceu sem recuar, o quadro vazio não desmontou a prévia e a mensagem final apareceu uma vez. O modelo voltou a produzir **um** `assistant_msg`; a ordem de duas mensagens foi provada somente pela fixture. |

Na `cx-native-parity-stream` real, criada só para esta prova, o processo confirmou `CLAUDE_CONFIG_DIR=/home/jefferson/.claude-claude-200-5`, `--model opus` e `--effort high`. O SSE trouxe **27 prévias não vazias** de 304 até 3001 caracteres em 14,52 s: 26 intervalos com mediana 0,60 s (mínimo 0,30; máximo 0,76). Todas tinham `md=true/full=true/vivo=false`, portanto vieram do sidecar/hook, não do pane instável `md=false/full=false`. O quadro vazio chegou 0,17 s antes do `assistant_msg` de 3097 caracteres; depois veio `idle`. A UI mostrou crescimento e a resposta canônica em `parity-real-claude-final.png`. `parity-real-claude.mp4` vale para o crescimento inicial: outra janela desapareceu e o Hyprland mudou a geometria desta durante a gravação, então o trecho posterior do vídeo não valida o enquadramento. A captura final foi feita na nova geometria. Esta prova real antecede a revelação gradual, que foi conferida na fixture final; não atribuí a ela a sequência sintética de prefixo/vazio.

### Codex (`vivo=true`)

A `cx-native-parity-codex` era terminal, como a sessão `hangar` do relato, mas descartável e sem acesso à conversa do usuário. A criação confirmou `codex_home=/home/jefferson/.codex-jefferson-felizardo`; o processo tinha `--model gpt-6-sol --effort high`. No primeiro turno real, o SSE entregou **476 prévias não vazias** de 3 a 2008 caracteres em 5,431 s, duas vazias (uma inicial e outra 0,077 s antes da mensagem canônica), sempre `md=true/full=true/vivo=true`. Nenhuma prévia não vazia encolheu. O modelo fez um único `assistant_msg` de 2008 caracteres, com preâmbulo e lista no mesmo bloco. Os comprimentos/tempos, sem texto nem token, estão em `../artifacts/parity-codex-sse-metadata.json`; a folha da janela mostra mais de seis instantes de crescimento enquanto trabalhava e a troca final. Não reconstruí o binário anterior para comparar este turno real.

O ajuste da rodada 2 retirou a inferência por `ts`: a canônica de A não pode apagar a prévia diferente de B, mesmo com relógio mais recente que o histórico. A fixture `codex-two` enviou B, depois A com `ts=1001` sobre histórico `ts=1000`, aguardou 1 s sem novo delta e terminou B. A folha final mostra B durante a pausa. A regressão de replay com mesmo ID e texto inalterado foi **compilada**, não executada.

No turno real curto do **binário final**, houve **118 prévias não vazias** de 1 a 490 caracteres em 3,246 s, duas vazias (inicial e 0,069 s antes de `assistant_msg`), sempre `md=true/full=true/vivo=true`, sem recuo de comprimento. A mensagem canônica de 490 caracteres veio uma vez; `idle` chegou 0,058 s depois. Dados em `../artifacts/parity-codex-r2-sse-metadata.json`. O modelo fez só um `assistant_msg`; a fixture separa o caso de dois blocos. A `cx-*` foi encerrada após a prova.

## Compilação e estado operacional

- `CARGO_BUILD_JOBS=2 cargo build --manifest-path desktop-native/Cargo.toml --locked`: `Finished dev profile [unoptimized] target(s) in 5.28s` no binário final. Restaram quatro avisos de campos DTO ainda não usados.
- `CARGO_BUILD_JOBS=2 cargo test --manifest-path desktop-native/Cargo.toml --locked --no-run`: `Finished test profile [unoptimized] target(s) in 2.17s`; casos compilados, **não executados**.
- As duas `cx-*` próprias foram encerradas; `hangar-send --list` não as mostra mais. A fixture foi parada. A janela final está conectada ao backend real em `../artifacts/parity-demo-final.png`.
- Unidade independente: `hangar-native-parity-demo-20260923.service`, `ActiveState=active`, PID `4190872`, cgroup `/user.slice/user-1000.slice/user@1000.service/app.slice/hangar-native-parity-demo-20260923.service`. Reiniciei apenas esta unidade depois do último build. O token foi lido da configuração existente do backend e digitado na janela via stdin; não foi copiado para arquivos desta entrega, argv, ambiente da unidade ou logs.
- `git status --short --branch`: `HEAD (no branch)`, `messages/{pt,en}.json` modificados da fase 1 e `desktop-native/` não rastreado. Índice vazio (`git diff --cached --stat` sem saída).

Hashes SHA-256 da entrega final: `app.rs` `0916232f1e186ca16a176aa568753b68c823f719e6e5b237a9cd74feab92c7cc`; `chat.rs` `9e085912278772e6020af76402b42141fe9114f4777be3b3a811f387fe1ececf`; `parity_stream_fixture.py` `97eca110405eb4200387a4a7a21740dba18575e8f32e48dc76d6b882c9d53f71`.

Limite conhecido: se o texto canônico não puder ser associado à prévia, a limpeza conservadora pode levar até 5 s; esse atraso evita apagar um bloco novo por engano. O teste real da fonte pane instável não ocorreu; seu comportamento de regressão foi reproduzido apenas pela fixture sintética. A sessão `hangar` do usuário não recebeu nenhum prompt de teste.
