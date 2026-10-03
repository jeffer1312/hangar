# Canal de testes pelo app nativo — planejar e executar

O dono quer ligar o canal de testes pelo app, sem editar o `.env` à mão.

Hoje (já na `main`): `CP_UPDATE_BRANCH=<branch>` no `backend/.env` faz o "Atualizar" puxar essa
branch em vez da `main` (`backend/app/config.py` campo `update_branch`, `backend/app/atualizar.py`,
regra em `docs/decisoes/instalacao.md`). O campo aparece só para leitura na tela de Avançado. O
backend lê o `.env` só ao subir.

## O que fazer

- **Onde:** pasta `/home/jefferson/pessoal/hangar/.claude/worktrees/canal-teste-app`, branch
  `canal-teste-app`, criada de `origin/main` (`876cb22a`).
- **Backend:** rota autenticada só do dono (convidado recusado) para ler e gravar o canal:
  - ler: canal atual (vazio = `main`), branch do checkout e o último canal usado;
  - gravar: validar o nome com a mesma regra do `update_branch`, conferir no `origin` que a branch
    existe (`git ls-remote`, falha = erro claro e nada muda), gravar `CP_UPDATE_BRANCH` no
    `backend/.env` de forma atômica preservando o resto do arquivo (`atomico`, encoding/linhas como
    as regras de `docs/decisoes/windows.md` e `instalacao.md` mandam), e fazer o valor valer sem o
    dono reiniciar à mão — escolha o menor jeito correto (ler o valor na hora em que o Atualizar
    roda, ou reiniciar o backend pelo caminho que já existe). Vazio = voltar para a `main`.
  - registrar no diário a troca de canal (código, branch), sem segredo.
- **App nativo (`desktop-native/`)**, tela "Sobre": seção "Canal de testes" com chave liga/desliga e
  campo da branch (preenchido com o último usado), botão para aplicar, estado de carregando/erro/
  sucesso, e depois de aplicar oferecer o "Atualizar" que já existe. Textos via `messages/pt.json`
  e `messages/en.json` no mesmo commit (o nativo lê esses arquivos). Siga o padrão das outras
  seções da tela "Sobre" e as skills `gpui-kit`/`gpui-kit-design-guides` do nativo.
- Fora deste trabalho: tela no celular (Expo) e no PWA — anote como próximo passo.

## Como trabalhar

1. Leia o `CLAUDE.md` da raiz, as "Regras vigentes" de `docs/decisoes/instalacao.md` e
   `docs/decisoes/windows.md`, e o código citado. Escreva spec e plano curtos em
   `docs/superpowers/specs|plans/2026-10-03-canal-teste-app*.md` (não commitar), formato
   `### Task N:` / `- [ ] **Step N: …**`.
2. Execute em seguida (o dono autorizou): testes focados de cada Task (`uv run pytest <arquivos>`,
   `cargo test` focado e `cargo check --locked` no `desktop-native/`), commit por Task com `git add`
   de caminhos explícitos e mensagem descritiva em inglês, revisão independente do diff ao fim de
   cada Task, marque os Steps `[x]`.
3. Não faça push. Ao terminar, mande via `hangar-send hangar "…"` os commits, testes e o que não
   conferiu.

## Regras

- Nunca suba, reinicie ou pare o backend vivo nem o serviço `hangar-backend`; nunca rode instaladores
  nem o Atualizar de verdade. Testes com `.env` e repositório git temporários.
- Identificador novo em inglês; comentário e texto de tela em português; comentário curto, sobre o
  porquê. Afirmação técnica com prova (arquivo:linha).
