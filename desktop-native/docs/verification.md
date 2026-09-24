# Verificação do Hangar Native

Esta entrega foi conferida em Linux CachyOS com Hyprland/Wayland, no HEAD destacado `55e0d909`, contra o backend Hangar existente. O cliente é experimental. A captura final da janela conectada diretamente ao backend é [`../artifacts/task5-final.png`](../artifacts/task5-final.png). A janela GPUI foi conferida por interação real e captura do desktop com `grim`; `hangar-preview` não inspeciona essa janela.

## Estado por plataforma

| Plataforma | Build | Janela e conversa | Limite |
| --- | --- | --- | --- |
| Linux | `cargo build --manifest-path desktop-native/Cargo.toml --locked` passou | Conexão, lista, histórico, streaming, envio, interrupção, rolagem, seleção/cópia e retorno à sessão conferidos | `delivered:false` e composição IME não foram exercitados na UI |
| Windows | Não executado | Não conferido | Falta máquina/toolchain e prova manual |
| macOS | Não executado | Não conferido | Falta máquina/toolchain e prova manual |

**Step 12 permanece pendente** porque o fluxo não foi repetido nos três sistemas. Os cenários Linux anteriores de token inválido e servidor ausente, mais os cenários sintéticos de erro/429/reset, estão documentados nos relatórios das Tasks 2–4; não foram repetidos sem risco novo. Nenhum teste automatizado foi executado nesta Task.

## Provas Linux desta etapa

- O diálogo recebeu URL/token do backend real; a lista apareceu em `task5-final-list.png`. A sessão descartável própria `cx-native-task4-jf` tinha `provider=codex`, `headless=true`, `codex_home=/home/jefferson/.codex-jefferson-felizardo`; o `turn_context` do primeiro turno registrou `gpt-6-sol/high`.
- Pedido simples enviado pela UI e resposta visível em `task5-simple-reply.png`. Rolagem para cima, seleção e cópia estão em `task5-scrolled-up.png` e `task5-selection.png`; `wl-paste` coincidiu exatamente com o trecho copiado. O retorno ao fim e a troca de sessão somente para leitura estão em `task5-latest.png` e `task5-switch-back.png`, sem mistura de texto.
- A resposta longa foi interrompida pela UI enquanto o estado era `working`: `task5-stream-before-stop.png`, `task5-after-stop.png` e `task5-after-stop-settled.png` mostram a transição para `idle` e a remoção da prévia. O pedido seguinte respondeu “after interruption.” em `task5-continued.png`. Nenhuma sessão alheia recebeu entrada ou interrupção.
- A conexão SSE com cursor foi conferida **no backend real**, por proxy efêmero em loopback de destino fixo, usado somente para fechar o socket SSE da cx. A primeira conexão não tinha cursor; após a queda, a reconexão enviou `Last-Event-ID: 01a0d000-f6ac-74e3-b19b-98d42af1a712:186885`. Duas mensagens marcadoras, uma antes e outra durante o intervalo, apareceram uma vez cada na UI (`task5-proxy-before-drop.png`, `task5-proxy-reconnected.png`); o histórico do backend continha um `user_msg` de cada. O proxy não registrou token, cabeçalho Authorization nem corpos em disco, foi parado, e a janela final usa o backend direto. Esta é prova real de transporte, distinta das fixtures sintéticas das Tasks 3–4.
- A recriação com o **mesmo nome** usou exclusivamente a cx criada por este trabalho, depois de confirmar conta, cwd, estado `idle` e ausência de fila pendente. O `jsonl` antigo era `rollout-2026-09-23T17-41-59-01a0d000-f6ac-74e3-b19b-98d42af1a712.jsonl`; a nova sessão recebeu `rollout-2026-09-23T19-41-50-01a0d06e-afee-74f3-9255-07ed0d517e8e.jsonl`. A UI descartou a seleção antiga (`task5-cx-recreated.png`) e mostrou a nova conversa vazia (`task5-cx-new-empty.png`); `GET /history?limit=400` retornou `[]`. A cx nova foi encerrada ao terminar a prova; o transcript antigo permaneceu no disco. Nenhuma sessão existente do usuário foi recriada.
- O Root da janela Linux recebeu fundo transparente após o último build de desenvolvimento. Sobre uma janela temporária própria, mudei apenas a cor do fundo de vermelho `#a22d43` para azul `#2460ae`. A mesma região vazia do chat mudou do pixel `srgb(27,17,21)` para `srgb(18,20,30)`, com texto legível (`task5-alpha-red.png`, `task5-alpha-blue.png`). A janela colorida temporária foi fechada. As dependências diretas foram fixadas nas versões já presentes no lockfile, sem atualização; o SHA-256 de `Cargo.lock` ficou `a99f19ddceba580b9d0ed8fb954e26d295098ea6b072edbc68f2b28660d43b80`.

As capturas citadas acima estão em [`../artifacts/`](../artifacts/). As capturas da conversa foram feitas no binário de desenvolvimento compilado após a correção do Root e dos pins, antes de alterar somente estes documentos. O binário da janela final é o mesmo. Não houve mudança posterior no código de produção.

## Comparação de recursos

**Step 13 permanece pendente.** O build de release foi iniciado, mas interrompido antes de terminar para encerrar a entrega sem prolongar uma medição que não teria comparador seguro. O único build concluído é o de desenvolvimento indicado acima. Não há três execuções equivalentes dos dois clientes, portanto não há percentual de ganho. Um segundo Electron com apenas `--user-data-dir` separado compartilha CDP na porta 9223, `~/.hangar/nav` e `_srv.json` com o Electron vivo do usuário; a comparação exige isolamento completo desses recursos. Nenhuma segunda instância foi aberta e o Electron do usuário não foi alterado. O backend e os agentes não entram em qualquer futuro somatório de memória dos clientes; no Linux, a medida comparável será PSS, com conversa/carga e cache equivalentes.

## Janela disponível para conferência

A janela final roda na unidade transitória de usuário `hangar-native-demo-20260923.service`, iniciada com `systemd-run --user --unit=hangar-native-demo-20260923 --collect` e o binário `desktop-native/target/debug/hangar-native`. No fechamento desta verificação: PID `3531731`, cgroup `/user.slice/user-1000.slice/user@1000.service/app.slice/hangar-native-demo-20260923.service`, fora do scope dos agentes. A unidade estava `active`, a janela `com.hangar.native` visível e conectada diretamente ao backend real (`task5-final.png`). O token foi digitado na janela e não está na linha de comando, nas variáveis da unidade ou neste documento. Para consultar: `systemctl --user show hangar-native-demo-20260923.service -p ActiveState -p MainPID -p ControlGroup`.
