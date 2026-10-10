# Hangar Native

Cliente desktop em Rust/GPUI para o backend Hangar **já em execução**. É a janela padrão do Hangar: o `install.sh`/`install.ps1` o baixam da release `native-latest` no pacote da máquina (`scripts/install-native.sh` ou `scripts/install-native.ps1`, que também servem sozinhos) e ele se atualiza pelo botão do topo ou pela página Sobre. No Windows e no Linux o nativo atende o `hangar-preview` (um navegador por sessão, sem abas) e a tela remota do navegador no celular, e a atualização remove o Electron dessas máquinas. No Linux o navegador é um Chromium sem janela (o Chrome ou o Chromium do sistema, ou o `chrome-headless-shell` que `scripts/install-chromium.sh` baixa). No macOS o navegador embutido do `hangar-preview` e a tela remota ainda moram no Electron, que fica onde já estava instalado como "Hangar (Electron)".

## Compilar e abrir

```bash
cargo +1.98.1 run --manifest-path desktop-native/Cargo.toml --locked
```

Execute o comando na raiz desta worktree. O seletor `+1.98.1` mantém a versão do Rust ao executar fora de `desktop-native/`; se ela ainda não estiver instalada, use `rustup toolchain install 1.98.1`. Para uma compilação otimizada, acrescente `--release` depois de `--locked`. A janela pede a URL HTTP(S) do backend existente e o token de acesso; o token fica apenas na memória do processo, sem ser salvo em disco. Para a interface em inglês, use `HANGAR_NATIVE_LANG=en` no ambiente do processo.

Em desenvolvimento, preserve o `target-dir` configurado no `~/.cargo/config.toml` da máquina; não abra um cache vazio por sessão. No Linux, builds de desenvolvimento rodam com `nice -n 19 ionice -c3` antes do comando Cargo. O script `tools/build-otimizado.sh` compila com o perfil `dist` (o da main publicada), que sai em `dist/` dentro do `target-dir` em uso (o que o `cargo metadata` informa), à parte do release comum.

### Assistente de instalação sem instalar

O assistente (Linux e Windows) roda os scripts do repositório. Para ver as telas sem instalar nada nesta máquina:

- `HANGAR_SETUP_BOOTSTRAP=desktop-native/tools/setup-fake-bootstrap.sh` usa o dublê no lugar do bootstrap baixado e pula a cópia do app; `FAKE_SCENARIO=ok|falha|tailscale|protocolo|askpass|interrompe` escolhe o caminho.
- `HANGAR_SETUP_DEST=<pasta>` troca a pasta da instalação (nada é escrito nela pelo dublê).
- `HANGAR_SETUP_DEMO=1` abre o assistente ao iniciar; `HANGAR_SETUP_DEMO=start` também aperta "Começar instalação" quando a conferência passa (`start-fora` escolhe "também fora de casa" antes).
- `XDG_CONFIG_HOME=<pasta>` isola a configuração e a instância única do app instalado.
- `FAKE_SCENARIO=erro FAKE_CODE=<código>` falha na etapa instalar com o código (`nenhum` = falha não prevista sem código); `FAKE_SCENARIO=pendencia FAKE_CODE=<código>` termina com a pendência; `FAKE_LEAK=1` imprime token, caminho, IP e nome `.ts.net` para provar a limpeza do relatório; com `FAKE_FIXED=<arquivo>`, o cenário `erro` passa quando o arquivo existe.
- `HANGAR_SETUP_FAKE_CLAUDE=desktop-native/tools/setup-fake-agent.sh` (e `HANGAR_SETUP_FAKE_CODEX`) troca o agente pelo dublê; `FAKE_AGENT_MODE=fora|dentro|trava`, `FAKE_AGENT_PROMPT=<arquivo>` guarda o prompt recebido. `HANGAR_SETUP_DEMO_AGENT=1` pede ajuda ao primeiro agente assim que o relatório fica pronto.
- `HANGAR_REPORT_URL=<url>` manda o relatório para outro endereço (um receptor local) em vez do Worker.

## Ambiente

- Rust 1.98.1, fixado em `rust-toolchain.toml`. `Cargo.lock` fixa as versões usadas nesta worktree.
- Linux: sessão Wayland ou X11 e bibliotecas de desenvolvimento de xkbcommon, fontconfig, freetype, Vulkan e ALSA. Compilação e uso real conferidos em CachyOS/Hyprland/Wayland.
- Windows: Rust com alvo MSVC, Visual Studio C++ Build Tools e Windows SDK para Win32/DirectWrite. A compilação e a janela **não foram conferidas** no Windows.
- macOS: Xcode e Command Line Tools para Metal. A compilação e a janela **não foram conferidas** no macOS.

No Linux, o fundo do chat usa transparência; a prova com dois fundos coloridos está em [verification.md](docs/verification.md). Windows e macOS mantêm fundo opaco até sua conferência visual. O estado exato das verificações e os limites desta entrega estão no mesmo documento.

O ícone do aplicativo vem de `assets/brand/icon.png`; o executável Windows incorpora
`assets/brand/icon.ico`. No Linux, rode `tools/install-linux.sh` após o build: ele instala
o binário, o PNG e `com.hangar.native.desktop`, nome que combina com o `app_id` da janela.
No macOS, `tools/package-macos.sh <binário> <Hangar.app>` monta o pacote com `icon.icns`.

## Baixar pronto e atualização

Cada push na `main` que mexe no app recompila e publica na release fixa [`native-latest`](https://github.com/jeffer1312/hangar/releases/tag/native-latest) (workflow `.github/workflows/native.yml`): `Hangar-linux-x86_64.tar.gz`, `Hangar-linux-x86_64.deb`, `Hangar-linux-x86_64.rpm`, `Hangar-windows-x86_64.zip` e `Hangar-macos-aarch64.zip`, cada um com o `.sha256` ao lado (`sha256sum -c`). O arquivo Linux contém `hangar`, `icon.png` e `install-linux.sh` (rode este último para instalar o atalho). O .deb e o .rpm (tools/package-linux.sh) instalam o binário em /usr/bin/hangar-native, o ícone e o atalho; o assistente de instalação copia o app para ~/.local/bin, onde a autoatualização consegue trocá-lo. O ZIP Windows contém `Hangar.exe`; o ZIP macOS contém `Hangar.app` com o ícone. Os binários crus continuam publicados à parte para o atualizador. Não são assinados: no macOS, abra pelo botão direito na primeira vez. Se Windows ou macOS não compilar, a release preserva o download anterior dessa plataforma e indica isso nas notas; somente os arquivos do `native-latest.json` pertencem à versão atual.

A versão é a do backend: `VERSION` da raiz + número de commits (`0.1.0.2533`), embutida pelo `build.rs` e mostrada em Configurações → Sobre. Ao abrir e a cada 6 h o app lê o `native-latest.json` da release; havendo versão maior, aparece **Atualizar** na barra do topo, ao lado da engrenagem. O clique baixa o binário da plataforma (`Hangar-<sistema>-<arquitetura>`), confere o sha256 do manifesto (diferente: recusa sem trocar nada), guarda o atual em `<executável>.old`, põe o novo no lugar e o abre. O novo prova que subiu gravando o próprio pid; se não fizer isso em 30 s ou morrer antes, o anterior volta para o lugar e a janela antiga continua aberta com o aviso. Build local de uma branch com mais commits que a `main` não recebe oferta.

Para provar sem o GitHub: `HANGAR_NATIVE_UPDATE_URL=http://127.0.0.1:<porta>` aponta para uma release falsa, servida por `tools/release_update_fixture.py`.
