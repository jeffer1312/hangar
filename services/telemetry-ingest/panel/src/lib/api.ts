import { keepPreviousData, QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import type { Summary, Trail } from "./types";

export class HttpError extends Error {
  readonly status: number;
  readonly code?: string;
  constructor(status: number, code?: string) {
    super(code ? "HTTP " + status + " (" + code + ")" : "HTTP " + status);
    this.status = status;
    this.code = code;
  }
}
export const isUnauthorized = (e: unknown) => e instanceof HttpError && e.status === 401;

export async function api<T>(path: string, init?: RequestInit & { json?: unknown }): Promise<T> {
  const { json, ...rest } = init ?? {};
  let res: Response;
  try {
    res = await fetch(path, { cache: "no-store", ...rest, ...(json === undefined ? {} : { body: JSON.stringify(json), headers: { "content-type": "application/json" } }) });
  } catch (e) {
    if ((e as Error).name === "AbortError") throw e;
    throw new Error("Sem conexão com o servidor.");
  }
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new HttpError(res.status, body?.error);
  }
  return (res.status === 204 ? undefined : await res.json()) as T;
}

export const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: (n, e) => !isUnauthorized(e) && n < 1, retryDelay: 1500, staleTime: 15_000 } },
});

export const REFRESH_MS = 60_000;

// Com o canal ao vivo aberto, a releitura vem dele; sem ele, a cada minuto com a página visível.
export function useSummary(days: number, live: boolean) {
  return useQuery({
    queryKey: ["data", days],
    queryFn: ({ signal }) => api<Summary>("/panel/data?days=" + days, { signal }),
    placeholderData: keepPreviousData,
    refetchInterval: live ? false : REFRESH_MS,
  });
}

export function useTrail(machine: string | undefined, day: string | undefined) {
  return useQuery({
    queryKey: ["trail", machine, day],
    queryFn: ({ signal }) => api<Trail>("/panel/trail?" + new URLSearchParams({ machine: machine!, day: day! }), { signal }),
    enabled: !!machine && !!day,
    placeholderData: keepPreviousData,
  });
}

export function useMe() {
  return useQuery({ queryKey: ["me"], queryFn: () => api<{ email: string }>("/panel/api/me"), retry: false, staleTime: Infinity });
}

// Pacote novo no receptor: relê o resumo e o dia aberto do caminho, juntando avisos seguidos.
export function useLive(enabled: boolean) {
  const client = useQueryClient();
  const [live, setLive] = useState(false);
  useEffect(() => {
    if (!enabled || typeof EventSource === "undefined") return;
    const source = new EventSource("/panel/events");
    let timer = 0;
    source.onopen = () => setLive(true);
    source.onerror = () => setLive(source.readyState === EventSource.OPEN);
    source.addEventListener("batch", () => {
      clearTimeout(timer);
      timer = window.setTimeout(() => {
        // Página escondida só marca como velho: relê quando voltar à tela.
        const refetchType = document.hidden ? "none" : "active";
        client.invalidateQueries({ queryKey: ["data"], refetchType });
        client.invalidateQueries({ queryKey: ["trail"], refetchType });
      }, 1200);
    });
    return () => { clearTimeout(timer); source.close(); setLive(false); };
  }, [enabled, client]);
  return live;
}
