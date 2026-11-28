import { animate, useReducedMotion } from "motion/react";
import { createContext, useContext, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { AlertCircle, CircleCheck, RotateCw, type LucideIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { fmt, pct0 } from "@/lib/format";

// Busca: cada seção e cada cartão que casam pelo título mostram tudo o que têm dentro.
const QueryContext = createContext("");
export const useQ = () => useContext(QueryContext);
export const QueryScope = QueryContext.Provider;
export const matches = (q: string, text: string) => !q || text.toLowerCase().includes(q);

export function Section({ id, title, explain, hue, icon: Icon, aside, children }: {
  id: string; title: string; explain: string; hue: string; icon: LucideIcon; aside?: ReactNode; children: ReactNode;
}) {
  const q = useQ();
  return (
    <QueryScope value={matches(q, title) ? "" : q}>
      <section id={"s-" + id} data-section={id} aria-labelledby={"h-" + id} className="scroll-mt-20 space-y-4" style={{ "--hue": hue } as React.CSSProperties}>
        <header className="flex items-start gap-3 pt-2">
          <span className="grid size-9 shrink-0 place-items-center rounded-[10px] bg-[color-mix(in_srgb,var(--hue)_16%,transparent)] text-(--hue)">
            <Icon className="size-[18px]" strokeWidth={1.75} />
          </span>
          <div className="min-w-0 flex-1">
            <h2 id={"h-" + id} className="text-[17px] font-semibold tracking-[-0.01em] text-balance">{title}</h2>
            <p className="text-[13px] text-muted-foreground text-pretty">{explain}</p>
          </div>
          {aside}
        </header>
        {children}
      </section>
    </QueryScope>
  );
}

type BlockProps<T> = {
  title: string; explain?: ReactNode; action?: ReactNode; className?: string; id?: string; flush?: boolean;
  items?: T[]; text?: (item: T) => string; children: ReactNode | ((shown: T[]) => ReactNode);
};
export function Block<T>({ title, explain, action, className, id, flush, items, text, children }: BlockProps<T>) {
  const outer = useQ();
  const q = matches(outer, title) ? "" : outer;
  const shown = items && text && q ? items.filter(i => matches(q, text(i))) : items ?? [];
  if (q && (!items || !shown.length)) return null;
  return (
    <QueryScope value={q}>
      <div id={id} data-block data-hits={outer ? Math.max(1, q ? shown.length : 1) : undefined}
        className={cn("flex min-w-0 flex-col gap-3 rounded-xl border bg-card", flush ? "gap-0 overflow-hidden" : "p-4 sm:p-5", className)}>
        <div className={cn("flex items-start gap-3", flush && "px-4 pt-4 pb-3 sm:px-5 sm:pt-5")}>
          <div className="min-w-0 flex-1">
            <h3 className="text-[14px] font-medium leading-snug text-balance">{title}</h3>
            {explain ? <p className="mt-0.5 text-[12.5px] text-muted-foreground text-pretty">{explain}</p> : null}
          </div>
          {action}
        </div>
        {typeof children === "function" ? children(shown) : children}
      </div>
    </QueryScope>
  );
}

export function Group({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="space-y-3 has-[[data-group-body]:empty]:hidden">
      <h3 className="pt-2 text-[12px] font-medium uppercase tracking-[0.06em] text-faint">{title}</h3>
      <div data-group-body className="grid gap-4 md:grid-cols-2 xl:grid-cols-3 empty:hidden">{children}</div>
    </div>
  );
}

// Número que rola do valor anterior até o novo; na releitura ao vivo, o que mudou ganha um realce breve.
export function Num({ value, format = (v: number) => fmt.format(Math.round(v)), className }: { value: number; format?: (v: number) => string; className?: string }) {
  const ref = useRef<HTMLSpanElement>(null);
  const last = useRef<number | null>(null);
  const reduce = useReducedMotion();
  useLayoutEffect(() => {
    const node = ref.current!;
    const from = last.current;
    last.current = value;
    if (from === null || from === value || reduce) { node.textContent = format(value); return; }
    node.classList.remove("flash"); void node.offsetWidth; node.classList.add("flash");
    const controls = animate(from, value, { duration: 0.7, ease: [0.23, 1, 0.32, 1], onUpdate: v => { node.textContent = format(v); } });
    return () => controls.stop();
  }, [value, format, reduce]);
  return <span ref={ref} className={cn("tabular", className)}>{format(value)}</span>;
}

export interface BarItem { key: string; label: ReactNode; text?: string; value: number; machines?: number; icon?: ReactNode; note?: ReactNode; color?: string }

// Lista de barras (como o BarList do Tremor): a barra fica atrás do rótulo, o número à direita.
export function BarList({ items, total, unit = ["vez", "vezes"], color = "var(--hue)", limit }: {
  items: BarItem[]; total?: number; unit?: [string, string]; color?: string; limit?: number;
}) {
  const [all, setAll] = useState(false);
  const q = useQ();
  const shown = !limit || all || q ? items : items.slice(0, limit);
  const max = Math.max(1, ...items.map(i => i.value));
  return (
    <div className="space-y-1">
      <ul className="space-y-[3px]">
        {shown.map((it, i) => (
          <li key={it.key} title={(it.text ?? String(it.label)) + ": " + fmt.format(it.value) + " " + unit[it.value === 1 ? 0 : 1] + (it.machines != null ? " · " + it.machines + " máquina" + (it.machines === 1 ? "" : "s") : "")}
            className="group relative flex min-h-9 items-center gap-3 rounded-md px-2.5 py-1.5 text-[13px]">
            <span className="grow-x absolute inset-y-0 left-0 rounded-md opacity-[.17] transition-opacity group-hover:opacity-[.28]"
              style={{ width: Math.max(1.5, (it.value / max) * 100) + "%", background: it.color ?? color, "--i": Math.min(i, 12) } as React.CSSProperties} />
            {it.icon ? <span className="relative shrink-0 text-muted-foreground">{it.icon}</span> : null}
            <span className="relative min-w-0 flex-1 break-words">{it.label}{it.note ? <span className="ml-2 text-[12px] text-destructive">{it.note}</span> : null}</span>
            <span className="relative shrink-0 text-right font-medium tabular"><Num value={it.value} /></span>
            {it.machines != null ? <span className="relative w-14 shrink-0 text-right text-[12px] text-muted-foreground tabular">{it.machines} máq.</span> : null}
            {total ? <span className="relative w-10 shrink-0 text-right text-[12px] text-muted-foreground tabular">{pct0.format(it.value / total)}</span> : null}
          </li>
        ))}
      </ul>
      {limit && items.length > limit && !q ? (
        <Button variant="ghost" size="sm" className="h-9 text-muted-foreground" onClick={() => setAll(a => !a)} aria-expanded={all}>
          {all ? "Mostrar só " + limit : "Mostrar todas as " + fmt.format(items.length)}
        </Button>
      ) : null}
    </div>
  );
}

export function Empty({ children, icon: Icon = CircleCheck, tone = "muted" }: { children: ReactNode; icon?: LucideIcon; tone?: "muted" | "ok" }) {
  return (
    <div className="flex items-center gap-3 rounded-lg border border-dashed px-4 py-5 text-[13px] text-muted-foreground">
      <Icon className={cn("size-[18px] shrink-0", tone === "ok" ? "text-ok" : "text-faint")} strokeWidth={1.75} />
      <span className="text-pretty">{children}</span>
    </div>
  );
}

export function ErrorBox({ message, detail, onRetry }: { message: string; detail?: string; onRetry?: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-3 rounded-lg border border-destructive/30 bg-destructive/10 px-4 py-3 text-[13px]">
      <AlertCircle className="size-[18px] shrink-0 text-destructive" strokeWidth={1.75} />
      <span className="min-w-0 flex-1">{message}{detail ? <span className="block text-[12px] text-muted-foreground">{detail}</span> : null}</span>
      {onRetry ? <Button variant="outline" className="h-10 sm:h-8" onClick={onRetry}><RotateCw />Tentar de novo</Button> : null}
    </div>
  );
}

export function BlockSkeleton({ rows = 4, chart = false }: { rows?: number; chart?: boolean }) {
  return (
    <div aria-hidden="true" className="flex flex-col gap-3 rounded-xl border bg-card p-5">
      <Skeleton className="h-4 w-40" />
      <Skeleton className="h-3 w-64 max-w-full" />
      {chart ? <Skeleton className="h-44 w-full" /> : Array.from({ length: rows }, (_, i) => <Skeleton key={i} className="h-8" style={{ width: 100 - i * 12 + "%" }} />)}
    </div>
  );
}

export const Mono = ({ children, className }: { children: ReactNode; className?: string }) => (
  <code className={cn("font-mono text-[12.5px] [overflow-wrap:anywhere]", className)}>{children}</code>
);
