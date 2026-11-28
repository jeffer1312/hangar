// Quantas versões cada máquina está atrás da mais atual publicada no canal e no sistema dela.
import type { MachineLast, Published, Release } from "./types.ts";

// Por segmento numérico: 0.1.0.9 é mais velha que 0.1.0.10.
export function compareVersions(a: string, b: string) {
  const x = a.split("."), y = b.split(".");
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (Number.parseInt(x[i] ?? "0", 10) || 0) - (Number.parseInt(y[i] ?? "0", 10) || 0);
    if (d) return Math.sign(d);
  }
  return 0;
}

// behind: versões publicadas mais novas; orMore quando a dela é mais velha que todas as guardadas no release.
export type Standing =
  | { kind: "current" }
  | { kind: "behind"; behind: number; orMore: boolean }
  | { kind: "checkout" }
  | { kind: "other" }
  | { kind: "unread" }
  | { kind: "unknown" };

const releasesOf = (m: MachineLast, published: Published) => (published.channels[m.channel] ?? []).filter(r => r.os === m.os);

export function standing(m: MachineLast, published: Published): Standing {
  if (m.install === "checkout") return { kind: "checkout" };
  // `other` é pacote de um branch fora de latest/beta: não há release para comparar.
  if (m.channel === "other") return { kind: "other" };
  if (!(m.channel in published.channels)) return { kind: "unread" };
  const releases = releasesOf(m, published);
  if (!releases.length) return { kind: "unknown" };
  const behind = releases.filter(r => compareVersions(r.version, m.version) > 0).length;
  if (!behind) return { kind: "current" };
  return { kind: "behind", behind, orMore: behind === releases.length };
}

export type Tier = "ok" | "near" | "far" | "none";
export const tierOf = (s: Standing): Tier => s.kind === "current" ? "ok" : s.kind === "behind" ? (s.behind <= 2 ? "near" : "far") : "none";

export function standingText(s: Standing) {
  if (s.kind === "current") return "Na mais atual";
  if (s.kind === "checkout") return "checkout";
  if (s.kind === "other") return "outro canal";
  if (s.kind === "unread") return "canal não lido";
  if (s.kind === "unknown") return "sem versão publicada";
  return s.behind + (s.orMore ? " ou mais" : "") + (s.behind === 1 && !s.orMore ? " versão atrás" : " versões atrás");
}

export interface VersionRow { machine: MachineLast; standing: Standing; tier: Tier }

export function versionView(machines: MachineLast[], published: Published) {
  const rank = (r: VersionRow) => r.standing.kind === "behind" ? r.standing.behind + (r.standing.orMore ? 0.5 : 0) : r.standing.kind === "current" ? 0 : -1;
  const rows: VersionRow[] = machines.map(machine => { const s = standing(machine, published); return { machine, standing: s, tier: tierOf(s) }; })
    .sort((a, b) => rank(b) - rank(a) || a.machine.label.localeCompare(b.machine.label));
  const count = (t: Tier) => rows.filter(r => r.tier === t).length;
  // A mais atual de cada canal e sistema que tem máquina.
  const latest: (Release & { channel: string })[] = [];
  for (const [channel, releases] of Object.entries(published.channels)) {
    for (const os of [...new Set(releases.map(r => r.os))].sort()) {
      if (!machines.some(m => m.channel === channel && m.os === os && m.install !== "checkout")) continue;
      const top = releases.filter(r => r.os === os).sort((a, b) => compareVersions(b.version, a.version))[0];
      latest.push({ ...top, channel });
    }
  }
  return { rows, ok: count("ok"), near: count("near"), far: count("far"), outside: count("none"), latest };
}
