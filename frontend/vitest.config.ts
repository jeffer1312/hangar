import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  resolve: {
    // O vitest transforma em modo "server" e o pacote svelte resolveria pro build SERVER
    // (`mount` indisponível). O teste de componente (PushQuiet.test.ts, happy-dom) precisa do
    // build CLIENT; os testes puros não importam svelte e não são afetados.
    conditions: ['browser'],
  },
  test: {
    // Um fork por núcleo (o padrão do vitest) com mais de uma suíte rodando ao mesmo tempo
    // enche a RAM e joga a máquina em swap. Dois bastam pro tamanho desta suíte.
    maxWorkers: 2,
    environment: 'node', // pure-function + WebCrypto units; no DOM needed
    // Teste que formata data (cota.test.ts) fixa o instante em -03:00 e espera a hora de
    // Brasília. Quem formata é o Intl, na timezone da máquina: o runner do GitHub roda em UTC
    // e 18h vira 21h. Fixar aqui deixa o resultado igual em qualquer máquina.
    env: { TZ: 'America/Sao_Paulo' },
    // `.test.svelte.ts` entra junto porque runes (`$state`) só compilam em arquivo com o sufixo
    // `.svelte.ts` — e testar componente que reage a MUDANÇA de prop (abrir/fechar) precisa de uma
    // prop reativa de verdade, não de um objeto solto.
    include: ['src/**/*.test.ts', 'src/**/*.test.svelte.ts'],
  },
});
