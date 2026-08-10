// Fixture do sessionsStore pro SessionTabs.test.ts (round 2): $state REAL. Um mock plain não
// re-computa o $derived do SessionTabs quando o modelo muda — e o teste do foco pós-rename
// precisa do REFLEXO do modelo (o SSE re-emite o nome novo) pra provar o foco na aba recriada.
// Arquivo .svelte.ts de propósito: runes só compilam em .svelte.ts, não em .test.ts.
// `$state` aqui é construto do COMPILADOR (global nos rune files) — não se importa de 'svelte'.
export const fixtureByServer = $state<unknown[]>([]);

export const fixtureStore = {
  get byServer() { return fixtureByServer; },
  get rows() { return []; },
  get servers() { return []; },
  get loading() { return false; },
};
