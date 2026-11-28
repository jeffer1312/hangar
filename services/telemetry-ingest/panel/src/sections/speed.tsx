import { useState } from "react";
import { Bot, Check, DoorOpen, MessageSquare, Plus, Server, Smartphone, TriangleAlert, X, type LucideIcon } from "lucide-react";
import { Block, Empty, Group, Section, useQ, matches } from "@/components/kit";
import { Columns } from "@/components/charts";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { AGENT_COLOR, agentKey, agentName } from "@/lib/agents";
import { quantileBucket, type Derived } from "@/lib/derive";
import { fmt, quantity } from "@/lib/format";
import { section } from "@/lib/sections";
import { SPEED_BUCKETS, SPEED_EVENTS, speedCards, type SpeedCard, type System, type Verdict } from "@/lib/speed";
import type { Summary } from "@/lib/types";
import { cn } from "@/lib/utils";

const TEXT: Record<string, { title: string; explain: string; icon: LucideIcon; unit: [string, string]; by: string; tech: string }> = {
  session_ready: { title: "Abrir uma conversa nova", explain: "Do toque em \"Nova conversa\" até o agente estar pronto para receber a mensagem.",
    icon: Plus, unit: ["abertura", "aberturas"], by: "Por sistema", tech: "opened_by=person" },
  session_first_send: { title: "Um agente abrindo outra sessão", explain: "Do pedido da sessão por outro agente até a primeira mensagem dele chegar nela.",
    icon: Bot, unit: ["abertura", "aberturas"], by: "Por agente aberto", tech: "opened_by=agent" },
  first_reply: { title: "Primeira resposta do agente", explain: "Da mensagem entregue ao agente até a resposta começar a aparecer.",
    icon: MessageSquare, unit: ["mensagem", "mensagens"], by: "Por agente", tech: "provider" },
  app_ready: { title: "Abrir o app", explain: "Do app começar a abrir até a lista de sessões aparecer.",
    icon: Smartphone, unit: ["abertura", "aberturas"], by: "Por app", tech: "client" },
  chat_open: { title: "Entrar numa conversa que já existe", explain: "Do toque na conversa até ela aparecer desenhada.",
    icon: DoorOpen, unit: ["vez", "vezes"], by: "Por app", tech: "client" },
  server_startup: { title: "Ligar o Hangar", explain: "Do servidor começar a subir até responder.",
    icon: Server, unit: ["subida", "subidas"], by: "Por sistema", tech: "" },
};
const VERDICT: Record<Verdict, { label: string; icon: LucideIcon; className: string }> = {
  ok: { label: "Rápido", icon: Check, className: "bg-ok/14 text-ok" },
  warn: { label: "Pode melhorar", icon: TriangleAlert, className: "bg-warn/14 text-warn" },
  bad: { label: "Lento", icon: X, className: "bg-destructive/14 text-destructive" },
  few: { label: "Poucas medidas", icon: TriangleAlert, className: "bg-muted text-muted-foreground" },
};
// Da faixa mais rápida (verde) à mais lenta (vermelho).
const SHADES = ["#2f9e5c", "#4cc777", "#8fd9a6", "#e8c35a", "#ffab2e", "#ff8a5c", "#ff6b61"];
const SYSTEMS: [System, string][] = [["all", "Todos"], ["windows", "Windows"], ["linux", "Linux"], ["macos", "macOS"]];
const OS: Record<string, string> = { windows: "Windows", linux: "Linux", macos: "macOS" };
const CLIENT: Record<string, string> = { native: "Desktop", ios: "iPhone", android: "Android", pwa: "Navegador" };
const LEVEL_COLOR = (level: number) => SHADES[level];

function lineLabel(card: SpeedCard, key: string) {
  if (card.by === "provider") return { label: agentName(agentKey(key) === "outro" ? key : agentKey(key)), color: AGENT_COLOR[agentKey(key)] };
  if (card.by === "client") return { label: CLIENT[key] ?? key, color: "#7c87e8" };
  return { label: OS[key] ?? key, color: "" };
}

function Card({ card, index, searching }: { card: SpeedCard; index: number; searching: boolean }) {
  const t = TEXT[card.event];
  const v = VERDICT[card.verdict];
  return (
    <div data-block data-hits={searching ? 1 : undefined} className="speed-card flex min-w-0 flex-col gap-3 rounded-xl border bg-card p-4" style={{ "--i": index } as React.CSSProperties}>
      <div className="flex items-start gap-2.5">
        <span className="grid size-8 shrink-0 place-items-center rounded-[9px] bg-[color-mix(in_srgb,var(--hue)_16%,transparent)] text-(--hue)">
          <t.icon className="size-[18px]" strokeWidth={1.9} />
        </span>
        <div className="min-w-0">
          <h3 className="text-[15px] font-semibold leading-snug text-balance">{t.title}</h3>
          <p className="text-[13px] text-muted-foreground text-pretty">{t.explain}</p>
        </div>
      </div>
      {!card.total ? <Empty>Ainda sem dados desta versão.</Empty> : (
        <>
          <div>
            <div className="flex flex-wrap items-baseline gap-2.5">
              <span className="text-[26px] font-[650] tracking-[-0.01em] tabular">{card.usually}</span>
              <span className={cn("inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[12px] font-semibold", v.className)}>
                <v.icon className="size-3.5" strokeWidth={2.2} />{v.label}
              </span>
            </div>
            <p className="text-[13px] text-muted-foreground">Normalmente · 9 em 10: {card.nine} · {quantity(card.total, t.unit[0], t.unit[1])}</p>
          </div>
          <div className="space-y-1.5">
            <div role="img" aria-label={"Distribuição: " + SPEED_BUCKETS.map((b, j) => b + " " + card.buckets[j]).join(", ")}
              className="speed-bar flex h-2.5 gap-0.5 overflow-hidden rounded-[5px]">
              {card.buckets.map((n, j) => n ? <i key={j} className="block h-full" style={{ flex: n / card.total, background: SHADES[j] }} /> : null)}
            </div>
            <div className="flex flex-wrap gap-x-3 text-[11.5px] text-faint">
              <span><b className="mr-1 inline-block size-2 rounded-[2px] bg-[#4cc777] align-[-1px]" />até 1 s</span>
              <span><b className="mr-1 inline-block size-2 rounded-[2px] bg-[#ffab2e] align-[-1px]" />1 a 10 s</span>
              <span><b className="mr-1 inline-block size-2 rounded-[2px] bg-[#ff6b61] align-[-1px]" />mais de 10 s</span>
            </div>
          </div>
          {card.rows.length ? (
            <div className="flex flex-col gap-1.5 border-t pt-2.5">
              <p className="text-[12px] text-muted-foreground">{t.by} · normalmente</p>
              {card.rows.slice(0, 6).map(line => {
                const { label, color } = lineLabel(card, line.key);
                const tone = color || LEVEL_COLOR(line.level);
                return (
                  <div key={line.key} title={quantity(line.total, t.unit[0], t.unit[1])} className="grid grid-cols-[96px_minmax(0,1fr)_auto] items-center gap-2.5 text-[13px]">
                    <span className="flex min-w-0 items-center gap-1.5 text-muted-foreground"><i className="size-2 shrink-0 rounded-full" style={{ background: tone }} /><span className="truncate">{label}</span></span>
                    <span className="h-1.5 overflow-hidden rounded-[3px] bg-white/5"><i className="block h-full rounded-[3px]" style={{ width: ((line.level + 1) / SPEED_BUCKETS.length) * 100 + "%", background: tone }} /></span>
                    <span className="whitespace-nowrap tabular">{line.usually}</span>
                  </div>
                );
              })}
            </div>
          ) : null}
        </>
      )}
      <div className="mt-auto flex justify-between gap-2 border-t border-dashed pt-2 font-mono text-[11px] text-faint">
        <span>{card.event}</span><span>{t.tech}</span>
      </div>
    </div>
  );
}

const SHORT: Record<string, string> = { lt100ms: "<0,1s", lt300ms: "<0,3s", lt1s: "<1s", lt3s: "<3s", lt10s: "<10s", lt30s: "<30s", ge30s: "30s+" };

export function Speed({ data, d }: { data: Summary; d: Derived }) {
  const s = section("speed");
  const [system, setSystem] = useState<System>("all");
  const outer = useQ();
  const q = matches(outer, s.name) ? "" : outer;
  const cards = speedCards(data.speed ?? [], system).filter(c => !q || matches(q, TEXT[c.event].title + " " + TEXT[c.event].explain + " " + c.event));
  const others = d.speed.filter(c => !SPEED_EVENTS.includes(c.event));
  if (q && !cards.length && !others.length) return null;
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon}>
      <ToggleGroup type="single" variant="outline" size="sm" className="flex-wrap justify-start" value={system} onValueChange={v => v && setSystem(v as System)} aria-label="Sistema">
        {SYSTEMS.map(([value, label]) => <ToggleGroupItem key={value} value={value} className="h-11 px-3 sm:h-8">{value === "all" ? <>Todos<span className="max-sm:hidden">&nbsp;os sistemas</span></> : label}</ToggleGroupItem>)}
      </ToggleGroup>
      <div className="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(min(320px,100%),1fr))]">
        {cards.map((c, i) => <Card key={c.event} card={c} index={i} searching={!!outer} />)}
      </div>
      {others.length ? (
        <Group title="Outros tempos">
          {others.map(c => {
            const median = quantileBucket(new Map(c.bars.map(b => [b.key, b.value])), 0.5);
            return (
              <Block key={c.event} title={c.title} explain={c.explain} items={c.bars} text={b => b.label}>
                {() => (
                  <>
                    <p className="text-[13.5px] font-medium text-pretty">{c.sentence}</p>
                    <Columns rows={c.bars.map(b => ({ key: b.key, label: b.label, value: b.value }))} color={s.hue} highlight={median}
                      unit={["vez", "vezes"]} tick={k => SHORT[k] ?? k} height={140} />
                    <p className="text-[12px] text-muted-foreground">{quantity(c.total, "medida", "medidas")} no período · todos os sistemas.</p>
                  </>
                )}
              </Block>
            );
          })}
        </Group>
      ) : null}
      <p className="sr-only">{fmt.format(cards.reduce((n, c) => n + c.total, 0))} medidas no bloco.</p>
    </Section>
  );
}
