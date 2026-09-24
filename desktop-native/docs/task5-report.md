# Task 5: conferência final Linux e entrega

HEAD destacado `55e0d909`; executor `gpt-6-sol/high`, `CODEX_HOME=/home/jefferson/.codex-jefferson-felizardo`. Fontes congeladas para revisão, sem stage, stash, commit ou push. `desktop-native/README.md` documenta como executar; [`verification.md`](verification.md) contém a matriz completa, método e limites das provas. A janela final conectada está em [`../artifacts/task5-final.png`](../artifacts/task5-final.png).

## Resultado conferido

- O build de desenvolvimento com `CARGO_BUILD_JOBS=2 cargo build --manifest-path desktop-native/Cargo.toml --locked` passou após a correção do Root transparente no Linux e a fixação das dependências diretas às versões já resolvidas. `Cargo.lock` não mudou. Não houve alteração de código de produção depois desse build nem depois das capturas Task 5.
- No backend **real**, a cx própria `cx-native-task4-jf` confirmou envio, streaming, histórico, rolagem, seleção/cópia, volta ao fim, troca de sessão sem mistura, interrupção e continuação. A prova do cursor SSE real usou apenas um proxy efêmero que fechou o socket da cx; a reconexão levou `Last-Event-ID` e as duas mensagens marcadoras apareceram uma vez cada. O proxy foi encerrado e a janela final voltou a falar diretamente com o backend. As fixtures sintéticas das Tasks anteriores estão separadas em seus relatórios.
- A cx, criada por este trabalho com `gpt-6-sol/high` na conta `jefferson-felizardo`, foi encerrada e recriada com **o mesmo nome** somente depois de confirmar identidade e estado `idle`. O `jsonl` novo era diferente; a UI retirou a conversa antiga e mostrou a nova vazia. A cx recriada foi encerrada após a prova. O transcript antigo permaneceu em disco. Nenhuma sessão alheia recebeu entrada, interrupção ou encerramento.
- A transparência foi provada com fundo temporário próprio vermelho e azul, refletidos na região vazia do chat. O fundo temporário foi encerrado. `Step 2` e `Step 6` estão marcados no plano principal com estas evidências.

## Estado da entrega

`Step 12` permanece aberto: o fluxo foi conferido em Linux, mas não em Windows/macOS; `delivered:false` e composição IME ainda não foram exercitados na UI. `Step 13` permanece aberto: o build de release foi iniciado e interrompido antes de terminar, e não houve três execuções equivalentes dos clientes nem isolamento completo para um segundo Electron, cujo estado compartilhado poderia afetar o aplicativo vivo do usuário. Não há estimativa de ganho. Nenhum teste automatizado foi executado nesta Task. As regressões previstas das Tasks anteriores foram apenas compiladas com `cargo test --no-run`, conforme os relatórios correspondentes.

`Step 14` entrega comando, captura e limites. A janela fica acessível em unidade **transitória** do systemd do usuário, fora do cgroup dos agentes, sem serviço permanente e sem token no ambiente/argv:

```text
unit: hangar-native-demo-20260923.service
MainPID: 3531731
ControlGroup: /user.slice/user-1000.slice/user@1000.service/app.slice/hangar-native-demo-20260923.service
FragmentPath: /run/user/1000/systemd/transient/hangar-native-demo-20260923.service
window class: com.hangar.native
```

Comando de uso na raiz da worktree: `cargo +1.98.1 run --manifest-path desktop-native/Cargo.toml --locked`; o seletor explícito mantém a versão ao executar fora de `desktop-native/`. A janela pede URL/token do backend existente. Consulta da demonstração: `systemctl --user show hangar-native-demo-20260923.service -p ActiveState -p MainPID -p ControlGroup`. O aplicativo Electron padrão e o backend existente não foram alterados.

## SHA-256 da rodada

```text
cc6ed88184b2f21252d3de30c3dd9aa7f17b798499450ff07ba21683155e3a2a  desktop-native/Cargo.toml
a99f19ddceba580b9d0ed8fb954e26d295098ea6b072edbc68f2b28660d43b80  desktop-native/Cargo.lock
9ed1df2bef7d5b9bf84fed19075bce78dd30fa5c9d379d2fd9d372828832be6e  desktop-native/src/app.rs
72ce358e846ba63f002edb8fcfcbc7a65d3f08738a008f8d8f1450d9f30d6948  desktop-native/src/chat.rs
6c9d269791a6342b76df4ebf8c4811f8f56bcdd4f7aeb8d2d092aefb13fab181  desktop-native/src/delivery.rs
0fa7dc4af4a6e4fd6d13b56e1cf9b0e9a1ed0215e0b3c632f7e43d5591881931  desktop-native/src/i18n.rs
3dabf501d990e605681391e71f25d9cd8042fe829372095a61e1ea9509d99734  desktop-native/src/main.rs
9c1a309c1879b82886a273461ed093dc6c2f4648962af49e86155d53f199faa9  desktop-native/src/theme.rs
984fbedd925b73d7182e5b87fe6e6c3f0dee478946d90f1d2d88440f9218b9e0  desktop-native/src/api/dto.rs
c259ab5e88e911acee07b45e2f9fba6ccadd55eaf8a2efe80c290a955e208672  desktop-native/src/api/mod.rs
c74df78a8a24d5642c165498ea4cc414c9176982de0aae45419139b8774827ce  desktop-native/src/api/sse.rs
e76b07b4cd781db33ecac0ae1786ec0000155bab0882098f1aa42b6b485c148c  messages/pt.json
08ae870bcfa49c5487270330116d950b26526649c48f2ddf9ad5882574a74139  messages/en.json
741afd8628e22551fd3a58a774b3a6e524405b3b8016f6decc1fd65cce06f497  desktop-native/artifacts/task5-final.png
7d52adea4914bbe2558804a61f389aedef811acaeaa5c2de84ccdfb310ae234b  desktop-native/artifacts/task5-alpha-red.png
7f6865d1437763c81b0ae04c46bf8776d96c01e965557fb19c232973cc15e892  desktop-native/artifacts/task5-alpha-blue.png
20802dc292deee3e7962245752c141a8b31a871b45ad57553a6f5d778061cb4c  desktop-native/artifacts/task5-proxy-reconnected.png
32ab4b72c9c7d992083678f2acb030ece9569869955b2bf0d5609cc789555d37  desktop-native/artifacts/task5-cx-new-empty.png
```

Hashes do README, `verification.md` e do próprio relatório são enviados no recado de entrega após o congelamento dos documentos.
