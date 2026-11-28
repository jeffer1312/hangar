import { QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { MotionConfig } from "motion/react";
import { queryClient } from "@/lib/api";
import { App } from "@/app";
import { Guard } from "@/components/kit";
import "./index.css";

// Sem isto o painel não pode ser instalado; o service worker só repassa à rede.
navigator.serviceWorker?.register("/panel/sw.js", { scope: "/" }).catch(e => console.warn("service worker:", e));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <MotionConfig reducedMotion="user">
        <Guard whole><App /></Guard>
      </MotionConfig>
    </QueryClientProvider>
  </StrictMode>,
);
