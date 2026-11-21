// Leitor dos blocos `<hangar-ref …>` (citação, elemento e linhas de arquivo) que o Hangar põe no texto da mensagem.
// O formato é o contrato do `packages/core` (seção 1 de
// docs/superpowers/specs/2026-10-15-t3-citacao-mermaid-elemento-contratos.md); o plugin não importa
// `packages/`, então repete aqui só a leitura. Os casos de teste são os mesmos
// (`contextRefs.fixtures.ts`, cópia de `packages/core/src/contextRefs.fixtures.json`).

export type QuoteRef = {
  kind: "quote";
  source: string;
  before: string;
  after: string;
  text: string;
  comment: string;
};

export type ElementRef = {
  kind: "element";
  url: string;
  title: string;
  role: string;
  name: string;
  tag: string;
  selector: string;
  html: string;
  comment: string;
};

export type CodeRef = {
  kind: "code";
  path: string;
  start: number;
  end: number;
  lang: string;
  text: string;
  comment: string;
};

export type ContextRef = QuoteRef | ElementRef | CodeRef;

export type ContextSegment = { text: string } | { ref: ContextRef };

// O corpo não pode conter outro `<hangar-ref`/`</hangar-ref` cru: sem essa trava, um bloco aberto sem
// fecho engoliria o bloco válido seguinte.
const BLOCK = /<hangar-ref((?: [a-z]+="[^"]*")*)>\n((?:(?!<\/?hangar-ref)[\s\S])*?)\n<\/hangar-ref>/g;
const ATTR = / ([a-z]+)="([^"]*)"/g;
const CHILD = /\n<hangar-([a-z]+)>\n([\s\S]*?)\n<\/hangar-\1>/y;
const ATTR_ENTITY = /&(amp|quot|lt|gt|#10|#13|#9);/g;
const ATTR_DECODED: Record<string, string> = {
  amp: "&", quot: '"', lt: "<", gt: ">", "#10": "\n", "#13": "\r", "#9": "\t",
};
const CONTENT_ESCAPE = /&(amp;)?((?:amp;)*)(lt;)(\/?hangar-)/gi;
// Número de linha: inteiro positivo, sem zero à esquerda nem sinal, até nove dígitos.
const LINE = /^[1-9][0-9]{0,8}$/;

export const hasContextRef = (message: string): boolean => message.includes("<hangar-ref");

// Só as sete entidades do contrato, numa passada: `&amp;lt;` vira `&lt;`, não `<`.
const decodeAttr = (value: string): string => value.replace(ATTR_ENTITY, (_, name: string) => ATTR_DECODED[name] ?? "");

// Desfaz o escape do conteúdo: tira um `amp;` de quem tinha, e o `&lt;` sem `amp;` volta a ser `<`.
// O `lt;` capturado volta como veio: `&AMP;LT;hangar-` não sai minúsculo.
const decodeContent = (value: string): string =>
  value.replace(CONTENT_ESCAPE, (_, first: string | undefined, rest: string, lt: string, tag: string) =>
    first ? `&${rest}${lt}${tag}` : `<${tag}`);

const readAttrs = (raw: string): Map<string, string> | null => {
  const attrs = new Map<string, string>();
  for (const [, name = "", value = ""] of raw.matchAll(ATTR)) {
    if (attrs.has(name)) return null;
    attrs.set(name, decodeAttr(value));
  }
  return attrs;
};

// O corpo é a nota (primeira linha, só para o modelo) seguida dos filhos, colados por `\n`.
// Sobra fora de um filho, filho repetido ou alheio ao tipo deixa o bloco malformado.
const readChildren = (body: string, allowed: readonly string[]): Map<string, string> | null => {
  const children = new Map<string, string>();
  const noteEnd = body.indexOf("\n");
  if (noteEnd < 0) return children;
  let at = noteEnd;
  while (at < body.length) {
    CHILD.lastIndex = at;
    const m = CHILD.exec(body);
    if (!m) return null;
    const [, name = "", content = ""] = m;
    if (!allowed.includes(name) || children.has(name)) return null;
    children.set(name, decodeContent(content));
    at = CHILD.lastIndex;
  }
  return children;
};

const readRef = (rawAttrs: string, body: string): ContextRef | null => {
  const attrs = readAttrs(rawAttrs);
  if (!attrs || attrs.get("v") !== "1") return null;
  const attr = (name: string) => attrs.get(name) ?? "";
  const kind = attrs.get("kind");
  if (kind === "quote") {
    const children = readChildren(body, ["text", "comment"]);
    const text = children?.get("text");
    if (!children || text === undefined || !text.trim()) return null;
    return {
      kind, source: attr("source"), before: attr("before"), after: attr("after"),
      text, comment: (children.get("comment") ?? "").trim(),
    };
  }
  if (kind === "code") {
    const children = readChildren(body, ["text", "comment"]);
    const text = children?.get("text");
    const start = attrs.get("start");
    const end = attrs.get("end");
    if (!children || text === undefined || !attr("path") || !start || !end || !LINE.test(start) || !LINE.test(end)) return null;
    if (Number(start) > Number(end)) return null;
    return {
      kind, path: attr("path"), start: Number(start), end: Number(end), lang: attr("lang"),
      text, comment: (children.get("comment") ?? "").trim(),
    };
  }
  if (kind === "element") {
    const children = readChildren(body, ["html", "comment"]);
    const html = children?.get("html");
    if (!children || html === undefined || !attrs.has("url") || !attr("tag")) return null;
    return {
      kind, url: attr("url"), title: attr("title"), role: attr("role"), name: attr("name"),
      tag: attr("tag"), selector: attr("selector"), html,
      comment: (children.get("comment") ?? "").trim(),
    };
  }
  return null;
};

// Segmentos na ordem da mensagem. Bloco que não se lê fica como texto cru, junto do texto vizinho.
export const parseContextRefs = (message: string): ContextSegment[] => {
  const segments: ContextSegment[] = [];
  const pushText = (text: string) => {
    if (!text) return;
    const last = segments[segments.length - 1];
    if (last && "text" in last) last.text += text;
    else segments.push({ text });
  };
  let at = 0;
  for (const m of message.matchAll(BLOCK)) {
    const ref = readRef(m[1] ?? "", m[2] ?? "");
    if (!ref) continue;
    pushText(message.slice(at, m.index));
    segments.push({ ref });
    at = m.index + m[0].length;
  }
  pushText(message.slice(at));
  return segments;
};

// Linhas do trecho na linha compacta da conversa; o resto fica para o Ctrl+O.
export const COMPACT_QUOTE_LINES = 3;
const COMPACT_QUOTE_CHARS = 240;

const compactQuote = (text: string): string[] => {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  let cut = lines.length > COMPACT_QUOTE_LINES;
  const kept = lines.slice(0, COMPACT_QUOTE_LINES);
  let room = COMPACT_QUOTE_CHARS;
  const out: string[] = [];
  for (const line of kept) {
    const chars = Array.from(line);
    if (chars.length > room) {
      out.push(chars.slice(0, room).join(""));
      cut = true;
      break;
    }
    out.push(line);
    room -= chars.length;
  }
  if (cut) out[out.length - 1] += "…";
  return out;
};

const elementLabel = (ref: ElementRef): string => {
  // Mesma regra do rótulo do contrato: papel e nome só juntos.
  return ref.role && ref.name ? `${ref.role} "${ref.name}"` : `<${ref.tag}>`;
};

// Mesma regra do rótulo do contrato: nome do arquivo e as linhas.
const codeLabel = (ref: CodeRef): string => {
  const name = ref.path.split(/[\\/]/).pop() || ref.path;
  return ref.start === ref.end ? `${name} L${ref.start}` : `${name} L${ref.start}–L${ref.end}`;
};

const refLines = (ref: ContextRef): string[] => {
  if (ref.kind === "quote") return compactQuote(ref.text).map((line) => `> ${line}`);
  if (ref.kind === "code") return [`Código: ${codeLabel(ref)}`, ...compactQuote(ref.text).map((line) => `> ${line}`)];
  return [`Elemento: ${elementLabel(ref)} — ${ref.url}`];
};

const refRow = (ref: ContextRef): string => {
  const lines = refLines(ref);
  if (ref.comment.trim()) lines.push(ref.comment);
  return lines.join("\n");
};

// O texto da linha do usuário com cada bloco no lugar de `> trecho`, `Código: …` ou `Elemento: …` e o comentário;
// `null` quando não há bloco lido (a linha fica como o engine a desenha).
export const compactUserRow = (message: string): string | null => {
  if (!hasContextRef(message)) return null;
  const segments = parseContextRefs(message);
  if (!segments.some((s) => "ref" in s)) return null;
  return segments
    .map((s) => ("ref" in s ? refRow(s.ref) : s.text.replace(/^\n+|\n+$/g, "")))
    .filter((part) => part.trim())
    .join("\n\n");
};
