/** Um painel que um mod abriu e o terminal colocou, como vai ao backend. Registrado no `ui.open`, antes
 *  do primeiro desenho, vai com a árvore vazia do engine e sem largura. */
export type PaneEntry = {
  id: string;
  title: string;
  placement: "dock" | "inline";
  columns: number | null;
  tree: unknown;
  /** Estado estruturado que o app desenha com o tema dele (o `/btw`); ausente nos outros painéis. */
  data?: unknown;
};

/** Árvore de painel aberto e ainda não desenhado: o app o mostra como aba de corpo vazio (S8). O
 *  terminal só desenha um painel quando ele vem à frente ((s)). */
export const UNDRAWN = { type: "engine", ref: 0 } as const;

const isEngine = (tree: unknown) => !tree || (tree as { type?: string }).type === "engine";

/** Colunas do `[-]` no fim da faixa: `bodyColumns` mais estas é a coluna da conversa, onde a prévia corta. */
export const CUT_EXTRA = 5;

/** Miolo do corpo do POST /ui (sem sessão e token). `caps`: o que o plugin desta sessão atende, para o app
 *  liberar só o que funciona aqui (hoje `btw`, quando o CLI tem `$.model.fork`). `columns` é o `bodyColumns` da faixa mais as colunas
 *  do `[-]`, como hoje: é o que o Python lê para cortar a prévia (na cópia que o Rust repassa e na reserva
 *  sem Rust). `bodyColumns` é o que o Rust publica no evento. Acima de `max` caracteres saem primeiro os
 *  painéis, depois a faixa: a faixa é o que todo mod tem. */
export function bandBody(above: unknown, columns: number | null, panes: readonly PaneEntry[], shown: string | null, max: number, caps: readonly string[] = []): string {
  const faixa = isEngine(above) ? null : above;
  const corte = columns === null ? "null" : String(columns + CUT_EXTRA);
  const corpo = (a: unknown, p: readonly PaneEntry[]) => {
    const frente = shown !== null && p.some((x) => x.id === shown) ? shown : null;
    return `"above":${JSON.stringify(a)},"columns":${corte},"bodyColumns":${columns ?? "null"},"panes":${JSON.stringify(p)},"shown":${JSON.stringify(frente)},"caps":${JSON.stringify(caps)}`;
  };
  const tudo = corpo(faixa, panes);
  if (tudo.length <= max) return tudo;
  const soFaixa = corpo(faixa, []);
  return soFaixa.length <= max ? soFaixa : corpo(null, []);
}

/** Árvore do próprio plugin espelhada antes de sair dele: o engine só carimba `press.plugin` na saída, e
 *  sem dono o app não aciona o botão. Preenche o dono vazio; o resto segue como veio. */
export function stampOwn(tree: unknown, plugin: string): unknown {
  if (Array.isArray(tree)) return tree.map((t) => stampOwn(t, plugin));
  if (!tree || typeof tree !== "object") return tree;
  const node = { ...(tree as Record<string, unknown>) };
  const press = node.press as { plugin?: string } | undefined;
  if (press && typeof press === "object" && !press.plugin) node.press = { ...press, plugin };
  if (Array.isArray(node.children)) node.children = node.children.map((c) => stampOwn(c, plugin));
  return node;
}

/** O painel na frente depois que `closed` fecha: no terminal aparece o vizinho anterior ((d)). */
export function shownAfterClose(ids: readonly string[], shown: string | null, closed: string): string | null {
  if (shown !== closed) return shown;
  const i = ids.indexOf(closed);
  const rest = ids.filter((id) => id !== closed);
  if (!rest.length) return null;
  return i > 0 ? ids[i - 1]! : rest[0]!;
}

/** Onde a janela do painel vai parar: o `offset` pedido, preso ao conteúdo como o engine faz. */
export function scrollOffset(e: { offset: number; bodyRows: number; contentRows: number }): number {
  return Math.max(0, Math.min(e.offset, Math.max(0, e.contentRows - e.bodyRows)));
}
