import { ChevronRight } from "lucide-react";
import { useState } from "react";
import { BarList, Block, Empty, ErrorBox, Group, Mono, Section } from "@/components/kit";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { bucketLabel, dimsLabel, type Derived } from "@/lib/derive";
import { fmt, localStamp } from "@/lib/format";
import { OS_LABEL, VALUES } from "@/lib/labels";
import { section } from "@/lib/sections";
import type { Summary } from "@/lib/types";
import { cn } from "@/lib/utils";
import { standingText, versionView, type Tier } from "@/lib/versions";
import { Big } from "./problems";

const ENV: [string, string, string][] = [["os", "Sistema", "Sistema operacional de cada máquina."], ["channel", "Canal de atualização", "Estável recebe só versões publicadas; testes recebe antes."]];

export function Machines({ data, d }: { data: Summary; d: Derived }) {
  const s = section("machines");
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon} aside={<Big value={data.active.machines_30d} note="ativas em 30 dias" />}>
      <Versions data={data} />
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
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

const osName = (os: string) => OS_LABEL[os] ?? os;
const TIERS: [Exclude<Tier, "none">, string, string][] = [["ok", "Na mais atual", "bg-ok"], ["near", "1 ou 2 atrás", "bg-warn"], ["far", "3 ou mais atrás", "bg-destructive"]];
const CHIP: Record<Tier, string> = { ok: "bg-ok/15 text-ok", near: "bg-warn/15 text-warn", far: "bg-destructive/15 text-destructive", none: "bg-secondary text-muted-foreground" };
const LIST_LIMIT = 12;
const TITLE = "Em dia com a última versão";

// Sem a comparação (versões publicadas não lidas, receptor antigo): só as versões de cada máquina.
function VersionsOnly({ data, children }: { data: Summary; children: React.ReactNode }) {
  const items = data.env.filter(e => e.key === "version").map(e => ({ key: e.value, text: e.value, label: <span className="font-mono text-[12.5px]">{e.value}</span>, value: e.machines }));
  return (
    <Block title={TITLE} explain="Versão instalada em cada máquina, pelo último pacote dela." items={items} text={i => i.text}>
      {shown => <>{children}{shown.length ? <BarList items={shown} unit={["máquina", "máquinas"]} limit={8} /> : <Empty>Sem dados.</Empty>}</>}
    </Block>
  );
}

function Versions({ data }: { data: Summary }) {
  const [all, setAll] = useState(false);
  const { machines, published } = data;
  if (!machines || !published) {
    return <VersionsOnly data={data}><ErrorBox message="O receptor não mandou as versões publicadas, então não dá para dizer quem está atrás." /></VersionsOnly>;
  }
  if (!Object.keys(published.channels).length) {
    return (
      <VersionsOnly data={data}>
        {published.error
          ? <ErrorBox message="Não deu para ler as versões publicadas no GitHub, então não dá para dizer quem está atrás." detail={published.error} />
          : <p role="status" className="text-[13px] text-muted-foreground">Lendo as versões publicadas no GitHub…</p>}
      </VersionsOnly>
    );
  }
  if (!machines.length) return <Block title={TITLE}><Empty>Nenhuma máquina mandou pacote no período.</Empty></Block>;
  const v = versionView(machines, published);
  const counted = v.ok + v.near + v.far;
  const rows = v.rows.map(r => ({ ...r, text: [r.machine.label, osName(r.machine.os), r.machine.version, standingText(r.standing)].join(" ") }));
  return (
    <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
      <Block title={TITLE} explain="Pelo último pacote de cada máquina no período, comparado com o que está publicado no canal e no sistema dela.">
        <div className="text-[30px] font-semibold leading-none tracking-[-0.02em] tabular">
          {fmt.format(v.ok)} <small className="text-[14px] font-normal tracking-normal text-muted-foreground">de {fmt.format(counted)} máquina{counted === 1 ? "" : "s"} na mais atual</small>
        </div>
        {counted ? (
          <div role="img" aria-label={TIERS.map(([t, label]) => label + ": " + v[t]).join(", ")} className="flex h-2.5 gap-0.5 overflow-hidden rounded-md">
            {TIERS.map(([t, label, bg]) => v[t] ? <i key={t} title={label + ": " + v[t]} className={cn("block", bg)} style={{ flex: v[t] }} /> : null)}
          </div>
        ) : null}
        <div className="flex flex-wrap gap-x-4 gap-y-1 text-[12.5px] text-muted-foreground">
          {TIERS.map(([t, label, bg]) => (
            <span key={t} className="inline-flex items-center gap-1.5"><span className={cn("size-2.5 rounded-[3px]", bg)} />{label} <b className="font-semibold text-foreground tabular">{v[t]}</b></span>
          ))}
          {v.outside ? <span>Fora da conta (checkout, outro canal ou sem versão publicada) <b className="font-semibold text-foreground tabular">{v.outside}</b></span> : null}
        </div>
        <div className="grid grid-cols-2 gap-2.5">
          {v.latest.map(l => (
            <div key={l.channel + l.os} className="rounded-lg border bg-deep px-3 py-2.5">
              <div className="text-[12px] text-muted-foreground">Mais atual no {osName(l.os)}{l.channel === "latest" ? "" : " · " + (VALUES.channel[l.channel] ?? l.channel)}</div>
              <Mono className="block">{l.version}</Mono>
              <div className="text-[12px] text-muted-foreground">publicada {localStamp(l.at)}</div>
            </div>
          ))}
        </div>
        {published.error ? <p className="text-[12px] text-muted-foreground">A última leitura no GitHub falhou ({published.error}); os canais já lidos usam a leitura anterior.</p> : null}
      </Block>
      <Block title="Cada máquina" explain="As máquinas são anônimas: o código é o começo do identificador dela." items={rows} text={r => r.text}>
        {shown => (
          <div>
            <ul className="divide-y">
              {(all || shown.length < rows.length ? shown : shown.slice(0, LIST_LIMIT)).map(r => (
                <li key={r.machine.label + r.machine.os} className="flex min-h-12 items-center gap-3 py-2.5 text-[13px]">
                  <div className="min-w-0 flex-1">
                    <Mono>{r.machine.label}</Mono> · {osName(r.machine.os)}
                    <div className="text-[12px] text-muted-foreground"><Mono>{r.machine.version}</Mono> · último envio {localStamp(r.machine.last)}</div>
                  </div>
                  <span className={cn("shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[12px] font-medium", CHIP[r.tier])}>
                    {r.tier === "ok" ? "✓ " : ""}{standingText(r.standing)}
                  </span>
                </li>
              ))}
            </ul>
            {rows.length > LIST_LIMIT && shown.length === rows.length ? (
              <Button variant="ghost" size="sm" className="h-9 text-muted-foreground" onClick={() => setAll(a => !a)} aria-expanded={all}>
                {all ? "Mostrar só " + LIST_LIMIT : "Mostrar todas as " + fmt.format(rows.length)}
              </Button>
            ) : null}
          </div>
        )}
      </Block>
    </div>
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
