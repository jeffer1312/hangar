import path from "node:path";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import { mock } from "./dev/mock";

// O receptor serve o app em /panel; os arquivos com hash ficam em /panel/assets.
export default defineConfig(({ command }) => ({
  base: "/panel/",
  plugins: [react(), tailwindcss(), ...(command === "serve" ? [mock()] : [])],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "src") } },
  build: { outDir: "dist", assetsDir: "assets", chunkSizeWarningLimit: 900 },
}));
