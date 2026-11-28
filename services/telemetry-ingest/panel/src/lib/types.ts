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
  by_dims: { event: string; dims: Dims; count: number; machines: number }[];
  by_bucket: { event: string; bucket: string; count: number; machines: number }[];
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
