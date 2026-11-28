export const fmt = new Intl.NumberFormat("pt-BR");
export const compact = new Intl.NumberFormat("pt-BR", { notation: "compact", maximumFractionDigits: 1 });
export const pct = new Intl.NumberFormat("pt-BR", { style: "percent", maximumFractionDigits: 1 });
export const pct0 = new Intl.NumberFormat("pt-BR", { style: "percent", maximumFractionDigits: 0 });
export const quantity = (n: number, single: string, plural: string) => fmt.format(n) + " " + (n === 1 ? single : plural);
export const dayMonth = (day: string) => day.slice(8, 10) + "/" + day.slice(5, 7);
export const fullDay = (day: string) => day.slice(0, 10).split("-").reverse().join("/");
export const when = (s: string | null) => (s ? dayMonth(s) + " " + s.slice(11, 19) : "");
export const upper = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

// Tempo desde um horário UTC "AAAA-MM-DD HH:MM:SS".
export function ago(stamp: string, now = Date.now()) {
  const t = Date.parse(stamp.replace(" ", "T") + "Z");
  if (!Number.isFinite(t)) return stamp;
  const min = Math.max(0, Math.round((now - t) / 60000));
  if (min < 1) return "agora";
  if (min < 60) return "há " + min + " min";
  if (min < 24 * 60) return "há " + Math.round(min / 60) + " h";
  if (min < 48 * 60) return "ontem";
  return "há " + Math.round(min / 1440) + " dias";
}
