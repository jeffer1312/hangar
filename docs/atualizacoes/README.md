# Passos de atualização

Quando uma versão exige que a máquina de quem já usa faça alguma coisa além de puxar o código —
uma dependência nova, um arquivo que precisa sair da frente, uma pasta que muda de nome —, essa
coisa vira **um arquivo aqui**, no mesmo commit que a exige.

O botão Atualizar do app lê estes arquivos, roda o que ainda não rodou naquela máquina, e nunca
pede nada a quem está usando. Quem lê o formato é `backend/app/atualizacoes.py`.

## Formato

```markdown
---
id: 2026-08-25-exemplo
titulo: Uma frase dizendo o que muda
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send
destrutivo: false
---

O texto que a pessoa lê no changelog. Uma ou duas frases, em português, sobre o que
mudou para ela — não sobre o que o comando faz.
```

| Campo | Para que serve |
|---|---|
| `id` | Chave no registro do que já rodou. Começa com a data para a ordem sair certa. Nunca mude o id de um passo já publicado — a máquina que já o rodou o reconheceria como novo. |
| `titulo` | Obrigatório. Sem ele o arquivo é ignorado (com aviso no log). |
| `comando` | Comando comum aos sistemas. Passa pelo shell da máquina, então `&&` e pipe funcionam — mas o shell é `cmd.exe` no Windows: `test`, `true` e amigos **não existem lá**. O diretório de trabalho é a raiz do repo. |
| `comando_posix` / `comando_windows` | Quando o comando precisa mudar por sistema, a variante correspondente vence `comando`. Pode declarar só uma variante; no outro sistema o passo vira apenas texto. |
| `prova` | Os caminhos que precisam existir depois. Separados por espaço (logo, caminho **com** espaço não cabe aqui); relativos à raiz do repo, ou absolutos, ou com `~`. Não é comando — é checagem de arquivo, que vale nos dois sistemas. **Sem prova, "sucesso" quer dizer só "o comando não deu erro"** — e foi assim que um `-Update` chegou a dizer ok com o processo antigo ainda no ar. |
| `destrutivo` | `true` quando o passo apaga ou sobrescreve algo. Passo destrutivo roda pelo botão, mas não roda sozinho na subida do backend. |
| `tipo` | `maquina` quando o passo muda a MÁQUINA, fora da pasta da versão. Sem o campo, o passo é de conteúdo. Ver "Passo de máquina" abaixo. |

## Duas regras

**O passo tem que poder rodar duas vezes.** Ele pode rodar de novo numa máquina que reclonou o
repo. Comando que só funciona uma vez nasce com guarda — o modelo é
`backend/app/migracao_sidecars.py`: destino já existe, para com aviso, nunca funde.

**O passo entra no registro só depois da prova passar.** Se a prova falhar, ele fica pendente e
tenta de novo na próxima — é o contrário de marcar como feito e deixar a máquina sem o efeito.

## Passo de máquina (`tipo: maquina`)

Na instalação por pacote (`~/.hangar/versions/<versao>/`), script, skill, hook e atalho chegam
dentro da versão e deixam de ser passo. Só roda o passo que muda a máquina fora da pasta da
versão: dependência do sistema, unit do systemd, tarefa agendada ou vigia, firewall, `PATH` ou
perfil do shell, chave no `.env`, migração de dados, instalação ou remoção do app nativo, remoção
do Electron, Chromium. Esse passo leva `tipo: maquina`. O que só recopia scripts, skills, o bloco
do CLAUDE.md, os links do `hangar-send` ou o registro do MCP apontando para o repo fica sem o
campo. Passo só de texto também.

**Passo de máquina tem que ser compatível com a versão anterior, porque não volta na volta.**
Voltar de versão troca só o apontador `current`; o que o passo mudou na máquina fica. A versão de
antes precisa continuar funcionando com a máquina já mudada.

Passos marcados:

| Passo | O que muda na máquina |
|---|---|
| `2026-09-28-app-nativo-padrao` | instala o app nativo |
| `2026-09-28-link-hangar-abre-o-app-nativo` | app nativo e registro do link `hangar://` |
| `2026-09-28-tmux-fechar-sem-reabrir` | `~/.tmux.conf` |
| `2026-09-29-backend-malloc-arena` | unit do systemd |
| `2026-09-29-lancador-electron-renomeado` | lançador `.desktop` do Electron |
| `2026-09-29-rede-local` | chave no `.env` |
| `2026-09-29-tarefas-windows-prioridade-normal` | tarefas agendadas do Windows |
| `2026-10-01-atualizar-windows-front-no-ar` | para o processo da tarefa do front no Windows |
| `2026-10-02-chromium-navegador-nativo-linux` | Chromium |
| `2026-10-02-electron-removido-linux` | remove o Electron |
| `2026-10-02-electron-removido-windows` | remove o Electron |
| `2026-10-09-python3-antes-da-loja` | ordem do `PATH` no Windows |
| `2026-10-10-chaves-do-push-no-celular` | chaves no `.env` |
| `2026-10-12-navegador-linux-motor-enxuto` | Chromium |
| `2026-10-20-backend-abre-o-proprio-log` | tarefa agendada do Windows |
| `2026-10-21-orq-durable-cleanup` | dados em `~/.hangar` |
| `2026-10-22-python-install-manager-windows` | interpretador Python e atalhos no `PATH` do Windows |

Ficam sem o campo, mesmo com comando: os que rodam `install-hangar-send.sh`,
`install-claude-wrapper.sh`/`setup-windows-wrappers.ps1` ou `registrar-mcp.py` (conteúdo e
atalhos), `2026-10-02-hangar-server-binarios` (o binário vem no pacote) e
`2026-10-19-trava-do-client-api` (hook do git do checkout, só no modo desenvolvedor).

## O que o botão faz sozinho, e o que só um passo faz

O botão Atualizar **não roda mais o instalador inteiro**. Sozinho ele faz: `git fetch` +
fast-forward, troca o `frontend/dist` pelo build do CI, `uv sync` no backend, `npm ci` quando o
`package-lock.json` mudou (e só onde já existe `node_modules`), no Linux o `install-chromium.sh`
quando há um chrome-headless-shell baixado (ele não se atualiza como o Chrome da distro), reinício
do serviço/tarefa e prova de vida por pid. Todo o resto só acontece quando um commit declara o passo aqui — e o pre-commit e
o CI recusam commit que mexa nesses arquivos sem o passo (`scripts/check-passo-de-atualizacao.sh`).

| O que mudou | `comando` no Linux | `comando` no Windows |
|---|---|---|
| Wrapper do `claude`/`codex` (`scripts/install-claude-wrapper.sh`, `windows-wrappers.ps1`) | `./scripts/install-claude-wrapper.sh` | `powershell -ExecutionPolicy Bypass -File scripts/setup-windows-wrappers.ps1` |
| `hangar-send`, skills, bloco no CLAUDE.md | `./scripts/install-hangar-send.sh` | `powershell -ExecutionPolicy Bypass -File install.ps1 -Update` |
| Painel (`install-hangar-panel.sh`) | `./scripts/install-hangar-panel.sh` | idem |
| Hooks do Claude (`install-hooks.sh`) | `./scripts/install-hooks.sh` | idem |
| Units do systemd / tarefas agendadas / vigia | `./install.sh --update` | `powershell -ExecutionPolicy Bypass -File install.ps1 -Update` |
| Statusline | `./install.sh --update` | idem |
| Chave nova no `backend/.env` | `./install.sh --update` | idem |
| Dependência nova do Electron (`shell/`) | `npm ci --prefix shell` | `npm ci --prefix shell` |

Escreva o comando mais estreito da linha. Quando só o instalador inteiro cobre (Windows sem
script por área), é ele mesmo — e a `prova` continua obrigatória, porque "saiu com 0" não é
"fez".
