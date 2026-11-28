import { AnimatePresence, motion } from "motion/react";
import { AlertCircle, Check, ChevronRight, CircleCheck, CircleStop, Copy, CornerDownRight, TriangleAlert } from "lucide-react";
import { useState, type ReactNode } from "react";
import { BarList, Block, Empty, Mono, Section, useQ } from "@/components/kit";
import { Columns, DailyArea, Spark } from "@/components/charts";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { fieldsOf, recordKey, recordMarkdown, SAMPLE_TIME, sparkNote, sparkSlots, type Derived } from "@/lib/derive";
import { afterText, happened, headline, stateRows, stepText, type StateRow } from "@/lib/failure";
import { ago, compact, dayMonth, fmt, fullDay, when } from "@/lib/format";
import { section } from "@/lib/sections";
import type { FailureRecord, LogLine, Summary } from "@/lib/types";
import { cn } from "@/lib/utils";

export function LevelIcon({ level, className }: { level: string; className?: string }) {
  const error = level === "error";
  const Icon = error ? AlertCircle : TriangleAlert;
  return <Icon role="img" aria-label={error ? "erro" : "aviso"} strokeWidth={1.9}
    className={cn("size-4 shrink-0", error ? "text-destructive" : "text-warn", className)} />;
}

export function LevelBadge({ level }: { level: string }) {
  const error = level === "error";
  return (
    <span className={cn("inline-flex h-6 items-center gap-1 rounded-md px-1.5 text-[11.5px] font-medium",
      error ? "bg-destructive/15 text-destructive" : "bg-warn/15 text-warn")}>
      <LevelIcon level={level} className="size-3.5" />{error ? "erro" : "aviso"}
    </span>
  );
}

type Sort = "machines" | "count" | "recent";
const recordText = (r: FailureRecord) => { const h = happened(r); return [headline(r), r.event, r.level === "error" ? "erro" : "aviso", h.summary, h.cause ?? "",
  ...fieldsOf(r).map(f => f.key + " " + f.label + " " + f.value), ...(r.trail ?? []), ...(r.versions ?? []), ...(r.os ?? [])].join(" "); };

function CopyForAgent({ r }: { r: FailureRecord }) {
  const [state, setState] = useState<"idle" | "busy" | "done" | "manual">("idle");
  const md = recordMarkdown(r);
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-3">
        <Button className="h-11 sm:h-9" disabled={state === "busy"} onClick={async () => {
          setState("busy");
          try { await navigator.clipboard.writeText(md); setState("done"); setTimeout(() => setState(s => (s === "done" ? "idle" : s)), 2500); }
          catch { setState("manual"); }
        }}>
          {state === "done" ? <Check /> : <Copy />}{state === "done" ? "Copiado" : "Copiar para o agente"}
        </Button>
        <span role="status" className="text-[12.5px] text-muted-foreground">
          {state === "manual" ? "Não deu para copiar. Selecione o texto abaixo e copie à mão." : "Markdown com os campos, os passos e onde aconteceu."}
        </span>
      </div>
      {state === "manual" ? <textarea readOnly rows={8} value={md} aria-label="Texto para copiar manualmente" autoFocus onFocus={e => e.currentTarget.select()}
        className="w-full rounded-lg border bg-deep p-3 font-mono text-[12px]" /> : null}
    </div>
  );
}

// Nome técnico, só para quem vai mexer no código: menor e apagado.
const Tech = ({ children, className }: { children: ReactNode; className?: string }) =>
  <span className={cn("font-mono text-[11px] text-faint [overflow-wrap:anywhere]", className)}>{children}</span>;
const H4 = ({ children }: { children: ReactNode }) => <h4 className="mb-2 text-[12.5px] font-medium text-muted-foreground">{children}</h4>;
const Caption = ({ children }: { children: ReactNode }) => <p className="mb-1 text-[11.5px] font-medium uppercase tracking-[0.06em] text-faint">{children}</p>;

function WhatHappened({ r }: { r: FailureRecord }) {
  const h = happened(r);
  const cause = typeof r.fields?.causa === "string" ? r.fields.causa : null;
  return (
    <div>
      <H4>O que aconteceu</H4>
      <div className="space-y-1.5 rounded-lg border bg-card px-3 py-2.5">
        <p className="text-[14px] font-medium text-pretty">{h.summary}</p>
        {h.tool ? <p className="text-[13px]">{r.event === "mcp.tool" ? "Ferramenta" : "Ação"}: {h.tool}</p> : null}
        {h.cause ? <p className="text-[13px] text-pretty">Por quê: {h.cause}.</p>
          : cause ? <p className="text-[13px] text-muted-foreground">A causa ainda não tem nome neste painel.</p> : null}
        {r.capped ? <p className="text-[12.5px] text-warn">A contagem bateu no teto: foram mais de {fmt.format(r.count)} vezes.</p> : null}
        <Tech className="block pt-0.5">{h.technical}</Tech>
      </div>
    </div>
  );
}

function StateGroup({ title, rows }: { title: string; rows: StateRow[] }) {
  if (!rows.length) return null;
  return (
    <div>
      <Caption>{title}</Caption>
      <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border">
        {rows.map((row, i) => (
          <div key={row.key} title={row.key} className={cn("min-w-0 bg-card px-3 py-2", rows.length % 2 && i === rows.length - 1 && "col-span-2")}>
            <dt className="text-[11.5px] text-muted-foreground">{row.label}</dt>
            <dd className="text-[13px] break-words">{row.value}</dd>
            {row.first ? <dd className="text-[11.5px] text-faint">na primeira vez: {row.first}</dd> : null}
          </div>
        ))}
      </dl>
    </div>
  );
}

function MachineState({ r }: { r: FailureRecord }) {
  const { build, machine, moment } = stateRows(r);
  return (
    <div>
      <H4>Em que estado a máquina estava</H4>
      {build.length || machine.length || moment.length ? (
        <div className="space-y-3">
          <StateGroup title={r.count > 1 ? "Na última vez" : "Na hora da falha"} rows={moment} />
          <StateGroup title="A máquina" rows={machine} />
          <StateGroup title="O Hangar" rows={build} />
        </div>
      ) : <p className="text-[13px] text-muted-foreground">Esta falha veio de uma versão que ainda não mandava o estado da máquina.</p>}
    </div>
  );
}

function Steps({ steps, r }: { steps: string[]; r: FailureRecord }) {
  return (
    <ol className="relative space-y-0.5 before:absolute before:top-3 before:bottom-3 before:left-[9px] before:w-px before:bg-border-strong">
      {steps.map((s, i) => { const t = stepText(s); return (
        <li key={i} className="relative flex min-h-8 items-center gap-3 pl-0.5">
          <span className="relative z-10 grid size-[18px] shrink-0 place-items-center rounded-full border border-border-strong bg-card text-[10px] text-muted-foreground tabular">{i + 1}</span>
          {t.known ? <span className="min-w-0 flex-1 text-[13px]">{t.text}</span> : <Mono className="min-w-0 flex-1 text-muted-foreground">{t.text}</Mono>}
          {t.known ? <Tech className="hidden max-w-[40%] truncate pr-2 sm:block">{s}</Tech> : null}
        </li>
      ); })}
      <li className={cn("relative flex min-h-9 items-center gap-3 rounded-md pl-0.5", r.level === "error" ? "bg-destructive/10" : "bg-warn/10")}>
        <span className="relative z-10 grid size-[18px] shrink-0 place-items-center rounded-full bg-card"><LevelIcon level={r.level} /></span>
        <span className="min-w-0 flex-1 text-[13px] font-medium">Falhou aqui</span>
        <Tech className="max-w-[45%] truncate pr-2">{r.event}</Tech>
      </li>
    </ol>
  );
}

const AFTER_ICON: Record<string, typeof CircleCheck> = { recovered: CircleCheck, other_action: CornerDownRight, none: CircleStop };

function Reproduce({ r }: { r: FailureRecord }) {
  const first = r.trail ?? [], last = r.trail_last ?? [];
  const showLast = r.count > 1 && last.length > 0 && JSON.stringify(last) !== JSON.stringify(first);
  const after = afterText(r.after);
  const Icon = r.after && Object.hasOwn(AFTER_ICON, r.after) ? AFTER_ICON[r.after] : CornerDownRight;
  return (
    <div className="space-y-3">
      <H4>Como reproduzir</H4>
      <div>
        <Caption>{showLast ? "Antes da primeira vez" : "Passos antes da falha"}</Caption>
        {first.length ? <Steps steps={first} r={r} /> : <p className="text-[13px] text-muted-foreground">O diário não guardou passos antes desta falha.</p>}
      </div>
      {showLast ? (
        <div>
          <Caption>Antes da última vez</Caption>
          <Steps steps={last} r={r} />
        </div>
      ) : null}
      {after ? (
        <p className="flex items-start gap-2 rounded-lg border px-3 py-2 text-[13px] text-pretty">
          <Icon className={cn("mt-0.5 size-4 shrink-0", r.after === "recovered" ? "text-ok" : "text-muted-foreground")} strokeWidth={1.9} />
          <span><span className="text-muted-foreground">Depois da última vez: </span>{after}</span>
        </p>
      ) : null}
    </div>
  );
}

function Detail({ r, days, skipToday }: { r: FailureRecord; days: number; skipToday: boolean }) {
  const fields = fieldsOf(r);
  const wide = (f: { key: string; value: string }) => f.key === "detalhe" || f.value.length > 60;
  const narrow = fields.filter(f => !wide(f));
  // Número ímpar de campos curtos: o último ocupa a linha toda, sem buraco na grade.
  const lone = narrow.length % 2 ? narrow.at(-1) : undefined;
  const slots = sparkSlots(r.daily, days, skipToday);
  const meta: [string, string][] = [["Versões", (r.versions ?? []).join(", ")], ["Sistemas", (r.os ?? []).join(", ")],
    ["Primeira vez no período", when(r.first) + " UTC"], ["Última no período", when(r.last) + " UTC"],
    ...(r.sample_at ? [[SAMPLE_TIME, when(r.sample_at) + " UTC"] as [string, string]] : [])];
  return (
    <div className="space-y-5 border-t px-4 pt-4 pb-5 sm:px-5">
      <div className="grid gap-5 lg:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]">
        <div className="min-w-0 space-y-5">
          <WhatHappened r={r} />
          <MachineState r={r} />
        </div>
        <div className="min-w-0 space-y-4">
          <Reproduce r={r} />
          <CopyForAgent r={r} />
        </div>
      </div>
      <div className="grid gap-5 lg:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]">
        <div className="min-w-0 space-y-5">
          <dl className="grid grid-cols-2 gap-x-4 gap-y-3 text-[12.5px] sm:grid-cols-3">
            {meta.map(([k, v]) => <div key={k} className="min-w-0"><dt className="text-muted-foreground">{k}</dt><dd className="font-medium break-words">{v}</dd></div>)}
          </dl>
          <div>
            <h4 className="mb-1 text-[12.5px] font-medium text-muted-foreground">Vezes {sparkNote(days)}</h4>
            <Columns rows={slots.map(x => ({ key: x.label, label: x.label.startsWith("semana") ? "Semana até " + fullDay(x.label.slice(-10)) : fullDay(x.label), value: x.value }))}
              unit={["vez", "vezes"]} height={120} color={r.level === "error" ? "var(--destructive)" : "var(--warn)"}
              tick={k => (k === slots[0].label || k === slots.at(-1)!.label ? dayMonth(k.slice(-10)) : "")} />
          </div>
        </div>
        <div className="min-w-0">
          <H4>Campos da amostra</H4>
          {fields.length ? (
            <dl className="grid gap-px overflow-hidden rounded-lg border bg-border sm:grid-cols-2">
              {fields.map(f => (
                <div key={f.key} className={cn("min-w-0 bg-card px-3 py-2", (wide(f) || f === lone) && "sm:col-span-2")}>
                  <dt className="text-[11.5px] text-muted-foreground">{f.label}</dt>
                  <dd className={cn("text-[13px] break-words", (f.key === "codigo" || f.key === "action" || f.key === "causa") && "font-mono text-[12.5px]")}>{f.value}</dd>
                </div>
              ))}
            </dl>
          ) : <p className="text-[13px] text-muted-foreground">Sem campos nesta falha.</p>}
        </div>
      </div>
    </div>
  );
}

function Issue({ id, r, days, skipToday, open, onToggle }: { id: string; r: FailureRecord; days: number; skipToday: boolean; open: boolean; onToggle: () => void }) {
  const technical = [r.event, r.action, r.codigo, typeof r.fields?.causa === "string" ? r.fields.causa : null, r.etapa, r.erro_tipo].filter(Boolean).join(" · ");
  const slots = sparkSlots(r.daily, days, skipToday);
  return (
    <li className={cn("border-b last:border-b-0", open && "bg-[color-mix(in_srgb,var(--foreground)_2.5%,transparent)]")}>
      <button type="button" onClick={onToggle} aria-expanded={open} aria-controls={id}
        className="grid w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 px-4 py-3 text-left transition-colors hover:bg-accent/40 sm:px-5 md:grid-cols-[auto_minmax(0,1fr)_120px_64px_64px_84px_auto]">
        <LevelIcon level={r.level} className="self-start mt-0.5 md:self-center md:mt-0" />
        <span className="min-w-0">
          <span className="block text-[13.5px] font-medium">{headline(r)}</span>
          <span className="mt-0.5 block font-mono text-[11px] text-faint break-all">{technical}</span>
          <span className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11.5px] text-muted-foreground md:hidden">
            <span><b className="font-medium text-foreground tabular">{compact.format(r.count)}</b> {r.count === 1 ? "vez" : "vezes"}</span>
            <span><b className="font-medium text-foreground tabular">{fmt.format(r.machines)}</b> {r.machines === 1 ? "máquina" : "máquinas"}</span>
            <span>{ago(r.last)}</span>
            {r.versions?.map(v => <span key={v} className="rounded bg-muted px-1.5 py-px font-mono">{v}</span>)}
          </span>
        </span>
        <ChevronRight className={cn("size-4 text-faint transition-transform duration-200 md:order-last", open && "rotate-90")} />
        <Spark slots={slots} tone={r.level === "error" ? "error" : "warn"} className="hidden md:flex"
          label={"Vezes " + sparkNote(days) + ": " + slots.map(s => s.label + " " + s.value).join(", ")} />
        <span className="hidden text-right md:block"><b className="block text-[14px] font-medium tabular">{compact.format(r.count)}</b><small className="text-[11px] text-muted-foreground">{r.count === 1 ? "vez" : "vezes"}</small></span>
        <span className="hidden text-right md:block"><b className="block text-[14px] font-medium tabular">{fmt.format(r.machines)}</b><small className="text-[11px] text-muted-foreground">{r.machines === 1 ? "máquina" : "máquinas"}</small></span>
        <span className="hidden text-right md:block" title={"Última no período: " + when(r.last) + " UTC"}>
          <b className="block text-[13px] font-medium">{ago(r.last)}</b><small className="text-[11px] text-muted-foreground">desde {dayMonth(r.first)}</small>
        </span>
      </button>
      <AnimatePresence initial={false}>
        {open ? (
          <motion.div id={id} key="d" initial={{ height: 0, opacity: 0 }} animate={{ height: "auto", opacity: 1 }} exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.22, ease: [0.23, 1, 0.32, 1] }} className="overflow-hidden">
            <Detail r={r} days={days} skipToday={skipToday} />
          </motion.div>
        ) : null}
      </AnimatePresence>
    </li>
  );
}

function Issues({ records, total, days, skipToday }: { records: FailureRecord[]; total: number; days: number; skipToday: boolean }) {
  const [level, setLevel] = useState<"" | "error" | "warn">("");
  const [sort, setSort] = useState<Sort>("machines");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [all, setAll] = useState(false);
  const q = useQ();
  const errors = records.filter(r => r.level === "error").length;
  // A busca escolhe o que existe; o nível e a ordem ficam dentro do cartão, que não some ao filtrar.
  const arrange = (xs: FailureRecord[]) => {
    const kept = xs.filter(r => !level || r.level === level);
    if (sort === "count") return [...kept].sort((a, b) => b.count - a.count);
    if (sort === "recent") return [...kept].sort((a, b) => b.last.localeCompare(a.last));
    return kept;
  };
  return (
    <Block id="failure-details" title="Falhas com detalhe" items={records} text={recordText} flush
      explain={"A mesma falha de todas as máquinas numa linha; a minibarra mostra as vezes " + sparkNote(days) + ". Abra para ver os campos e os passos da amostra ou copiar o contexto para um agente."}
      action={null}>
      {found => { const shown = arrange(found); return (
        <>
          <div className="flex flex-wrap items-center gap-2 border-t px-4 py-2.5 sm:px-5">
            <ToggleGroup type="single" value={level || "all"} onValueChange={v => v && setLevel(v === "all" ? "" : (v as "error" | "warn"))} variant="outline" size="sm" aria-label="Nível">
              <ToggleGroupItem value="all" className="h-10 px-3 sm:h-8">Todas · {records.length}</ToggleGroupItem>
              <ToggleGroupItem value="error" className="h-10 px-3 sm:h-8">Erros · {errors}</ToggleGroupItem>
              <ToggleGroupItem value="warn" className="h-10 px-3 sm:h-8">Avisos · {records.length - errors}</ToggleGroupItem>
            </ToggleGroup>
            <Select value={sort} onValueChange={v => setSort(v as Sort)}>
              <SelectTrigger className="ml-auto h-10! w-auto min-w-44 sm:h-8!" aria-label="Ordenar"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="machines">Mais máquinas</SelectItem>
                <SelectItem value="count">Mais vezes</SelectItem>
                <SelectItem value="recent">Mais recente</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {total > records.length ? <p className="border-t px-5 py-2 text-[12px] text-muted-foreground">O painel recebeu {fmt.format(total)} falhas distintas no período. A lista está limitada a {fmt.format(records.length)}.</p> : null}
          {shown.length ? (
            <ul className="border-t">
              {(all || q ? shown : shown.slice(0, 20)).map(r => {
                const k = recordKey(r);
                return <Issue key={k} id={"rec-" + records.indexOf(r)} r={r} days={days} skipToday={skipToday} open={open.has(k)}
                  onToggle={() => setOpen(s => { const n = new Set(s); if (n.has(k)) n.delete(k); else n.add(k); return n; })} />;
              })}
            </ul>
          ) : <div className="border-t p-4"><Empty>Nenhuma falha com esse filtro.</Empty></div>}
          {shown.length > 20 && !q ? (
            <div className="border-t px-3 py-2"><Button variant="ghost" className="h-10 text-muted-foreground" onClick={() => setAll(a => !a)}>{all ? "Mostrar só as 20" : "Mostrar todas as " + shown.length}</Button></div>
          ) : null}
        </>
      ); }}
    </Block>
  );
}

export function Problems({ data, d }: { data: Summary; d: Derived }) {
  const s = section("problems");
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon}
      aside={<Big value={d.failTotal} note="falhas nos contadores" />}>
      {d.failures.length ? (
        <div className="grid gap-4 lg:grid-cols-2">
          <Block title="Falhas nos contadores" explain="Cada linha é uma falha com o motivo; a barra compara com a maior." items={d.failures} text={f => f.label}>
            {shown => <BarList items={shown.map(f => ({ ...f, text: f.label }))} limit={10} />}
          </Block>
          <Block title="Falhas por dia" explain="Soma das falhas de cada dia.">
            <DailyArea rows={d.failuresDaily} unit={["falha", "falhas"]} color={s.hue} label="Falhas por dia" height={240} />
          </Block>
        </div>
      ) : <Block title="Falhas nos contadores"><Empty tone="ok">Nenhuma falha nos contadores do período.</Empty></Block>}
      {data.records.length ? <Issues records={data.records} total={data.records_total ?? data.records.length} days={data.days} skipToday={d.skipToday} />
        : <Block title="Falhas com detalhe"><Empty tone="ok">Nenhuma falha com detalhe no período.</Empty></Block>}
    </Section>
  );
}

export function Big({ value, note }: { value: number | string; note: string }) {
  return (
    <div className="hidden shrink-0 text-right sm:block">
      <b className="block text-[22px] font-semibold leading-tight tracking-[-0.02em] tabular">{typeof value === "number" ? fmt.format(value) : value}</b>
      <small className="text-[12px] text-muted-foreground">{note}</small>
    </div>
  );
}

const logText = (l: LogLine) => l.message + " " + l.target + " " + (l.level === "error" ? "erro" : "aviso");
export function Log({ data }: { data: Summary }) {
  const s = section("log");
  const [level, setLevel] = useState<"" | "error" | "warn">("");
  const logs = data.logs;
  const errors = logs.filter(l => l.level === "error").length;
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon} aside={<Big value={logs.length} note="linhas distintas" />}>
      {!logs.length ? <Block title="Log de erros"><Empty tone="ok">Nenhuma linha de erro no período.</Empty></Block> : (
        <Block title="Linhas do período" flush items={logs} text={logText}
          explain="Cada linha distinta, somada entre as máquinas. Mais máquinas primeiro.">
          {found => { const shown = found.filter(l => !level || l.level === level); return (
            <>
              <div className="flex flex-wrap items-center gap-2 border-t px-4 py-2.5 sm:px-5">
                <ToggleGroup type="single" value={level || "all"} onValueChange={v => v && setLevel(v === "all" ? "" : (v as "error" | "warn"))} variant="outline" size="sm" aria-label="Nível">
                  <ToggleGroupItem value="all" className="h-10 px-3 sm:h-8">Todos · {logs.length}</ToggleGroupItem>
                  <ToggleGroupItem value="error" className="h-10 px-3 sm:h-8">Erros · {errors}</ToggleGroupItem>
                  <ToggleGroupItem value="warn" className="h-10 px-3 sm:h-8">Avisos · {logs.length - errors}</ToggleGroupItem>
                </ToggleGroup>
                <span className="ml-auto text-[12px] text-muted-foreground">{shown.length} de {logs.length} linhas</span>
              </div>
              {shown.length ? (
                <ul className="border-t">
                  {shown.map((l, i) => (
                    <li key={i} className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 border-b px-4 py-3 last:border-b-0 sm:px-5 md:grid-cols-[auto_minmax(0,1fr)_auto]">
                      <LevelBadge level={l.level} />
                      <div className="min-w-0">
                        <Mono className="block text-foreground">{l.message}</Mono>
                        <span className="block font-mono text-[11.5px] text-muted-foreground break-all">{l.target}</span>
                      </div>
                      <div className="col-start-2 flex flex-wrap gap-x-3 gap-y-0.5 text-[12px] text-muted-foreground md:col-start-3 md:justify-end md:text-right">
                        <span><b className="font-medium text-foreground tabular">{fmt.format(l.count)}</b> {l.count === 1 ? "vez" : "vezes"}</span>
                        <span><b className="font-medium text-foreground tabular">{l.machines}</b> {l.machines === 1 ? "máquina" : "máquinas"}</span>
                        <span>de {dayMonth(l.first_day)} a {dayMonth(l.last_day)}</span>
                        <span className="font-mono">{(l.versions ?? []).join(", ")}</span>
                      </div>
                    </li>
                  ))}
                </ul>
              ) : <div className="border-t p-4"><Empty>Nenhuma linha com esse filtro.</Empty></div>}
            </>
          ); }}
        </Block>
      )}
    </Section>
  );
}
