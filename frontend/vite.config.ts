import { defineConfig, type PluginOption } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { VitePWA } from 'vite-plugin-pwa'
import type { IncomingMessage, ServerResponse } from 'http'

// Multi-PC: quando o app (servido por uma origem, ex: casa) fala com o backend de OUTRA maquina
// (ex: trabalho), o browser faz preflight CORS (OPTIONS) por causa do header Authorization. O vite
// proxy repassa o ACAO=* do backend no GET/POST, MAS responde o OPTIONS sozinho (204 SEM CORS) ->
// o browser bloqueia. Este plugin responde o OPTIONS de /api com os headers CORS (token continua
// exigido pelo backend nas chamadas reais — preflight nao carrega credencial).
function apiCorsPreflight(): PluginOption {
  return {
    name: 'api-cors-preflight',
    configureServer(server) {
      server.middlewares.use((req: IncomingMessage, res: ServerResponse, next: () => void) => {
        if (req.method === 'OPTIONS' && req.url?.startsWith('/api')) {
          const reqHeaders = req.headers['access-control-request-headers'] ?? 'authorization,content-type';
          res.setHeader('Access-Control-Allow-Origin', '*');
          res.setHeader('Access-Control-Allow-Methods', 'GET,POST,DELETE,OPTIONS');
          res.setHeader('Access-Control-Allow-Headers', reqHeaders);
          res.setHeader('Access-Control-Max-Age', '86400');
          res.statusCode = 204;
          res.end();
          return;
        }
        next();
      });
    },
  };
}

export default defineConfig({
  server: {
    // Desliga o CORS embutido do vite: ele só libera origens localhost e intercepta o preflight
    // OPTIONS de /api SEM setar Access-Control-Allow-Origin pra origens .ts.net (quebrava o
    // multi-PC cross-origin). Com false, o preflight cai no apiCorsPreflight (plugin) / no backend.
    cors: false,
    // Dev: proxy /api to the backend so the browser talks same-origin — no CORS, and
    // the cp_token cookie reaches the EventSource (SSE). Mirrors the prod reverse-proxy.
    // Leave the Login "URL do servidor" empty in dev so requests stay relative.
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8765',
        changeOrigin: true,
      },
    },
    // Allow access via the Tailscale MagicDNS host (tailscale serve → this dev server),
    // so the phone can reach it over trusted HTTPS on the tailnet.
    allowedHosts: ['.ts.net'],
  },
  plugins: [
    apiCorsPreflight(),
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
        background_color: '#100e11',
        theme_color: '#100e11',
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
