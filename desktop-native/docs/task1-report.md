# Task 1: janela nativa

Modelo desta sessão: `gpt-6-sol`, esforço `high` (confirmado no `turn_context` do rollout). Base: `55e0d909`, HEAD destacado. O pacote inicial e as traduções `native_*` foram transferidos pelo Astra; esta sessão corrigiu chamadas da API GPUI, definiu fundo opaco fora do Linux, documentou o pacote e conferiu a janela.

## Resultado

- **Step 1 concluído:** `CARGO_BUILD_JOBS=2 cargo build --manifest-path desktop-native/Cargo.toml --locked` produziu `desktop-native/target/debug/hangar-native` no Linux x86_64. Última linha: `Finished dev profile [unoptimized] target(s) in 3.84s`. Restaram cinco avisos de campos DTO ainda não usados pelo código inicial; não houve erro. `Cargo.lock` foi preservado.
- **Step 2 concluído no Linux:** janela GPUI com `app_id=com.hangar.native` e título `Hangar Native — Experimental` abriu com lista à esquerda, chat à direita e diálogo de conexão. Digitação de `ação` no endereço, máscara do token, Tab entre os campos, botão Conectar e erro para esquema `ftp` foram conferidos na interface. O endereço foi restaurado e a janela foi reaberta limpa.
- **Fechamento isolado:** fechar a primeira janela encerrou somente o processo 2837193 com código 0. O serviço `hangar-backend.service` permaneceu `ActiveState=active`, `MainPID=2771049` antes e depois. A janela final está aberta no PID **2870401**, workspace 9.
- **Step 3 pendente:** não houve build nem abertura em Windows/macOS. O fallback opaco nesses sistemas está no código, mas não foi conferido visualmente.

Ambiente observado: CachyOS/Hyprland 0.56.2 em Wayland, Intel Iris Xe (Alder Lake-UP3 GT2), `x86_64-unknown-linux-gnu`, Rust 1.98.1. Sem conexão autenticada ao backend nesta Task; lista real, histórico, envio, streaming e interrupção pertencem às Tasks seguintes. Nenhuma suíte automatizada foi executada, conforme o plano aprovado.

Capturas locais (ignoradas pelo Git): `desktop-native/artifacts/task1-start.png` (janela ampla), `task1-input.png` (acentos), `task1-invalid-url.png` (erro), `task1-final.png` (janela final). O navegador embutido do Hangar não valida GPUI; as capturas vieram de `grim` sobre a janela identificada pelo Hyprland.

## Receita Wayland/Hyprland desta máquina

O shell desta sessão tinha somente estas variáveis de display relevantes (nenhuma credencial):

```text
XDG_SESSION_TYPE=wayland
XDG_RUNTIME_DIR=/run/user/1000
WAYLAND_DISPLAY=wayland-1
DISPLAY=:1
DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
HYPRLAND_INSTANCE_SIGNATURE=efb50993780079460b0cbed1363e2166a2de1d9f_1789995766_638740967
```

Em outro shell da mesma sessão gráfica, exporte essas variáveis antes de chamar `hyprctl`, `grim` ou o binário. A assinatura do Hyprland muda quando o compositor reinicia; nesse caso, leia a assinatura do ambiente de um processo que já roda nessa sessão gráfica. O binário foi iniciado em primeiro plano num terminal persistente, a partir da raiz da worktree:

```bash
desktop-native/target/debug/hangar-native
```

`hyprctl -j clients` identificou `class=com.hangar.native`, PID 2870401, workspace 9, posição `[1537,1506]` e tamanho `[949,474]` na janela final. A captura final foi feita com `grim -g '1537,1506 949x474' desktop-native/artifacts/task1-final.png`. Na primeira abertura, a janela ocupava `[578,1022]` com tamanho `[1908,958]`; essa geometria produziu `task1-start.png`. Releia `clients` antes de capturar porque o gerenciador de janelas pode redimensioná-la.

## Inventário congelado

Todos os caminhos abaixo pertencem a esta Task. `target/` e `artifacts/` estão ignorados. Nenhum arquivo foi stageado, guardado em stash, commitado ou enviado.

```text
be9078a32af747dd914ec0e2c733b28a23629c20b7b804f24019f8a0f86c42a6  desktop-native/.gitignore
a99f19ddceba580b9d0ed8fb954e26d295098ea6b072edbc68f2b28660d43b80  desktop-native/Cargo.lock
c7c5c7b31c0de5362e154ca7e951dee28247f7c3b4c4bcc05ddf5da6930fc6ee  desktop-native/Cargo.toml
422f0cd5e3414bc0d47b818308bff006e90c2dd72d661077425d7c8fa6f790ab  desktop-native/README.md
72da9f9e503464ba7393560f6f51a69fb06be72633d52ae7725fa0ba59dfda22  desktop-native/rust-toolchain.toml
fb6eaff25bd0d6ed9c5cee3a3cc45090a5162ab4ccdc693f530cf6af1125571e  desktop-native/src/api/dto.rs
d08b4acdde3bf47dd1512a9d411ffd9062bb48a368620713ccc524e6c494cbb2  desktop-native/src/api/mod.rs
279d78215454a21b739091622e64966dfac86eb34720856a061544235658150d  desktop-native/src/api/sse.rs
233153e5afc2bf73f4ae92eb12c968aba2d3b50fe5c24ec313fa1749456f6796  desktop-native/src/app.rs
2f51d2c0fc71d8fe9ca3e4435085f737767cf626e32bf54233aa6810720c17db  desktop-native/src/chat.rs
0fa7dc4af4a6e4fd6d13b56e1cf9b0e9a1ed0215e0b3c632f7e43d5591881931  desktop-native/src/i18n.rs
90aa5d81b49debdb42773729cca569ac1c832b2c281de5d5f1cecdf78edaa8bf  desktop-native/src/main.rs
9c1a309c1879b82886a273461ed093dc6c2f4648962af49e86155d53f199faa9  desktop-native/src/theme.rs
9be2fbad167521fe0ce8c661a18c68f943fcd972e10cec6046d0c215aacd6938  messages/pt.json
31ee2458d73630569071d0eab17702d56eb4271016962e4a6beee985cc1633a3  messages/en.json
```

O plano principal pertence ao árbitro; ele pode marcar Steps 1 e 2 após ler esta evidência. Step 3 permanece aberto.
