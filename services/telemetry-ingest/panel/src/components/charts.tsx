import { Area, AreaChart, Bar, BarChart, CartesianGrid, Rectangle, Tooltip, XAxis, YAxis, type TooltipContentProps } from "recharts";
import type { NameType, ValueType } from "recharts/types/component/DefaultTooltipContent";
import type { ReactNode } from "react";
import { ChartContainer } from "@/components/ui/chart";
import { compact, dayMonth, fmt, fullDay } from "@/lib/format";
import { cn } from "@/lib/utils";

export interface TipRow { color: string; label: ReactNode; value: number }
export function TipBox({ title, rows, total }: { title: ReactNode; rows: TipRow[]; total?: number }) {
  return (
    <div className="min-w-40 rounded-lg border border-border-strong bg-popover px-3 py-2 text-[12.5px] shadow-xl shadow-black/40">
      <div className="mb-1 font-medium">{title}</div>
      {rows.map((r, i) => (
        <div key={i} className="flex items-center gap-2 py-px">
          <i className="size-2.5 shrink-0 rounded-[3px]" style={{ background: r.color }} />
          <span className="flex-1 text-muted-foreground">{r.label}</span>
          <span className="font-medium tabular">{fmt.format(r.value)}</span>
        </div>
      ))}
      {total != null ? (
        <div className="mt-1 flex items-center gap-2 border-t pt-1"><span className="flex-1 text-muted-foreground">Total</span><span className="font-medium tabular">{fmt.format(total)}</span></div>
      ) : null}
    </div>
  );
}

const axis = { tickLine: false, axisLine: false, tick: { fontSize: 11, fill: "var(--faint)" } } as const;
// Poucas marcas no eixo dos dias: começo, fim e algumas no meio, conforme a largura.
const dayTicks = (days: string[], n: number) => {
  if (days.length <= n) return days;
  return Array.from({ length: n }, (_, i) => days[Math.round((i * (days.length - 1)) / (n - 1))]);
};

export function DailyArea({ rows, unit, color, height = 220, label }: {
  rows: { day: string; value: number }[]; unit: [string, string]; color: string; height?: number; label: string;
}) {
  const id = "g" + label.replace(/\W/g, "");
  return (
    <ChartContainer config={{ value: { label, color } }} className="aspect-auto w-full" style={{ height }}
      role="img" aria-label={label + ". " + rows.map(r => dayMonth(r.day) + ": " + fmt.format(r.value)).join("; ")}>
      <AreaChart data={rows} margin={{ top: 8, right: 18, bottom: 0, left: 0 }}>
        <defs>
          <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stopColor={color} stopOpacity={0.3} />
            <stop offset="1" stopColor={color} stopOpacity={0} />
          </linearGradient>
        </defs>
        <CartesianGrid vertical={false} stroke="var(--grid)" />
        <XAxis dataKey="day" {...axis} ticks={dayTicks(rows.map(r => r.day), 5)} tickFormatter={dayMonth} interval={0} minTickGap={4} tickMargin={8} />
        <YAxis {...axis} width={34} allowDecimals={false} tickFormatter={v => compact.format(v)} />
        <Tooltip cursor={{ stroke: "var(--border-strong)" }} content={(p: TooltipContentProps<ValueType, NameType>) => p.active && p.payload?.length ? (
          <TipBox title={fullDay(String(p.label))} rows={[{ color, label: unit[1], value: Number(p.payload[0].value) }]} />) : null} />
        <Area dataKey="value" type="monotone" stroke={color} strokeWidth={2} fill={"url(#" + id + ")"} animationDuration={600}
          activeDot={{ r: 4, fill: color, stroke: "var(--card)", strokeWidth: 2 }} />
      </AreaChart>
    </ChartContainer>
  );
}

// Colunas empilhadas por dia: 2 px de fundo entre as fatias, só a fatia de cima com o canto arredondado.
export function StackedDays<K extends string>({ rows, keys, color, name, focus, height = 260 }: {
  rows: ({ day: string; total: number } & Partial<Record<K, number>>)[]; keys: K[]; color: (k: K) => string; name: (k: K) => string;
  focus?: K | null; height?: number;
}) {
  const data = rows.map(r => ({ ...r, __top: [...keys].reverse().find(k => r[k]) }));
  return (
    <ChartContainer config={Object.fromEntries(keys.map(k => [k, { label: name(k), color: color(k) }]))} className="aspect-auto w-full" style={{ height }}
      role="img" aria-label={"Por dia: " + rows.map(r => dayMonth(r.day) + " " + fmt.format(r.total)).join("; ")}>
      <BarChart data={data} margin={{ top: 8, right: 18, bottom: 0, left: 0 }} barCategoryGap={rows.length > 45 ? "12%" : "22%"}>
        <CartesianGrid vertical={false} stroke="var(--grid)" />
        <XAxis dataKey="day" {...axis} ticks={dayTicks(rows.map(r => r.day), 5)} tickFormatter={dayMonth} interval={0} tickMargin={8} />
        <YAxis {...axis} width={34} allowDecimals={false} tickFormatter={v => compact.format(v)} />
        <Tooltip cursor={{ fill: "rgb(255 248 244 / 0.04)" }} content={(p: TooltipContentProps<ValueType, NameType>) => {
          const r = p.payload?.[0]?.payload as (typeof data)[number] | undefined;
          return p.active && r ? <TipBox title={fullDay(r.day)} total={r.total}
            rows={[...keys].reverse().filter(k => r[k]).map(k => ({ color: color(k), label: name(k), value: r[k] as number }))} /> : null;
        }} />
        {keys.map(k => (
          <Bar key={k} dataKey={k} stackId="a" fill={color(k)} isAnimationActive={false}
            fillOpacity={focus && focus !== k ? 0.22 : 1}
            shape={(s: any) => <Rectangle {...s} y={s.y + (s.height > 3 ? 2 : 0)} height={Math.max(0, s.height - (s.height > 3 ? 2 : 0))}
              radius={s.payload.__top === k ? [3, 3, 0, 0] : 0} style={{ transition: "fill-opacity .2s" }} />} />
        ))}
      </BarChart>
    </ChartContainer>
  );
}

// Colunas de uma série curta (histograma das faixas de tempo, vezes por dia de uma falha).
export function Columns({ rows, color, height = 150, highlight, unit, tick, title }: {
  rows: { key: string; label: string; value: number }[]; color: string; height?: number; highlight?: string; unit: [string, string];
  tick?: (key: string) => string; title?: (key: string, label: string) => string;
}) {
  return (
    <ChartContainer config={{ value: { label: unit[1], color } }} className="aspect-auto w-full" style={{ height }}
      role="img" aria-label={rows.map(r => r.label + ": " + fmt.format(r.value)).join("; ")}>
      <BarChart data={rows} margin={{ top: 6, right: 16, bottom: 0, left: 16 }} barCategoryGap="14%">
        <XAxis dataKey="key" {...axis} interval={0} tickFormatter={tick ?? (k => k)} tickMargin={6} height={22} />
        <Tooltip cursor={{ fill: "rgb(255 248 244 / 0.04)" }} content={(p: TooltipContentProps<ValueType, NameType>) => {
          const r = p.payload?.[0]?.payload as (typeof rows)[number] | undefined;
          return p.active && r ? <TipBox title={title ? title(r.key, r.label) : r.label} rows={[{ color, label: unit[r.value === 1 ? 0 : 1], value: r.value }]} /> : null;
        }} />
        <Bar dataKey="value" radius={[3, 3, 0, 0]} animationDuration={500}
          shape={(s: any) => <Rectangle {...s} fill={color} fillOpacity={!highlight || s.payload.key === highlight ? 1 : 0.45} />} />
      </BarChart>
    </ChartContainer>
  );
}

// Minibarra da linha de um problema: CSS puro, sem eixo (o detalhe abre o gráfico com eixo).
export function Spark({ slots, tone, label, className }: { slots: { label: string; value: number }[]; tone: "error" | "warn"; label: string; className?: string }) {
  const max = Math.max(1, ...slots.map(s => s.value));
  return (
    <span role="img" aria-label={label} className={cn("flex h-7 items-end gap-px", className)}>
      {slots.map((s, i) => (
        <i key={i} className={cn("min-w-[2px] flex-1 rounded-t-[1.5px]", s.value ? (tone === "error" ? "bg-destructive" : "bg-warn") : "bg-border-strong")}
          style={{ height: s.value ? Math.max(14, (s.value / max) * 100) + "%" : "2px" }} />
      ))}
    </span>
  );
}
