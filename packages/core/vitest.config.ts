import { defineConfig } from 'vitest/config';
export default defineConfig({
  test: {
    // Um fork por núcleo (o padrão do vitest) com mais de uma suíte rodando ao mesmo tempo
    // enche a RAM e joga a máquina em swap. Dois bastam pro tamanho desta suíte.
    maxWorkers: 2,
    environment: 'node',
    include: ['src/**/*.test.ts'],
    globals: true,
  },
});
