# Árvore de arquivos e diff por arquivo — Implementation Plan

> ## ✅ CONCLUÍDO em 16/08/2026 — pushado, nada pendente
>
> As **12 Tasks do plano** foram executadas e aprovadas, mais **4 fora dele** (13, 14, 15 e a
> blindagem) nascidas de achados de revisão e de pedidos do usuário testando o app instalado. Os
> **85 Steps** estão marcados.
>
> - **Ponta do trabalho:** `01af0f18` · `origin/main` · 66 commits, 56 arquivos, +6502 −117
> - **Portão:** um executor e um revisor independentes por commit, de famílias de modelo diferentes;
>   **duas revisões de conjunto** por sessões que não participaram (a segunda achou 2 defeitos que os
>   portões individuais não pegaram).
> - **Retrospectiva (fase 5):** `~/.claude/orq-retros/2026-08-15-arv.md` — 21 propostas, todas
>   aplicadas na skill `orchestrating-idea-to-push` (commits `6ffc077b` e `12be968d`).
> - **Registro da execução:** `~/.claude/.claude-pocket-pair/grupo-arv.md`.
>
> Arquivado em `docs/superpowers/plans/feitos/` — o app só lista os planos soltos em `plans/`, então
> daqui ele não aparece mais como pendente no seletor.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> Este plano roda sob a skill `orchestrating-idea-to-push`, em **lote paralelo com uma worktree por
> Task** (`references/paralelo-worktree.md`) — ver a seção "Lotes". Marque `- [x]` ao fim de cada
> Step: é o que alimenta a barra de progresso no celular.

**Goal:** navegar os arquivos do repositório da sessão, achar arquivo por nome ou por conteúdo, ver
o que mudou sem expandir nada, e clicar num arquivo para ver tudo que mudou nele somando os turnos.

**Architecture:** três módulos novos e independentes no backend (`filetree.py` lista e lê,
`filesearch.py` busca, `path_diff` entra no `git_ops.py`), ligados às rotas num passo próprio. No
front, quatro peças independentes (`FileTree`, `FileViewer`, `FileSearchBar`, `filesStore`), montadas
depois em dois hospedeiros: aba nova no `DesktopSessionContext` (desktop) e aba nova no modal de Git
(celular). O arquivo aberto cobre a área da conversa e deixa a árvore viva ao lado.

**Tech Stack:** Python 3.14 + FastAPI (rotas `def` → threadpool), pytest com repositórios git
temporários; Svelte 5 (runes) + TypeScript + vitest; Paraglide para as duas línguas; o
`DiffView.svelte` existente renderiza o diff.

**Spec:** [`docs/pesquisa-referencias-2026-08-13.md`](../../pesquisa-referencias-2026-08-13.md) —
tarefas **C1** (linha 803) e **C2** (linha 829). O levantamento do terreno real, que corrige duas
afirmações otimistas da spec, está em
[`docs/pesquisa-c1-c2-terreno.md`](../../pesquisa-c1-c2-terreno.md) e é **leitura obrigatória** antes
da primeira linha de código.

## Global Constraints

- **Nenhuma string crua na UI.** `import * as m from '.../paraglide/messages'`; `m.chave()` ou
  `m.chave({ n })`. Prefixo **`arq_`** para as chaves deste trabalho. Chave nova entra em
  `frontend/messages/pt.json` **e** `frontend/messages/en.json`, no mesmo commit. A trava
  `frontend/src/lib/i18nGuard.test.ts` reprova o `npm run test` se escapar string crua.
- **Antes de criar chave, procure a existente** — há 153 de git/arquivo. `m.git_ver_diff()`,
  `m.git_tag_mod()`, `m.git_arvore_limpa()`, `m.nav_arquivo()` já existem.
- **Erro do backend sai no envelope**, nunca texto solto:
  `raise HTTPException(status_code=415, detail=erro("erro_arq_binario", "arquivo binario", path=p))`
  (`backend/app/mensagens.py:16`). Quem traduz é `frontend/src/lib/errosApi.ts:216`.
- **Todo comando git leva `-c core.quotePath=false`.** Medido em 15/08/2026: sem isso,
  `git status --porcelain` devolve `"sess\303\243o-\303\272nica.md"` — com aspas e escape octal — em
  vez de `sessão-única.md`. Este repositório é cheio de nome em português.
- **Argumentos de git em lista, nunca string de shell.** Todo `path` do cliente passa pela contenção
  antes de virar argumento, e nenhum pode começar com `-`.
- **Código de saída do git é lido com cuidado:** `git grep` sai com **1 quando não acha nada** e
  **128 fora de um repositório**. Tratar todo não-zero como falha vira "nenhum resultado" em erro.
- Rotas FastAPI novas: `def` (não `async def`), `dependencies=[Depends(require_auth)]`, body com
  `_StrictBody`.
- **As duas telas, sempre**: desktop (≥820px) e celular. **Os dois idiomas, sempre.**
- **Transparência é padrão.** Superfície nova nasce `transparent`; precisando de material,
  `--surface-raised` (chip, botão, menu) ou `--surface-inset` (campo, área de diff).
  `--bg-elevated`/`--bg-base` crus só para realce de estado.
- Comentários em português. Casar a indentação do arquivo. **Nunca** rodar formatter.
- Stage por caminho explícito. **Nunca** `git add -A` nem `git add .`.
- Portões, na forma que não depende do diretório atual:
  ```bash
  uv run --directory /home/jefferson/pessoal/hangar/backend pytest -v
  npm --prefix /home/jefferson/pessoal/hangar/frontend run check
  npm --prefix /home/jefferson/pessoal/hangar/frontend run test
  ```
  Os dois do front já rodam `i18n:compile` antes (`frontend/package.json:12-13`), então chave
  inexistente vira erro de tipo e string crua derruba o teste.

---

## Step 0 — TODA sessão roda isto antes de tocar em código

```bash
/home/jefferson/pessoal/hangar/scripts/checar-skills.sh
```

Sai `0` e segue; sai `1`, **pare e avise o árbitro** — não comece a Task sem as skills.

Isto não é zelo: em 15/08/2026 seis plugins (o `superpowers` e o `example` entre eles) estavam
habilitados no `settings.json` com o `installPath` apontando para `~/.claude-work/`, que não existe.
O Claude Code carregava o plugin **vazio, sem erro nenhum** — a skill simplesmente não aparecia na
lista, e a sessão só descobria ao tentar invocar, no meio da Task. O Pi tem o problema irmão: a
ponte `~/.pi/agent/skills-bridge/` cobria só plugins, e **nenhuma** skill pessoal de
`~/.claude/skills/` — inclusive a `orchestrating-idea-to-push`, que é a que diz o papel de cada
sessão. Os dois foram consertados no mesmo dia; o script existe para que a próxima vez seja
descoberta no primeiro minuto, e não na terceira hora.

O script procura por **`SKILL.md` no disco**, não por plugin habilitado na configuração — é
exatamente a diferença que os dois defeitos exploravam. Ele também avisa quando algum plugin
continua com caminho quebrado, mesmo que as skills pedidas tenham sido achadas.

Skills que o executor deve ter à mão nesta empreitada: `test-driven-development` (o formato
teste-primeiro de cada Task), `verification-before-completion` (antes de dizer que acabou),
`using-git-worktrees` (o lote paralelo) e `systematic-debugging` (quando o portão ficar vermelho e a
causa não for óbvia).

---

## File Structure

| Arquivo | Responsabilidade | Task |
|---|---|---|
| `backend/app/filetree.py` | listar um nível do diretório e ler um arquivo, com contenção no `cwd` | 1 |
| `backend/app/filesearch.py` | buscar por nome e por conteúdo, via git | 2 |
| `backend/app/git_ops.py` (+) | `path_diff` com escopo, e `_cap` nos diffs por arquivo | 3 |
| `backend/app/api.py` (+) · `models.py` (+) | as quatro rotas e os modelos | 4 |
| `frontend/src/lib/api.ts` · `types.ts` (+) · `messages/*.json` (+) | clientes, tipos e chaves | 5 |
| `frontend/src/components/files/FileTree.svelte` | desenhar a árvore (apresentacional) | 6 |
| `frontend/src/components/files/FileViewer.svelte` | desenhar diff ou conteúdo (apresentacional) | 7 |
| `frontend/src/components/files/FileSearchBar.svelte` | campo + segmentado Nomes/Conteúdo | 8 |
| `frontend/src/lib/filesStore.svelte.ts` | estado por sessão, corrida de resposta, recarga | 9 |
| `frontend/src/components/files/FilesPanel.svelte` | junta as quatro peças | 10 |
| `frontend/src/components/DesktopSessionContext.svelte` (+) | a aba no painel do desktop | 10 |
| `frontend/src/screens/Chat.svelte` (+) | o visor cobrindo a conversa | 11 |
| `frontend/src/lib/gitTabs.ts` (+) · `git/GitTabs.svelte` (+) | a aba no celular | 12 |

**Por que `filetree` e `filesearch` são dois módulos:** um fala com o sistema de arquivos e o outro
com o git. Separados, viram duas Tasks que rodam ao mesmo tempo; juntos, seriam uma fila.

---

## A barra das Tasks de tela é o mock aprovado

Decisão do usuário, 15/08/2026. O mock foi feito no planejamento com os tokens reais de
`frontend/src/app.css`, revisado em três rodadas e aprovado. Ele é a barra porque tira do executor a
única saída que o print de outro produto deixa em pé — *"não dá pra ficar igual, o material é outro"*.

| Print aprovado | Trava |
|---|---|
| `docs/mocks/2026-08-15-arvore/prints/1-desktop-painel.png` | a aba **Arquivos** no painel de 264px |
| `docs/mocks/2026-08-15-arvore/prints/2-desktop-visualizador.png` | o arquivo cobrindo a conversa, árvore viva |
| `docs/mocks/2026-08-15-arvore/prints/3-celular.png` | a mesma aba na folha do celular, 390px |

O **HTML do mock fica ao lado dos prints** (`1-desktop-painel.html`, `2-desktop-visualizador.html`,
`3-celular.html`, `base.css`, `arvore.js`). O executor **lê o HTML** para espaçamento e token
exatos — o mock não é só imagem, é código.

### A barra tem DUAS perguntas, não uma

Decisão do usuário, 15/08/2026, e ela existe por experiência dele: *"a maioria das vezes o mock que
eu aprovo não é o que é construído"*. O mock é um HTML solto — ele **não conhece** papel de parede,
o slider de Transparência, o tema claro, nem a densidade das listas que já existem no app. Cobrar
só fidelidade ao PNG entrega uma tela fiel ao mock e **estranha dentro do produto**.

Então cada Task de tela passa por duas comparações cegas, nesta ordem:

**1. Fidelidade — "qual destes dois é o mock?"** Um subagente novo recebe o print do mock e o print
do resultado, sem saber qual é qual. Cobra a **anatomia**: o que aparece, onde, em que ordem, com
que hierarquia. Aqui o mock manda.

**2. Integração — "estas duas telas são do mesmo app?"** Outro subagente novo recebe o print do
resultado e o print de uma **tela irmã real**, capturada no mesmo momento e na mesma largura:

| Task | Tela irmã para a comparação de integração |
|---|---|
| 10 | a aba **Contexto** do mesmo painel — troca de aba e captura |
| 11 | o **`GitChangesTab`** no modal de Git, com um arquivo selecionado |
| 12 | as abas **Alterações** e **Histórico** do mesmo modal, no celular |

**Onde as duas discordam, o app real ganha.** O mock foi feito fora do app; ele acertou a anatomia,
não o material. Divergência encontrada e resolvida a favor do app **entra no commit como comentário**,
explicando o quê e por quê — é assim que o mock deixa de ser um contrato cego.

### O que o mock NÃO cobre, e a Task tem que resolver sozinha

Lista fechada, para não virar descoberta no meio:

- **Papel de parede ligado** (`html[data-bg="image"]`) com o slider de Transparência no meio e nos
  extremos. Superfície nova nasce `transparent`; precisando de material, `--surface-raised` ou
  `--surface-inset` — **nunca** `--bg-elevated`/`--bg-base` crus, que viram retângulo chapado
  boiando sobre a foto. Qualquer retângulo que não deixe a foto atravessar, enquanto o painel em
  volta deixa, é bug.
- **Tema claro.** O mock só tem o escuro. Todo token usado precisa existir nos dois.
- **Densidade dos vizinhos.** A altura de linha da árvore tem que conversar com as listas que já
  existem no painel, não com o número que eu escolhi no HTML.
- **Estado de foco e navegação por teclado**, que print nenhum mostra.
- **Largura real do painel em cada breakpoint** — 264px é o padrão, mas ele vai a 300 e 340px em
  telas maiores (`Chat.svelte`). O mock só tem 264.

**Teto de 2 rodadas por Task**, contando as duas comparações juntas. Estourou → para e chama o
usuário (é o teto declarado mais abaixo).

### Onde a barra é cobrada, e onde não é

Distinção que o plano precisa fazer explícita, senão o executor tenta comparar print de um
componente que não monta sozinho:

| Tasks | O que são | Como a barra funciona ali |
|---|---|---|
| **6, 7, 8, 9** (lote B) | peças soltas — não existe tela para abrir | **Sem comparação cega.** O executor confere contra o **HTML do mock**: classe por classe, token por token, valor por valor. É leitura de código contra código. |
| **10, 11, 12** (montagem) | as telas de verdade, montadas e abríveis | **Comparação cega**, print contra print, teto de 2 rodadas. É aqui que a barra decide. |

Quem escreve o componente na Task 6 e quem monta a tela na Task 10 podem ser sessões diferentes —
por isso o mock precisa ser **código legível**, e não só imagem. É o `base.css` que responde "qual
é o tamanho da fonte do contador", não o pixel de um PNG.

**O Paseo é a referência que o mock honra**, e o executor consulta quando o mock não responder:
`.refs/paseo-live/desktop/10-aba-arquivos.png`, `11-arquivos-visualizador.png`,
`09-alteracoes-diff-arquivo.png`. Do Orca vêm o segmentado `Names | Contents` e a marca herdada:
`.refs/orca-live/13-arvore-arquivos.png`.

**Estados que precisam de print** (desktop 1440px e celular 390px): árvore filtrada; árvore com
tudo; arquivo aberto com diff; busca com resultado; busca sem resultado; erro de arquivo grande.

---

## Lotes — o que corre em paralelo

Declarado na fase 1, com o usuário. O árbitro não muda depois.

| Lote | Tasks | Escritores juntos | Por que pode |
|---|---|---|---|
| **A** (paralelo) | 1, 2, 3 | **3** | três módulos sem um arquivo em comum; nada que um cria o outro usa; cada um tem seu arquivo de teste |
| **Costura 1** (serial) | 4 | 1 | `api.py`/`models.py` são de todos — é o arquivo compartilhado que tirou as rotas do lote A |
| **Costura 2** (serial) | 5 | 1 | `api.ts`, `types.ts` e os dois `.json` de mensagem, idem |
| **B** (paralelo) | 6, 7, 8, 9 | **4** | quatro arquivos novos que não se tocam; todos verificam por vitest, sem tela montada |
| **Montagem** (serial) | 10, 11, 12 | 1 | monta tela: brigariam pelas portas 8765/5173, e duas tocam arquivo com teste existente |

**Por que não mais:** o gatilho exige arquivos disjuntos, nenhum símbolo atravessando e verificação
isolada. Uma quinta Task no lote B teria que compartilhar `FilesPanel.svelte`, e arquivo
compartilhado é a serialização voltando como conflito de merge.

**Receita** (o árbitro executa; a base vai no contrato):

**Lote A** — a `BASE` é o HEAD da `main` no momento da largada, e vai no contrato:

```bash
BASE=$(git rev-parse HEAD)
git worktree add /home/jefferson/pessoal/wt-arv-t1 -b arv-t1 "$BASE"
git worktree add /home/jefferson/pessoal/wt-arv-t2 -b arv-t2 "$BASE"
git worktree add /home/jefferson/pessoal/wt-arv-t3 -b arv-t3 "$BASE"
```

**Lote B** — criado **depois** das costuras, e a base é OUTRA: as Tasks 6 a 9 consomem os tipos e os
clientes da Task 5, então elas nascem do HEAD já com as costuras dentro. Usar a `BASE` do lote A
aqui daria quatro sessões trabalhando contra um `api.ts` que ainda não tem os clientes.

```bash
BASE_B=$(git rev-parse HEAD)     # depois de mergear as Tasks 1-5
git worktree add /home/jefferson/pessoal/wt-arv-t6 -b arv-t6 "$BASE_B"
git worktree add /home/jefferson/pessoal/wt-arv-t7 -b arv-t7 "$BASE_B"
git worktree add /home/jefferson/pessoal/wt-arv-t8 -b arv-t8 "$BASE_B"
git worktree add /home/jefferson/pessoal/wt-arv-t9 -b arv-t9 "$BASE_B"
```

Cada worktree do lote B precisa do **próprio `node_modules`** (`npm --prefix <worktree> ci`) e de uma
compilação de mensagens (`npm --prefix <worktree> run i18n:compile`), senão o `vitest` não acha as
funções do paraglide.

Cada worktree quer o próprio `.venv` (lote A) ou `node_modules` (lote B) — custo real, contado.
Merge de uma branch por vez, só depois do `APROVA` daquela Task, com **verificação completa depois
de cada merge**. Conflito significa que as Tasks não eram independentes: o árbitro para, e a Task
perdedora vira Task nova, serial. `git worktree remove` ao fim de cada lote.

**O paralelo torna a revisão final obrigatória**, sobre `$BASE..ponta`, em sessão nova.

## Intocáveis

- `docs/pesquisa-referencias-2026-08-13.md`, `docs/pesquisa-c1-c2-terreno.md`, este plano.
- `.refs/**` e `docs/mocks/**` — as referências e a barra. Mudar a barra durante a execução é mudar
  o alvo depois do tiro.
- `CLAUDE.md` — só o árbitro escreve, e só com o trabalho aprovado.
- **Estado em 15/08/2026:** `main == origin/main` em `2d1df848`, que já traz **todo o trabalho de
  internacionalização**. Não versionados e fora de commit de Task: `docs/pesquisa-c1-c2-terreno.md`,
  este plano, `docs/mocks/`.

## O que a revisão precisa cobrir

Em vigor antes da Task 1, cobrado em cada commit:

- **Contenção de caminho, com tentativa real de escapar:** `..`, caminho absoluto, symlink para fora
  do cwd, e `path` começando com `-`. O revisor escreve o caso e mostra a recusa.
- **Caminho com acento e com espaço** em tudo que fala com git. É o defeito mais provável aqui.
- **Callers irmãos:** mexeu em `file_diff` ou `_cap`? Liste quem chama e prove que não quebrou —
  `GitChangesTab` depende disso hoje.
- **Concorrência na UI:** dois cliques rápidos (a resposta do primeiro não pode pintar sobre o
  segundo), trocar de sessão com diff carregando, desmontar no meio da busca.
- **Estado final:** pasta aberta continua aberta ao voltar? A aba sobrevive à troca de sessão?
- **Falha visível:** o revisor abre um binário e um arquivo grande de verdade.
- **Os dois idiomas:** troca o app para inglês e percorre a tela. A trava pega string crua; ela
  **não** pega chave que existe só em `pt.json`.
- Skills: backend → `ecc:python-reviewer` + `ecc:security-reviewer`; front →
  `ecc:typescript-reviewer` + `ecc:a11y-architect`.

## Non-goals

Editar arquivo pelo app. Diff lado a lado. Esconder espaço em branco. Árvore de repositório que não
é o da sessão. Prévia renderizada de markdown. Busca por expressão regular. Atualização automática
por SSE (não há evento de arquivo; o botão de recarregar da Task 10 é a resposta).

**Persistência na URL** — a aba escolhida e o arquivo aberto **não** sobrevivem a um reload, porque
`frontend/src/lib/route.ts` não é tocado por Task nenhuma. É decisão, não esquecimento: o estado vive
no `filesStore`, por sessão, e sobrevive a trocar de sessão e voltar — que é o caso do dia a dia.
Deep-link de arquivo é feature própria, e o precedente (`#/board/<servidor>/<nome>`) mostra que ela
custa mais que um parâmetro.

---

## LOTE A — paralelo, três worktrees

### Task 1: `filetree.py` — listar diretório e ler arquivo

**Files:**
- Create: `backend/app/filetree.py`
- Create: `backend/tests/test_filetree.py`
- **Não toca `api.py` nem `models.py`** — é o que mantém esta Task disjunta.

**Interfaces:**
- Consumes: `git_ops._run`, `git_ops.changed_files` (existentes).
- Produces:
  - `class FileError(Exception)` com `.status: int`, `.code: str`, `.msg: str`
  - `list_dir(cwd: str, path: str | None = None, so_modificados: bool = True) -> dict`
    → `{"entries": [{"name","path","is_dir","size","changed","add","del"}], "truncated": bool}`
  - `read_file(cwd: str, path: str) -> dict` → `{"path","text","size","truncated"}`
  - `MAX_ENTRADAS = 1000`, `MAX_BYTES = 512 * 1024`

- [x] **Step 1: Escrever o teste que falha (listar)**

Criar `backend/tests/test_filetree.py`:

```python
import pytest

from app import filetree, git_ops
from app.filetree import FileError


def _repo(tmp_path):
    """Repo git com um commit, pra changed_files funcionar."""
    d = str(tmp_path)
    git_ops._run(d, "init", "-q", ".")
    git_ops._run(d, "config", "user.email", "t@t")
    git_ops._run(d, "config", "user.name", "t")
    (tmp_path / "base.txt").write_text("base\n")
    git_ops._run(d, "add", "base.txt")
    git_ops._run(d, "commit", "-q", "-m", "base")
    return d


def test_lista_pastas_antes_de_arquivos(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "zeta").mkdir()
    (tmp_path / "alfa.txt").write_text("a")
    nomes = [e["name"] for e in filetree.list_dir(d, so_modificados=False)["entries"]]
    assert nomes.index("zeta") < nomes.index("alfa.txt")


def test_esconde_git_mas_mostra_dotfile(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / ".env.example").write_text("X=1")
    nomes = [e["name"] for e in filetree.list_dir(d, so_modificados=False)["entries"]]
    assert ".git" not in nomes
    assert ".env.example" in nomes


def test_recusa_escapar_da_raiz(tmp_path):
    d = _repo(tmp_path)
    for ruim in ("..", "../..", "/etc"):
        with pytest.raises(FileError) as e:
            filetree.list_dir(d, ruim)
        assert e.value.code == "erro_arq_fora_da_raiz"
```

- [x] **Step 2: Rodar e ver falhar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filetree.py -v`
Expected: FAIL com `ModuleNotFoundError: No module named 'app.filetree'`.

- [x] **Step 3: Implementar o esqueleto e a contenção**

Criar `backend/app/filetree.py`:

```python
"""Arvore de arquivos do repo DA SESSAO.

Nao reusa a allowlist do fs.py de proposito: la a raiz e a lista de projetos (o seletor
de pasta na criacao de sessao), aqui a raiz e o cwd da sessao. Mesma trava de caminho,
politica de raiz diferente.
"""

import os
from pathlib import Path

MAX_ENTRADAS = 1000
MAX_BYTES = 512 * 1024


class FileError(Exception):
    def __init__(self, status: int, code: str, msg: str):
        super().__init__(msg)
        self.status, self.code, self.msg = status, code, msg


def _real(p: str) -> Path:
    return Path(os.path.realpath(os.path.expanduser(p)))


def _within(child: Path, root: Path) -> bool:
    return child == root or root in child.parents


def _resolver(cwd: str, path: str | None) -> tuple[Path, Path]:
    """Devolve (raiz, alvo) com o alvo provado dentro da raiz."""
    raiz = _real(cwd)
    # Recusado ANTES de virar caminho: um path assim acabaria como flag num comando git.
    if path and path.startswith("-"):
        raise FileError(400, "erro_arq_caminho_invalido", "caminho invalido")
    alvo = _real(os.path.join(cwd, path)) if path else raiz
    if not _within(alvo, raiz):
        raise FileError(400, "erro_arq_fora_da_raiz", "caminho sai da raiz da sessao")
    if not alvo.exists():
        raise FileError(404, "erro_arq_inexistente", "caminho nao existe")
    return raiz, alvo
```

- [x] **Step 4: Rodar — os dois primeiros ainda falham, o terceiro passa**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filetree.py -v`
Expected: `test_recusa_escapar_da_raiz` PASS; os outros dois FAIL com `AttributeError: list_dir`.

- [x] **Step 5: Implementar `list_dir`**

Acrescentar a `filetree.py`:

```python
def _prefixo_no_repo(cwd: str) -> str:
    """Onde o cwd fica DENTRO do repo, com barra no fim ('' se for o topo).

    Existe porque o git devolve caminho relativo ao TOPO do repositorio, e esta arvore
    lista relativo ao cwd DA SESSAO. Uma sessao aberta em /repo/backend recebia
    {"backend/app/x.py": "M"} do git e comparava com "app/x.py" — nunca casava, e a arvore
    voltava VAZIA no modo so_modificados (que e o padrao). Bug encontrado na auditoria do
    plano, antes de custar uma Task.
    """
    from app import git_ops
    p = git_ops._run(cwd, "rev-parse", "--show-prefix")
    return p.stdout.strip() if p.returncode == 0 else ""


def _marcas(cwd: str) -> dict[str, str]:
    """path RELATIVO AO CWD -> letra do porcelain. Fora de repo git, vazio."""
    from app import git_ops
    pref = _prefixo_no_repo(cwd)
    try:
        brutas = git_ops.changed_files(cwd)
    except Exception:
        return {}
    fora = {}
    for c in brutas:
        p = c["path"]
        if pref:
            if not p.startswith(pref):
                continue                    # mudou fora do cwd desta sessao: nao e da arvore
            p = p[len(pref):]
        fora[p.rstrip("/")] = c["code"].strip()[:1] or "?"
    return fora


def _numstat(cwd: str) -> dict[str, tuple[int, int]]:
    """path RELATIVO AO CWD -> (add, del). UMA chamada pro repo inteiro, nunca uma por arquivo."""
    from app import git_ops
    pref = _prefixo_no_repo(cwd)
    p = git_ops._run(cwd, "-c", "core.quotePath=false", "diff", "--numstat", "HEAD")
    if p.returncode != 0:
        return {}
    fora = {}
    for linha in p.stdout.splitlines():
        partes = linha.split("\t")
        if len(partes) != 3 or partes[0] == "-":       # "-" = binario
            continue
        cam = partes[2]
        if pref:
            if not cam.startswith(pref):
                continue
            cam = cam[len(pref):]
        fora[cam] = (int(partes[0]), int(partes[1]))
    return fora


def list_dir(cwd: str, path: str | None = None, so_modificados: bool = True) -> dict:
    raiz, alvo = _resolver(cwd, path)
    if not alvo.is_dir():
        raise FileError(400, "erro_arq_nao_e_pasta", "nao e uma pasta")

    marcas, nums = _marcas(cwd), _numstat(cwd)
    entradas, cortou = [], False
    for e in sorted(os.scandir(alvo), key=lambda e: (not e.is_dir(), e.name.lower())):
        if e.name == ".git":
            continue
        filho = _real(e.path)
        if not _within(filho, raiz):        # symlink apontando pra fora
            continue
        try:
            tam = 0 if e.is_dir() else e.stat().st_size
        except OSError:                      # symlink quebrado: aparece, sem tamanho
            tam = 0
        rel = str(filho.relative_to(raiz))
        # Pasta herda a marca e SOMA o +N -M dos descendentes.
        dentro = [p for p in marcas if p == rel or p.startswith(rel + "/")]
        marca = marcas.get(rel) or (marcas[dentro[0]] if e.is_dir() and dentro else None)
        add = sum(nums.get(p, (0, 0))[0] for p in dentro)
        rem = sum(nums.get(p, (0, 0))[1] for p in dentro)
        if so_modificados and marca is None:
            continue
        if len(entradas) >= MAX_ENTRADAS:
            cortou = True
            break
        entradas.append({
            "name": e.name, "path": rel, "is_dir": e.is_dir(), "size": tam,
            "changed": marca, "add": add, "del": rem,
        })
    return {"entries": entradas, "truncated": cortou}
```

- [x] **Step 6: Rodar e ver passar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filetree.py -v`
Expected: 3 PASS.

- [x] **Step 7: Teste da marca herdada, da soma e do acento**

Acrescentar a `test_filetree.py`:

```python
def test_pasta_herda_marca_e_soma_do_neto(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "src" / "lib").mkdir(parents=True)
    alvo = tmp_path / "src" / "lib" / "x.txt"
    alvo.write_text("a\nb\n")
    git_ops._run(d, "add", "src/lib/x.txt")
    git_ops._run(d, "commit", "-q", "-m", "x")
    alvo.write_text("a\nb\nc\nd\n")
    src = [e for e in filetree.list_dir(d)["entries"] if e["name"] == "src"][0]
    assert src["changed"] == "M"
    assert src["add"] == 2 and src["del"] == 0


def test_nome_com_acento_nao_volta_escapado(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "sessão-única.md").write_text("x")
    paths = [e["path"] for e in filetree.list_dir(d)["entries"]]
    assert "sessão-única.md" in paths
    assert not any("\\303" in p for p in paths)


def test_so_modificados_esconde_intocado_mas_mantem_o_caminho(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "novo.txt").write_text("n")
    (tmp_path / "intocado.txt").write_text("i")
    git_ops._run(d, "add", "intocado.txt")
    git_ops._run(d, "commit", "-q", "-m", "i")
    nomes = [e["name"] for e in filetree.list_dir(d)["entries"]]
    assert "src" in nomes
    assert "intocado.txt" not in nomes


def test_sessao_aberta_numa_SUBPASTA_do_repo(tmp_path):
    """O git devolve caminho relativo ao TOPO do repo; a arvore lista relativo ao cwd DA
    SESSAO. Sem o prefixo, a arvore volta vazia no modo padrao. Nenhum outro teste pega
    isto, porque todos abrem no topo."""
    d = _repo(tmp_path)
    (tmp_path / "backend" / "app").mkdir(parents=True)
    alvo = tmp_path / "backend" / "app" / "x.py"
    alvo.write_text("a\n")
    git_ops._run(d, "add", "backend/app/x.py")
    git_ops._run(d, "commit", "-q", "-m", "x")
    alvo.write_text("a\nb\n")

    sub = str(tmp_path / "backend")          # a sessao vive AQUI, nao no topo
    ent = {e["name"]: e for e in filetree.list_dir(sub)["entries"]}
    assert "app" in ent, "arvore vazia: o prefixo do repo nao foi descontado"
    assert ent["app"]["changed"] == "M"
    assert ent["app"]["add"] == 1


def test_recusa_path_comecando_com_traco(tmp_path):
    """Global Constraint: nenhum path do cliente pode virar flag de git."""
    d = _repo(tmp_path)
    for ruim in ("-rf", "--output=/tmp/x"):
        with pytest.raises(FileError) as e:
            filetree.list_dir(d, ruim)
        assert e.value.code == "erro_arq_caminho_invalido"
        with pytest.raises(FileError):
            filetree.read_file(d, ruim)
```

- [x] **Step 8: Rodar, corrigir se falhar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filetree.py -v`
Expected: 6 PASS. Se o teste do acento falhar, falta `-c core.quotePath=false` — mas note que ele
vem de `changed_files` (`git_ops.py:320`), que **não** passa a opção hoje: acrescente lá, e rode
`pytest tests/test_git_ops.py -v` para provar que não quebrou nada.

- [x] **Step 9: Commit**

```bash
git add backend/app/filetree.py backend/tests/test_filetree.py backend/app/git_ops.py
git commit -m "feat(arquivos): lista um nivel do repo da sessao com marca e contador de git"
```

- [x] **Step 10: Teste que falha (ler arquivo)**

Acrescentar a `test_filetree.py`:

```python
def test_le_texto_inteiro(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "a.txt").write_text("linha\n")
    r = filetree.read_file(d, "a.txt")
    assert r["text"] == "linha\n" and r["truncated"] is False


def test_corta_arquivo_grande(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "g.txt").write_text("x" * (filetree.MAX_BYTES + 5000))
    r = filetree.read_file(d, "g.txt")
    assert r["truncated"] is True
    assert len(r["text"].encode()) <= filetree.MAX_BYTES


def test_recusa_binario(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "i.png").write_bytes(b"\x89PNG\r\n\x1a\n\x00\x00binario")
    with pytest.raises(FileError) as e:
        filetree.read_file(d, "i.png")
    assert e.value.status == 415 and e.value.code == "erro_arq_binario"
```

- [x] **Step 11: Rodar e ver falhar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filetree.py -k read -v`
Expected: FAIL com `AttributeError: module 'app.filetree' has no attribute 'read_file'`.

- [x] **Step 12: Implementar `read_file`**

```python
def read_file(cwd: str, path: str) -> dict:
    _raiz, alvo = _resolver(cwd, path)
    if alvo.is_dir():
        raise FileError(400, "erro_arq_e_pasta", "isso e uma pasta")
    try:
        with alvo.open("rb") as fh:
            cabeca = fh.read(8192)
            if b"\x00" in cabeca:
                raise FileError(415, "erro_arq_binario", "arquivo binario")
            resto = fh.read(MAX_BYTES - len(cabeca) + 1)
    except PermissionError:
        raise FileError(403, "erro_arq_sem_permissao", "sem permissao de leitura")
    bruto = cabeca + resto
    cortou = len(bruto) > MAX_BYTES
    return {
        "path": path,
        "text": bruto[:MAX_BYTES].decode("utf-8", errors="replace"),
        "size": alvo.stat().st_size,
        "truncated": cortou,
    }
```

- [x] **Step 13: Rodar e ver passar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filetree.py -v`
Expected: 9 PASS.

- [x] **Step 14: Commit**

```bash
git add backend/app/filetree.py backend/tests/test_filetree.py
git commit -m "feat(arquivos): le o conteudo de um arquivo com teto e recusa de binario"
```

**Barra:** nenhuma — Task sem pixel.

---

### Task 2: `filesearch.py` — buscar por nome e por conteúdo

**Files:**
- Create: `backend/app/filesearch.py`
- Create: `backend/tests/test_filesearch.py`
- **Não toca `api.py`, `models.py` nem `filetree.py`.**

**Interfaces:**
- Consumes: `git_ops._run`.
- Produces:
  - `class SearchError(Exception)` com `.status`, `.code`, `.msg` (mesma forma do `FileError`)
  - `search(cwd: str, q: str, mode: str) -> dict`
    → `{"hits": [{"path", "line": int | None, "text": str | None}], "truncated": bool, "mode": str}`
  - `MAX_HITS = 200`

- [x] **Step 1: Escrever o teste que falha**

Criar `backend/tests/test_filesearch.py`:

```python
import pytest

from app import filesearch, git_ops
from app.filesearch import SearchError


def _repo(tmp_path):
    d = str(tmp_path)
    git_ops._run(d, "init", "-q", ".")
    git_ops._run(d, "config", "user.email", "t@t")
    git_ops._run(d, "config", "user.name", "t")
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "alvo.py").write_text("def buscar():\n    return 1\n")
    (tmp_path / ".gitignore").write_text("escondido.txt\n")
    (tmp_path / "escondido.txt").write_text("buscar")
    git_ops._run(d, "add", ".")
    git_ops._run(d, "commit", "-q", "-m", "base")
    return d


def test_nomes_acha_por_trecho_do_meio(tmp_path):
    d = _repo(tmp_path)
    paths = [h["path"] for h in filesearch.search(d, "alv", "names")["hits"]]
    assert "src/alvo.py" in paths


def test_nomes_respeita_gitignore(tmp_path):
    d = _repo(tmp_path)
    paths = [h["path"] for h in filesearch.search(d, "escondido", "names")["hits"]]
    assert paths == []


def test_conteudo_devolve_linha_e_numero(tmp_path):
    d = _repo(tmp_path)
    hits = filesearch.search(d, "def buscar", "contents")["hits"]
    assert hits[0]["path"] == "src/alvo.py" and hits[0]["line"] == 1


def test_sem_resultado_e_lista_vazia_nao_erro(tmp_path):
    """git grep sai com codigo 1 quando nao acha nada. Isso nao e falha."""
    d = _repo(tmp_path)
    r = filesearch.search(d, "coisaquenaoexiste", "contents")
    assert r["hits"] == [] and r["truncated"] is False


def test_termo_com_traco_nao_vira_flag(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "flags.txt").write_text("usa --force aqui\n")
    hits = filesearch.search(d, "--force", "contents")["hits"]
    assert any(h["path"] == "flags.txt" for h in hits)


def test_fora_de_repo_git_explica(tmp_path):
    with pytest.raises(SearchError) as e:
        filesearch.search(str(tmp_path), "x", "names")
    assert e.value.status == 409 and e.value.code == "erro_arq_nao_e_repo_git"


def test_q_vazio_recusado(tmp_path):
    d = _repo(tmp_path)
    with pytest.raises(SearchError) as e:
        filesearch.search(d, "   ", "names")
    assert e.value.code == "erro_arq_busca_vazia"
```

- [x] **Step 2: Rodar e ver falhar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filesearch.py -v`
Expected: FAIL com `ModuleNotFoundError: No module named 'app.filesearch'`.

- [x] **Step 3: Implementar**

Criar `backend/app/filesearch.py`:

```python
"""Busca por NOME e por CONTEUDO, os dois via git.

Por que git e nao os.walk + leitura: o `ls-files` ja respeita o .gitignore (senao a busca
mergulharia em node_modules) e o `grep -I` ja pula binario. Preco: os dois modos exigem um
repositorio git, e isso e dito na cara em vez de virar lista vazia.
"""

from app import git_ops

MAX_HITS = 200


class SearchError(Exception):
    def __init__(self, status: int, code: str, msg: str):
        super().__init__(msg)
        self.status, self.code, self.msg = status, code, msg


def _e_repo(cwd: str) -> bool:
    p = git_ops._run(cwd, "rev-parse", "--is-inside-work-tree")
    return p.returncode == 0 and p.stdout.strip() == "true"


def search(cwd: str, q: str, mode: str) -> dict:
    if not q or not q.strip():
        raise SearchError(400, "erro_arq_busca_vazia", "digite algo pra buscar")
    if mode not in ("names", "contents"):
        raise SearchError(400, "erro_arq_modo_invalido", "modo de busca invalido")
    if not _e_repo(cwd):
        # Vale pros DOIS modos: names usa ls-files, contents usa grep.
        raise SearchError(409, "erro_arq_nao_e_repo_git", "a busca precisa de um repositorio git")

    hits = (_por_nome if mode == "names" else _por_conteudo)(cwd, q)
    return {"hits": hits[:MAX_HITS], "truncated": len(hits) > MAX_HITS, "mode": mode}


def _por_nome(cwd: str, q: str) -> list[dict]:
    p = git_ops._run(cwd, "-c", "core.quotePath=false",
                     "ls-files", "-z", "--cached", "--others", "--exclude-standard")
    if p.returncode != 0:
        raise SearchError(409, "erro_arq_busca_falhou", (p.stderr or "git ls-files falhou").strip())
    alvo = q.lower()
    return [{"path": c, "line": None, "text": None}
            for c in p.stdout.split("\0") if c and alvo in c.lower()]


def _por_conteudo(cwd: str, q: str) -> list[dict]:
    # -e <termo>: a forma documentada de dizer "isto e o padrao, nao uma flag".
    p = git_ops._run(cwd, "-c", "core.quotePath=false",
                     "grep", "-n", "-I", "--untracked", "-F", "-e", q)
    if p.returncode == 1:                      # 1 = nao achou nada. NAO e erro.
        return []
    if p.returncode != 0:
        raise SearchError(409, "erro_arq_busca_falhou", (p.stderr or "git grep falhou").strip())
    fora = []
    for linha in p.stdout.splitlines():
        partes = linha.split(":", 2)
        if len(partes) == 3 and partes[1].isdigit():
            fora.append({"path": partes[0], "line": int(partes[1]), "text": partes[2]})
    return fora
```

- [x] **Step 4: Rodar e ver passar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filesearch.py -v`
Expected: 7 PASS.

- [x] **Step 5: Teste do teto e do acento**

```python
def test_corta_em_200(tmp_path):
    d = _repo(tmp_path)
    for i in range(210):
        (tmp_path / f"m{i}.txt").write_text("agulha\n")
    r = filesearch.search(d, "agulha", "contents")
    assert len(r["hits"]) == 200 and r["truncated"] is True


def test_acento_no_nome_nao_volta_escapado(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "sessão-única.md").write_text("x")
    paths = [h["path"] for h in filesearch.search(d, "sess", "names")["hits"]]
    assert "sessão-única.md" in paths
```

- [x] **Step 6: Rodar e ver passar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filesearch.py -v`
Expected: 9 PASS.

- [x] **Step 7: Commit**

```bash
git add backend/app/filesearch.py backend/tests/test_filesearch.py
git commit -m "feat(arquivos): busca por nome e por conteudo com teto e erro explicado"
```

**Barra:** nenhuma.

---

### Task 3: `path_diff` — o diff do arquivo com escopo, e o limite que falta

**Files:**
- Modify: `backend/app/git_ops.py` (funções novas depois de `commit_file_diff`, antes do `__main__`)
- Modify: `backend/tests/test_git_ops.py` (acrescentar ao fim)
- **Não toca `api.py`.**

**Interfaces:**
- Consumes: `_run`, `_cap`, `GitError` (existentes).
- Produces:
  - `path_diff(cwd: str, path: str, escopo: str) -> dict` →
    `{"path", "diff", "truncated", "escopo_pedido", "escopo_usado", "base", "motivo"}`
  - `_base_da_branch(cwd: str) -> str | None`
  - `escopo ∈ {"branch", "nao_commitado"}`

**Os quatro casos em que a base não existe** — o plano anterior errava aqui, e o mais comum é o
segundo:

1. Base = `git merge-base HEAD @{upstream}`; sem upstream, tenta `origin/HEAD`.
2. **Base igual ao HEAD** — a `main` em dia. A branch não tem commit próprio.
3. Sem upstream e sem `origin/HEAD`.
4. Repositório sem nenhum commit, ou `HEAD` solto (detached).

Nos quatro, `escopo_usado` volta `"nao_commitado"` com `motivo`, e é disso que a tela vive para
desabilitar a opção. **Nunca** devolver só `base: null` e deixar o front adivinhar.

- [x] **Step 1: Escrever o teste que falha**

Acrescentar ao fim de `backend/tests/test_git_ops.py`:

```python
def _repo_com_upstream(tmp_path):
    """Clone de um repo NORMAL que ja tem commit.

    Medido em 15/08/2026: clonar um `--bare` VAZIO nao serve — nao existe `origin/HEAD` nem
    `@{upstream}`, e `_base_da_branch` devolveria None, fazendo o escopo cair pra
    "nao_commitado" e contradizendo o proprio teste. Com um repo de origem que ja tem commit,
    a branch clonada tem upstream; depois de `checkout -b`, a branch nova NAO tem — e e o
    fallback `origin/HEAD` que responde, que e justamente o caminho a exercitar.
    """
    origem = tmp_path / "origem"
    origem.mkdir()
    o = str(origem)
    git_ops._run(o, "init", "-q", ".")
    git_ops._run(o, "config", "user.email", "t@t")
    git_ops._run(o, "config", "user.name", "t")
    (origem / "a.txt").write_text("um\n")
    git_ops._run(o, "add", "a.txt")
    git_ops._run(o, "commit", "-q", "-m", "um")

    git_ops._run(str(tmp_path), "clone", "-q", "origem", "trab")
    trab = tmp_path / "trab"
    d = str(trab)
    git_ops._run(d, "config", "user.email", "t@t")
    git_ops._run(d, "config", "user.name", "t")
    return d, trab / "a.txt"


def test_escopo_branch_soma_commits_e_disco(tmp_path):
    d, f = _repo_com_upstream(tmp_path)
    git_ops._run(d, "checkout", "-q", "-b", "trabalho")
    f.write_text("um\ndois\n")
    git_ops._run(d, "add", "a.txt")
    git_ops._run(d, "commit", "-q", "-m", "dois")
    f.write_text("um\ndois\ntres\n")                 # nao commitado
    r = git_ops.path_diff(d, "a.txt", "branch")
    assert r["escopo_usado"] == "branch"
    assert "dois" in r["diff"] and "tres" in r["diff"]
    so_disco = git_ops.path_diff(d, "a.txt", "nao_commitado")
    assert "tres" in so_disco["diff"] and "+dois" not in so_disco["diff"]


def test_branch_sem_commit_proprio_cai_e_diz(tmp_path):
    d, f = _repo_com_upstream(tmp_path)
    f.write_text("um\nmexido\n")
    r = git_ops.path_diff(d, "a.txt", "branch")
    assert r["escopo_pedido"] == "branch"
    assert r["escopo_usado"] == "nao_commitado"
    assert r["motivo"]


def test_repo_sem_commit_nao_estoura(tmp_path):
    d = str(tmp_path)
    git_ops._run(d, "init", "-q", ".")
    (tmp_path / "novo.txt").write_text("x\n")
    r = git_ops.path_diff(d, "novo.txt", "branch")
    assert r["escopo_usado"] == "nao_commitado"


def test_arquivo_novo_aparece_no_diff(tmp_path):
    """`git diff` NAO mostra untracked. Sem isto, clicar num arquivo recem-criado pela sessao
    abre um diff VAZIO — que e o caso mais comum de todos, porque a sessao acabou de criar o
    arquivo. `file_diff` ja resolve com `--no-index` (git_ops.py:338); `path_diff` precisa do
    mesmo tratamento."""
    d, _f = _repo_com_upstream(tmp_path)
    (tmp_path / "trab" / "novinho.txt").write_text("linha nova\n")
    r = git_ops.path_diff(d, "novinho.txt", "nao_commitado")
    assert "linha nova" in r["diff"]


def test_path_com_traco_recusado(tmp_path):
    d, _f = _repo_com_upstream(tmp_path)
    with pytest.raises(GitError):
        git_ops.path_diff(d, "--output=/tmp/x", "nao_commitado")
```

- [x] **Step 2: Rodar e ver falhar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_git_ops.py -k path_diff -v`
Expected: FAIL com `AttributeError: module 'app.git_ops' has no attribute 'path_diff'`.

- [x] **Step 3: Implementar**

Acrescentar a `backend/app/git_ops.py`, depois de `commit_file_diff`:

```python
def _base_da_branch(cwd: str) -> str | None:
    """Onde esta branch nasceu. None quando nao da pra saber (sem upstream, sem origin/HEAD,
    repo sem commit, HEAD solto) — o chamador decide o que fazer, e DIZ."""
    p = _run(cwd, "rev-parse", "--verify", "-q", "HEAD")
    if p.returncode != 0:                                  # repo sem nenhum commit
        return None
    for ref in ("@{upstream}", "origin/HEAD"):
        b = _run(cwd, "merge-base", "HEAD", ref)
        if b.returncode == 0 and b.stdout.strip():
            return b.stdout.strip()
    return None


def path_diff(cwd: str, path: str, escopo: str) -> dict:
    """Diff de UM arquivo. escopo="branch" soma desde onde a branch nasceu ate o disco agora."""
    if escopo not in ("branch", "nao_commitado"):
        raise GitError(400, "escopo invalido")
    if path.startswith("-"):
        raise GitError(400, "caminho invalido")

    usado, base, motivo = escopo, None, None
    if escopo == "branch":
        base = _base_da_branch(cwd)
        cabeca = _run(cwd, "rev-parse", "HEAD")
        atual = cabeca.stdout.strip() if cabeca.returncode == 0 else None
        if base is None:
            usado, motivo = "nao_commitado", "esta branch nao tem base conhecida"
        elif base == atual:
            usado, base, motivo = "nao_commitado", None, "esta branch nao tem commit proprio"

    # Arquivo ainda nao rastreado nao aparece em `git diff` — e e o caso MAIS comum aqui, porque
    # a sessao acabou de criar o arquivo. Mesmo tratamento que file_diff ja da (git_ops.py:338).
    untracked = any(c["path"] == path and c["code"] == "??" for c in changed_files(cwd))
    if untracked:
        p = _run(cwd, "-c", "core.quotePath=false", "diff", "--no-index", "/dev/null", path)
        texto, cortou = _cap(p.stdout)      # --no-index sai com 1 quando ha diferenca: normal
        return {"path": path, "diff": texto, "truncated": cortou,
                "escopo_pedido": escopo, "escopo_usado": usado, "base": base, "motivo": motivo}

    args = ["-c", "core.quotePath=false", "diff"]
    if usado == "branch":
        args.append(base)
    elif _run(cwd, "rev-parse", "--verify", "-q", "HEAD").returncode == 0:
        args.append("HEAD")                                # sem commit nenhum, diff sem ref
    args += ["--", path]

    p = _run(cwd, *args)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git diff falhou").strip() or "git diff falhou")
    texto, cortou = _cap(p.stdout)
    return {"path": path, "diff": texto, "truncated": cortou,
            "escopo_pedido": escopo, "escopo_usado": usado, "base": base, "motivo": motivo}
```

- [x] **Step 4: Rodar e ver passar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_git_ops.py -k path_diff -v`
Expected: 4 PASS.

- [x] **Step 5: Pôr `_cap` no `file_diff`, que hoje não tem teto**

O retorno atual é `return {"path": path, "diff": p.stdout}` (`git_ops.py:351`) — **a variável se
chama `p`, não `out`**. Trocar por, mantendo a chave `diff` que o `GitChangesTab` já consome:

```python
    texto, cortou = _cap(p.stdout)
    return {"path": path, "diff": texto, "truncated": cortou}
```

**Leia a função inteira antes de editar**: se o nome da variável tiver mudado, é ele que manda, não
este plano.

- [x] **Step 6: Teste que prova que o caller irmão não quebrou**

```python
def test_file_diff_mantem_o_formato_do_git_changes_tab(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("linha modificada\n")
    r = git_ops.file_diff(d, "tracked.txt")
    assert set(r) >= {"path", "diff"}          # o que o front ja lia
    assert r["truncated"] is False             # o campo novo
```

- [x] **Step 7: Rodar a suíte inteira do backend**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest -v`
Expected: tudo verde, sem aviso novo.

- [x] **Step 8: Commit**

```bash
git add backend/app/git_ops.py backend/tests/test_git_ops.py
git commit -m "feat(git): diff por arquivo com escopo de branch e teto de tamanho"
```

**Barra:** nenhuma.

---

## COSTURA 1 — serial

### Task 4: As rotas, os modelos e as chaves de erro

**Files:**
- Modify: `backend/app/api.py` (rotas novas depois de `git/commit/{sha}/branches`, `api.py:3115`)
- Modify: `frontend/messages/pt.json`, `frontend/messages/en.json`
- Modify: `frontend/src/lib/errosApi.ts`
- Modify: `backend/tests/test_filetree.py`, `backend/tests/test_filesearch.py`

**`models.py` NÃO é tocado**, e isso é decisão, não esquecimento: as três rotas de leitura devolvem
o `dict` que os módulos já montam, e o único corpo de entrada (`GitPathDiffBody`) nasce em `api.py`,
ao lado dos outros `_StrictBody`. Modelo Pydantic aqui seria uma segunda definição do mesmo formato,
para manter em dois lugares.

**Interfaces:**
- Consumes: `filetree.list_dir/read_file/FileError`, `filesearch.search/SearchError`,
  `git_ops.path_diff`, `_session_cwd` (`api.py:2855`), `_StrictBody` (`api.py:932`),
  `mensagens.erro` (`mensagens.py:16`).
- Produces:
  - `GET /api/sessions/{name}/files/list?path=&so_modificados=`
  - `GET /api/sessions/{name}/files/read?path=`
  - `GET /api/sessions/{name}/files/search?q=&mode=`
  - `POST /api/sessions/{name}/git/path-diff` (body `{path, escopo}`)

- [x] **Step 1: Escrever o teste de rota que falha**

Acrescentar a `backend/tests/test_filetree.py`:

**Antes de escrever:** abra `backend/tests/test_api.py` e copie a fixture que arma o token — hoje
ela faz `settings.auth_token = "secret"` (`test_api.py:14`). **Sem ela, `Bearer secret` devolve 401**
e o teste do envelope nunca chega a 415. `test_filetree.py` não herda fixture de outro arquivo.

```python
import pytest
from fastapi.testclient import TestClient

from app.config import settings


@pytest.fixture
def cliente():
    """Mesmo arranjo de test_api.py: sem armar o token, toda rota devolve 401."""
    anterior = settings.auth_token
    settings.auth_token = "secret"
    yield TestClient(__import__("app.main", fromlist=["app"]).app)
    settings.auth_token = anterior


def test_rota_list_exige_auth(cliente):
    assert cliente.get("/api/sessions/x/files/list").status_code == 401


def test_rota_binario_devolve_envelope(monkeypatch, tmp_path, cliente):
    from app import api
    d = _repo(tmp_path)
    (tmp_path / "i.png").write_bytes(b"\x89PNG\x00bin")
    monkeypatch.setattr(api, "_session_cwd", lambda name: d)
    r = cliente.get("/api/sessions/s/files/read", params={"path": "i.png"},
                    headers={"Authorization": "Bearer secret"})
    assert r.status_code == 415
    assert r.json()["detail"]["code"] == "erro_arq_binario"     # envelope, nao texto solto
```

- [x] **Step 2: Rodar e ver falhar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filetree.py -k rota -v`
Expected: FAIL 404 (a rota ainda não existe).

- [x] **Step 3: Implementar as quatro rotas**

Acrescentar a `backend/app/api.py`, depois da última rota de git:

```python
# ATENCAO: api.py importa NOMES de git_ops (`from app.git_ops import list_branches, ...`,
# api.py:57-59) — o modulo `git_ops` NAO existe como nome ali. Sem esta linha, `git_ops.path_diff`
# e NameError. Ou acrescente o import do modulo, ou importe a funcao pelo nome; o plano usa o
# modulo pra deixar obvio de onde ela vem.
from app import filesearch, filetree, git_ops
from app.filesearch import SearchError
from app.filetree import FileError
from app.mensagens import erro


class GitPathDiffBody(_StrictBody):
    path: str
    escopo: Literal["branch", "nao_commitado"] = "branch"


def _erro_arq(e: FileError | SearchError) -> HTTPException:
    # O `msg` vai TAMBEM como parametro: as chaves `erro_arq_busca_falhou` e `erro_git_diff`
    # trazem `{msg}` no texto, e a funcao do paraglide exige o argumento — sem ele o front
    # renderiza `undefined` ou nem compila.
    return HTTPException(status_code=e.status, detail=erro(e.code, e.msg, msg=e.msg))


@app.get("/api/sessions/{name}/files/list", dependencies=[Depends(require_auth)])
def files_list(name: str, path: str | None = None, so_modificados: bool = True):
    try:
        return filetree.list_dir(_session_cwd(name), path, so_modificados)
    except FileError as e:
        raise _erro_arq(e)


@app.get("/api/sessions/{name}/files/read", dependencies=[Depends(require_auth)])
def files_read(name: str, path: str):
    try:
        return filetree.read_file(_session_cwd(name), path)
    except FileError as e:
        raise _erro_arq(e)


@app.get("/api/sessions/{name}/files/search", dependencies=[Depends(require_auth)])
def files_search(name: str, q: str, mode: Literal["names", "contents"] = "names"):
    try:
        return filesearch.search(_session_cwd(name), q, mode)
    except SearchError as e:
        raise _erro_arq(e)


@app.post("/api/sessions/{name}/git/path-diff", dependencies=[Depends(require_auth)])
def git_path_diff(name: str, body: GitPathDiffBody):
    try:
        return git_ops.path_diff(_session_cwd(name), body.path, body.escopo)
    except GitError as e:
        raise HTTPException(status_code=e.status, detail=erro("erro_git_diff", str(e)))
```

- [x] **Step 4: Rodar e ver passar**

Run: `uv run --directory /home/jefferson/pessoal/hangar/backend pytest tests/test_filetree.py tests/test_filesearch.py -v`
Expected: tudo verde.

- [x] **Step 5: As chaves de erro nas duas línguas**

Acrescentar a `frontend/messages/pt.json` e `frontend/messages/en.json` (as mesmas chaves nos dois):

```json
{
  "erro_arq_fora_da_raiz": "Esse caminho sai da pasta da sessão.",
  "erro_arq_inexistente": "Esse arquivo não existe mais.",
  "erro_arq_e_pasta": "Isso é uma pasta, não um arquivo.",
  "erro_arq_nao_e_pasta": "Isso é um arquivo, não uma pasta.",
  "erro_arq_binario": "Arquivo binário — não dá pra mostrar aqui.",
  "erro_arq_sem_permissao": "Sem permissão para ler esse arquivo.",
  "erro_arq_nao_e_repo_git": "A busca precisa de um repositório git.",
  "erro_arq_busca_vazia": "Digite algo para buscar.",
  "erro_arq_busca_falhou": "A busca falhou: {msg}",
  "erro_arq_modo_invalido": "Modo de busca inválido.",
  "erro_git_diff": "Não consegui montar o diff: {msg}"
}
```

Inglês correspondente (`en.json`), mesma ordem:

```json
{
  "erro_arq_fora_da_raiz": "That path is outside the session folder.",
  "erro_arq_inexistente": "That file no longer exists.",
  "erro_arq_e_pasta": "That's a folder, not a file.",
  "erro_arq_nao_e_pasta": "That's a file, not a folder.",
  "erro_arq_binario": "Binary file — can't show it here.",
  "erro_arq_sem_permissao": "No permission to read that file.",
  "erro_arq_nao_e_repo_git": "Search needs a git repository.",
  "erro_arq_busca_vazia": "Type something to search for.",
  "erro_arq_busca_falhou": "Search failed: {msg}",
  "erro_arq_modo_invalido": "Invalid search mode.",
  "erro_git_diff": "Couldn't build the diff: {msg}"
}
```

- [x] **Step 6: Reconhecer os códigos no `errosApi.ts`**

Seguir o padrão que já está em `frontend/src/lib/errosApi.ts:216` (`mensagemDeErro`), acrescentando
os códigos novos ao mapa que ele consulta. **Ler a função antes de editar** — o formato do mapa é o
que manda, não este plano.

- [x] **Step 7: Rodar os três portões**

```bash
uv run --directory /home/jefferson/pessoal/hangar/backend pytest -v
npm --prefix /home/jefferson/pessoal/hangar/frontend run check
npm --prefix /home/jefferson/pessoal/hangar/frontend run test
```
Expected: tudo verde. Chave faltando em `en.json` aparece aqui.

- [x] **Step 8: Commit**

```bash
git add backend/app/api.py backend/tests/test_filetree.py \
        backend/tests/test_filesearch.py frontend/messages/pt.json \
        frontend/messages/en.json frontend/src/lib/errosApi.ts
git commit -m "feat(arquivos): expoe listar, ler, buscar e diff por caminho nas rotas da sessao"
```

**Barra:** nenhuma.

---

## COSTURA 2 — serial

### Task 5: Clientes, tipos e as chaves da tela

**Files:**
- Modify: `frontend/src/lib/api.ts`, `frontend/src/lib/types.ts`
- Modify: `frontend/messages/pt.json`, `frontend/messages/en.json`

**Interfaces:**
- Produces: `listFiles`, `readFile`, `searchFiles`, `pathDiff`; tipos `TreeEntry`, `TreeListing`,
  `FileContent`, `SearchHit`, `SearchResult`, `PathDiff`; chaves `arq_*`.

- [x] **Step 1: Os tipos**

Acrescentar a `frontend/src/lib/types.ts`:

```ts
export interface TreeEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  changed: 'M' | 'A' | 'D' | '?' | null;
  add: number;
  del: number;
}
export interface TreeListing { entries: TreeEntry[]; truncated: boolean }
export interface FileContent { path: string; text: string; size: number; truncated: boolean }
export interface SearchHit { path: string; line: number | null; text: string | null }
export interface SearchResult { hits: SearchHit[]; truncated: boolean; mode: 'names' | 'contents' }
export interface PathDiff {
  path: string; diff: string; truncated: boolean;
  escopo_pedido: 'branch' | 'nao_commitado';
  escopo_usado: 'branch' | 'nao_commitado';
  base: string | null;
  motivo: string | null;
}
```

- [x] **Step 2: Os clientes**

Acrescentar a `frontend/src/lib/api.ts`, seguindo o formato dos vizinhos (`getFileDiff`, linha ~935
— **ler antes de escrever**, o helper de fetch e o tratamento de erro são de lá):

O helper do projeto é **`apiFetch`** (`api.ts:110`) — **não existe `req`**. Ele já põe o
`Content-Type`, o cabeçalho de autenticação e passa por `ensureOk`; querystring vai montada na URL e
corpo vai como **string** já serializada. A forma do vizinho é `getFileDiff` (`api.ts:950`).

```ts
export function listFiles(name: string, path?: string, soModificados = true): Promise<TreeListing> {
  const q = new URLSearchParams({ so_modificados: String(soModificados) });
  if (path) q.set('path', path);
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/files/list?${q}`);
}

export function readFile(name: string, path: string): Promise<FileContent> {
  const q = new URLSearchParams({ path });
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/files/read?${q}`);
}

export function searchFiles(name: string, q: string, mode: 'names' | 'contents'): Promise<SearchResult> {
  const qs = new URLSearchParams({ q, mode });
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/files/search?${qs}`);
}

export function pathDiff(name: string, path: string, escopo: 'branch' | 'nao_commitado'): Promise<PathDiff> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/path-diff`, {
    method: 'POST',
    body: JSON.stringify({ path, escopo }),
  });
}
```

- [x] **Step 3: As chaves da tela, nas duas línguas**

`pt.json` / `en.json`:

```json
{
  "arq_aba": "Arquivos",
  "arq_buscar": "Buscar arquivos",
  "arq_modo_nomes": "Nomes",
  "arq_modo_conteudo": "Conteúdo",
  "arq_so_modificados": "Só os modificados",
  "arq_mostrar_tudo": "mostrar tudo",
  "arq_mostrar_so_modificados": "Mostrar só os modificados",
  "arq_recarregar": "Recarregar",
  "arq_ordenar_nome": "Nome",
  "arq_sem_nome": "Nenhum arquivo com esse nome.",
  "arq_sem_conteudo": "Nenhuma linha com esse texto.",
  "arq_primeiros_200": "Mostrando os primeiros 200 resultados.",
  "arq_pasta_grande": "Pasta grande: mostrando os primeiros 1000 itens.",
  "arq_nada_mudou": "Nada mudou nesta sessão.",
  "arq_sessao_encerrada": "Esta sessão foi encerrada.",
  "arq_voltar_conversa": "voltar à conversa",
  "arq_fechar": "Fechar o arquivo",
  "arq_escopo_branch": "Nesta branch",
  "arq_escopo_nao_commitado": "Não commitado",
  "arq_escopo_desde": "desde {base}",
  "arq_meta_arquivo": "{tam} · {linhas} linhas",
  "arq_diff_cortado": "Diff cortado em 200 KB.",
  "arq_arquivo_cortado": "Arquivo cortado em 512 KB.",
  "arq_sem_mudanca": "Este arquivo não mudou — mostrando o conteúdo."
}
```

```json
{
  "arq_aba": "Files",
  "arq_buscar": "Search files",
  "arq_modo_nomes": "Names",
  "arq_modo_conteudo": "Contents",
  "arq_so_modificados": "Changed only",
  "arq_mostrar_tudo": "show all",
  "arq_mostrar_so_modificados": "Show changed only",
  "arq_recarregar": "Reload",
  "arq_ordenar_nome": "Name",
  "arq_sem_nome": "No file with that name.",
  "arq_sem_conteudo": "No line with that text.",
  "arq_primeiros_200": "Showing the first 200 results.",
  "arq_pasta_grande": "Large folder: showing the first 1000 items.",
  "arq_nada_mudou": "Nothing changed in this session.",
  "arq_sessao_encerrada": "This session has ended.",
  "arq_voltar_conversa": "back to the conversation",
  "arq_fechar": "Close the file",
  "arq_escopo_branch": "On this branch",
  "arq_escopo_nao_commitado": "Uncommitted",
  "arq_escopo_desde": "since {base}",
  "arq_meta_arquivo": "{tam} · {linhas} lines",
  "arq_diff_cortado": "Diff truncated at 200 KB.",
  "arq_arquivo_cortado": "File truncated at 512 KB.",
  "arq_sem_mudanca": "This file hasn't changed — showing its contents."
}
```

- [x] **Step 4: Rodar os portões do front**

```bash
npm --prefix /home/jefferson/pessoal/hangar/frontend run check
npm --prefix /home/jefferson/pessoal/hangar/frontend run test
```
Expected: verde. As chaves viram funções tipadas em `src/paraglide/messages`.

- [x] **Step 5: Commit**

```bash
git add frontend/src/lib/api.ts frontend/src/lib/types.ts \
        frontend/messages/pt.json frontend/messages/en.json
git commit -m "feat(arquivos): clientes, tipos e as chaves das duas linguas"
```

**Barra:** nenhuma.

---

## LOTE B — paralelo, quatro worktrees

Os quatro verificam por vitest, sem tela montada — é o que os torna paralelizáveis.

### Como se escreve teste de componente NESTE projeto

Conferido em 15/08/2026. **`@testing-library` não está instalado** (`frontend/package.json` tem só
`happy-dom`), então nada de `render`, `screen`, `userEvent`, `toBeInTheDocument` ou `toBeDisabled` —
esses testes não rodariam. O padrão real, copiado de
`frontend/src/components/DesktopSessionContext.test.ts`:

```ts
// @vitest-environment happy-dom          <- PRIMEIRA linha do arquivo; sem ela, environment é 'node'
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import MeuComponente from './MeuComponente.svelte';
import { overwriteGetLocale } from '../../paraglide/runtime';

// Mock de módulo é `vi.mock`, NUNCA `vi.spyOn` num export const: o namespace de um módulo ES é
// somente leitura, e o spy estoura "Cannot redefine property".
vi.mock('../../lib/api', () => ({
  listFiles: vi.fn(),
  readFile: vi.fn(),
}));

function montar(props: Record<string, unknown>) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(MeuComponente, { target: el, props });
  return { el, comp };
}

beforeEach(() => overwriteGetLocale(() => 'pt'));   // sem isto o texto sai no idioma do sistema
```

Asserções são sobre o DOM cru: `el.querySelectorAll('.no').length`, `.getAttribute('aria-expanded')`,
`.textContent`, `(botao as HTMLButtonElement).disabled`. Clique é `.dispatchEvent(new MouseEvent(
'click', { bubbles: true }))` seguido de `await tick()`. Sempre `unmount(comp)` no fim.

### Task 6: `FileTree.svelte` — a árvore

**Files:**
- Create: `frontend/src/components/files/FileTree.svelte`
- Create: `frontend/src/components/files/FileTree.test.ts`

**Interfaces:**
- Consumes: `TreeEntry` (Task 5), `m.arq_*` (Task 5).
- Produces: componente com props
  `{ entries: TreeEntry[]; abertos: Set<string>; selecionado: string | null; onToggle: (p: string) => void; onPick: (p: string) => void }`.
  **Apresentacional puro**, como o `DiffView` — não busca nada.

- [x] **Step 1: Escrever o teste que falha**

```ts
// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import FileTree from './FileTree.svelte';
import { overwriteGetLocale } from '../../paraglide/runtime';

const ent = (o: Record<string, unknown> = {}) => ({
  name: 'a.txt', path: 'a.txt', is_dir: false, size: 1,
  changed: 'M', add: 4, del: 2, ...o,
});

function montar(props: Record<string, unknown>) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return { el, comp: mount(FileTree, { target: el, props }) };
}

const base = { abertos: new Set<string>(), selecionado: null, onToggle: vi.fn(), onPick: vi.fn() };

describe('FileTree', () => {
  beforeEach(() => overwriteGetLocale(() => 'pt'));

  it('pasta tem aria-expanded, arquivo nao', () => {
    const { el, comp } = montar({
      ...base,
      entries: [ent({ name: 'src', path: 'src', is_dir: true }), ent()],
      abertos: new Set(['src']),
    });
    const linhas = el.querySelectorAll('.no');
    expect(linhas[0].getAttribute('aria-expanded')).toBe('true');
    expect(linhas[1].getAttribute('aria-expanded')).toBeNull();
    unmount(comp);
  });

  it('mostra +N -M e some quando nao mudou', () => {
    const { el, comp } = montar({
      ...base,
      entries: [ent(), ent({ name: 'b.txt', path: 'b.txt', changed: null, add: 0, del: 0 })],
    });
    const nums = [...el.querySelectorAll('.num')].map((n) => n.textContent?.trim());
    expect(nums[0]).toContain('+4');
    expect(nums[1]).toBe('');
    unmount(comp);
  });

  it('clique em arquivo chama onPick com o caminho', async () => {
    const onPick = vi.fn();
    const { el, comp } = montar({ ...base, entries: [ent()], onPick });
    el.querySelector('.no')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await tick();
    expect(onPick).toHaveBeenCalledWith('a.txt');
    unmount(comp);
  });
});
```

- [x] **Step 2: Rodar e ver falhar**

Run: `npm --prefix /home/jefferson/pessoal/hangar/frontend run test -- FileTree`
Expected: FAIL — o componente não existe.

- [x] **Step 3: Implementar**

Criar `frontend/src/components/files/FileTree.svelte`. Regras do desenho, todas visíveis no mock
(`docs/mocks/2026-08-15-arvore/1-desktop-painel.html` e `base.css` — **ler os dois**):

- chevron só em pasta; ícone à esquerda do nome; recuo de 14px por nível, 8px de base
- nome com `overflow: hidden; text-overflow: ellipsis; white-space: nowrap`
- `+N −M` em fonte mono 10.5px, **antes** da marca; some quando `add === 0 && del === 0`
- marca na extrema direita, largura fixa de 12px: `M` âmbar (`--warning`), `A` verde
  (`--success`), `D` vermelho (`--error`), `?` cinza (`--text-muted`)
- linha selecionada em `--accent-dim`; `:hover` em `--bg-hover`
- fundo `transparent` — quem carrega o material é o painel
- teclado: `↑`/`↓` andam, `→` abre, `←` fecha, `Enter` escolhe
- todo `aria-label`/`title` sai de `m.arq_*`

- [x] **Step 4: Rodar e ver passar**

Run: `npm --prefix /home/jefferson/pessoal/hangar/frontend run test -- FileTree`
Expected: 3 PASS.

- [x] **Step 5: Rodar o `check` e commitar**

```bash
npm --prefix /home/jefferson/pessoal/hangar/frontend run check
git add frontend/src/components/files/FileTree.svelte frontend/src/components/files/FileTree.test.ts
git commit -m "feat(arquivos): componente da arvore com marca, contador e teclado"
```

**Barra:** o **HTML** `docs/mocks/2026-08-15-arvore/1-desktop-painel.html` + `base.css`, classes
`.no`, `.chev`, `.ico`, `.nome`, `.num`, `.marca`. Sem comparação cega aqui — o componente não monta
numa tela sozinho. A comparação de print acontece na Task 10, contra
`prints/1-desktop-painel.png`.

---

### Task 7: `FileViewer.svelte` — diff ou conteúdo

**Files:**
- Create: `frontend/src/components/files/FileViewer.svelte`
- Create: `frontend/src/components/files/FileViewer.test.ts`

**Interfaces:**
- Consumes: `DiffView.svelte` (existente), `PathDiff`, `FileContent`, `m.arq_*`.
- Produces: componente com props
  `{ path: string; diff: PathDiff | null; conteudo: FileContent | null; loading: boolean; onEscopo: (e: 'branch' | 'nao_commitado') => void; onFechar: () => void }`.

**Por que é componente novo e não "reusar o `DiffView`":** o `DiffView` só aceita `rows: DiffRow[]` —
ele não sabe mostrar conteúdo de arquivo. O `FileViewer` **usa** o `DiffView` quando há diff, e mostra
o conteúdo quando não há.

**Duas coisas que a auditoria do plano pegou, e que mudam o desenho deste componente:**

1. **O `DiffView` já desenha um cabeçalho** com o caminho e o `+N −M`
   (`DiffView.svelte:23-27`, classes `.git-diff-head`, `.git-diff-name`, `.git-diff-stat`). Se o
   `FileViewer` desenhar o dele por cima, ficam **dois cabeçalhos empilhados** dizendo a mesma
   coisa. Decisão: o `FileViewer` desenha o cabeçalho **de cima** (caminho, `+N −M`, fechar) e a
   linha de controles (escopo, meta, voltar); o cabeçalho interno do `DiffView` fica **escondido por
   CSS** dentro do `FileViewer` (`:global(.git-diff-head) { display: none }` no escopo dele) —
   **sem** alterar o `DiffView`, que continua servindo o `GitChangesTab` como está hoje. Alternativa
   aceitável: dar ao `DiffView` uma prop `semCabecalho`, se o revisor preferir explícito a CSS.
2. **A string de diff não é `DiffRow[]`.** Quem converte é
   `highlightDiff(diffText: string, path: string): Promise<DiffRow[]>` (`highlight.ts:120`), e ela é
   **assíncrona** (import dinâmico do Shiki). O `FileViewer` recebe a `PathDiff` com `diff` em
   texto e chama `highlightDiff` num `$effect`, guardando as linhas em estado — enquanto não voltam,
   passa `loading` ao `DiffView`. Sem isso, o componente não desenha nada e não dá erro.

- [x] **Step 1: Escrever o teste que falha**

```ts
// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount } from 'svelte';
import FileViewer from './FileViewer.svelte';
import { overwriteGetLocale } from '../../paraglide/runtime';

const base = { path: 'a.py', loading: false, onEscopo: vi.fn(), onFechar: vi.fn() };

function montar(props: Record<string, unknown>) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return { el, comp: mount(FileViewer, { target: el, props: { ...base, ...props } }) };
}

describe('FileViewer', () => {
  beforeEach(() => overwriteGetLocale(() => 'pt'));

  it('sem mudanca, mostra o conteudo', () => {
    const { el, comp } = montar({
      diff: null,
      conteudo: { path: 'a.py', text: 'print(1)\n', size: 9, truncated: false },
    });
    expect(el.textContent).toContain('print(1)');
    unmount(comp);
  });

  it('escopo que caiu aparece desabilitado com o motivo', () => {
    const { el, comp } = montar({
      conteudo: null,
      diff: {
        path: 'a.py', diff: '@@ -1 +1 @@\n-a\n+b\n', truncated: false,
        escopo_pedido: 'branch', escopo_usado: 'nao_commitado',
        base: null, motivo: 'esta branch nao tem commit proprio',
      },
    });
    const b = el.querySelector('.escopo') as HTMLButtonElement;
    expect(b.disabled).toBe(true);
    expect(b.title).toContain('commit');
    unmount(comp);
  });

  it('diff cortado mostra o aviso', () => {
    const { el, comp } = montar({
      conteudo: null,
      diff: {
        path: 'a.py', diff: '@@ -1 +1 @@\n+x\n', truncated: true,
        escopo_pedido: 'branch', escopo_usado: 'branch', base: 'abc1234', motivo: null,
      },
    });
    expect(el.textContent).toContain('200 KB');
    unmount(comp);
  });
});
```

- [x] **Step 2: Rodar e ver falhar**

Run: `npm --prefix /home/jefferson/pessoal/hangar/frontend run test -- FileViewer`
Expected: FAIL — o componente não existe.

- [x] **Step 3: Implementar**

Criar `frontend/src/components/files/FileViewer.svelte`, seguindo
`docs/mocks/2026-08-15-arvore/2-desktop-visualizador.html`:

- cabeçalho em duas linhas. Primeira: caminho em mono com a **pasta em `--text-muted`** e o nome do
  arquivo em `--text-primary`, `+N −M` à direita, botão de fechar. Segunda: seletor de escopo,
  `m.arq_escopo_desde({ base })`, `m.arq_meta_arquivo({ tam, linhas })`, e o link
  `m.arq_voltar_conversa()` à direita.
- corpo: `DiffView` quando há `diff`; senão o conteúdo em `<pre>` com `--surface-inset`.
- `escopo_usado !== escopo_pedido` → botão do escopo pedido desabilitado, `title` = `motivo`.
- `truncated` → `m.arq_diff_cortado()` ou `m.arq_arquivo_cortado()`.

- [x] **Step 4: Rodar e ver passar**

Run: `npm --prefix /home/jefferson/pessoal/hangar/frontend run test -- FileViewer`
Expected: 3 PASS.

- [x] **Step 5: `check` e commit**

```bash
npm --prefix /home/jefferson/pessoal/hangar/frontend run check
git add frontend/src/components/files/FileViewer.svelte frontend/src/components/files/FileViewer.test.ts
git commit -m "feat(arquivos): visualizador que mostra diff ou conteudo, com seletor de escopo"
```

**Barra:** o **HTML** `docs/mocks/2026-08-15-arvore/2-desktop-visualizador.html` + `base.css`,
classes `.cab`, `.caminho`, `.stat`, `.escopo`, `.meta`, `.voltar`, `.diff`, `.dl`, `.dn`, `.dt`.
Sem comparação cega aqui — ela acontece na Task 11, contra `prints/2-desktop-visualizador.png`.

---

### Task 8: `FileSearchBar.svelte` — o campo e o segmentado

**Files:**
- Create: `frontend/src/components/files/FileSearchBar.svelte`
- Create: `frontend/src/components/files/FileSearchBar.test.ts`

**Interfaces:**
- Produces: componente com props
  `{ q: string; mode: 'names' | 'contents'; onBusca: (q: string, mode: 'names' | 'contents') => void }`.
  O atraso de digitação (250ms) mora **aqui**; quem chama a rede é o store.

- [x] **Step 1: Escrever o teste que falha**

```ts
// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import FileSearchBar from './FileSearchBar.svelte';
import { overwriteGetLocale } from '../../paraglide/runtime';

function montar(props: Record<string, unknown>) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return { el, comp: mount(FileSearchBar, { target: el, props }) };
}

describe('FileSearchBar', () => {
  beforeEach(() => overwriteGetLocale(() => 'pt'));
  afterEach(() => vi.useRealTimers());

  it('espera 250ms antes de avisar', async () => {
    vi.useFakeTimers();
    const onBusca = vi.fn();
    const { el, comp } = montar({ q: '', mode: 'names', onBusca });
    const campo = el.querySelector('input') as HTMLInputElement;
    campo.value = 'abc';
    campo.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    expect(onBusca).not.toHaveBeenCalled();
    vi.advanceTimersByTime(260);
    expect(onBusca).toHaveBeenCalledWith('abc', 'names');
    unmount(comp);
  });

  it('trocar de aba refaz a busca sem limpar o campo', async () => {
    const onBusca = vi.fn();
    const { el, comp } = montar({ q: 'abc', mode: 'names', onBusca });
    const abas = el.querySelectorAll('.seg button');
    abas[1].dispatchEvent(new MouseEvent('click', { bubbles: true }));   // "Conteúdo"
    await tick();
    expect(onBusca).toHaveBeenCalledWith('abc', 'contents');
    expect((el.querySelector('input') as HTMLInputElement).value).toBe('abc');
    unmount(comp);
  });
});
```

- [x] **Step 2: Rodar e ver falhar**

Run: `npm --prefix /home/jefferson/pessoal/hangar/frontend run test -- FileSearchBar`
Expected: FAIL — o componente não existe.

- [x] **Step 3: Implementar**

Seguir o mock (`base.css`, classes `.busca` e `.seg`): campo com lupa em `--surface-inset`, e logo
abaixo o segmentado de duas colunas com `--fill-subtle` de trilho e `--surface-raised` na escolhida.
Rótulos: `m.arq_buscar()`, `m.arq_modo_nomes()`, `m.arq_modo_conteudo()`.

- [x] **Step 4: Rodar e ver passar**

Run: `npm --prefix /home/jefferson/pessoal/hangar/frontend run test -- FileSearchBar`
Expected: 2 PASS.

- [x] **Step 5: `check` e commit**

```bash
npm --prefix /home/jefferson/pessoal/hangar/frontend run check
git add frontend/src/components/files/FileSearchBar.svelte frontend/src/components/files/FileSearchBar.test.ts
git commit -m "feat(arquivos): barra de busca com as abas Nomes e Conteudo"
```

**Barra:** o **HTML** do mock, classes `.busca` e `.seg` em `base.css`. Sem comparação cega aqui —
o segmentado é julgado dentro da tela montada, na Task 10.

---

### Task 9: `filesStore.svelte.ts` — o estado

**Files:**
- Create: `frontend/src/lib/filesStore.svelte.ts`
- Create: `frontend/src/lib/filesStore.test.ts`

**Interfaces:**
- Consumes: `listFiles`, `readFile`, `searchFiles`, `pathDiff` (Task 5).
- Produces: `class FilesStore` com
  `abertos: Set<string>`, `selecionado: string | null`, `entries: TreeEntry[]`, `erro: string | null`,
  `soModificados: boolean`, e os métodos
  `abrir(path)`, `alternarPasta(path)`, `buscar(q, mode)`, `recarregar()`, `trocarEscopo(e)`.

- [x] **Step 1: Escrever o teste que falha**

`vi.spyOn(api, 'readFile')` **não funciona** aqui: o namespace de um módulo ES é somente leitura e
o spy estoura `Cannot redefine property`. O padrão do projeto é `vi.mock` do módulo inteiro
(precedente em `frontend/src/lib/PushQuiet.test.ts`).

```ts
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { listFiles, readFile, searchFiles } from './api';
import { FilesStore } from './filesStore.svelte';

vi.mock('./api', () => ({
  listFiles: vi.fn(),
  readFile: vi.fn(),
  searchFiles: vi.fn(),
  pathDiff: vi.fn(),
}));

describe('FilesStore', () => {
  beforeEach(() => vi.clearAllMocks());

  it('resposta atrasada de um alvo abandonado e descartada', async () => {
    let libera: (v: unknown) => void = () => {};
    vi.mocked(readFile)
      .mockImplementationOnce(() => new Promise((r) => (libera = r)) as never)
      .mockResolvedValueOnce({ path: 'b.txt', text: 'B', size: 1, truncated: false });
    const s = new FilesStore('sessao');
    const primeiro = s.abrir('a.txt');
    await s.abrir('b.txt');
    libera({ path: 'a.txt', text: 'A', size: 1, truncated: false });
    await primeiro;
    expect(s.conteudo?.text).toBe('B');       // o primeiro nao pinta por cima
  });

  it('guarda pasta aberta por sessao', async () => {
    vi.mocked(listFiles).mockResolvedValue({ entries: [], truncated: false });
    const s = new FilesStore('sessao');
    await s.alternarPasta('src');
    expect(s.abertos.has('src')).toBe(true);
    await s.alternarPasta('src');
    expect(s.abertos.has('src')).toBe(false);
  });

  it('erro do backend vira mensagem, nao excecao solta', async () => {
    vi.mocked(searchFiles).mockRejectedValue(
      { detail: { code: 'erro_arq_nao_e_repo_git', params: {}, msg: 'x' } });
    const s = new FilesStore('sessao');
    await s.buscar('x', 'contents');
    expect(s.erro).toBeTruthy();
  });
});
```

- [x] **Step 2: Rodar e ver falhar**

Run: `npm --prefix /home/jefferson/pessoal/hangar/frontend run test -- filesStore`
Expected: FAIL — o módulo não existe.

- [x] **Step 3: Implementar**

A corrida se resolve com um contador de pedido: cada chamada guarda o número dela, e ao voltar só
pinta se ainda for a mais recente. Erro passa por `formataErro` (`frontend/src/lib/errosApi.ts:45`),
que já sabe ler o envelope `{code, params, msg}`. O estado é guardado por nome de sessão.

- [x] **Step 4: Rodar e ver passar**

Run: `npm --prefix /home/jefferson/pessoal/hangar/frontend run test -- filesStore`
Expected: 3 PASS.

- [x] **Step 5: `check` e commit**

```bash
npm --prefix /home/jefferson/pessoal/hangar/frontend run check
git add frontend/src/lib/filesStore.svelte.ts frontend/src/lib/filesStore.test.ts
git commit -m "feat(arquivos): estado por sessao, com descarte de resposta atrasada"
```

**Barra:** nenhuma — não desenha nada.

---

## MONTAGEM — serial

### Task 10: A aba no painel do desktop  ⟨mexe em pixel⟩

**Files:**
- Create: `frontend/src/components/files/FilesPanel.svelte`
- Modify: `frontend/src/components/DesktopSessionContext.svelte`
- Modify: `frontend/src/components/DesktopSessionContext.test.ts`

**Interfaces:**
- Consumes: `FileTree` (Task 6, props
  `{entries, abertos, selecionado, onToggle, onPick}`), `FileSearchBar` (Task 8, props
  `{q, mode, onBusca}`), `FilesStore` (Task 9, campos `abertos`, `selecionado`, `entries`, `erro`,
  `soModificados`; métodos `abrir`, `alternarPasta`, `buscar`, `recarregar`, `trocarEscopo`), e as
  chaves `m.arq_*` (Task 5).
- Produces: `FilesPanel.svelte` com props `{ sessionName: string; desktop: boolean }` — é o que as
  Tasks 11 e 12 hospedam. **Este é o nome e a assinatura que as duas usam**; mudar aqui quebra as
  duas.

**Três armadilhas levantadas na auditoria, que esta Task resolve de propósito:**

1. **Já existe uma barra de abas na tela** — a tira de sessões do `DesktopShell.svelte:239-249`
   (`barraDeAbas`). A barra nova fica a poucos pixels dela: resolver por peso, posição ou material,
   e registrar a escolha no commit.
2. **O painel tem colapso próprio** (`ctxPanel.recolhido`, guardado no `localStorage`,
   `ctxPanel.svelte.ts:20-33`). Recolhido, o painel **some** — a barra de abas some junto.
3. **Três testes travam o cabeçalho** (`DesktopSessionContext.test.ts:34,41,55`): nenhum
   `.ctx-fold`, recolhido some, porta acessível no topo preservada. Ou o desenho os respeita, ou o
   teste é atualizado **com justificativa no commit**.

- [x] **Step 1: `FilesPanel.svelte`** — junta `FileSearchBar`, `FileTree`, o `filesStore` e a barra
  de controles do mock: `m.arq_ordenar_nome()` à esquerda; à direita o botão de filtro (ativo por
  padrão, `--accent-dim` + `--accent`, rótulo `m.arq_mostrar_tudo()`) e o de recarregar
  (`m.arq_recarregar()`). Abaixo do segmentado, a linha `m.arq_so_modificados()` +
  `m.arq_mostrar_tudo()`, que existe porque o filtro vem ligado e "cadê o README" não pode virar
  dúvida.
- [x] **Step 2: Os vazios, um por causa** — `m.arq_sem_nome()`, `m.arq_sem_conteudo()`,
  `m.arq_primeiros_200()`, `m.arq_pasta_grande()`, `m.arq_nada_mudou()` (filtro ligado e nada
  mudou), `m.arq_sessao_encerrada()` (404 do `_session_cwd`).
- [x] **Step 3: A barra de abas** no `DesktopSessionContext` — `m.ctx_aba_contexto()` e
  `m.arq_aba()`. A aba Contexto mantém tudo que já mostra, sem mudança.
  **`ctx_aba_contexto` NÃO existe** em `frontend/messages/pt.json` (conferido em 15/08/2026) — crie
  nos dois arquivos, junto com as chaves desta Task: `"ctx_aba_contexto": "Contexto"` /
  `"ctx_aba_contexto": "Context"`. Chave que não existe vira erro de tipo no `check`, então isto
  aparece cedo — mas descobrir no meio da Task custa uma ida ao `messages/` que o plano podia ter
  evitado.
- [x] **Step 4: Rodar os três portões e ABRIR A TELA.** O Vite às vezes serve componente vazio
  depois de editar: se a aba montar em branco, `systemctl --user restart hangar-frontend.service` e
  recarregar ignorando cache.
- [x] **Step 5: Conferir o que o mock não cobre** — ligar o **papel de parede** e mover o slider de
  Transparência de ponta a ponta; trocar para o **tema claro**; percorrer com **Tab** e setas; e
  esticar a janela até o painel ir para 300 e 340px. Qualquer superfície que não deixe a foto
  atravessar enquanto o painel em volta deixa é bug, não estilo.
- [x] **Step 6: Comparação cega 1 — fidelidade** contra
  `docs/mocks/2026-08-15-arvore/prints/1-desktop-painel.png`, 1440px.
- [x] **Step 7: Comparação cega 2 — integração.** Capturar a aba **Contexto** do mesmo painel, no
  mesmo momento e largura, e perguntar a um subagente novo se as duas telas são do mesmo app.
  Divergência resolvida a favor do app entra como comentário no commit.
- [x] **Step 8: Commit.**

**Barra:** `1-desktop-painel.png` (fidelidade) **+** a aba Contexto do próprio painel (integração).

---

### Task 11: O arquivo cobrindo a conversa  ⟨mexe em pixel⟩

**Files:**
- Modify: `frontend/src/screens/Chat.svelte`
- Modify: `frontend/src/components/files/FilesPanel.svelte`

**Interfaces:**
- Consumes: `FilesPanel` (Task 10, props `{sessionName, desktop}`), `FileViewer` (Task 7, props
  `{path, diff, conteudo, loading, onEscopo, onFechar}`), `FilesStore.abrir/trocarEscopo` (Task 9),
  `pathDiff` e `readFile` (Task 5).
- Produces: nada que outra Task consuma — é folha. O que ela **muda** para a Task 12 é o contrato de
  quem decide onde o arquivo aparece: no desktop quem monta o `FileViewer` é o `Chat.svelte`; no
  celular é o próprio `FilesPanel`, como nível do drill-down. O `FilesPanel` recebe isso pela prop
  `desktop`, e é a única diferença de comportamento entre as duas telas.

**A decisão de desenho, com o motivo:** o arquivo **não** abre em modal com véu. Ele cobre **só a
área da conversa**, e a árvore continua viva e clicável no painel de 264px — é o que o Paseo e o
Orca fazem (prints `10-aba-arquivos.png` e `11-arquivos-visualizador.png`), e é o que permite
percorrer vários arquivos sem abrir e fechar a cada um.

- [x] **Step 1: O visor** — clicar num arquivo monta o `FileViewer` no lugar da conversa. Clicar em
  outro arquivo **troca o conteúdo**, sem fechar nada.
- [x] **Step 2: Duas saídas** — o `×` no cabeçalho e o link `m.arq_voltar_conversa()`. Os dois
  existem porque, sem véu, não fica claro que a conversa continua viva atrás.
- [x] **Step 3: Linha fantasma** — arquivo apagado entre listar e abrir devolve 404: mostra
  `m.erro_arq_inexistente()` **e recarrega a árvore**, em vez de deixar a linha clicável para sempre.
- [x] **Step 4: Rodar os três portões** e abrir de verdade um arquivo mudado, um intocado, um
  binário e um grande.
- [x] **Step 5: Conferir o que o mock não cobre** — papel de parede com a Transparência nos extremos
  (o visor cobre a conversa, então ele é uma superfície grande sobre a foto e é onde um
  `--bg-base` cru mais aparece), tema claro, e teclado: `Esc` fecha o visor.
- [x] **Step 6: Comparação cega 1 — fidelidade** contra `prints/2-desktop-visualizador.png`.
- [x] **Step 7: Comparação cega 2 — integração** contra o **`GitChangesTab`** no modal de Git com um
  arquivo selecionado, mesma largura. É o vizinho mais próximo: mesma função, mesmo `DiffView`.
- [x] **Step 8: Commit.**

**Barra:** `2-desktop-visualizador.png` (fidelidade) **+** o `GitChangesTab` (integração).

---

### Task 12: A aba no celular  ⟨mexe em pixel⟩

**Files:**
- Modify: `frontend/src/lib/gitTabs.ts` (`GIT_TABS`, linha 13), `frontend/src/lib/gitTabs.test.ts`
- Modify: `frontend/src/components/git/GitTabs.svelte`

**Interfaces:**
- Consumes: `FilesPanel` (Task 10, props `{sessionName, desktop}` — aqui com `desktop={false}`),
  `GIT_TABS` (`gitTabs.ts:13`).
- Produces: uma entrada nova em `GIT_TABS` com `id: 'files'` e `maxLevel: 1` (nível 0 = árvore,
  nível 1 = arquivo). **O `maxLevel` é o contrato:** `maxOf` o consulta para saber até onde o
  drill-down pode ir, e um valor menor prende o usuário na árvore sem erro nenhum.

**Três coisas do `gitTabs.ts` que a auditoria pegou, e que fazem esta Task ser maior do que parece:**

1. **`GitTabId` é união fechada** — `'changes' | 'history' | 'branches'` (`gitTabs.ts:11`). Precisa
   ganhar `'files'`.
2. **`GitNav.levels` é `Record<GitTabId, number>`**, e o `initialNav()` devolve o objeto **literal**
   com as três chaves (`gitTabs.ts:22-24`). Acrescentar um id sem acrescentar a chave lá é erro de
   tipo — e é o que o `check` vai apontar primeiro.
3. **`maxOf` NÃO é exportado** (`const` privado, `gitTabs.ts:25`). Se o `GitTabs.svelte` precisar
   dele, exporte; se não, não invente o import.

Onde a aba nova entra na ordem é decisão de desenho: no mock ela fica **entre** Alterações e
Histórico. Mexer na ordem de `GIT_TABS` muda a aba inicial? Não — `initialNav()` fixa `'changes'`.

- [x] **Step 1: A aba** em `GIT_TABS`, com `maxLevel` que comporte árvore → arquivo, e o teste
  correspondente em `gitTabs.test.ts`.
- [x] **Step 2: Hospedar o `FilesPanel`** — o mesmo componente, largura do celular. Aqui o arquivo
  abre como **nível do drill-down**, não cobrindo tela: no celular não há conversa ao lado para
  preservar.
- [x] **Step 3: Rodar os três portões** e percorrer a 390px: árvore → arquivo → diff → voltar.
- [x] **Step 4: Conferir o que o mock não cobre** — papel de parede e tema claro a 390px, e a área
  de toque: alvo menor que 44px na árvore é erro, e o mock foi desenhado com mouse.
- [x] **Step 5: Comparação cega 1 — fidelidade** contra `prints/3-celular.png`.
- [x] **Step 6: Comparação cega 2 — integração** contra as abas **Alterações** e **Histórico** do
  mesmo modal, no celular. São as vizinhas de porta: se a aba nova tiver outra densidade ou outro
  peso de rótulo, aparece na hora.
- [x] **Step 7: Commit.**

**Barra:** `3-celular.png` (fidelidade) **+** as outras abas do mesmo modal (integração).

---

## O que ficou de fora, com o motivo

- **Atualização automática da árvore.** O SSE de hoje tem `message`, `state`, `preview`,
  `ask_question`, `ping` e `reset` — nenhum evento de arquivo. A marca e o diff congelam até alguém
  recarregar, e é por isso que o botão de recarregar (Task 10, Step 1) é obrigatório, não enfeite.
- **Cache de diff.** Nenhum diff tem cache e cada clique forka um `git`
  (`docs/pesquisa-c1-c2-terreno.md`, seção 3). Aceito: o clique é humano, não laço.
- **Paginação de diretório.** Teto de 1000 com aviso. Sem biblioteca de virtualização instalada,
  paginar sem virtualizar troca um problema por outro.
- **Prévia renderizada de markdown** (o `Prévia | Fonte` do print 11): só `Fonte` nesta rodada.

## Decisões em aberto

Nenhuma. Fechadas com o usuário em 15/08/2026: a árvore mora numa aba dentro do painel docado que já
existe; o arquivo abre cobrindo a conversa, com a árvore viva; o padrão é **só os modificados**, com
botão para ver tudo; cada linha carrega `+N −M`; a barra de toda Task visual é o mock aprovado, tendo
o Paseo como referência honrada; e o plano é escrito para o máximo de escritores em paralelo.

## Teto

Parar e chamar o usuário se: duas Tasks seguidas não fecharem o portão; qualquer Task visual pedir
terceira rodada de comparação; ou um merge de lote der conflito duas vezes. Custo em conta de
assinatura apenas.

## A vigia, a cota e o teto de contexto

Três coisas que mantêm o tubo andando sozinho de madrugada. As três são decisão do usuário,
15/08/2026.

### Vigia: ninguém fica parado mais de 10 minutos

A skill traz o script (`skills/orchestrating-idea-to-push/scripts/vigia.sh`), e ele acorda por
`cp-send --tmux`, que reanima até turno morto. Ele aceitava exatamente três sessões; foi
**generalizado em 15/08/2026 para N**, porque o nosso pico é sete e uma vigia por par enxergaria só
o próprio pedaço — acordaria o árbitro enquanto outro executor ainda trabalhava. **Uma vigia só,
cobrindo todo mundo:**

```bash
SK=/home/jefferson/pessoal/hangar/skills/orchestrating-idea-to-push
setsid nohup "$SK/scripts/vigia.sh" \
  arv-t6 arv-t7 arv-t8 arv-t9 arv-review arv-review2 arv-arbitro 10 \
  > /tmp/vigia-lote-b.log 2>&1 < /dev/null &
```

Três coisas que a assinatura decide, e erram calado se ignoradas:

- **O último nome é sempre o árbitro** — é para ele que os avisos vão e é ele que a vigia reanima.
- **O `10` é o silêncio em minutos**, lido como número por ser o último argumento (o padrão é 5).
  A chamada antiga de três nomes continua valendo, sem mudança.
- **`setsid nohup` é obrigatório**: sem isso a vigia é filha do turno do árbitro e morre junto com
  ele — que é justamente o caso que ela existe para cobrir.

Ela dispara quando **todas** estão paradas ao mesmo tempo. Duas exceções que avisam na hora, sem
esperar o silêncio: sessão **travada** (diz `working` mas não produz evento há 10 min — o clássico é
um seletor bloqueando o turno) e sessão **sem cota**.

Ao encerrar cada lote, **matar a vigia daquele lote** — vigia órfã acorda sessão que já morreu.

### Conta sem cota: troca de conta, não espera

A vigia já detecta (`semcota`) e avisa o árbitro; ela **não** resolve sozinha. Quando o aviso chegar,
o árbitro pede a uma sessão que **ainda tenha cota** que abra a substituta. As contas permitidas
para isso, e só elas:

| Conta | Modelo | Onde |
|---|---|---|
| `claude-200-01` | Claude | conta Anthropic do usuário |
| `opencode-go` | DeepSeek V4 Flash | motor `deepseek-direto` |
| `openai-codex` | GPT 5.6 Luna | via Pi |

Conta fora desta lista: **pare e pergunte**. A sessão nova recebe o **mesmo kick-off** da que parou,
com a Task em aberto e o `HEAD esperado` da worktree dela — o kick-off carrega caminhos, não estado,
então serve inteiro para a substituta.

### Teto de contexto do escritor: 500k

Escritor que passar de **500k de contexto** não começa Task nova. Ele **termina a que está fazendo**,
reporta, e o árbitro abre uma sessão fresca na mesma worktree, com o mesmo kick-off. Trocar no meio
de uma Task perderia o raciocínio dela; trocar depois não perde nada, porque o estado que importa
está no plano, no contrato e no commit.

O contexto de cada sessão aparece na linha de status (`💬 ctx …`), que o app já lê por sidecar. O
árbitro confere ao receber cada relato de Task — é o momento natural, e não custa uma varredura.

## O time

| Papel | Sessão | Agente/motor | Conta | Como abrir |
|---|---|---|---|---|
| árbitro | esta | Claude Opus 5 | Anthropic (assinatura) | já aberta |
| executor ×3 (lote A) | `arv-t1/t2/t3` | **DeepSeek V4 Flash, sempre no max** | `opencode-go` | `POST /api/sessions` com `engine: "deepseek-direto"`, `effort: "max"`, `cwd` = a worktree |
| executor ×4 (lote B) | `arv-t6…t9` | idem | `opencode-go` | idem, uma worktree cada |
| revisor | `arv-review` | **GPT 5.6 Luna, max** | `openai-codex` | `POST /api/sessions` com `provider: "pi"`, `effort: "max"`; depois `/cp-model openai-codex gpt-5.6-luna` |
| 2º revisor (**só nos lotes paralelos**) | `arv-review2` | **Claude Opus, NÃO no max** | `~/.claude-jefferson` | `POST /api/sessions` com `config_dir: "/home/jefferson/.claude-jefferson"`, **sem** `effort` |
| revisão da branch | `arv-final` | a decidir | | dispara quando as doze Tasks estiverem aprovadas |

### Por que pela API e não pelo `cp-send --new`

Conferido em 15/08/2026, com o backend já atualizado: o `POST /api/sessions` aceita **`config_dir`,
`model` e `effort`**, e valida os três **antes** de tocar no disco (`api.py:1207-1236`). O
`cp-send --new` expõe só `--engine` e `--provider` — ele é um atalho, não o teto do que a máquina faz.

Isso resolve duas coisas de uma vez:

- **A conta `jefferson` tem caminho pronto.** `/home/jefferson/.claude-jefferson` está na lista que o
  backend reconhece (rótulo `jefferson`), e um `config_dir` fora dessa lista volta `400`
  `erro_config_dir_invalido` — a sessão **não nasce** na conta errada.
- **O `max` do executor entra no nascimento.** `EFFORT_CLAUDE = ("low","medium","high","xhigh","max")`
  e `EFFORT_PI` acrescenta `off`/`minimal` (`model_args.py:42-46`). A skill de orquestração avisa que
  `cp-send --new` não configura esforço e que a sessão precisa **provar** o nível ao vivo antes da
  primeira Task; pedindo `effort` na criação, o pedido é validado na entrada — mas **a prova ao vivo
  continua obrigatória**, porque validar o pedido não é o mesmo que confirmar o que a sessão está
  usando.

O árbitro monta a chamada com o token do backend; o `cp-send` segue valendo para **falar** com as
sessões depois de criadas.

Famílias diferentes em cada portão: DeepSeek escreve, GPT revisa, e o Claude entra como segundo
revisor quando há Tasks em paralelo pedindo dois ao mesmo tempo. **O executor não enxerga imagem**,
então o protocolo de visão (`see <caminho>`) é obrigatório nas Tasks 6 a 12 e entra no contrato.

**Ambiguidade resolvida:** `gpt-5.6-luna` existe em `openai-codex` (272K) e em `opencode-go` (1.1M).
Vale `openai-codex`, porque a `opencode-go` está registrada como travada em `deepseek-v4-flash` em
`~/.claude/orquestracao-contas.md`.

### As skills chegam nos dois motores — conferido em 15/08/2026

O revisor roda em Pi, e o Pi tem ponte própria de skills (`~/.pi/agent/skills-bridge/`, 155
entradas, todas apontando para `~/.claude`). Antes deste trabalho ela cobria **só plugins**: as
skills pessoais, incluindo a `orchestrating-idea-to-push`, não estavam lá — o revisor não teria como
descobrir o próprio papel. Foram ligadas. O `Step 0` de cada sessão prova isso de novo antes de
qualquer Task, porque symlink some.

### O kick-off de cada sessão

```
Invoque a skill orchestrating-idea-to-push e leia a página do seu papel.
Papel: <executor único | revisor | revisão da branch>.
ANTES DE QUALQUER COISA: rode /home/jefferson/pessoal/hangar/scripts/checar-skills.sh
  Saiu 1? PARE e me avise. Não comece a Task sem as skills.
Repo/branch: <caminho da worktree> / <branch>.   HEAD esperado: <hash da BASE>.
Plano: /home/jefferson/pessoal/hangar/docs/superpowers/plans/2026-08-15-arvore-de-arquivos-e-diff.md
Contrato: <caminho do grupo-<gid>.md>.
Intocáveis: docs/pesquisa-referencias-2026-08-13.md, docs/pesquisa-c1-c2-terreno.md,
  o próprio plano, .refs/**, docs/mocks/**, CLAUDE.md.
Sua vez agora: <Task N>.
Ao terminar, reporte para <sessão do árbitro> e PARE.
```

**`HEAD esperado` é a `BASE` do lote, não o HEAD da `main`** — as três worktrees nascem do mesmo
commit, e é ele que a sessão confere. Errar isso é uma sessão trabalhando na árvore da outra.

## Self-review do plano

Feito em 15/08/2026, conforme o `writing-plans`:

1. **Cobertura da spec.** C1 (árvore navegável + busca por nome e conteúdo + marca herdada) →
   Tasks 1, 2, 6, 8, 10, 12. C2 (diff do arquivo somando os turnos) → Tasks 3, 7, 11. O "cuidado"
   que a spec mandou decidir antes de começar (desktop × celular) está resolvido em Tasks 10 e 12.
2. **Sem espaços em branco.** Todo Step de código traz o código. Os três lugares que dizem "ler
   antes de escrever" (`api.ts`, `errosApi.ts`, `highlight.ts`) apontam arquivo e linha — são
   pedidos de conferência contra o código real, não pendências.
3. **Tipos batendo.** `TreeEntry` (Task 5) é o que a Task 6 consome; `PathDiff` (Task 5) é o que a
   Task 3 produz e a Task 7 consome; `FileError`/`SearchError` (Tasks 1 e 2) têm a mesma forma
   (`status`, `code`, `msg`), que é o que a Task 4 traduz num único helper `_erro_arq`.
4. **Barras coerentes.** Tasks 6–9 são peças soltas (três componentes e um store) que não montam tela: a barra delas é o
   **HTML** do mock, conferido classe a classe. Tasks 10–12 montam tela: aí são **duas** comparações
   cegas — fidelidade contra o mock e integração contra a tela irmã real —, teto de 2 rodadas
   somando as duas. Nenhuma Task de backend tem barra.
5. **Disjunção dos lotes, conferida arquivo a arquivo.** Lote A: `filetree.py`+`test_filetree.py` /
   `filesearch.py`+`test_filesearch.py` / `git_ops.py`+`test_git_ops.py` — nenhum arquivo em
   comum, e nenhuma das três importa o que as outras criam. Lote B: `FileTree.svelte` /
   `FileViewer.svelte` / `FileSearchBar.svelte` / `filesStore.svelte.ts`, mais os quatro arquivos de
   teste — também disjuntos, e todos consomem só o que a Task 5 já entregou.

## Registro de decisões (para quem pegar este plano depois)

Tudo fechado com o usuário em 15/08/2026, na ordem em que foi decidido:

1. A árvore mora numa **aba dentro do painel docado que já existe**, não num painel novo.
2. O **Paseo é a trava de visão**; o mock feito a partir dele, e aprovado, é a barra — **mas a barra
   tem duas perguntas**: fidelidade ao mock e integração com o layout real do app, e onde as duas
   discordam, o app ganha. *"A maioria das vezes o mock que eu aprovo não é o que é construído."*
3. O arquivo abre **cobrindo a conversa, com a árvore viva** — não em modal com véu. Decidido
   depois de ver que o painel tem **264px**, e não os 530px que a primeira versão deste plano supôs.
4. Cada linha da árvore carrega **`+N −M`**, com a pasta somando os filhos.
5. O padrão é **só os modificados**, com botão para ver tudo — porque a árvore inteira já é o que o
   app não tem, mas quem abre a aba acabou de trabalhar.
6. O plano é escrito para o **máximo de escritores em paralelo**: 3 no backend, 4 no front.
7. **DeepSeek V4 Flash sempre no max escreve; GPT 5.6 Luna max revisa;** o Claude Jefferson (não no
   max) entra como segundo revisor só quando há Tasks em paralelo pedindo dois ao mesmo tempo.
8. Toda a UI nasce **traduzida nas duas línguas** — o i18n foi concluído e está na `main`.
9. **Vigia de 10 minutos** em toda sessão, uma por par; **conta sem cota troca por outra da lista
   permitida** em vez de esperar; **escritor acima de 500k de contexto** termina a Task e cede o
   lugar a uma sessão fresca.
10. A estimativa de tempo fica registrada **antes** da execução, em
    [`2026-08-15-arvore-estimativa-vs-real.md`](2026-08-15-arvore-estimativa-vs-real.md), para ser
    comparada com o real no fim.
