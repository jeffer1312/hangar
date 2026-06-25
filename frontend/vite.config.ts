import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { VitePWA } from 'vite-plugin-pwa'

export default defineConfig({
  server: {
    // Dev: proxy /api to the backend so the browser talks same-origin — no CORS, and
    // the cp_token cookie reaches the EventSource (SSE). Mirrors the prod reverse-proxy.
    // Leave the Login "URL do servidor" empty in dev so requests stay relative.
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8765',
        changeOrigin: true,
      },
    },
  },
  plugins: [
    svelte(),
    VitePWA({
      registerType: 'autoUpdate',
      strategies: 'injectManifest',
      srcDir: 'src',
      filename: 'sw.ts',
      injectManifest: {
        swSrc: 'src/sw.ts',
        swDest: 'dist/sw.js',
      },
      manifest: {
        name: 'Claude Pocket',
        short_name: 'Pocket',
        display: 'standalone',
        background_color: '#0d0d0f',
        theme_color: '#0d0d0f',
        orientation: 'portrait',
        start_url: '/',
        icons: [
          { src: '/icons/icon-192.png', sizes: '192x192', type: 'image/png' },
          { src: '/icons/icon-512.png', sizes: '512x512', type: 'image/png' },
          { src: '/icons/icon-180.png', sizes: '180x180', type: 'image/png' },
        ],
      },
    }),
  ],
})
