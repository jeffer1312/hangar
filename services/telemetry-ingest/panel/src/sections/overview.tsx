import { ArrowRight, CalendarX, Info, MessagesSquare, Monitor, TriangleAlert, Users, type LucideIcon } from "lucide-react";
import { Area, AreaChart } from "recharts";
import { BarList, Block, Empty, Num, Section } from "@/components/kit";
import { DailyArea } from "@/components/charts";
import type { Derived } from "@/lib/derive";
import { compact, fmt, pct0, quantity } from "@/lib/format";
import { section } from "@/lib/sections";
import type { Summary } from "@/lib/types";
import { cn } from "@/lib/utils";

function Kpi({ href, icon: Icon, label, value, note, hue, bad, trend }: {
  href: string; icon: LucideIcon; label: string; value: number; note: string; hue: string; bad?: boolean; trend?: { value: number }[];
}) {
  return (
    <a href={href} className={cn("group relative flex min-h-[120px] flex-col overflow-hidden rounded-xl border bg-card p-4 transition-colors hover:border-border-strong sm:p-5",
      bad && "border-destructive/35 bg-[color-mix(in_srgb,var(--destructive)_7%,var(--card))]")}>
      <span className="flex items-center gap-2 text-[12.5px] text-muted-foreground">
        <Icon className="size-4" style={{ color: bad ? "var(--destructive)" : hue }} strokeWidth={1.75} />{label}
        <ArrowRight className="ml-auto size-3.5 opacity-0 transition-opacity group-hover:opacity-60" />
      </span>
      <Num value={value} format={v => compact.format(Math.round(v))} className="mt-2 text-[30px] font-semibold leading-none tracking-[-0.025em]" />
      <span className="mt-auto pt-2 text-[12px] text-muted-foreground">{note}</span>
      {trend && trend.length > 1 ? (
        <div className="pointer-events-none absolute right-0 bottom-0 hidden h-12 w-[42%] opacity-70 md:block" aria-hidden="true">
          <AreaChart width={160} height={48} data={trend} margin={{ top: 4, right: 0, bottom: 0, left: 0 }} style={{ width: "100%", height: "100%" }}>
            <Area dataKey="value" type="monotone" stroke={hue} strokeWidth={1.5} fill={hue} fillOpacity={0.12} isAnimationActive={false} />
          </AreaChart>
        </div>
      ) : null}
    </a>
  );
}

const ACCEPT = { send: "#5fb98a", declined: "#d97757", revoked: "#c08a1c" };
function Acceptance({ data }: { data: Summary }) {
  const send = data.period.machines || 0, declined = data.refusals?.declined || 0, revoked = data.refusals?.revoked || 0;
  const refused = declined + revoked, total = send + refused;
  const parts = ([["send", send, ["enviou", "enviam"]], ["declined", declined, ["recusou no aviso", "recusaram no aviso"]],
    ["revoked", revoked, ["desligou depois", "desligaram depois"]]] as const).filter(p => p[1]);
  return (
    <Block title="Aceite da telemetria" explain="Máquinas que mandaram pacote no período contra as recusas do período. As recusas contam uma vez por máquina e não trazem identificação.">
      {!total ? <Empty icon={Info}>Nenhuma máquina mandou pacote nem recusou no período.</Empty> : (
        <div className="space-y-4">
          <p className="text-[15px] font-medium">{refused ? "Cerca de " + pct0.format(send / total) + " das máquinas enviam dados" : "Nenhuma recusa no período."}</p>
          <div className="flex h-3 gap-0.5 overflow-hidden rounded-full" role="img"
            aria-label={parts.map(([, n, verb]) => fmt.format(n) + " " + verb[n === 1 ? 0 : 1]).join(", ")}>
            {parts.map(([k, n]) => <span key={k} className="grow-x min-w-[3px]" style={{ flex: n, background: ACCEPT[k] }} />)}
          </div>
          <div className="flex flex-wrap gap-x-8 gap-y-3">
            {parts.map(([k, n, verb]) => (
              <div key={k} className="flex items-baseline gap-2 text-[13px] text-muted-foreground">
                <i className="size-2.5 self-center rounded-[3px]" style={{ background: ACCEPT[k] }} aria-hidden="true" />
                <Num value={n} className="text-[20px] font-semibold tracking-[-0.02em] text-foreground" />
                <span>{(n === 1 ? "máquina " + verb[0] : "máquinas " + verb[1]) + " · " + pct0.format(n / total)}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </Block>
  );
}

export function Overview({ data, d }: { data: Summary; d: Derived }) {
  const s = section("overview"), a = data.active;
  if (!data.period.batches) {
    const refused = (data.refusals?.declined || 0) + (data.refusals?.revoked || 0);
    return (
      <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon}>
        <Block title="Nenhum pacote chegou neste período">
          <Empty icon={CalendarX}>Nas versões com envio horário, cada máquina atualiza o dia a cada hora e manda o pacote final depois que o dia fecha em UTC.</Empty>
        </Block>
        {refused ? <Acceptance data={data} /> : null}
      </Section>
    );
  }
  const partial = data.period.partial_batches || 0, dropped = d.droppedRecords + d.droppedSteps;
  const onlyRecords = !d.failTotal && data.records.length > 0;
  const notes = [
    partial ? "Dados parciais: " + quantity(partial, "pacote ainda não é final", "pacotes ainda não são finais") + "." : "",
    dropped ? "Corte no envio: " + [d.droppedRecords ? quantity(d.droppedRecords, "registro de falha não coube", "registros de falha não couberam") : "",
      d.droppedSteps ? quantity(d.droppedSteps, "passo do caminho não coube", "passos do caminho não couberam") : ""].filter(Boolean).join("; ") + "." : "",
  ].filter(Boolean);
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon}>
      {notes.length ? (
        <p className="flex items-start gap-2 rounded-lg bg-muted/60 px-3 py-2 text-[12.5px] text-muted-foreground">
          <Info className="mt-0.5 size-3.5 shrink-0" />{notes.join(" ")}
        </p>
      ) : null}
      <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
        <Kpi href="#s-machines" icon={Monitor} hue={section("machines").hue} label="Máquinas ativas" value={a.machines_30d}
          note={a.machines_7d + " nos últimos 7 dias"} trend={d.machinesDaily} />
        <Kpi href="#s-machines" icon={Users} hue={s.hue} label="Pessoas" value={a.owners_30d} note={a.owners_7d + " nos últimos 7 dias"} />
        <Kpi href="#s-agents" icon={MessagesSquare} hue={section("agents").hue} label="Mensagens enviadas" value={d.messages}
          note={fmt.format(d.turns) + " respostas do agente"} trend={d.agentDays.map(r => ({ value: r.total }))} />
        <Kpi href={onlyRecords ? "#failure-details" : "#s-problems"} icon={TriangleAlert} hue={section("problems").hue} bad={d.failTotal > 0}
          label={onlyRecords ? "Falhas nos contadores" : "Falhas"} value={d.failTotal}
          note={onlyRecords ? "Ver falhas com detalhe" : d.failTotal ? quantity(d.failures.length, "tipo de falha", "tipos de falha") : "nenhuma no período"} />
      </div>
      <div className="grid gap-4 lg:grid-cols-[1.4fr_1fr]">
        <Block title="Máquinas ativas por dia" explain="Quantas máquinas mandaram pacote em cada dia (UTC).">
          <DailyArea rows={d.machinesDaily} unit={["máquina", "máquinas"]} color={section("machines").hue} label="Máquinas ativas por dia" height={230} />
        </Block>
        <Block title="Falhas mais comuns" explain="O que mais deu errado no período.">
          {d.failTotal ? (
            <div style={{ "--hue": section("problems").hue } as React.CSSProperties} className="flex flex-1 flex-col">
              <BarList items={d.failures.slice(0, 5).map(f => ({ ...f, text: f.label }))} />
              <a href="#s-problems" className="mt-auto inline-flex min-h-11 items-center gap-1 pt-2 text-[13px] text-primary hover:underline">
                Ver as {fmt.format(d.failTotal)} falhas<ArrowRight className="size-3.5" />
              </a>
            </div>
          ) : onlyRecords ? <Empty icon={Info}>Há falhas com detalhe que não aparecem nos contadores do período. <a className="text-primary" href="#failure-details">Ver em Problemas</a></Empty>
            : <Empty tone="ok">Nenhuma falha no período.</Empty>}
        </Block>
      </div>
      <Acceptance data={data} />
    </Section>
  );
}
