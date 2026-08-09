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
    environment: 'node', // pure-function + WebCrypto units; no DOM needed
    include: ['src/**/*.test.ts'],
  },
});
