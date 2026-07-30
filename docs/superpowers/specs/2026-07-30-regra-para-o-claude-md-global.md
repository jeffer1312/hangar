# Regra proposta para o `~/.claude/CLAUDE.md` global (não aplicada)

Texto pronto pra o usuário colar no próprio `~/.claude/CLAUDE.md` se quiser — este repo não edita
config global do usuário por conta própria.

```markdown
- **Executando plano do superpowers:** ao terminar cada Step, marcar `- [ ]` → `- [x]` no arquivo do plano. Step que precisa de conferência humana leva "verificação manual" no título. O progresso que aparece no celular (claude-cockpit) lê daí.
```

Sem "no mesmo commit": `docs/superpowers/` é gitignored e metade dos planos é untracked — a regra
seria impossível de cumprir e falharia em silêncio.
