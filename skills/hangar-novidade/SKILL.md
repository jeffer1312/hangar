---
name: hangar-novidade
description: Escreve a novidade de uma mudança visível do Hangar (arquivo em docs/novidades/ que a tela de atualização mostra) e, se for recurso novo, a animação feita com os componentes reais do app nativo e do Expo. Use quando o hook do `gh pr create` ou o CI avisar "Mudança visível no app sem novidade", quando pedirem "escreve a novidade", "anuncia isso na atualização", e antes de abrir PR que muda tela em desktop-native/, mobile/ ou messages/. Não serve para passo de atualização da máquina (docs/atualizacoes/).
---

# Novidade da atualização

A novidade é o cartão que a pessoa vê no "Atualizar?" e no "O que chegou para você". Quem lê não é
programador: escreva o que ela ganha, não o que o código fez.

## 1. Ler o que mudou

```bash
git fetch -q origin main
git log --no-merges --format='%h %s' origin/main..HEAD
git diff --stat origin/main...HEAD -- desktop-native/src mobile/src mobile/app messages
```

Leia o diff das telas até entender o que a pessoa passa a ver ou fazer. A lista de commits não é a
novidade: nunca a copie.

## 2. Classificar

| O que mudou | `tipo` | Animação |
|---|---|---|
| Recurso que não existia e a pessoa vê | `novo` | sim, nos apps de `onde` |
| Algo que já existia ficou melhor | `melhoria` | só se a mudança for de movimento ou layout |
| Algo quebrado voltou a funcionar | `correcao` | nunca |
| Refatoração, teste, texto interno, nada visível | — | nada |

Ajuste interno não ganha arquivo. Diga ao autor para pôr `HANGAR_SEM_NOVIDADE: <motivo>` na
mensagem de um commit (é o que cala o aviso do CI) e pare aqui.

`onde`: `desktop` (só `desktop-native/`), `celular` (só `mobile/`) ou `todos` (os dois apps têm a
mudança). Outro valor faz a rota descartar o arquivo sem aviso.

`sistema`: só quando a mudança existe em parte dos sistemas da máquina que atualiza (`linux`,
`windows`, `macos`; vários separados por vírgula). Ex.: chaveiro do GNOME = `sistema: linux`. Leia
no diff se o código é de um sistema só (`#[cfg(windows)]`, `install.ps1`, D-Bus, `launchd`).
Vale para todos? Omita o campo.

## 3. Escrever `docs/novidades/<AAAA-MM-DD>-<nome>.md`

Formato e campos em `docs/novidades/README.md`. Data de hoje, `<nome>` curto em kebab-case;
`id` = nome do arquivo sem `.md` (não escreva `id:` no cabeçalho).

```markdown
---
titulo: Histórico do que você digitou
tipo: novo
onde: desktop
experimente: Com a caixa de mensagem vazia, aperte ↑.
animacao: historico-compositor
---

A seta ↑ na caixa de mensagem traz de volta as mensagens que você já mandou nesta conversa.
```

- `titulo` diz o que a pessoa ganha ("Histórico do que você digitou"), não o nome técnico
  ("TextareaState com histórico").
- Corpo: 1 a 3 frases em português do dia a dia, sem nome de arquivo, função ou sigla interna.
- `experimente`: uma ação concreta, onde clicar ou o que apertar. Sem ação possível, omita.
- `animacao` só quando houver animação registrada com esse nome; `correcao` nunca leva.

## 4. Animação (só recurso novo visível)

Use os componentes REAIS do app, nunca um desenho imitando a tela. Mesmo nome nos dois apps.

**Nativo** (`onde` = `desktop` ou `todos`):
- Leia `desktop-native/src/news/mod.rs`: o registro `animation(name)` e, na documentação do
  módulo, como abrir a pré-visualização de uma animação.
- Modelo: `desktop-native/src/news/composer_history.rs`. Monte a miniatura chamando as mesmas
  funções de vista que a tela real usa; se a parte que você precisa ainda está presa dentro de um
  `render_*` grande, extraia as partes visuais em funções que recebem dados simples e faça a tela
  real chamá-las também, sem mudar aparência nem comportamento.
- Ciclo curto (~6 s) em loop, quadro pedido só enquanto visível, `reduce_motion` = quadro parado
  no momento que explica o recurso. Miniatura só leitura, sem foco.
- Registre no `animation(name)` com o nome do `animacao`.

**Expo** (`onde` = `celular` ou `todos`):
- Registre em `mobile/src/features/updates/animations.ts` pelo mesmo nome.
- Use os componentes reais (ex.: `ComposerBox`, `ComposerInput`, `ComposerActions` de
  `mobile/src/chat/ComposerField.tsx`) e anime com `react-native-reanimated` na thread de UI.
  Animação para fora da tela; respeite `reduceMotion`.

Texto que aparece dentro da animação vem de `m.<chave>()` (nativo e Expo leem `messages/*.json`):
chave nova em `messages/pt.json` e `messages/en.json` no mesmo commit.

## 5. Conferir com o autor

1. Nativo: build de debug (`cargo build` em `desktop-native`), abra o app com
   `HANGAR_NATIVE_PREVIEW=<nome da animação>` (ou `update-confirm` / `whats-new` para os modais
   inteiros) e tire um print. A pré-visualização não existe no build de release. Cite o caminho
   absoluto do PNG na resposta.
2. Expo: abra a folha de atualização no emulador e tire um print do cartão.
3. Mostre ao autor o texto do cartão e os prints, e ajuste o que ele pedir.

## Regras

- Uma novidade por mudança que a pessoa percebe; várias correções pequenas podem ser um arquivo
  `correcao` só.
- Nunca liste commits, nomes de arquivo ou de função no texto.
- Novidade não substitui passo de atualização: o que a máquina precisa rodar vai em
  `docs/atualizacoes/`.
