# Hangar Native (experimental)

Cliente desktop em Rust/GPUI para o backend Hangar **já em execução**. Mostra a lista de sessões, histórico e conversa ao vivo; permite enviar texto e interromper a geração na sessão escolhida. Não substitui o aplicativo Electron padrão. Perguntas e aprovações pendentes ainda são respondidas no Electron.

## Compilar e abrir

```bash
cargo +1.98.1 run --manifest-path desktop-native/Cargo.toml --locked
```

Execute o comando na raiz desta worktree. O seletor `+1.98.1` mantém a versão do Rust ao executar fora de `desktop-native/`; se ela ainda não estiver instalada, use `rustup toolchain install 1.98.1`. Para uma compilação otimizada, acrescente `--release` depois de `--locked`. A janela pede a URL HTTP(S) do backend existente e o token de acesso; o token fica apenas na memória do processo, sem ser salvo em disco. Para a interface em inglês, use `HANGAR_NATIVE_LANG=en` no ambiente do processo.

## Ambiente

- Rust 1.98.1, fixado em `rust-toolchain.toml`. `Cargo.lock` fixa as versões usadas nesta worktree.
- Linux: sessão Wayland ou X11 e bibliotecas de desenvolvimento de xkbcommon, fontconfig, freetype, Vulkan e ALSA. Compilação e uso real conferidos em CachyOS/Hyprland/Wayland.
- Windows: Rust com alvo MSVC, Visual Studio C++ Build Tools e Windows SDK para Win32/DirectWrite. A compilação e a janela **não foram conferidas** no Windows.
- macOS: Xcode e Command Line Tools para Metal. A compilação e a janela **não foram conferidas** no macOS.

No Linux, o fundo do chat usa transparência; a prova com dois fundos coloridos está em [verification.md](docs/verification.md). Windows e macOS mantêm fundo opaco até sua conferência visual. O estado exato das verificações e os limites desta entrega estão no mesmo documento.
