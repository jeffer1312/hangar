import { Bug, Check, Cloud, Globe, Key, Link, List, MessageCircle, Monitor, Pencil, Plus, RefreshCw, Reply, Search, Send, Smartphone, Square, SquareArrowOutUpRight,
  SquareTerminal, Users, Wrench, X, Zap, FileText, FilePen, type LucideIcon } from "lucide-react";
import { useState } from "react";
import { BarList, Block, Empty, Group, Num, Section, useQ, matches } from "@/components/kit";
import { DailyArea, StackedDays } from "@/components/charts";
import { AGENT_COLOR, AgentLogo, agentName, type AgentKey } from "@/lib/agents";
import { effortColor, effortLabel, effortRank, type Derived } from "@/lib/derive";
import { fmt, pct, pct0, quantity, upper } from "@/lib/format";
import { AGENTS, EVENTS, FEATURE_ICON, MODEL_LABEL, TOOLS, VALUES, type IconName } from "@/lib/labels";
import { section } from "@/lib/sections";
import { cn } from "@/lib/utils";
import { Big } from "./problems";

const ICONS: Record<IconName, LucideIcon> = { search: Search, send: Send, reply: Reply, square: Square, link: Link, list: List, bug: Bug, pencil: Pencil,
  key: Key, x: X, globe: Globe, users: Users, refresh: RefreshCw, terminal: SquareTerminal, plus: Plus, external: SquareArrowOutUpRight, filePen: FilePen,
  file: FileText, check: Check, chat: MessageCircle, wrench: Wrench, zap: Zap };
const Ico = ({ name }: { name: IconName }) => { const I = ICONS[name] ?? Zap; return <I className="size-4" strokeWidth={1.75} />; };

export function Agents({ d }: { d: Derived }) {
  const s = section("agents");
  const [pinned, setPinned] = useState<AgentKey | null>(null);
  const [hover, setHover] = useState<AgentKey | null>(null);
  const q = useQ();
  const sum = d.agents.reduce((n, k) => n + (d.agentTotals[k] ?? 0), 0);
  const ranked = [...d.agents].sort((x, y) => (d.agentTotals[y] ?? 0) - (d.agentTotals[x] ?? 0));
  const focus = hover ?? pinned;
  const max = d.agentTotals[ranked[0]] ?? 1;
  const visible = ranked.filter(k => matches(q, agentName(k) + " " + k));
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon} aside={<Big value={d.messages} note="mensagens enviadas" />}>
      {!sum ? <Block title="Mensagens por dia"><Empty>Nenhuma mensagem enviada no período.</Empty></Block> : (
        <div className="grid gap-4 xl:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)]">
          <Block title="Mensagens por dia" explain="Cada cor é um agente; a altura é o total do dia. Toque num agente ao lado para destacá-lo.">
            <StackedDays rows={d.agentDays} keys={d.agents} color={k => AGENT_COLOR[k]} name={agentName} focus={focus} height={280} />
          </Block>
          <Block title="Quem leva o uso" explain="A fatia de cada agente no período." items={ranked} text={k => agentName(k) + " " + k}>
            {() => (
              <div className="space-y-3">
                <div className="flex h-2.5 gap-0.5 overflow-hidden rounded-full" aria-hidden="true">
                  {ranked.map(k => <span key={k} className="grow-x transition-opacity" style={{ flex: d.agentTotals[k], background: AGENT_COLOR[k], opacity: focus && focus !== k ? 0.25 : 1 }} />)}
                </div>
                <ul className="space-y-0.5">
                  {visible.map(k => {
                    const n = d.agentTotals[k] ?? 0;
                    return (
                      <li key={k}>
                        <button type="button" aria-pressed={pinned === k} onClick={() => setPinned(p => (p === k ? null : k))}
                          onMouseEnter={() => setHover(k)} onMouseLeave={() => setHover(null)}
                          aria-label={agentName(k) + ": " + fmt.format(n) + " mensagens, " + pct.format(n / sum) + ". Destacar no gráfico"}
                          className={cn("grid min-h-11 w-full grid-cols-[22px_72px_minmax(0,1fr)_auto_44px] items-center gap-3 rounded-lg px-2 text-left text-[13px] transition-colors hover:bg-accent/50",
                            pinned === k && "bg-accent/70")}>
                          <AgentLogo provider={k} size={18} />
                          <span className="font-medium">{agentName(k)}</span>
                          <span className="h-1.5 overflow-hidden rounded-full bg-muted"><i className="grow-x block h-full rounded-full" style={{ width: (n / max) * 100 + "%", background: AGENT_COLOR[k] }} /></span>
                          <Num value={n} className="text-right font-medium" />
                          <span className="text-right text-[12px] text-muted-foreground tabular">{pct.format(n / sum)}</span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </div>
            )}
          </Block>
        </div>
      )}
      <Block title="Respostas do agente por dia" explain={EVENTS.turn_finished[2]}>
        <DailyArea rows={d.turnsDaily} unit={["resposta", "respostas"]} color={s.hue} label="Respostas do agente por dia" height={200} />
      </Block>
    </Section>
  );
}

export function Top({ d }: { d: Derived }) {
  const s = section("top");
  const fItems = d.features.map(([key, value]) => ({ key, value, text: (VALUES.feature[key] ?? key) + " " + key,
    label: VALUES.feature[key] ? upper(VALUES.feature[key]) : key, icon: <Ico name={FEATURE_ICON[key] ?? "zap"} /> }));
  const tItems = d.tools.map(([key, t]) => ({ key, value: t.value, text: (TOOLS[key]?.[0] ?? key) + " " + key,
    label: TOOLS[key]?.[0] ?? key, icon: <Ico name={TOOLS[key]?.[1] ?? "wrench"} />, note: t.failed ? quantity(t.failed, "falhou", "falharam") : null }));
  const fTotal = fItems.reduce((n, i) => n + i.value, 0), tTotal = tItems.reduce((n, i) => n + i.value, 0);
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon}>
      <div className="grid gap-4 lg:grid-cols-2">
        <Block title="Recursos mais usados" explain="Ações que deram certo no app." items={fItems} text={i => i.text}>
          {shown => shown.length ? <BarList items={shown} total={fTotal} limit={10} /> : <Empty>Nenhum recurso usado no período.</Empty>}
        </Block>
        <Block title="Ferramentas que os agentes mais chamam" explain="Chamadas às ferramentas do Hangar; em vermelho, as que falharam." items={tItems} text={i => i.text}>
          {shown => shown.length ? <BarList items={shown} total={tTotal} limit={10} unit={["chamada", "chamadas"]} /> : <Empty>Nenhuma ferramenta chamada no período.</Empty>}
        </Block>
      </div>
      <Group title="Outros eventos de uso">
        {d.usage.map(c => (
          <Block key={c.event} title={c.title} explain={c.explain} items={c.bars} text={b => b.label}>
            {shown => <BarList items={shown.map(b => ({ ...b, text: b.label }))} limit={8} />}
          </Block>
        ))}
      </Group>
    </Section>
  );
}

const TOP_MODELS = 8;
export function Effort({ d }: { d: Derived }) {
  const s = section("effort");
  const rows = d.models.map(m => ({ ...m, name: MODEL_LABEL[m.model] ?? m.model,
    efforts: [...m.efforts].sort((x, y) => effortRank(x[0]) - effortRank(y[0])),
    most: [...m.efforts].sort((x, y) => y[1] - x[1])[0][0] }));
  const text = (m: (typeof rows)[number]) => [m.name, m.model, m.provider, ...m.efforts.map(([e]) => effortLabel(e))].join(" ");
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon}>
      <Block title="Sessões abertas por modelo" explain="Cada faixa da barra é um nível de esforço, do mais leve ao maior." items={rows} text={text}>
        {shown => {
          if (!shown.length) return <Empty>Nenhuma sessão aberta no período.</Empty>;
          const top = shown.slice(0, TOP_MODELS), max = top[0].total;
          const used = [...new Set(top.flatMap(m => m.efforts.map(([e]) => e)))].sort((x, y) => effortRank(x) - effortRank(y));
          return (
            <div className="space-y-4">
              <div className="flex flex-wrap gap-x-4 gap-y-1.5 text-[12px] text-muted-foreground">
                {used.map(e => <span key={e} className="flex items-center gap-1.5"><i className="size-2.5 rounded-[3px]" style={{ background: effortColor(e) }} />{effortLabel(e)}</span>)}
              </div>
              <ul className="space-y-1">
                {top.map(m => (
                  <li key={m.provider + m.model} className="grid min-h-10 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1.5 sm:grid-cols-[200px_minmax(0,1fr)_128px]">
                    <span className="flex min-w-0 items-center gap-2" title={(AGENTS[m.provider] ?? m.provider) + " · " + m.name}>
                      <AgentLogo provider={m.provider} size={16} /><b className="truncate text-[13px] font-medium">{m.name}</b>
                    </span>
                    <span className="col-span-2 row-start-2 flex h-3 overflow-hidden rounded-full sm:col-span-1 sm:row-start-1 sm:col-start-2" style={{ width: Math.max(4, (m.total / max) * 100) + "%" }}>
                      {m.efforts.map(([e, n], i) => (
                        <i key={e} className="grow-x h-full" title={m.name + " · " + effortLabel(e) + ": " + quantity(n, "sessão", "sessões") + " (" + pct0.format(n / m.total) + ")"}
                          style={{ flex: n, background: effortColor(e), marginLeft: i ? 2 : 0 }} />
                      ))}
                    </span>
                    <span className="text-right text-[12px] text-muted-foreground sm:col-start-3">
                      <Num value={m.total} className="block text-[13px] font-medium text-foreground" />mais: {effortLabel(m.most)}
                    </span>
                  </li>
                ))}
              </ul>
              {shown.length > TOP_MODELS ? <p className="text-[12px] text-muted-foreground">+ {shown.length - TOP_MODELS} modelos em Dados brutos</p> : null}
            </div>
          );
        }}
      </Block>
    </Section>
  );
}

const DEVICES: [string, string, LucideIcon, string][] = [["native", "App do PC", Monitor, "#8b93f0"], ["ios", "iPhone/iPad", Smartphone, "#45c2a8"],
  ["android", "Android", Smartphone, "#e8a23a"], ["pwa", "Navegador", Globe, "#5aaee8"]];
const SURFACES: [string, string, LucideIcon, string][] = [["terminal", "com terminal", SquareTerminal, "#d97757"], ["headless", "sem terminal", Cloud, "#b48cf0"]];

export function Places({ d }: { d: Derived }) {
  const s = section("places");
  const known = DEVICES.map(x => x[0]);
  const devices = [...DEVICES, ...[...d.minutes.keys()].filter(k => !known.includes(k)).sort()
    .map(k => [k, VALUES.client[k] ?? k, Monitor, "#857c81"] as [string, string, LucideIcon, string])].filter(([k]) => d.minutes.get(k));
  const total = devices.reduce((n, [k]) => n + d.minutes.get(k)!, 0);
  const knownS = SURFACES.map(x => x[0]);
  const kinds = [...SURFACES, ...[...d.surfaces.keys()].filter(k => !knownS.includes(k)).sort().map(k => [k, k, SquareTerminal, "#857c81"] as [string, string, LucideIcon, string])]
    .filter(([k]) => d.surfaces.get(k));
  const sTotal = kinds.reduce((n, [k]) => n + d.surfaces.get(k)!, 0);
  return (
    <Section id={s.id} title={s.name} explain={s.explain} hue={s.hue} icon={s.icon}>
      <div className="grid gap-4 lg:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)]">
        <Block title="Minutos com o app aberto" explain="Tempo de tela de cada app no período." items={devices} text={([k, label]) => label + " " + k}>
          {shown => shown.length ? (
            <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
              {shown.map(([k, label, Icon, color]) => {
                const share = d.minutes.get(k)! / total;
                return (
                  <div key={k} className="flex min-w-0 flex-col gap-1 rounded-lg bg-muted/50 p-3">
                    <Icon className="size-4" style={{ color }} strokeWidth={1.75} />
                    <Num value={share * 100} format={v => Math.round(v) + "%"} className="mt-1 text-[22px] font-semibold tracking-[-0.02em]" />
                    <span className="text-[13px]">{label}</span>
                    <span className="text-[12px] text-muted-foreground tabular">{fmt.format(d.minutes.get(k)!)} min</span>
                    <span className="mt-1 h-1 overflow-hidden rounded-full bg-border"><i className="grow-x block h-full rounded-full" style={{ width: share * 100 + "%", background: color }} /></span>
                  </div>
                );
              })}
            </div>
          ) : <Empty>Nenhum app mandou o tempo de tela no período.</Empty>}
        </Block>
        <Block title="Com ou sem terminal" explain="Mensagens a sessões com terminal (tmux) e sem terminal." items={kinds} text={([, label]) => label}>
          {shown => shown.length ? (
            <div className="space-y-4">
              <div className="flex h-3 gap-0.5 overflow-hidden rounded-full" aria-hidden="true">
                {kinds.map(([k, , , color]) => <span key={k} className="grow-x" style={{ flex: d.surfaces.get(k), background: color }} />)}
              </div>
              <ul className="space-y-2">
                {shown.map(([k, label, Icon, color]) => (
                  <li key={k} className="flex items-center gap-3 text-[13px]">
                    <Icon className="size-4" style={{ color }} strokeWidth={1.75} />
                    <b className="w-12 text-[18px] font-semibold tracking-[-0.02em] tabular">{pct0.format(d.surfaces.get(k)! / sTotal)}</b>
                    <span className="flex-1">{label}</span>
                    <span className="text-[12px] text-muted-foreground tabular">{fmt.format(d.surfaces.get(k)!)}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : <Empty>Nenhuma mensagem enviada no período.</Empty>}
        </Block>
      </div>
    </Section>
  );
}
