/**
 * Relatórios de falha do assistente de instalação (spec "Worker hangar.dev.br") e de diagnóstico do servidor (plano
 * "Reportar problema"): guarda no KV por 90 dias e avisa por e-mail. O carimbo barra robô genérico, não alguém
 * determinado: o app é código aberto.
 */
export const MAX_BYTES = 256 * 1024;
export const TTL_SECONDS = 90 * 24 * 60 * 60;
const STAMP = /^hangar-(native|server)\/[\w.+-]{1,64}$/;
const CODE = /^[a-z0-9-]{1,64}$/;
const SHORT = /^[\w .:+()/·-]{0,200}$/;
const SUBJECTS = { consertado: "consertado ali", aberto: "aberto" } as const;
const FROM = "relatorio@hangar.dev.br";
/** A tabela de códigos do app (`codes.rs`); fora dela o assunto diz "falha não prevista" e o código vai no corpo. */
const KNOWN_CODES = new Set(["sem-internet", "sem-winget", "sem-sudo", "senha-cancelada", "politica-travada", "checkout-sujo",
	"sem-systemd", "pacotes-desatualizados", "tailscale-https", "tailscale-login", "agente-nao-instalou", "sem-agente",
	"versao-diferente", "modo-desenvolvedor", "roda-do-mouse"]);
/** O e-mail leva só o começo do relatório: o inteiro fica no KV, e e-mail grande é o que um abuso multiplicaria. */
export const MAIL_REPORT_MAX = 8 * 1024;
/** Metade das ~1000 escritas grátis por dia da conta, contada por local da Cloudflare: freio aproximado, não garantia. */
export const KV_DAILY_MAX = 500;
/** Diário inteiro compactado (C2b); o KV aceita até 25 MiB por valor. */
export const DIARY_MAX_BYTES = 10 * 1024 * 1024;
/** Prazo da senha de envio do diário: o servidor manda logo depois do relatório. */
export const UPLOAD_TTL_SECONDS = 10 * 60;
const DIAG_CODE = /^[a-z2-7]{8}$/;
/** Máquinas de um mesmo envio ("todas as minhas máquinas"): o primeiro relatório e as que levam o código dele, até 7
 *  por minuto pelo `GROUP_LIMITER`. A assinatura do grupo vale por este prazo: as máquinas vão uma por vez, cada uma
 *  com até 150 s no servidor. */
export const GROUP_TTL_SECONDS = 30 * 60;
const GROUP_TOKEN = /^([0-9a-z]{1,10})\.([a-z2-7]{26})$/;
const UPLOAD = /^[a-z2-7]{32}$/;
const ALPHABET = "abcdefghijklmnopqrstuvwxyz234567";

export type Outcome = keyof typeof SUBJECTS;
/** Sem `kind` no corpo é instalação: o assistente de instalação já publicado não manda o campo. Também escolhe o limite
 * por IP: só o diagnóstico tem limitador próprio. */
export type Kind = "instalacao" | "diagnostico";

export interface Report {
	v: 1; kind: Kind; app: string; commit: string; os: string; step: string;
	code: string | null; outcome: Outcome; agent: string | null; report: string;
}

/** O que o handler usa do mundo: os testes trocam limite e e-mail sem tocar na conta. */
export interface Deps {
	reports: KVNamespace;
	/** `grupo`: a chave é o código do grupo, não o IP. */
	allow(key: string, limit: Kind | "diario" | "grupo"): Promise<boolean>;
	/** Teto de todos juntos: muitos IPs (ou um que troca) não esgotam as escritas do KV nem a caixa de e-mail. As máquinas
	 *  de um envio agrupado têm teto próprio (`grouped`): a instalação, que manda e-mail, não ganha a folga delas. */
	allowAll(grouped?: boolean): Promise<boolean>;
	/** Assina o código do primeiro relatório; sem ele o envio não agrupa e cada máquina paga o limite por IP. */
	groupSecret?: string;
	/** Soma as escritas no KV ao contador do dia e devolve o total; aproximado (por local da Cloudflare). */
	countToday(writes: number): Promise<number>;
	mail(subject: string, text: string): Promise<void>;
	waitUntil(work: Promise<unknown>): void;
}

function json(status: number, body: Record<string, unknown>): Response {
	return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

/** Lê o corpo parando no limite: corpo enorme nunca fica inteiro na memória. */
async function readCapped(body: ReadableStream<Uint8Array> | null, max: number): Promise<Uint8Array | null> {
	if (!body) return new Uint8Array();
	const reader = body.getReader();
	const chunks: Uint8Array[] = [];
	let total = 0;
	for (;;) {
		const { done, value } = await reader.read();
		if (done) break;
		total += value.byteLength;
		if (total > max) { await reader.cancel(); return null; }
		chunks.push(value);
	}
	const out = new Uint8Array(total);
	let at = 0;
	for (const chunk of chunks) { out.set(chunk, at); at += chunk.byteLength; }
	return out;
}

/** `n` caracteres de `[a-z2-7]`: 5 bits por caractere, sem viés (256 é múltiplo de 32). */
export function randomCode(n: number): string {
	return Array.from(crypto.getRandomValues(new Uint8Array(n)), (b) => ALPHABET[b & 31]).join("");
}

export function parseReport(value: unknown): Report | null {
	if (typeof value !== "object" || value === null) return null;
	const { v, kind: rawKind, app, commit, os, step, code, outcome, agent, report } = value as Record<string, unknown>;
	if (rawKind !== undefined && rawKind !== "diagnostico") return null;
	const kind: Kind = rawKind ?? "instalacao";
	const short = (x: unknown): x is string => typeof x === "string" && SHORT.test(x);
	const codeLike = (x: unknown): x is string | null => x === null || (typeof x === "string" && CODE.test(x));
	if (v !== 1 || !short(app) || app === "" || !short(commit) || !short(os) || !short(step)) return null;
	if (!codeLike(code) || !codeLike(agent)) return null;
	// Diagnóstico só tem "aberto": ninguém o consertou ali.
	if (outcome !== "aberto" && (kind === "diagnostico" || outcome !== "consertado")) return null;
	if (typeof report !== "string" || report === "") return null;
	return { v, kind, app, commit, os, step, code, outcome, agent, report };
}

export function subject(r: Report): string {
	const code = r.code !== null && KNOWN_CODES.has(r.code) ? r.code : "falha não prevista";
	return `[Hangar] ${SUBJECTS[r.outcome]}: ${code} · ${r.os}`;
}

/** As primeiras linhas do texto em até `max` bytes. */
export function head(text: string, max: number): string {
	const bytes = new TextEncoder().encode(text);
	if (bytes.byteLength <= max) return text;
	// Recua até o começo de um caractere: corte no meio vira U+FFFD no e-mail.
	let end = max;
	while (end > 0 && (bytes[end] & 0xc0) === 0x80) end--;
	const cut = new TextDecoder().decode(bytes.slice(0, end));
	const line = cut.lastIndexOf("\n");
	return `${line > 0 ? cut.slice(0, line) : cut}\n…`;
}

export async function handle(request: Request, deps: Deps): Promise<Response> {
	const url = new URL(request.url);
	if (url.pathname === "/api/relatorio") {
		return request.method === "POST" ? handleReport(request, deps) : json(405, { erro: "metodo" });
	}
	if (url.pathname === "/api/relatorio/diario") {
		return request.method === "PUT" ? handleDiary(request, deps) : json(405, { erro: "metodo" });
	}
	return json(404, { erro: "nao-encontrado" });
}

/** Acima do teto diário o KV não grava: as escritas grátis são da conta inteira e acabariam. */
async function overDailyCeiling(deps: Deps, writes: number): Promise<boolean> {
	return deps.countToday(writes).then((n) => n > KV_DAILY_MAX, (e: unknown) => {
		logErr("contador falhou", undefined, e);
		return false;
	});
}

/** Só o nome do erro vai ao log: a mensagem pode levar dado de quem mandou ou o destinatário do e-mail. */
function logErr(message: string, id: string | undefined, e: unknown, extra?: Record<string, unknown>): void {
	console.error(JSON.stringify({ message, id, error: e instanceof Error ? e.name : typeof e, ...extra }));
}

/** E-mail que falha vai ao log, não desfaz o envio. */
function sendMail(deps: Deps, id: string, subjectLine: string, text: string): void {
	deps.waitUntil(deps.mail(subjectLine, text).catch((e: unknown) => {
		logErr("email falhou", id, e, { code: typeof e === "object" && e !== null && "code" in e ? String(e.code) : null });
	}));
}

/** O app do carimbo (`native` ou `server`); sem carimbo válido, `undefined`. */
function stampedApp(request: Request): string | undefined {
	return STAMP.exec(request.headers.get("x-hangar-stamp") ?? "")?.[1];
}

function record(report: Report, request: Request, group?: string): string {
	return JSON.stringify({ ...report, ...(group ? { group } : {}), received: new Date().toISOString(), country: request.cf?.country ?? null });
}

async function groupSignature(secret: string, code: string, issued: string): Promise<string> {
	const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
	const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(`${code}.${issued}`)));
	let bits = 0, value = 0, out = "";
	for (const byte of mac) {
		value = (value << 8) | byte; bits += 8;
		while (bits >= 5 && out.length < 26) { out += ALPHABET[(value >>> (bits - 5)) & 31]; bits -= 5; }
	}
	return out;
}

/** A assinatura que o primeiro relatório recebe e as outras máquinas devolvem: o grupo se confere sem ler o KV, e só
 *  quem não veio em grupo a recebe, então um membro não abre outro grupo. */
async function groupToken(secret: string, code: string, now: number): Promise<string> {
	const issued = Math.floor(now / 1000).toString(36);
	return `${issued}.${await groupSignature(secret, code, issued)}`;
}

async function groupValid(secret: string | undefined, code: string, token: string, now: number): Promise<boolean> {
	const parts = GROUP_TOKEN.exec(token);
	if (!secret || !DIAG_CODE.test(code) || !parts) return false;
	const age = Math.floor(now / 1000) - parseInt(parts[1], 36);
	return age >= -60 && age <= GROUP_TTL_SECONDS && sameSecret(await groupSignature(secret, code, parts[1]), parts[2]);
}

async function handleReport(request: Request, deps: Deps): Promise<Response> {
	const stamped = stampedApp(request);
	if (!stamped) return json(403, { erro: "carimbo" });
	if (!(request.headers.get("content-type") ?? "").toLowerCase().startsWith("application/json")) return json(415, { erro: "tipo" });
	if (Number(request.headers.get("content-length") ?? "0") > MAX_BYTES) return json(413, { erro: "grande" });
	const bytes = await readCapped(request.body, MAX_BYTES);
	if (bytes === null) return json(413, { erro: "grande" });
	let parsed: unknown;
	let broken = false;
	try { parsed = JSON.parse(new TextDecoder().decode(bytes)); } catch { broken = true; }
	// O limite do IP depende do `kind`, que está no corpo: a leitura vem antes (com teto de MAX_BYTES), o limite antes
	// de qualquer escrita, e JSON quebrado também paga o limite.
	const limit: Kind = (parsed as { kind?: unknown } | null | undefined)?.kind === "diagnostico" ? "diagnostico" : "instalacao";
	const ip = request.headers.get("cf-connecting-ip") ?? "sem-ip";
	// As outras máquinas da pessoa saem do mesmo IP logo depois da primeira: com o código e a assinatura dela, o teto é o
	// do grupo. A conferência não lê nada: o limite continua antes de qualquer leitura ou escrita.
	const groupCode = request.headers.get("x-hangar-group");
	const group = groupCode !== null && limit === "diagnostico"
		&& await groupValid(deps.groupSecret, groupCode, request.headers.get("x-hangar-group-token") ?? "", Date.now()) ? groupCode : null;
	if (!group && !(await deps.allow(ip, limit))) return json(429, { erro: "limite" });
	if (groupCode !== null && !group) return json(400, { erro: "grupo" });
	if (group && !(await deps.allow(group, "grupo"))) return json(429, { erro: "grupo-cheio" });
	if (broken) return json(400, { erro: "json" });
	const report = parseReport(parsed);
	if (!report) return json(400, { erro: "campos" });
	// Cada tipo só vem do seu app: o diagnóstico do servidor, a instalação do assistente nativo.
	if (stamped !== (report.kind === "diagnostico" ? "server" : "native")) return json(403, { erro: "carimbo" });
	// O teto de todos só conta relatório válido: lixo com o carimbo público não tira a vez de quem precisa.
	if (!(await deps.allowAll(group !== null))) return json(429, { erro: "limite" });
	if (report.kind === "diagnostico") return storeDiagnostic(report, request, deps, group);
	const id = `${new Date().toISOString().slice(0, 10)}/${crypto.randomUUID()}`;
	// Acima do teto diário só o e-mail sai.
	const overDaily = await overDailyCeiling(deps, 1);
	let stored = !overDaily;
	if (!overDaily) try {
		await deps.reports.put(id, record(report, request), { expirationTtl: TTL_SECONDS });
	} catch (e: unknown) {
		// O e-mail sai mesmo assim, marcado: vira o único rastro do relatório.
		stored = false;
		logErr("kv falhou", id, e);
	}
	const text = [stored ? `chave no KV: ${id}` : "não guardado no KV", `resultado: ${report.outcome}`, `código: ${report.code ?? "-"}`, `sistema: ${report.os}`,
		`app: ${report.app} · ${report.commit}`, `etapa: ${report.step}`, `agente: ${report.agent ?? "-"}`, "",
		head(report.report, MAIL_REPORT_MAX)].join("\n");
	sendMail(deps, id, subject(report), text);
	console.log(JSON.stringify({ message: "relatorio", id, stored, overDaily, outcome: report.outcome, code: report.code }));
	return stored || overDaily ? json(201, { id }) : json(503, { erro: "armazenamento" });
}

/** Diagnóstico só responde com código que abre: sem gravar no KV é 503 e nenhum e-mail sai. */
async function storeDiagnostic(report: Report, request: Request, deps: Deps, group: string | null): Promise<Response> {
	// shortcut: código de 40 bits sem conferir colisão; com milhões de relatórios em 90 dias, conferir antes de gravar.
	const code = randomCode(8);
	const id = `diag/${code}`;
	if (await overDailyCeiling(deps, group ? 3 : 2)) {
		console.error(JSON.stringify({ message: "diagnostico sem kv", id, reason: "teto-diario" }));
		return json(503, { erro: "armazenamento" });
	}
	const upload = randomCode(32);
	try {
		// Senha primeiro: se o relatório falhar, sobra só ela, que vence em 10 min, e não um relatório órfão de 90 dias.
		await deps.reports.put(`${id}/upload`, upload, { expirationTtl: UPLOAD_TTL_SECONDS });
		// Cada máquina numa chave própria sob o primeiro (`diag/<primeiro>/m/<código>`, lidas por prefixo): nada é lido e
		// regravado, então envios seguidos de outros locais da Cloudflare não perdem membros. Antes do relatório, como a
		// senha: se ele falhar, sobra só uma marca que não abre nada.
		if (group) await deps.reports.put(`diag/${group}/m/${code}`, "", { expirationTtl: TTL_SECONDS });
		await deps.reports.put(id, record(report, request, group ?? code), { expirationTtl: TTL_SECONDS });
	} catch (e: unknown) {
		logErr("kv falhou", id, e);
		return json(503, { erro: "armazenamento" });
	}
	// Sem o segredo o envio não agrupa: as outras máquinas vão avulsas, pagando o limite por IP.
	const token = !group && deps.groupSecret ? await groupToken(deps.groupSecret, code, Date.now()) : undefined;
	if (!group && !deps.groupSecret) console.error(JSON.stringify({ message: "grupo sem segredo", id }));
	const text = [`relatório: ${code}`, `chave no KV: ${id}`, `sistema: ${report.os}`, `app: ${report.app} · ${report.commit}`, "",
		head(report.report, MAIL_REPORT_MAX)].join("\n");
	// As outras máquinas do envio ficam sob o código do primeiro, que é o único que a pessoa passa adiante.
	if (!group) sendMail(deps, id, `[Hangar] diagnóstico ${code} · ${report.os}`, text);
	console.log(JSON.stringify({ message: "relatorio", id, kind: report.kind }));
	return json(201, { id, code, upload, ...(group ? { group } : {}), ...(token ? { group_token: token } : {}) });
}

function sameSecret(a: string, b: string): boolean {
	const x = new TextEncoder().encode(a);
	const y = new TextEncoder().encode(b);
	return x.byteLength === y.byteLength && crypto.subtle.timingSafeEqual(x, y);
}

/** Diário inteiro (C2b): o corpo vai ao KV como chegou, sem abrir — gzip e JSON não cabem nos 10 ms de CPU do Free. */
async function handleDiary(request: Request, deps: Deps): Promise<Response> {
	if (stampedApp(request) !== "server") return json(403, { erro: "carimbo" });
	const code = request.headers.get("x-hangar-code") ?? "";
	const upload = request.headers.get("x-hangar-upload") ?? "";
	if (!DIAG_CODE.test(code) || !UPLOAD.test(upload)) return json(403, { erro: "senha" });
	const length = Number(request.headers.get("content-length") ?? "");
	// Sem tamanho declarado não dá para barrar o corpo grande antes de gravar.
	if (!Number.isInteger(length) || length <= 0 || !request.body) return json(411, { erro: "tamanho" });
	if (length > DIARY_MAX_BYTES) return json(413, { erro: "grande" });
	// Cada tentativa custa uma leitura do KV, que é da conta inteira.
	// O diário vem logo após o relatório, que já gastou a vez no limitador do diagnóstico; as máquinas de um mesmo envio
	// mandam cada uma o seu, do mesmo IP: limitador próprio, mais largo, já que a senha de uso único barra o resto.
	if (!(await deps.allow(request.headers.get("cf-connecting-ip") ?? "sem-ip", "diario"))) return json(429, { erro: "limite" });
	const id = `diag/${code}`;
	try {
		const expected = await deps.reports.get(`${id}/upload`);
		if (expected === null || !sameSecret(expected, upload)) return json(403, { erro: "senha" });
		if (await overDailyCeiling(deps, 2)) {
			console.error(JSON.stringify({ message: "diario sem kv", id, reason: "teto-diario" }));
			return json(503, { erro: "armazenamento" });
		}
		// Apaga antes de gravar: a senha não serve duas vezes, nem quando a gravação falha.
		// shortcut: o KV é eventualmente consistente, então dois PUTs quase simultâneos com a mesma senha podem gravar os
		// dois (só quem tem a senha); trocar por Durable Object se o uso único precisar ser garantido.
		await deps.reports.delete(`${id}/upload`);
		await deps.reports.put(`${id}/diario.jsonl.gz`, request.body, { expirationTtl: TTL_SECONDS });
	} catch (e: unknown) {
		logErr("kv falhou", id, e, { step: "diario" });
		return json(503, { erro: "armazenamento" });
	}
	console.log(JSON.stringify({ message: "diario", id, bytes: length }));
	return json(201, { id: `${id}/diario.jsonl.gz` });
}

/** `REPORT_TO` e `GROUP_SECRET` são segredos (`wrangler secret put`), fora do `wrangler.jsonc` e por isso fora do `Env` gerado. */
// shortcut: DIAG_LIMITER declarado aqui até `npm run types` regenerar o worker-configuration.d.ts.
type Bindings = Env & { REPORT_TO?: string; GROUP_SECRET?: string; DIAG_LIMITER: RateLimit; DIARY_LIMITER: RateLimit;
	GROUP_LIMITER: RateLimit; GROUPS_LIMITER: RateLimit };

export default {
	async fetch(request, env: Bindings, ctx): Promise<Response> {
		return handle(request, {
			reports: env.REPORTS,
			allow: async (key, limit) => (await ({ diagnostico: env.DIAG_LIMITER, diario: env.DIARY_LIMITER, instalacao: env.LIMITER,
				grupo: env.GROUP_LIMITER })[limit].limit({ key })).success,
			allowAll: async (grouped) => (await (grouped ? env.GROUPS_LIMITER : env.GLOBAL_LIMITER).limit({ key: "global" })).success,
			groupSecret: env.GROUP_SECRET,
			countToday: async (writes) => {
				// ponytail: contador no Cache API, por local e sem atomicidade; Durable Object se precisar exato.
				const key = new Request(`https://hangar.dev.br/__contador/${new Date().toISOString().slice(0, 10)}`);
				const cache = caches.default;
				const n = Number((await (await cache.match(key))?.text()) ?? "0") + writes;
				ctx.waitUntil(cache.put(key, new Response(String(n), { headers: { "cache-control": "max-age=90000" } })));
				return n;
			},
			mail: async (subject, text) => {
				if (!env.REPORT_TO) throw new Error("REPORT_TO ausente");
				await env.MAILER.send({ from: FROM, to: env.REPORT_TO, subject, text });
			},
			waitUntil: (work) => ctx.waitUntil(work),
		});
	},
} satisfies ExportedHandler<Bindings>;
