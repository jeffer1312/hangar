// O pacote so declara tipos pro caminho raiz, e a gente importa de `/vanilla` de proposito: a
// condicao `svelte` do package.json aponta pro FONTE em Svelte 3/4, que o vite-plugin-svelte
// tentaria compilar com o compilador da 5. Aqui os tipos do raiz valem pro caminho compilado.
declare module 'bigger-picture/vanilla' {
  export * from 'bigger-picture';
  export { default } from 'bigger-picture';
}

declare module 'bigger-picture/css';
