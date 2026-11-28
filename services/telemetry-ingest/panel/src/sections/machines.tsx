import { ChevronRight } from "lucide-react";
import { useState } from "react";
import { BarList, Block, Empty, Group, Mono, Section } from "@/components/kit";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { bucketLabel, dimsLabel, type Derived } from "@/lib/derive";
import { fmt } from "@/lib/format";
import { VALUES } from "@/lib/labels";
import { section } from "@/lib/sections";
import type { Summary } from "@/lib/types";
import { cn } from "@/lib/utils";
import { Big } from "./problems";

const ENV: [string, string, string][] = [["version", "Versão do Hangar", "Versão instalada em cada máquina, pelo último pacote dela."],
  ["os", "Sistema", "Sistema operacional de cada máquina."], ["channel", "Canal de atualização", "Estável recebe só versões publicadas; testes recebe antes."]];

export function Machines({ data, d }: { data: Summary; d: Derived }) {
  const s = section("machines");
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon} aside={<Big value={data.active.machines_30d} note="ativas em 30 dias" />}>
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
        {ENV.map(([key, title, explain]) => {
          const items = data.env.filter(e => e.key === key).map(e => ({ key: e.value, text: e.value,
            label: key === "channel" ? VALUES.channel[e.value] ?? e.value : <span className={key === "version" ? "font-mono text-[12.5px]" : ""}>{e.value}</span>, value: e.machines }));
          return (
            <Block key={key} title={title} explain={explain} items={items} text={i => i.text}>
              {shown => shown.length ? <BarList items={shown} unit={["máquina", "máquinas"]} limit={8} /> : <Empty>Sem dados.</Empty>}
            </Block>
          );
        })}
        <Block title="Máquinas por pessoa" explain="Quantas pessoas usam o Hangar em 1, 2, 3… máquinas (últimos 30 dias)."
          items={data.per_owner} text={p => p.machines + " máquinas"}>
          {shown => shown.length ? <BarList unit={["pessoa", "pessoas"]}
            items={shown.map(p => ({ key: String(p.machines), label: p.machines + " máquina" + (p.machines === 1 ? "" : "s"), value: p.owners }))} /> : <Empty>Sem dados.</Empty>}
        </Block>
      </div>
      {d.health.length ? (
        <Group title="Saúde do servidor">
          {d.health.map(c => (
            <Block key={c.event} title={c.title} explain={c.explain} items={c.bars} text={b => b.label}>
              {shown => <BarList items={shown.map(b => ({ ...b, text: b.label }))} limit={8} />}
            </Block>
          ))}
        </Group>
      ) : null}
    </Section>
  );
}

export function Settings({ d }: { d: Derived }) {
  const s = section("settings");
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon}>
      {!d.settingGroups.length ? <Block title="Preferências"><Empty>Nenhuma máquina mandou as preferências ainda: elas chegam a partir da versão que coleta esse retrato.</Empty></Block>
        : d.settingGroups.map(([group, cards]) => (
          <Group key={group} title={group}>
            {cards.map(c => (
              <Block key={c.event} title={c.title} explain={c.explain} items={c.bars} text={b => b.label}>
                {shown => <BarList items={shown.map(b => ({ ...b, text: b.label }))} unit={["máquina", "máquinas"]} />}
              </Block>
            ))}
          </Group>
        ))}
    </Section>
  );
}

export function Raw({ data }: { data: Summary }) {
  const s = section("raw");
  const [open, setOpen] = useState(false);
  const rows = data.counters.map((c, i) => ({ i, c, detail: dimsLabel(c.dims), bucket: c.bucket ? bucketLabel(c.bucket) : "" }));
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon}>
      <Block title={rows.length + " linhas, como chegaram"} items={rows} text={r => r.c.event + " " + r.detail + " " + r.bucket}
        action={<button type="button" onClick={() => setOpen(o => !o)} aria-expanded={open}
          className="inline-flex min-h-10 items-center gap-1 rounded-md px-2 text-[13px] text-primary hover:bg-accent/50">
          {open ? "Esconder" : "Mostrar"}<ChevronRight className={cn("size-4 transition-transform", open && "rotate-90")} /></button>}>
        {(shown, ) => (open || shown.length < rows.length) ? (
          shown.length ? (
            <>
              <div className="hidden md:block">
                <Table>
                  <TableHeader><TableRow><TableHead>Evento</TableHead><TableHead>Detalhe</TableHead><TableHead>Duração</TableHead>
                    <TableHead className="text-right">Vezes</TableHead><TableHead className="text-right">Máquinas</TableHead></TableRow></TableHeader>
                  <TableBody>
                    {shown.map(({ i, c, detail, bucket }) => (
                      <TableRow key={i}>
                        <TableCell><Mono>{c.event}</Mono></TableCell>
                        <TableCell className="whitespace-normal">{detail}</TableCell>
                        <TableCell className="text-muted-foreground">{bucket}</TableCell>
                        <TableCell className="text-right tabular">{fmt.format(c.count)}</TableCell>
                        <TableCell className="text-right tabular">{c.machines}</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
              <ul className="divide-y md:hidden">
                {shown.map(({ i, c, detail, bucket }) => (
                  <li key={i} className="flex items-start gap-3 py-2.5">
                    <div className="min-w-0 flex-1"><Mono className="block">{c.event}</Mono><span className="text-[12.5px] text-muted-foreground">{detail}{bucket ? " · " + bucket : ""}</span></div>
                    <div className="text-right text-[12px] text-muted-foreground"><b className="block text-[13px] font-medium text-foreground tabular">{fmt.format(c.count)}</b>{c.machines} máq.</div>
                  </li>
                ))}
              </ul>
            </>
          ) : <Empty>Nenhuma linha.</Empty>
        ) : null}
      </Block>
    </Section>
  );
}
