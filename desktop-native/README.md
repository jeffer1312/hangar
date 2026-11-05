# Hangar Native

Cliente desktop em Rust/GPUI para o backend Hangar **já em execução**. É a janela padrão do Hangar: o `install.sh`/`install.ps1` o baixam da release `native-latest` no pacote da máquina (`scripts/install-native.sh` ou `scripts/install-native.ps1`, que também servem sozinhos) e ele se atualiza pelo botão do topo ou pela página Sobre. No Windows e no Linux o nativo atende o `hangar-preview` (um navegador por sessão, sem abas) e a tela remota do navegador no celular, e a atualização remove o Electron dessas máquinas. No Linux o navegador é um Chromium sem janela (o Chrome ou o Chromium do sistema, ou o `chrome-headless-shell` que `scripts/install-chromium.sh` baixa). No macOS o navegador embutido do `hangar-preview` e a tela remota ainda moram no Electron, que fica onde já estava instalado como "Hangar (Electron)".

## Compilar e abrir

```bash
cargo +1.99.1 run --manifest-path desktop-native/Cargo.toml --locked
```

Execute o comando na raiz desta worktree. O seletor `+1.99.1` mantém a versão do Rust ao executar fora de `desktop-native/`; se ela ainda não estiver instalada, use `rustup toolchain install 1.99.1`. Para uma compilação otimizada, acrescente `--release` depois de `--locked`. A janela pede a URL HTTP(S) do backend existente e o token de acesso; o token fica apenas na memória do processo, sem ser salvo em disco. Para a interface em inglês, use `HANGAR_NATIVE_LANG=en` no ambiente do processo.

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

## Leitor de Markdown

O visor de arquivos oferece **Leitura**, **Fonte** e **Lado a lado**. A Fonte usa o editor existente:
rascunhos, busca, salvar, descartar, recarregar e avisos de mudança no disco continuam disponíveis.
Edições ainda não salvas também atualizam a leitura e o sumário. **A−/A+** mudam apenas o tamanho
na leitura da aba aberta, sem alterar a conversa, o editor ou o arquivo.

O sumário vem do AST do mesmo renderizador GFM, não de uma busca por linhas que começam com `#`.
Ele reconhece títulos ATX e setext, acompanha a seção visível e navega por clique ou teclado.
Pode ser ocultado; em painéis estreitos fica oculto automaticamente, sem perder a preferência.
Tabelas largas têm rolagem horizontal própria e a largura do documento permanece limitada.
Em painéis muito estreitos, o modo Lado a lado empilha as vistas, mantendo leitura e fonte acessíveis.

| Conteúdo | Comportamento |
|---|---|
| Títulos H1–H6, parágrafos e separadores | Hierarquia tipográfica e espaçamento próprios de documento |
| Negrito, itálico, tachado e código inline | Formatação do GFM, com seleção de texto |
| Listas ordenadas, não ordenadas, aninhadas e checklists | Marcadores, numeração e estado das tarefas preservados |
| Citações e tabelas | Filetes e cabeçalho legíveis, usando as cores do tema |
| Código cercado ou indentado | Indentação, realce das linguagens disponíveis e cópia do bloco |
| Links comuns, por referência e autolinks | HTTP/HTTPS abre no navegador externo; arquivos relativos ficam no visor |
| Âncoras internas | Navegação por título, incluindo acentos e sufixos de títulos repetidos |
| Imagens comuns e por referência | Caminho relativo à pasta do documento; arquivo remoto pela API autenticada da sessão |
| Imagens HTTP/HTTPS | Download sem o token do Hangar; nenhuma credencial do Hangar é adicionada à URL |

Imagens dentro da raiz da sessão não precisam ser citadas individualmente na conversa. Fora da
raiz, a política de arquivo citado continua valendo, inclusive para imagens de um documento externo.

**Mermaid:** numa resposta já assentada, o bloco ` ```mermaid ` vira o diagrama desenhado pelo servidor
da sessão (`POST /api/mermaid`), na largura da coluna e no tema da aparência; clicar abre em tela cheia
com zoom. Enquanto desenha, durante o streaming e na falha (com a mensagem do parser e, se a falha for
do servidor, "Tentar de novo"), o bloco continua como código. Copiar e buscar acham a fonte.

**Limites:** outros diagramas cercados aparecem como código legível, sem gerar um diagrama.
Fórmulas permanecem como texto ou bloco de código, sem composição matemática. O kit
aceita um subconjunto de HTML de formatação, não uma página com CSS ou scripts. HTML executável,
componentes MDX/JSX, JavaScript e motores de extensões arbitrárias não são executados. A extensão
`.mdx` permite consultar a fonte e o Markdown reconhecido, não executar componentes.

### Artefatos da conversa

O botão **Artefatos** ao lado de pasta/branch mostra o catálogo publicado pela conversa Claude
selecionada. A lista é lida ao selecionar a conversa e ao abrir ou atualizar o painel. Publicações
atualizadas da mesma identidade ocupam uma linha só; títulos vazios têm um rótulo próprio.

Cada item abre sua URL real `https://claude.ai/artifact/…` ou
`https://claude.ai/code/artifact/…` no navegador externo. O Hangar não baixa o conteúdo do artefato,
não transforma a URL em HTML local e não usa cookies do navegador. Login e permissões do Claude
continuam sendo responsabilidade desse navegador. O catálogo pertence ao dono, não ao convidado;
trocar, recriar ou limpar a conversa invalida as respostas em voo e a lista anterior.

## Ambiente

- Rust 1.99.1, fixado em `rust-toolchain.toml`. `Cargo.lock` fixa as versões usadas nesta worktree.
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
