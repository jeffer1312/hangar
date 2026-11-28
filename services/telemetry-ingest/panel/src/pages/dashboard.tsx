import { useQueryClient } from "@tanstack/react-query";
import { LogOut, Search, UserPlus, X, Copy, Check } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { BlockSkeleton, ErrorBox, QueryScope } from "@/components/kit";
import { Logo } from "@/components/logo";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { api, isUnauthorized, useLive, useSummary } from "@/lib/api";
import { derive } from "@/lib/derive";
import { compact, fmt } from "@/lib/format";
import { SECTIONS } from "@/lib/sections";
import { cn } from "@/lib/utils";
import { Overview } from "@/sections/overview";
import { Log, Problems } from "@/sections/problems";
import { Agents, Effort, Places, Top } from "@/sections/usage";
import { Speed, Trail } from "@/sections/flow";
import { Machines, Raw, Settings } from "@/sections/machines";

const PERIODS = [7, 30, 90];
const initialDays = () => { const n = Number(new URLSearchParams(location.search).get("days")); return PERIODS.includes(n) ? n : 30; };

function useNow(ms: number) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => { const t = setInterval(() => setNow(Date.now()), ms); return () => clearInterval(t); }, [ms]);
  return now;
}

function Fresh({ live, updatedAt, failed, fetching, onRefresh }: { live: boolean; updatedAt: number; failed: boolean; fetching: boolean; onRefresh: () => void }) {
  const now = useNow(5000);
  if (!updatedAt) return null;
  const s = Math.max(0, Math.round((now - updatedAt) / 1000));
  const age = s < 10 ? "agora" : s < 60 ? "há " + Math.floor(s / 10) * 10 + " s" : "há " + Math.floor(s / 60) + " min";
  const text = failed ? "Não atualizou · " + age : live ? "Ao vivo" : "Atualizado " + age;
  return (
    <button type="button" onClick={onRefresh} title={failed ? "A última releitura falhou. Toque para tentar de novo." : live ? "Atualiza quando chega pacote novo. Lido " + age + "." : "Relê a cada minuto. Toque para reler agora."}
      className={cn("inline-flex h-10 shrink-0 items-center gap-2 rounded-full border px-3 text-[12.5px] text-muted-foreground transition-colors hover:bg-accent/50 sm:h-8",
        failed && "border-destructive/40 text-destructive")}>
      <span className="relative flex size-2">
        {live && !failed ? <span className="absolute inline-flex size-full animate-ping rounded-full bg-ok opacity-60 motion-reduce:hidden" /> : null}
        <span className={cn("relative inline-flex size-2 rounded-full", failed ? "bg-destructive" : live ? "bg-ok" : "bg-faint", fetching && "animate-pulse")} />
      </span>
      <span className="whitespace-nowrap">{text}</span>
    </button>
  );
}

function Invite({ open, onOpenChange }: { open: boolean; onOpenChange: (v: boolean) => void }) {
  const [state, setState] = useState<{ link?: string; days?: number; error?: string }>({});
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!open) return;
    setState({}); setCopied(false);
    api<{ path: string; days: number }>("/panel/api/invites", { method: "POST" })
      .then(b => setState({ link: location.origin + b.path, days: b.days }))
      .catch(e => { if (isUnauthorized(e)) location.replace("/panel"); else setState({ error: (e as Error).message }); });
  }, [open]);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Convidar para o painel</DialogTitle>
          <DialogDescription>
            {state.error ? "Não deu para gerar o convite (" + state.error + ")." : state.link ? "Mande este link a quem você quer dar acesso. Vale " + state.days + " dias e cria uma conta só." : "Gerando o link…"}
          </DialogDescription>
        </DialogHeader>
        <div className="flex gap-2">
          <input readOnly value={state.link ?? ""} aria-label="Link do convite" onFocus={e => e.currentTarget.select()}
            className="h-11 min-w-0 flex-1 rounded-lg border bg-deep px-3 font-mono text-[12.5px] sm:h-9" />
          <Button className="h-11 sm:h-9" disabled={!state.link} onClick={async () => {
            try { await navigator.clipboard.writeText(state.link!); setCopied(true); setTimeout(() => setCopied(false), 2500); } catch { /* o campo fica selecionado para copiar à mão */ }
          }}>{copied ? <Check /> : <Copy />}{copied ? "Copiado" : "Copiar"}</Button>
        </div>
        <DialogFooter><Button variant="ghost" className="h-11 sm:h-9" onClick={() => onOpenChange(false)}>Fechar</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function Account({ email }: { email: string }) {
  const [invite, setInvite] = useState(false);
  const [logoutError, setLogoutError] = useState("");
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" aria-label="Conta" className="grid size-10 shrink-0 place-items-center rounded-full bg-secondary text-[13px] font-semibold uppercase ring-1 ring-border-strong hover:bg-accent sm:size-8">
            {email[0] ?? "?"}
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-64">
          <DropdownMenuLabel className="truncate font-normal text-muted-foreground">{email}</DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem className="min-h-11 sm:min-h-8" onSelect={() => setInvite(true)}><UserPlus />Convidar alguém</DropdownMenuItem>
          <DropdownMenuItem className="min-h-11 sm:min-h-8" onSelect={async e => {
            e.preventDefault();
            try {
              await api("/panel/api/logout", { method: "POST" });
              location.replace("/panel");
            } catch (err) {
              // 401: a sessão já tinha acabado, o que também é ter saído.
              if (isUnauthorized(err)) location.replace("/panel"); else setLogoutError((err as Error).message);
            }
          }}><LogOut />{logoutError ? "Não saiu (" + logoutError + "), tentar de novo" : "Sair"}</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Invite open={invite} onOpenChange={setInvite} />
    </>
  );
}

function SearchBox({ value, onChange, className }: { value: string; onChange: (v: string) => void; className?: string }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = document.activeElement?.tagName;
      if (e.key === "/" && t !== "INPUT" && t !== "TEXTAREA") { e.preventDefault(); ref.current?.focus(); }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);
  return (
    <label className={cn("relative flex h-11 min-w-0 items-center rounded-lg border bg-deep/60 transition-colors focus-within:border-ring sm:h-9", className)}>
      <Search className="ml-3 size-4 shrink-0 text-faint" />
      <input ref={ref} type="search" value={value} onChange={e => onChange(e.target.value)} autoComplete="off" aria-label="Buscar no painel"
        onKeyDown={e => { if (e.key === "Escape") { onChange(""); e.currentTarget.blur(); } }}
        placeholder="Buscar erro, passo, versão ou opção" className="h-full min-w-0 flex-1 bg-transparent px-2.5 text-[13.5px] outline-none placeholder:text-faint [&::-webkit-search-cancel-button]:hidden" />
      {value ? <button type="button" aria-label="Limpar busca" onClick={() => { onChange(""); ref.current?.focus(); }} className="mr-1 grid size-9 place-items-center rounded-md text-muted-foreground hover:bg-accent sm:size-7"><X className="size-4" /></button>
        : <kbd className="mr-2 hidden rounded border px-1.5 text-[11px] text-faint sm:block">/</kbd>}
    </label>
  );
}

function Period({ days, onChange }: { days: number; onChange: (d: number) => void }) {
  return (
    <ToggleGroup type="single" variant="outline" value={String(days)} onValueChange={v => v && onChange(Number(v))} aria-label="Período" className="shrink-0">
      {PERIODS.map(p => <ToggleGroupItem key={p} value={String(p)} className="h-11 px-3 text-[13px] sm:h-9">{p} dias</ToggleGroupItem>)}
    </ToggleGroup>
  );
}

export function Dashboard({ email }: { email: string }) {
  const [days, setDays] = useState(initialDays);
  const [q, setQ] = useState("");
  const live = useLive(true);
  const query = useSummary(days, live);
  const client = useQueryClient();
  const data = query.data;
  const d = useMemo(() => (data ? derive(data) : null), [data]);
  const needle = q.trim().toLowerCase();
  const [current, setCurrent] = useState("overview");
  const [found, setFound] = useState<{ hits: number; sections: Set<string> }>({ hits: 0, sections: new Set(SECTIONS.map(s => s.id)) });
  const main = useRef<HTMLElement>(null);

  useEffect(() => { history.replaceState(null, "", "?days=" + days + location.hash); }, [days]);
  useEffect(() => { if (isUnauthorized(query.error)) client.invalidateQueries({ queryKey: ["me"] }); }, [query.error, client]);

  // Depois de desenhar: quais seções têm resultado e quantos achados a busca deu.
  useLayoutEffect(() => {
    const root = main.current;
    if (!root) return;
    const secs = [...root.querySelectorAll<HTMLElement>("[data-section]")];
    const sections = new Set(secs.filter(s => !needle || s.querySelector("[data-block]")).map(s => s.dataset.section!));
    const hits = [...root.querySelectorAll<HTMLElement>("[data-hits]")].reduce((n, b) => n + Number(b.dataset.hits || 0), 0);
    setFound(f => (f.hits === hits && [...sections].join() === [...f.sections].join() ? f : { hits, sections }));
  });

  // A seção na tela acende na navegação.
  useEffect(() => {
    const root = main.current;
    if (!root) return;
    const seen = new Map<string, boolean>();
    const io = new IntersectionObserver(entries => {
      for (const e of entries) seen.set((e.target as HTMLElement).dataset.section!, e.isIntersecting);
      const first = SECTIONS.find(s => seen.get(s.id));
      if (first) setCurrent(first.id);
    }, { rootMargin: "-120px 0px -60% 0px" });
    root.querySelectorAll("[data-section]").forEach(s => io.observe(s));
    return () => io.disconnect();
  }, [d, needle]);

  // Na faixa do celular, o item aceso nunca fica escondido de lado.
  useEffect(() => {
    const chip = document.querySelector<HTMLElement>('[data-chip="' + current + '"]');
    const strip = chip?.parentElement;
    if (!chip || !strip) return;
    const c = chip.getBoundingClientRect(), b = strip.getBoundingClientRect();
    if (c.left < b.left + 8 || c.right > b.right - 8) strip.scrollTo({ left: strip.scrollLeft + c.left - b.left - (b.width - c.width) / 2, behavior: "smooth" });
  }, [current]);

  const counts: Record<string, number | undefined> = d && data ? { problems: d.failTotal, log: data.logs.length, agents: d.messages,
    trail: data.trail_days_total, speed: d.speed.length, machines: data.active.machines_30d, settings: d.settingCount, raw: data.counters.length } : {};
  const hot: Record<string, boolean> = d && data ? { problems: d.hasErrors, log: data.logs.some(l => l.level === "error") } : {};
  const nav = SECTIONS.filter(s => found.sections.has(s.id) && (s.id !== "speed" || (d?.speed.length ?? 0) > 0));
  const fresh = <Fresh live={live} updatedAt={query.dataUpdatedAt} failed={query.isError && !!data} fetching={query.isFetching} onRefresh={() => query.refetch()} />;
  const sub = data ? fmt.format(data.active.machines_30d) + (data.active.machines_30d === 1 ? " máquina · " : " máquinas · ") + fmt.format(data.active.owners_30d)
    + (data.active.owners_30d === 1 ? " pessoa" : " pessoas") + " · dados anônimos" : "dados anônimos, envio a cada hora";

  return (
    <div className="lg:grid lg:grid-cols-[232px_minmax(0,1fr)]" style={{ paddingLeft: "var(--sl)", paddingRight: "var(--sr)" }}>
      <aside className="sticky top-0 hidden h-dvh flex-col border-r bg-[color-mix(in_srgb,var(--deep)_55%,var(--background))] lg:flex">
        <div className="flex items-center gap-2.5 px-4 pt-5 pb-4">
          <Logo size={30} />
          <div className="min-w-0"><div className="text-[14px] font-semibold leading-tight">Telemetria</div><div className="truncate text-[11.5px] text-muted-foreground">do Hangar</div></div>
        </div>
        <nav aria-label="Seções" className="flex-1 space-y-px overflow-y-auto px-2 pb-4">
          {nav.map(s => (
            <a key={s.id} href={"#s-" + s.id} aria-current={current === s.id ? "true" : undefined}
              className={cn("flex h-9 items-center gap-2.5 rounded-lg px-2.5 text-[13px] text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground",
                current === s.id && "bg-accent text-foreground")}>
              <s.icon className="size-4 shrink-0" style={{ color: s.hue }} strokeWidth={1.75} />
              <span className="flex-1 truncate">{s.name}</span>
              {counts[s.id] ? <span className={cn("rounded-md px-1.5 text-[11px] tabular", hot[s.id] ? "bg-destructive/15 text-destructive" : "text-faint")}>{compact.format(counts[s.id]!)}</span> : null}
            </a>
          ))}
        </nav>
        <div className="flex items-center gap-2 border-t px-3 py-3">
          <Account email={email} />
          <span className="min-w-0 flex-1 truncate text-[12px] text-muted-foreground">{email}</span>
        </div>
      </aside>

      <div className="min-w-0">
        {/* Celular: o topo rola junto; só a faixa de seções fica presa. */}
        <header className="px-4 pt-[calc(var(--st)+12px)] pb-3 lg:hidden">
          <div className="flex items-center gap-3">
            <Logo size={30} />
            <div className="min-w-0 flex-1"><h1 className="text-[15px] font-semibold leading-tight">Telemetria do Hangar</h1><p className="truncate text-[12px] text-muted-foreground">{sub}</p></div>
            <Account email={email} />
          </div>
          <div className="mt-3 flex items-center gap-2"><SearchBox value={q} onChange={setQ} className="flex-1" /></div>
          <div className="mt-2 flex items-center justify-between gap-2"><Period days={days} onChange={setDays} />{fresh}</div>
        </header>
        {/* Faixa do entalhe: fixa e opaca, para nada passar por baixo do relógio do app instalado. */}
        <div aria-hidden="true" className="fixed inset-x-0 top-0 z-30 h-[var(--st)] bg-background lg:hidden" />
        <div className="sticky top-[var(--st)] z-20 border-y bg-background/92 backdrop-blur-md lg:hidden">
          <nav aria-label="Seções" className="flex gap-1.5 overflow-x-auto overscroll-x-contain px-4 py-2 [scrollbar-width:none]">
            {nav.map(s => (
              <a key={s.id} data-chip={s.id} href={"#s-" + s.id} aria-current={current === s.id ? "true" : undefined}
                className={cn("inline-flex h-10 shrink-0 items-center gap-1.5 rounded-full border px-3 text-[13px] text-muted-foreground",
                  current === s.id && "border-transparent bg-accent text-foreground")}>
                <s.icon className="size-3.5" style={{ color: s.hue }} strokeWidth={1.9} />{s.name}
                {counts[s.id] ? <em className={cn("not-italic text-[11px] tabular", hot[s.id] ? "text-destructive" : "text-faint")}>{compact.format(counts[s.id]!)}</em> : null}
              </a>
            ))}
          </nav>
        </div>

        <div className="sticky top-0 z-20 hidden items-center gap-3 border-b bg-background/90 px-6 py-3 backdrop-blur-md lg:flex xl:px-8">
          <div className="mr-2 min-w-0"><h1 className="text-[15px] font-semibold leading-tight">Telemetria do Hangar</h1><p className="truncate text-[12px] text-muted-foreground">{sub}</p></div>
          <SearchBox value={q} onChange={setQ} className="ml-auto w-full max-w-sm" />
          <Period days={days} onChange={setDays} />
          {fresh}
        </div>

        <main ref={main} data-searching={needle ? "" : undefined} aria-busy={query.isFetching}
          className={cn("mx-auto max-w-[1360px] space-y-12 px-4 pt-5 pb-[calc(var(--sb)+48px)] transition-opacity sm:px-6 lg:pt-6 xl:px-8",
            query.isPlaceholderData && "opacity-60")}>
          {needle ? (
            <div role="status" className="flex items-center gap-3 rounded-lg border bg-card px-4 py-2.5 text-[13px]">
              <Search className="size-4 text-faint" />
              <span className="flex-1">{found.hits ? fmt.format(found.hits) + (found.hits === 1 ? " resultado" : " resultados") + " para “" + q.trim() + "”" : "Nada encontrado para “" + q.trim() + "”."}</span>
              <Button variant="ghost" className="h-10 sm:h-8" onClick={() => setQ("")}>Limpar busca</Button>
            </div>
          ) : null}
          {!data ? (
            query.isError ? <ErrorBox message="Não foi possível ler os dados do painel." detail={(query.error as Error).message} onRetry={() => query.refetch()} /> : (
              <div className="space-y-4" aria-label="Carregando dados do painel">
                <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">{Array.from({ length: 4 }, (_, i) => <div key={i} className="h-[120px] animate-pulse rounded-xl border bg-card" />)}</div>
                <div className="grid gap-4 lg:grid-cols-[1.4fr_1fr]"><BlockSkeleton chart /><BlockSkeleton /></div>
                <BlockSkeleton rows={6} />
              </div>
            )
          ) : (
            <QueryScope value={needle}>
              {!needle ? <Overview data={data} d={d!} /> : null}
              {data.period.batches ? (
                <>
                  <Problems data={data} d={d!} />
                  <Log data={data} />
                  <Agents d={d!} />
                  <Top d={d!} />
                  <Effort d={d!} />
                  <Places d={d!} />
                  <Speed d={d!} />
                  <Trail data={data} />
                  <Machines data={data} d={d!} />
                  <Settings d={d!} />
                  <Raw data={data} />
                </>
              ) : null}
            </QueryScope>
          )}
        </main>
      </div>
    </div>
  );
}
