import { env, exports } from "cloudflare:workers";
import { beforeEach, describe, expect, it } from "vitest";
import { DIARY_MAX_BYTES, handle, head, KV_DAILY_MAX, MAIL_REPORT_MAX, MAX_BYTES, TTL_SECONDS, UPLOAD_TTL_SECONDS, type Deps, type Kind } from "../src";

function report(over: Record<string, unknown> = {}) {
	return { v: 1, app: "0.20.1.3456", commit: "abc123", os: "linux-x86_64", step: "instalar", code: "sem-systemd",
		outcome: "aberto", agent: null, report: "log limpo", ...over };
}

function post(body: string, headers: Record<string, string> = {}) {
	return new Request("https://hangar.dev.br/api/relatorio", { method: "POST", body,
		headers: { "content-type": "application/json", "x-hangar-stamp": "hangar-native/0.20.1.3456", "cf-connecting-ip": "203.0.113.7", ...headers } });
}

function deps(allow = true, mailFails = false, allowAll = true, today = 1) {
	const mails: { subject: string; text: string }[] = [];
	const pending: Promise<unknown>[] = [];
	const calls = { all: 0, limits: [] as (Kind | "diario")[], writes: [] as number[] };
	const d: Deps = {
		reports: env.REPORTS,
		allow: async (_ip, limit) => { calls.limits.push(limit); return allow; },
		allowAll: async () => { calls.all++; return allowAll; },
		countToday: async (writes) => { calls.writes.push(writes); return today; },
		mail: async (subject, text) => { if (mailFails) throw new Error("sem rota de e-mail"); mails.push({ subject, text }); },
		waitUntil: (work) => { pending.push(work); },
	};
	return { d, mails, pending, calls };
}

async function stored() { return (await env.REPORTS.list()).keys; }

beforeEach(async () => { for (const key of await stored()) await env.REPORTS.delete(key.name); });

describe("POST /api/relatorio", () => {
	it("stores for 90 days and mails 'aberto'", async () => {
		const { d, mails, pending } = deps();
		const before = Math.floor(Date.now() / 1000);
		const res = await handle(post(JSON.stringify(report())), d);
		expect(res.status).toBe(201);
		await Promise.all(pending);
		const keys = await stored();
		expect(keys).toHaveLength(1);
		expect(keys[0].expiration).toBeGreaterThanOrEqual(before + TTL_SECONDS - 5);
		expect(keys[0].expiration).toBeLessThanOrEqual(before + TTL_SECONDS + 60);
		expect(mails[0].subject).toContain("aberto");
		expect(mails[0].subject).toContain("sem-systemd");
		expect(mails[0].text).toContain("log limpo");
		expect(JSON.parse((await env.REPORTS.get(keys[0].name))!).report).toBe("log limpo");
	});

	it("over the daily KV ceiling only mails, marked as not stored", async () => {
		const { d, mails, pending } = deps(true, false, true, KV_DAILY_MAX + 1);
		const res = await handle(post(JSON.stringify(report())), d);
		expect(res.status).toBe(201);
		await Promise.all(pending);
		expect(await stored()).toHaveLength(0);
		expect(mails[0].text).toContain("não guardado no KV");
	});

	it("says 'consertado ali' when the agent fixed it", async () => {
		const { d, mails, pending } = deps();
		await handle(post(JSON.stringify(report({ outcome: "consertado", agent: "claude" }))), d);
		await Promise.all(pending);
		expect(mails[0].subject).toContain("consertado ali");
	});

	it("rejects bodies over 256 KB without storing", async () => {
		const { d } = deps();
		const res = await handle(post(JSON.stringify(report({ report: "x".repeat(MAX_BYTES) }))), d);
		expect(res.status).toBe(413);
		expect(await stored()).toHaveLength(0);
	});

	it("limits by IP before touching the global limit", async () => {
		const { d, mails, calls } = deps(false);
		const res = await handle(post(JSON.stringify(report())), d);
		expect(res.status).toBe(429);
		expect(await stored()).toHaveLength(0);
		expect(mails).toHaveLength(0);
		expect(calls.all).toBe(0);
	});

	it("limits everyone together", async () => {
		const { d, mails, pending } = deps(true, false, false);
		const res = await handle(post(JSON.stringify(report())), d);
		await Promise.all(pending);
		expect(res.status).toBe(429);
		expect(await stored()).toHaveLength(0);
		expect(mails).toHaveLength(0);
	});

	it("charges the global limit only for valid reports", async () => {
		const { d, calls } = deps();
		expect((await handle(post("{"), d)).status).toBe(400);
		expect((await handle(post(JSON.stringify(report({ outcome: "talvez" }))), d)).status).toBe(400);
		expect(calls.all).toBe(0);
		expect((await handle(post(JSON.stringify(report())), d)).status).toBe(201);
		expect(calls.all).toBe(1);
	});

	it("mails the report marked as not stored when KV fails", async () => {
		const { d, mails, pending } = deps();
		d.reports = { put: async () => { throw new Error("kv fora"); } } as unknown as KVNamespace;
		const res = await handle(post(JSON.stringify(report())), d);
		await Promise.all(pending);
		expect(res.status).toBe(503);
		expect(await res.json()).toEqual({ erro: "armazenamento" });
		expect(mails[0].text).toContain("não guardado no KV");
		expect(mails[0].text).not.toContain("chave no KV");
		expect(mails[0].text).toContain("log limpo");
	});

	it("never cuts a UTF-8 character in half", () => {
		// "ç" ocupa 2 bytes: um corte em 3 cairia no meio do segundo.
		expect(head("çççç", 3)).toBe("ç\n…");
		expect(head("ação\nfim", 4)).not.toContain("\ufffd");
	});

	it("mails a short summary and keeps the whole report in KV", async () => {
		const { d, mails, pending } = deps();
		const big = Array.from({ length: 20_000 }, (_, n) => `linha ${n}`).join("\n");
		const res = await handle(post(JSON.stringify(report({ report: big }))), d);
		await Promise.all(pending);
		expect(res.status).toBe(201);
		const { id } = await res.json<{ id: string }>();
		const text = mails[0].text;
		for (const line of [`chave no KV: ${id}`, "resultado: aberto", "código: sem-systemd", "sistema: linux-x86_64", "app: 0.20.1.3456"]) {
			expect(text).toContain(line);
		}
		expect(text).toContain("linha 0\n");
		expect(text).not.toContain("linha 19999");
		expect(new TextEncoder().encode(text).byteLength).toBeLessThanOrEqual(MAIL_REPORT_MAX + 512);
		expect(JSON.parse((await env.REPORTS.get(id))!).report).toBe(big);
	});

	it("puts an unknown code in the body, not in the subject", async () => {
		const { d, mails, pending } = deps();
		await handle(post(JSON.stringify(report({ code: "disco-cheio" }))), d);
		await Promise.all(pending);
		expect(mails[0].subject).toContain("falha não prevista");
		expect(mails[0].subject).not.toContain("disco-cheio");
		expect(mails[0].text).toContain("código: disco-cheio");
	});

	it("accepts the content type in any case", async () => {
		const { d } = deps();
		expect((await handle(post(JSON.stringify(report()), { "content-type": "Application/JSON; charset=UTF-8" }), d)).status).toBe(201);
	});

	it("needs the app stamp, JSON and POST on the right path", async () => {
		const { d } = deps();
		expect((await handle(post(JSON.stringify(report()), { "x-hangar-stamp": "curl/8" }), d)).status).toBe(403);
		expect((await handle(post(JSON.stringify(report()), { "content-type": "text/plain" }), d)).status).toBe(415);
		expect((await handle(new Request("https://hangar.dev.br/api/relatorio"), d)).status).toBe(405);
		expect((await handle(new Request("https://hangar.dev.br/api/outro", { method: "POST" }), d)).status).toBe(404);
	});

	it("rejects broken JSON and fields out of contract", async () => {
		const { d } = deps();
		expect((await handle(post("{"), d)).status).toBe(400);
		expect((await handle(post(JSON.stringify(report({ outcome: "talvez" }))), d)).status).toBe(400);
		expect((await handle(post(JSON.stringify(report({ code: "Disco Cheio" }))), d)).status).toBe(400);
		expect((await handle(post(JSON.stringify(report({ report: "" }))), d)).status).toBe(400);
		expect(await stored()).toHaveLength(0);
	});

	it("keeps the report when the e-mail fails", async () => {
		const { d, pending } = deps(true, true);
		const res = await handle(post(JSON.stringify(report())), d);
		await Promise.all(pending);
		expect(res.status).toBe(201);
		expect(await stored()).toHaveLength(1);
	});

	it("wires the real bindings", async () => {
		const res = await exports.default.fetch(post(JSON.stringify(report())));
		expect(res.status).toBe(201);
	});

	it("treats a report without kind as installation, as before", async () => {
		const { d, calls } = deps();
		const res = await handle(post(JSON.stringify(report())), d);
		const { id } = await res.json<{ id: string }>();
		expect(res.status).toBe(201);
		expect(id).toMatch(/^\d{4}-\d{2}-\d{2}\//);
		expect(calls.limits).toEqual(["instalacao"]);
		expect(calls.writes).toEqual([1]);
		expect(JSON.parse((await env.REPORTS.get(id))!).kind).toBe("instalacao");
	});
});

function diagnostic(over: Record<string, unknown> = {}) {
	return report({ kind: "diagnostico", app: "hangar-server 0.1.0.8380", commit: "fe204bec6", step: "diagnostico", code: null,
		report: "resumo do pacote", ...over });
}

function postDiag(body: string, headers: Record<string, string> = {}) {
	return post(body, { "x-hangar-stamp": "hangar-server/0.1.0.8380", ...headers });
}

interface DiagCreated { id: string; code: string; upload: string }

describe("POST /api/relatorio with kind diagnostico", () => {
	it("stores under diag/<code> with the kind and a one-time upload password", async () => {
		const { d, calls } = deps();
		const before = Math.floor(Date.now() / 1000);
		const res = await handle(postDiag(JSON.stringify(diagnostic())), d);
		expect(res.status).toBe(201);
		const body = await res.json<DiagCreated>();
		expect(body.code).toMatch(/^[a-z2-7]{8}$/);
		expect(body.id).toBe(`diag/${body.code}`);
		expect(body.upload).toMatch(/^[a-z2-7]{32}$/);
		expect(calls.limits).toEqual(["diagnostico"]);
		expect(calls.all).toBe(1);
		expect(calls.writes).toEqual([2]);
		const saved = JSON.parse((await env.REPORTS.get(body.id))!);
		expect(saved.kind).toBe("diagnostico");
		expect(saved.report).toBe("resumo do pacote");
		expect(await env.REPORTS.get(`${body.id}/upload`)).toBe(body.upload);
		const keys = await stored();
		const upload = keys.find((k) => k.name === `${body.id}/upload`)!;
		expect(upload.expiration).toBeLessThanOrEqual(before + UPLOAD_TTL_SECONDS + 60);
		expect(keys.find((k) => k.name === body.id)!.expiration).toBeGreaterThanOrEqual(before + TTL_SECONDS - 5);
	});

	it("mails the code in the subject and at the top of the body", async () => {
		const { d, mails, pending } = deps();
		const { code } = await (await handle(postDiag(JSON.stringify(diagnostic())), d)).json<DiagCreated>();
		await Promise.all(pending);
		expect(mails[0].subject).toBe(`[Hangar] diagnóstico ${code} · linux-x86_64`);
		expect(mails[0].text.startsWith(`relatório: ${code}\nchave no KV: diag/${code}\n`)).toBe(true);
		expect(mails[0].text).toContain("resumo do pacote");
	});

	it("uses its own per-IP limit", async () => {
		const { d, calls } = deps(false);
		const res = await handle(postDiag(JSON.stringify(diagnostic())), d);
		expect(res.status).toBe(429);
		expect(calls.limits).toEqual(["diagnostico"]);
		expect(calls.all).toBe(0);
		expect(await stored()).toHaveLength(0);
	});

	it("still goes through the global limit", async () => {
		const { d } = deps(true, false, false);
		expect((await handle(postDiag(JSON.stringify(diagnostic())), d)).status).toBe(429);
		expect(await stored()).toHaveLength(0);
	});

	it("answers 503 and mails nothing when KV fails", async () => {
		const { d, mails, pending } = deps();
		d.reports = { put: async () => { throw new Error("kv fora"); } } as unknown as KVNamespace;
		const res = await handle(postDiag(JSON.stringify(diagnostic())), d);
		await Promise.all(pending);
		expect(res.status).toBe(503);
		expect(await res.json()).toEqual({ erro: "armazenamento" });
		expect(mails).toHaveLength(0);
	});

	it("leaves no 90-day report when only the report write fails", async () => {
		const { d } = deps();
		const real = env.REPORTS;
		d.reports = { put: async (k: string, v: string, o: KVNamespacePutOptions) => {
			if (k.endsWith("/upload")) return real.put(k, v, o);
			throw new Error("kv fora");
		} } as unknown as KVNamespace;
		expect((await handle(postDiag(JSON.stringify(diagnostic())), d)).status).toBe(503);
		const keys = await stored();
		expect(keys).toHaveLength(1);
		expect(keys[0].name).toMatch(/^diag\/[a-z2-7]{8}\/upload$/);
	});

	it("answers 503 over the daily KV ceiling, unlike installation", async () => {
		const { d, mails, pending } = deps(true, false, true, KV_DAILY_MAX + 1);
		const res = await handle(postDiag(JSON.stringify(diagnostic())), d);
		await Promise.all(pending);
		expect(res.status).toBe(503);
		expect(await stored()).toHaveLength(0);
		expect(mails).toHaveLength(0);
	});

	it("rejects an unknown kind and a fixed diagnostic", async () => {
		const { d } = deps();
		expect((await handle(postDiag(JSON.stringify(report({ kind: "outro" }))), d)).status).toBe(400);
		expect((await handle(postDiag(JSON.stringify(report({ kind: "instalacao" }))), d)).status).toBe(400);
		expect((await handle(postDiag(JSON.stringify(diagnostic({ outcome: "consertado" }))), d)).status).toBe(400);
		expect(await stored()).toHaveLength(0);
	});

	it("accepts each kind only from its own app", async () => {
		const { d } = deps();
		expect((await handle(postDiag(JSON.stringify(diagnostic())), d)).status).toBe(201);
		expect((await handle(postDiag(JSON.stringify(diagnostic()), { "x-hangar-stamp": "hangar-x/1.0" }), d)).status).toBe(403);
		expect((await handle(post(JSON.stringify(report()), { "x-hangar-stamp": "hangar-server/0.1.0.8380" }), d)).status).toBe(403);
		const native = await handle(post(JSON.stringify(diagnostic())), d);
		expect(native.status).toBe(403);
		expect(await native.json()).toEqual({ erro: "carimbo" });
	});
});

function putDiary(body: Uint8Array, created: Pick<DiagCreated, "code" | "upload">, headers: Record<string, string> = {}) {
	return new Request("https://hangar.dev.br/api/relatorio/diario", { method: "PUT", body,
		headers: { "content-type": "application/gzip", "content-length": String(body.byteLength), "x-hangar-stamp": "hangar-server/0.1.0.8380",
			"x-hangar-code": created.code, "x-hangar-upload": created.upload, "cf-connecting-ip": "203.0.113.7", ...headers } });
}

const GZIP = new Uint8Array([0x1f, 0x8b, 0x08, 0x00, 1, 2, 3, 250, 251, 252]);

async function createDiag(d: Deps): Promise<DiagCreated> {
	return (await handle(postDiag(JSON.stringify(diagnostic())), d)).json<DiagCreated>();
}

describe("PUT /api/relatorio/diario", () => {
	it("stores the body untouched for 90 days and burns the password", async () => {
		const { d, calls } = deps();
		const created = await createDiag(d);
		const before = Math.floor(Date.now() / 1000);
		const res = await handle(putDiary(GZIP, created), d);
		expect(res.status).toBe(201);
		const key = `diag/${created.code}/diario.jsonl.gz`;
		expect(new Uint8Array((await env.REPORTS.get(key, "arrayBuffer"))!)).toEqual(GZIP);
		expect((await stored()).find((k) => k.name === key)!.expiration).toBeGreaterThanOrEqual(before + TTL_SECONDS - 5);
		expect(await env.REPORTS.get(`diag/${created.code}/upload`)).toBeNull();
		// Limite próprio do diário: as várias máquinas de um envio mandam cada uma o seu, do mesmo IP.
		expect(calls.limits).toEqual(["diagnostico", "diario"]);
	});

	it("refuses a wrong password without storing", async () => {
		const { d } = deps();
		const created = await createDiag(d);
		const wrong = created.upload.startsWith("a") ? `b${created.upload.slice(1)}` : `a${created.upload.slice(1)}`;
		expect((await handle(putDiary(GZIP, { code: created.code, upload: wrong }), d)).status).toBe(403);
		expect(await env.REPORTS.get(`diag/${created.code}/diario.jsonl.gz`)).toBeNull();
		expect(await env.REPORTS.get(`diag/${created.code}/upload`)).toBe(created.upload);
	});

	it("refuses an expired password", async () => {
		const { d } = deps();
		const created = await createDiag(d);
		await env.REPORTS.delete(`diag/${created.code}/upload`);
		expect((await handle(putDiary(GZIP, created), d)).status).toBe(403);
		expect(await env.REPORTS.get(`diag/${created.code}/diario.jsonl.gz`)).toBeNull();
	});

	it("refuses the same password twice", async () => {
		const { d } = deps();
		const created = await createDiag(d);
		expect((await handle(putDiary(GZIP, created), d)).status).toBe(201);
		expect((await handle(putDiary(new Uint8Array([9, 9, 9]), created), d)).status).toBe(403);
		expect(new Uint8Array((await env.REPORTS.get(`diag/${created.code}/diario.jsonl.gz`, "arrayBuffer"))!)).toEqual(GZIP);
	});

	it("refuses a declared length over 10 MiB before reading the password", async () => {
		const { d } = deps();
		const created = await createDiag(d);
		expect((await handle(putDiary(GZIP, created, { "content-length": String(DIARY_MAX_BYTES + 1) }), d)).status).toBe(413);
		expect(await env.REPORTS.get(`diag/${created.code}/upload`)).toBe(created.upload);
	});

	it("needs a declared length", async () => {
		const { d } = deps();
		const created = await createDiag(d);
		const headers = new Headers(putDiary(GZIP, created).headers);
		headers.delete("content-length");
		// Corpo em stream: sem tamanho conhecido, nada preenche o content-length.
		const body = new ReadableStream({ start(c) { c.enqueue(GZIP); c.close(); } });
		const res = await handle(new Request("https://hangar.dev.br/api/relatorio/diario", { method: "PUT", body, headers }), d);
		expect(res.status).toBe(411);
		expect(await env.REPORTS.get(`diag/${created.code}/upload`)).toBe(created.upload);
	});

	it("refuses the right code with another report's password", async () => {
		const { d } = deps();
		const first = await createDiag(d);
		const second = await createDiag(d);
		expect((await handle(putDiary(GZIP, { code: first.code, upload: second.upload }), d)).status).toBe(403);
		expect(await env.REPORTS.get(`diag/${first.code}/diario.jsonl.gz`)).toBeNull();
		expect(await env.REPORTS.get(`diag/${first.code}/upload`)).toBe(first.upload);
	});

	it("answers 503 over the daily KV ceiling, counting two writes", async () => {
		const { d, calls } = deps();
		const created = await createDiag(d);
		d.countToday = async (writes) => { calls.writes.push(writes); return KV_DAILY_MAX + 1; };
		const res = await handle(putDiary(GZIP, created), d);
		expect(res.status).toBe(503);
		expect(await res.json()).toEqual({ erro: "armazenamento" });
		expect(calls.writes).toEqual([2, 2]);
		expect(await env.REPORTS.get(`diag/${created.code}/diario.jsonl.gz`)).toBeNull();
	});

	it("needs the server stamp, the code, the password and PUT", async () => {
		const { d } = deps();
		const created = await createDiag(d);
		expect((await handle(putDiary(GZIP, created, { "x-hangar-stamp": "curl/8" }), d)).status).toBe(403);
		expect((await handle(putDiary(GZIP, created, { "x-hangar-stamp": "hangar-native/0.20.1.3456" }), d)).status).toBe(403);
		expect((await handle(putDiary(GZIP, created, { "x-hangar-code": "" }), d)).status).toBe(403);
		expect((await handle(putDiary(GZIP, created, { "x-hangar-upload": "" }), d)).status).toBe(403);
		expect((await handle(new Request("https://hangar.dev.br/api/relatorio/diario"), d)).status).toBe(405);
		expect(await env.REPORTS.get(`diag/${created.code}/upload`)).toBe(created.upload);
	});

	it("answers 503 when KV fails", async () => {
		const { d } = deps();
		const created = await createDiag(d);
		const real = env.REPORTS;
		d.reports = { get: (k: string) => real.get(k), delete: (k: string) => real.delete(k),
			put: async () => { throw new Error("kv fora"); } } as unknown as KVNamespace;
		const res = await handle(putDiary(GZIP, created), d);
		expect(res.status).toBe(503);
		expect(await res.json()).toEqual({ erro: "armazenamento" });
	});
});

describe("POST /api/relatorio with x-hangar-group (one report per machine)", () => {
	async function first(d: Deps) { return (await handle(postDiag(JSON.stringify(diagnostic())), d)).json<DiagCreated>(); }
	const member = (group: string) => postDiag(JSON.stringify(diagnostic()), { "x-hangar-group": group });

	it("skips the per-IP limit, records the group in each report and keeps the list", async () => {
		const lead = await first(deps().d);
		const { d, calls } = deps(false);
		const res = await handle(member(lead.code), d);
		expect(res.status).toBe(201);
		const body = await res.json<DiagCreated>();
		expect(calls.limits).toEqual([]);
		expect(calls.all).toBe(1);
		expect(calls.writes).toEqual([3]);
		expect(JSON.parse((await env.REPORTS.get(body.id))!).group).toBe(lead.code);
		expect(JSON.parse((await env.REPORTS.get(lead.id))!).group).toBe(lead.code);
		expect(JSON.parse((await env.REPORTS.get(`${lead.id}/group`))!)).toEqual([lead.code, body.code]);
		const keys = await stored();
		expect(keys.find((k) => k.name === `${lead.id}/group`)!.expiration).toBeGreaterThan(Math.floor(Date.now() / 1000) + TTL_SECONDS - 60);
	});

	it("caps a group at 8 machines", async () => {
		const lead = await first(deps().d);
		for (let i = 0; i < 7; i++) expect((await handle(member(lead.code), deps().d)).status).toBe(201);
		const res = await handle(member(lead.code), deps().d);
		expect(res.status).toBe(429);
		expect(await res.json()).toEqual({ erro: "grupo-cheio" });
		expect(JSON.parse((await env.REPORTS.get(`${lead.id}/group`))!)).toHaveLength(8);
	});

	it("refuses an unknown or malformed group after charging the per-IP limit", async () => {
		for (const group of ["abcdefgh", "NAO-VALE", ""]) {
			const { d, calls } = deps();
			const res = await handle(member(group), d);
			expect(res.status).toBe(400);
			expect(await res.json()).toEqual({ erro: "grupo" });
			expect(calls.limits).toEqual(["diagnostico"]);
			expect(calls.all).toBe(0);
		}
		expect(await stored()).toHaveLength(0);
	});

	it("refuses a member's code as a group: groups don't chain past the per-IP limit", async () => {
		const lead = await first(deps().d);
		const joined = await (await handle(member(lead.code), deps().d)).json<DiagCreated>();
		const { d, calls } = deps();
		const res = await handle(member(joined.code), d);
		expect(res.status).toBe(400);
		expect(await res.json()).toEqual({ erro: "grupo" });
		expect(calls.limits).toEqual(["diagnostico"]);
	});

	it("still pays the per-IP limit for an unknown group", async () => {
		const { d } = deps(false);
		expect((await handle(member("abcdefgh"), d)).status).toBe(429);
	});

	it("still goes through the global limit", async () => {
		const lead = await first(deps().d);
		const res = await handle(member(lead.code), deps(true, false, false).d);
		expect(res.status).toBe(429);
		expect(await env.REPORTS.get(`${lead.id}/group`)).toBeNull();
	});

	it("only groups diagnostics", async () => {
		const lead = await first(deps().d);
		const res = await handle(post(JSON.stringify(report()), { "x-hangar-group": lead.code }), deps().d);
		expect(res.status).toBe(400);
		expect(await res.json()).toEqual({ erro: "grupo" });
	});
});
