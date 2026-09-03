---
name: hangar-preview
description: |
  Use quando a sessão tem um navegador embutido aberto no app Hangar (o painel "Navegador" ao lado
  do chat, só no app desktop Electron) e a tarefa pede pra VER, ler, clicar, testar ou tirar
  screenshot da página que está nele — "vê o que tem na minha página", "testa o fluxo no preview",
  "clica no botão lá", "tira um print do preview", "lê o console da página". Cada sessão tem o SEU
  navegador; esta skill é o jeito de dirigir o da sua (ou, com --sessao, o de outra sessão quando o
  usuário pedir). NÃO use para: sites fora do Hangar (isso é o agent-browser), o túnel de porta do
  celular (PreviewSheet), ou abrir navegador pra uma sessão que não tem — o painel é aberto pelo
  usuário, nunca pelo agente.
allowed-tools: Bash(hangar-preview:*)
---

# hangar-preview — dirigir o navegador embutido da sessão

O navegador embutido é um Chromium de verdade (view nativo do Electron), um por sessão, dirigível
por CDP. Este CLI resolve sozinho QUAL é o da sua sessão (pelo nome da sessão tmux) e fala com o
target certo — duas sessões com a mesma URL aberta não se confundem.

## Comandos

- `hangar-preview open <url>` — abre o navegador desta sessão com a url: o painel monta sozinho na
  tela do usuário (se a sessão estiver fora da tela, abre quando ele abrir ela). **Avise o usuário**
  no texto da resposta que você abriu — a janela dele muda na hora.
- `hangar-preview url` — URL atual da página.
- `hangar-preview shot [arq.png]` — screenshot; default `/tmp/hangar-preview-<sessao>.png`. Leia o
  PNG com a ferramenta de leitura de imagem.
- `hangar-preview eval '<js>'` — roda JS na página e imprime o resultado. Clicar, ler DOM e navegar
  é tudo eval:
  - clicar: `eval 'document.querySelector(".btn-entrar").click()'`
  - ler texto: `eval 'document.querySelector("h1")?.textContent'`
  - navegar: `eval 'location.href = "http://localhost:3000/login"'`
  - console da página: não existe canal de console; use `eval` pra ler estado ou `window.onerror`.
- `hangar-preview list` — quais sessões têm navegador vivo agora.
- `--sessao <nome>` opera o navegador de OUTRA sessão — só quando o usuário pedir, e avise-o.

## Regras

- O view continua vivo quando o usuário troca de sessão no app — você pode seguir trabalhando
  nele via CLI em background, sem atrapalhar a tela dele.
- Não fique abrindo e fechando navegador em loop nem trocando a url a cada passo: cada open pisca
  na tela do usuário. Abra uma vez, trabalhe com `eval`/`shot`.
- CDP cru, se precisar de mais (teclado, rede, emulação): http://127.0.0.1:9223 — cada navegador é
  um target; o sidecar da sua sessão está em `~/.hangar/nav/`.
