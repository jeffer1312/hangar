// O bloco Velocidade: as faixas de cada medida viram "normalmente" (mediana) e "9 em 10" (p90), nunca um número exato.
// Sem import de runtime: o teste roda direto no node.
export type SpeedRow = { event: string; dims: Record<string, string>; bucket: string; os: string; count: number; machines: number };
export type Verdict = "ok" | "warn" | "bad" | "few";
export type System = "all" | "windows" | "linux" | "macos";

export const SPEED_BUCKETS = ["lt100ms", "lt300ms", "lt1s", "lt3s", "lt10s", "lt30s", "ge30s"];
const UPPER = ["100 ms", "300 ms", "1 s", "3 s", "10 s", "30 s"];
// Menos que isto de medidas e o selo não julga: a mediana de 3 vezes não diz nada.
const FEW = 5;

interface Def { event: string; by: "os" | "provider" | "client"; ok: string; warn: string; keep?: (dims: Record<string, string>) => boolean }
// Até `ok` é Rápido, até `warn` é Pode melhorar; depois, Lento. Medido pela faixa da mediana.
const DEFS: Def[] = [
  { event: "session_ready", by: "os", ok: "lt1s", warn: "lt10s", keep: d => d.opened_by === "person" },
  { event: "session_first_send", by: "provider", ok: "lt3s", warn: "lt30s" },
  { event: "first_reply", by: "provider", ok: "lt3s", warn: "lt10s" },
  { event: "app_ready", by: "client", ok: "lt1s", warn: "lt3s" },
  { event: "chat_open", by: "client", ok: "lt1s", warn: "lt3s" },
  { event: "server_startup", by: "os", ok: "lt3s", warn: "lt10s" },
];
export const SPEED_EVENTS = DEFS.map(d => d.event);

const index = (bucket: string) => SPEED_BUCKETS.indexOf(bucket);
export const bucketPhrase = (i: number) => (i >= UPPER.length ? "30 s ou mais" : "menos de " + UPPER[i]);

function quantile(buckets: number[], q: number) {
  const total = buckets.reduce((s, n) => s + n, 0);
  let seen = 0;
  for (let i = 0; i < buckets.length; i++) { seen += buckets[i]; if (seen >= q * total) return i; }
  return buckets.length - 1;
}

export interface SpeedLine { key: string; usually: string; level: number; total: number }
export interface SpeedCard {
  event: string; by: Def["by"]; total: number; machines: number; buckets: number[];
  usually: string; nine: string; verdict: Verdict; rows: SpeedLine[];
}

export function speedCards(rows: SpeedRow[], system: System): SpeedCard[] {
  return DEFS.map(def => {
    const mine = rows.filter(r => r.event === def.event && index(r.bucket) >= 0 && (system === "all" || r.os === system) && (!def.keep || def.keep(r.dims ?? {})));
    const buckets = SPEED_BUCKETS.map(() => 0);
    const groups = new Map<string, number[]>();
    for (const r of mine) {
      buckets[index(r.bucket)] += r.count;
      const key = def.by === "os" ? r.os : r.dims?.[def.by] ?? "other";
      const g = groups.get(key) ?? groups.set(key, SPEED_BUCKETS.map(() => 0)).get(key)!;
      g[index(r.bucket)] += r.count;
    }
    const total = buckets.reduce((s, n) => s + n, 0);
    const median = quantile(buckets, 0.5), p90 = quantile(buckets, 0.9);
    const verdict: Verdict = total < FEW ? "few" : median <= index(def.ok) ? "ok" : median <= index(def.warn) ? "warn" : "bad";
    const lines = [...groups].map(([key, g]) => {
      const level = quantile(g, 0.5);
      return { key, usually: bucketPhrase(level), level, total: g.reduce((s, n) => s + n, 0) };
    }).sort((a, b) => b.total - a.total);
    // Máquinas por (combinação, faixa, sistema) não somam sem contar a mesma de novo: fica a maior.
    const machines = mine.reduce((m, r) => Math.max(m, r.machines), 0);
    return { event: def.event, by: def.by, total, machines, buckets, usually: bucketPhrase(median), nine: bucketPhrase(p90), verdict, rows: lines };
  });
}
