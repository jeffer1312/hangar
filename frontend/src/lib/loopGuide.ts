// Guia in-app do loop runner (botão ? no LoopSheet). Texto estático, sem endpoint — VERBATIM da
// seção "Guia in-app" de docs/superpowers/specs/2026-07-22-loop-runner-design.md. Colapsável por
// seção, cabe no sheet mobile.

export interface LoopGuideSection {
  title: string;
  body: string;
}

export const LOOP_GUIDE: LoopGuideSection[] = [
  {
    title: 'Objetivo — uma coisa só.',
    body: 'Resultado final, não passos; pequeno e verificável. ✗ "refatore o projeto" / ✓ "migre utils/date.ts pra date-fns e mantenha npm run check verde". Tarefa grande → quebrar num arquivo de plano e mandar o loop fazer "o próximo item pendente".',
  },
  {
    title: 'Check — quem decide que acabou é um comando.',
    body: 'Exit 0 = pronto. Usar o gate real do projeto (chips sugeridos). Check bom: rápido (<2min), determinístico, roda igual duas vezes. Sem check possível → o loop só para com a TUA confirmação; supervise mais de perto. O check roda como comando único (argv), sem shell: "&&", "|" e redirecionamento não funcionam — pra encadear, crie um script no repo e use ele como check.',
  },
  {
    title: 'Iterações — o freio.',
    body: 'Começar com 5–10. Estourou sem passar? Não aumente o número — melhore o objetivo ou o check. Loop que "precisa da noite toda" = escopo grande demais.',
  },
  {
    title: 'Sinais de problema.',
    body: 'Mesmo erro 2× → o app já para sozinho (estagnação); fix A quebra B alternando → divida o escopo; agente mexendo em teste → o re-prompt já proíbe, mas revise o diff.',
  },
  {
    title: 'Dica final:',
    body: 'peça evidência, não promessa — inclua no objetivo "rode o check e mostre a saída".',
  },
];
