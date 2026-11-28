import { useState } from "react";
import { BarList, Block, Empty, ErrorBox, Mono, Section, useQ, matches } from "@/components/kit";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useTrail } from "@/lib/api";
import { fmt, fullDay, quantity } from "@/lib/format";
import { section } from "@/lib/sections";
import type { Summary, TrailDay } from "@/lib/types";
import { cn } from "@/lib/utils";
import { Big, LevelBadge } from "./problems";

function DayTrail({ days, total }: { days: TrailDay[]; total: number }) {
  const key = (t: TrailDay) => t.machine + "|" + t.day;
  const [pick, setPick] = useState(key(days[0]));
  const [onlyProblems, setOnlyProblems] = useState(false);
  const chosen = days.find(t => key(t) === pick) ?? days[0];
  const trail = useTrail(chosen.machine, chosen.day);
  const q = useQ();
  const steps = (trail.data?.machine === chosen.machine && trail.data.day === chosen.day ? trail.data.steps : null);
  const shown = steps?.filter(st => (!onlyProblems || st.level !== "ok") && matches(q, [st.t, st.event, st.screen, st.level === "error" ? "erro" : st.level === "warn" ? "aviso" : ""].join(" ")));
  return (
    <Block title="Um dia de uso" explain="Passos de uma máquina na ordem registrada, com horários em UTC. Falhas e avisos ficam marcados."
      items={[0]} text={() => (steps ?? []).map(st => st.event + " " + st.t).join(" ") + " " + days.map(t => t.label + " " + t.os + " " + t.version).join(" ")}>
      {() => (
        <div className="space-y-3">
          {total > days.length ? <p className="text-[12px] text-muted-foreground">O painel recebeu {fmt.format(total)} dias de máquinas com caminho. A lista está limitada a {fmt.format(days.length)}.</p> : null}
          <div className="flex flex-wrap items-end gap-2">
            <label className="min-w-0 flex-1 basis-64 text-[12px] text-muted-foreground">
              Máquina e dia
              <Select value={key(chosen)} onValueChange={setPick}>
                <SelectTrigger className="mt-1 h-11! w-full sm:h-9!"><SelectValue /></SelectTrigger>
                <SelectContent className="max-h-80">
                  {days.map(t => (
                    <SelectItem key={key(t)} value={key(t)}>
                      {fullDay(t.day)} · máquina <span className="font-mono">{t.label}</span> · {t.os} {t.version}{t.problems ? " · " + quantity(t.problems, "problema", "problemas") : ""}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </label>
            <ToggleGroup type="single" variant="outline" size="sm" value={onlyProblems ? "p" : "all"} onValueChange={v => v && setOnlyProblems(v === "p")} aria-label="Filtrar passos">
              <ToggleGroupItem value="all" className="h-11 px-3 sm:h-9">Todos</ToggleGroupItem>
              <ToggleGroupItem value="p" className="h-11 px-3 sm:h-9">Só falhas e avisos</ToggleGroupItem>
            </ToggleGroup>
          </div>
          {trail.isError && !steps ? <ErrorBox message="Não foi possível ler o caminho desse dia." detail={(trail.error as Error).message} onRetry={() => trail.refetch()} />
            : !steps ? <div className="space-y-2" aria-busy="true">{Array.from({ length: 5 }, (_, i) => <Skeleton key={i} className="h-9" />)}</div>
            : (
              <>
                <p role="status" className="text-[12.5px] text-muted-foreground">
                  {shown!.length ? quantity(shown!.length, "passo", "passos") + "." : !steps.length ? "Nenhum passo recebido nesse dia." : "Nenhuma falha nem aviso nesse dia."}
                  {trail.data?.partial ? " Há pacotes parciais neste dia: os passos ainda podem mudar." : ""}
                  {trail.isError ? <span className="text-destructive"> Não foi possível reler este dia; os passos podem estar velhos.</span> : null}
                </p>
                <ol className={cn("max-h-[460px] overflow-y-auto overscroll-contain rounded-lg border", trail.isFetching && "opacity-80")}>
                  {shown!.map((st, i) => (
                    <li key={i} className={cn("grid grid-cols-[64px_minmax(0,1fr)] items-center gap-3 border-b px-3 py-2 last:border-b-0",
                      st.level === "error" && "bg-destructive/8", st.level === "warn" && "bg-warn/7")}>
                      <time dateTime={chosen.day + "T" + st.t + "Z"} className="font-mono text-[12px] text-muted-foreground tabular">{st.t}</time>
                      <span className="flex min-w-0 flex-wrap items-center gap-2">
                        <Mono>{st.event}</Mono>
                        {st.screen ? <span className="rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground">{st.screen}</span> : null}
                        {st.count > 1 ? <span className="text-[12px] text-muted-foreground tabular">×{fmt.format(st.count)}</span> : null}
                        {st.level !== "ok" ? <LevelBadge level={st.level} /> : null}
                      </span>
                    </li>
                  ))}
                </ol>
              </>
            )}
        </div>
      )}
    </Block>
  );
}

export function Trail({ data }: { data: Summary }) {
  const s = section("trail");
  const transitions = data.transitions ?? [], days = data.trail_days ?? [];
  const dayTotal = data.trail_days_total ?? days.length;
  const items = transitions.map(t => ({ key: t.from + ">" + t.to, label: <span className="font-mono text-[12.5px]">{t.from} <span className="text-faint">→</span> {t.to}</span>,
    text: t.from + " → " + t.to, value: t.count, machines: t.machines }));
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon} aside={<Big value={dayTotal} note={dayTotal === 1 ? "dia com caminho" : "dias com caminho"} />}>
      {!transitions.length && !days.length ? (
        <Block title="Caminho de uso"><Empty>Nenhuma máquina enviou o caminho de uso no período. Ele chega a cada hora nas versões que coletam o diário.</Empty></Block>
      ) : (
        <div className="grid gap-4 xl:grid-cols-2">
          {days.length ? <DayTrail days={days} total={dayTotal} /> : <Block title="Um dia de uso"><Empty>Nenhum dia com caminho no período.</Empty></Block>}
          <Block title="Passagens mais comuns" explain="De uma ação para a seguinte dentro de cada pacote, somando todas as máquinas." items={items} text={i => i.text}>
            {shown => shown.length ? (
              <>
                {data.transitions_total > transitions.length ? <p className="text-[12px] text-muted-foreground">O painel recebeu {fmt.format(data.transitions_total)} passagens distintas no período. A lista está limitada a {fmt.format(transitions.length)}.</p> : null}
                <BarList items={shown} limit={12} />
              </>
            ) : <Empty>Nenhuma passagem no período.</Empty>}
          </Block>
        </div>
      )}
    </Section>
  );
}
