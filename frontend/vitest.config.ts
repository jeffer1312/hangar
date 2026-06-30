import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node', // pure-function + WebCrypto units; no DOM needed
    include: ['src/**/*.test.ts'],
  },
});
