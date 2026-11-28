// Formato de /panel/data (store::summary) e /panel/trail (store::trail) do receptor.
export type Dims = Record<string, string>;
export type Level = "error" | "warn";

export interface FailureRecord {
  level: Level;
  event: string;
  codigo: string | null;
  etapa: string | null;
  erro_tipo: string | null;
  action: string | null;
  count: number;
  machines: number;
  versions: string[];
  os: string[];
  first: string;
  last: string;
  sample_at: string | null;
  fields: Record<string, unknown> | null;
  trail: string[] | null;
  daily: { day: string; count: number }[];
  // Da amostra; receptor antigo não manda, pacote antigo vem vazio.
  trail_last?: string[] | null;
  ctx_first?: Record<string, string> | null;
  ctx_last?: Record<string, string> | null;
  after?: string | null;
  capped?: boolean;
  batch?: SampleBatch | null;
}

// O lote de onde veio a amostra: binário, instalação, relógio e ambiente da máquina.
export interface SampleBatch {
  version: string;
  commit: string;
  install: string | null;
  os: string;
  clock_skew_s: number | null;
  machine: Record<string, string> | null;
}

export interface LogLine {
  level: Level;
  target: string;
  message: string;
  count: number;
  machines: number;
  first_day: string;
  last_day: string;
  versions: string[];
}

export interface TrailDay {
  machine: string;
  label: string;
  day: string;
  os: string;
  version: string;
  steps: number;
  problems: number;
}

// Versões publicadas por canal (`latest`, `beta`), lidas pelo receptor nos releases do GitHub.
export interface Release { version: string; os: string; at: string }
export interface Published { channels: Record<string, Release[]>; error: string | null }
// O último pacote de cada máquina no período.
export interface MachineLast { label: string; os: string; version: string; channel: string; last: string }

export interface Summary {
  records: FailureRecord[];
  records_total: number;
  transitions: { from: string; to: string; count: number; machines: number }[];
  transitions_total: number;
  trail_days: TrailDay[];
  trail_days_total: number;
  refusals?: { declined: number; revoked: number };
  days: number;
  active: { machines_7d: number; machines_30d: number; owners_7d: number; owners_30d: number };
  period: { batches: number; machines: number; partial_batches: number };
  daily: { day: string; machines: number }[];
  env: { key: string; value: string; machines: number }[];
  per_owner: { machines: number; owners: number }[];
  // Receptor antigo não manda.
  machines?: MachineLast[];
  published?: Published;
  by_dims: { event: string; dims: Dims; count: number; machines: number }[];
  by_bucket: { event: string; bucket: string; count: number; machines: number }[];
  // Faixas das medidas do bloco Velocidade com o sistema do pacote; receptor antigo não manda.
  speed?: { event: string; dims: Dims; bucket: string; os: string; count: number; machines: number }[];
  logs: LogLine[];
  trend: { day: string; event: string; dims: Dims; count: number }[];
  counters: { event: string; dims: Dims; bucket: string | null; count: number; machines: number }[];
}

export interface TrailStep {
  t: string;
  event: string;
  level: "ok" | "warn" | "error" | string;
  screen: string | null;
  count: number;
}

export interface Trail {
  machine: string;
  day: string;
  partial: boolean;
  steps: TrailStep[];
}
