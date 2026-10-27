# Novidades

Toda mudança que a pessoa vê no app ganha **um arquivo aqui**, no mesmo commit. A tela de
atualização do app nativo e do Expo anuncia, antes de atualizar e depois, as novidades que a
versão nova traz e que a máquina ainda não tem. Quem lê os arquivos é a rota
`GET /api/update/notes` (`crates/hangar-server/src/update_notes/`).

## Formato

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

O nome do arquivo é `<data>-<nome>.md` (ex.: `2026-10-26-historico-do-compositor.md`); sem o
`.md`, ele é o id da novidade. O corpo é o texto do cartão, Markdown curto.

| Campo | Para que serve |
|---|---|
| `titulo` | Obrigatório. Sem ele o arquivo é ignorado. |
| `tipo` | `novo`, `melhoria` ou `correcao`. Outro valor: arquivo ignorado. A tela mostra nessa ordem. |
| `onde` | `desktop`, `celular` ou `todos`. Outro valor: arquivo ignorado. |
| `sistema` | Opcional. O sistema da máquina que atualiza: `linux`, `windows` e/ou `macos`, separados por vírgula ou espaço (`sistema: linux, macos`). Sem o campo, vale para todos. Nome desconhecido: arquivo ignorado. |
| `experimente` | Opcional. Uma frase dizendo como testar agora. |
| `animacao` | Opcional. Nome de um roteiro animado que os apps conhecem. Nome que o app não conhece vira cartão só com texto. `correcao` nunca tem animação: o campo é descartado. |

## Mudança visível sem novidade

Se a mudança aparece na tela mas não merece anúncio, a mensagem do commit leva a linha
`HANGAR_SEM_NOVIDADE: <motivo>`.
