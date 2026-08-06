/**
 * Rich Status Line for Pi
 *
 * Replaces the default footer with a Claude Code/OmniRoute-like status line showing:
 * - Current model + thinking level
 * - Project folder + optional ticket
 * - Git branch + dirty status
 * - Tmux session + optional peers
 * - Service + environment badge
 * - kubectl current-context (red alert on prod)
 * - Token usage (input/output + context used/total)
 * - Session cost
 * - Session duration
 * - Clock
 *
 * Configure via ~/.pi/agent/rich-status-line.json:
 * {
 *   "projectName": "my-web-app",
 *   "ticket": "TICKET-123",
 *   "service": "my-service",
 *   "environment": "k8s-prod",
 *   "environmentColor": "purple",
 *   "tmuxPeers": ["agent-2"]
 * }
 *
 * Toggle with /statusline
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { truncateToWidth } from "@earendil-works/pi-tui";
import { execFile, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { homedir } from "node:os";

// ── claude-cockpit: publica a linha INTEIRA num sidecar ────────────────────────────────────────
// O `truncateToWidth` lá embaixo corta a linha na largura do terminal ANTES de imprimir, e o app
// (que lê o pane) herdava o corte: numa janela de 99 colunas sumiam ⚡5h/📅7d, custo e a janela de
// contexto — o medidor do app virava "medição indisponível" só por causa da largura. Aqui a versão
// completa (sem ANSI) vai pra <config>/.claude-pocket-status/<stem>.json, mesma chave dos outros
// marcadores do cockpit (o stem do .jsonl da sessão). Só escreve quando o texto MUDA: o render
// roda a cada tecla.
const CP_STATUS_DIR = join(process.env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude"),
	".claude-pocket-status");
const CP_ANSI = /\x1b\[[0-9;:?]*[ -/]*[@-~]/g;
let cpLastLine = "";

function cpPublishStatus(ctx: any, line: string): void {
	try {
		const file = ctx?.sessionManager?.getSessionFile?.();
		const plain = line.replace(CP_ANSI, "");
		if (!file || !plain.trim() || plain === cpLastLine) return;
		cpLastLine = plain;
		mkdirSync(CP_STATUS_DIR, { recursive: true });
		const target = join(CP_STATUS_DIR, `${basename(file, ".jsonl")}.json`);
		const tmp = `${target}.${process.pid}.tmp`;   // nome unico: duas sessoes/renders nao brigam
		writeFileSync(tmp, JSON.stringify({ line: plain, ts: Date.now() / 1000 }));
		renameSync(tmp, target);   // atômico: o backend pode ler no meio da escrita
	} catch {
		// Sidecar é conveniência: falhar aqui não pode derrubar o render do rodapé. O app cai no
		// pane, que é o comportamento de antes.
	}
}

interface StatusLineConfig {
	projectName?: string;
	ticket?: string;
	service?: string;
	environment?: string;
	environmentColor?: "purple" | "blue" | "green" | "red" | "yellow";
	showKubectl?: boolean;
	tmuxPeers?: string[];
}

const CONFIG_PATH = join(homedir(), ".pi/agent/rich-status-line.json");
const PI_AUTH_PATH = join(homedir(), ".pi/agent/auth.json");
const PI_MODELS_PATH = join(homedir(), ".pi/agent/models.json");

function loadConfig(): StatusLineConfig {
	if (!existsSync(CONFIG_PATH)) return {};
	try {
		return JSON.parse(readFileSync(CONFIG_PATH, "utf8")) as StatusLineConfig;
	} catch {
		return {};
	}
}

function formatTokens(n: number): string {
	if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
	if (n >= 1000) return `${Math.round(n / 1000)}k`;
	return `${n}`;
}

function formatDurationMinutes(totalMinutes: number): string {
	const h = Math.floor(totalMinutes / 60);
	const m = totalMinutes % 60;
	if (h > 0) return `${h}h${m > 0 ? `${m}m` : ""}`;
	return `${m}m`;
}

function formatDurationSeconds(totalSeconds: number): string {
	const d = Math.floor(totalSeconds / 86400);
	const h = Math.floor((totalSeconds % 86400) / 3600);
	const m = Math.floor((totalSeconds % 3600) / 60);
	if (d > 0) return `${d}d${h > 0 ? `${h}h` : ""}`;
	if (h > 0) return `${h}h${m > 0 ? `${m}m` : ""}`;
	return `${m}m`;
}

let codexUsageCache: { text: string; fetchedAt: number } | null = null;

function codexUsageSummary(): string {
	const now = Date.now();
	if (codexUsageCache && now - codexUsageCache.fetchedAt < 300_000) {
		return codexUsageCache.text;
	}
	try {
		const auth = JSON.parse(readFileSync(PI_AUTH_PATH, "utf8"));
		const token = auth?.["openai-codex"]?.access;
		if (!token) return "";
		const body = execFileSync(
			"curl",
			[
				"-sS",
				"-m",
				"2",
				"-H",
				`Authorization: Bearer ${token}`,
				"https://chatgpt.com/backend-api/wham/usage",
			],
			{ encoding: "utf8", timeout: 2500, stdio: ["ignore", "pipe", "ignore"] },
		);
		const data = JSON.parse(body);
		const primary = data?.rate_limit?.primary_window;
		const secondary = data?.rate_limit?.secondary_window;
		const weekly = primary
			? `📅7d:${primary.used_percent ?? "?"}% ↻${formatDurationSeconds(primary.reset_after_seconds ?? 0)}`
			: "📅7d:?";
		const fiveHour = secondary
			? `⚡5h:${secondary.used_percent ?? "?"}% ↻${formatDurationSeconds(secondary.reset_after_seconds ?? 0)}`
			: "⚡5h:ok";
		const text = `${fiveHour} ${weekly}`;
		codexUsageCache = { text, fetchedAt: now };
		return text;
	} catch {
		return codexUsageCache?.text ?? "";
	}
}

// Store estruturado (pct + resetTime como timestamp), NÃO texto pronto: o ↻countdown é
// calculado na hora do render a partir do resetAt, então ele anda a cada render (30s) em vez
// de ficar congelado por 5min dentro do texto cacheado — que era o que fazia o chip parecer morto.
interface KimiUsageWindow {
	pct: number | "?";
	resetAt: number | null; // epoch ms
}
let kimiUsage: {
	token: string;
	fiveHour: KimiUsageWindow | null;
	weekly: KimiUsageWindow | null;
	fetchedAt: number;
} | null = null;
let kimiFetchInFlight = false;
let kimiLastAttempt = 0;

// Busca ASSÍNCRONA e desacoplada do render: o render nunca bloqueia no curl, e o timer de 30s
// do footer chama isto diretamente — o chip não depende mais de acontecer um render depois do
// cache vencer pra buscar de novo (era por isso que ele só atualizava quando entrava mensagem).
function kimiUsageRefresh(token: string): void {
	const now = Date.now();
	if (kimiFetchInFlight) return;
	if (kimiUsage && kimiUsage.token === token && now - kimiUsage.fetchedAt < 300_000) return;
	if (now - kimiLastAttempt < 60_000) return; // backoff: falhou, não martela a API a cada render
	kimiFetchInFlight = true;
	kimiLastAttempt = now;
	execFile(
		"curl",
		[
			"-sS",
			"-m",
			"5",
			"-H",
			`Authorization: Bearer ${token}`,
			"-H",
			"User-Agent: KimiCLI/1.5",
			"https://api.kimi.com/coding/v1/usages",
		],
		{ encoding: "utf8", timeout: 7000 },
		(err, stdout) => {
			kimiFetchInFlight = false;
			if (err) return; // mantém o dado velho; próxima tentativa em 60s
			try {
				const data = JSON.parse(stdout);
				if (data?.error) return; // 401 & cia voltam 200 no curl sem -f: não sobrescreve dado bom
				const windowOf = (d: any): KimiUsageWindow => ({
					pct: d?.limit ? Math.round((Number(d?.used ?? 0) / Number(d.limit)) * 100) : "?",
					resetAt: d?.resetTime ? new Date(d.resetTime).getTime() : null,
				});
				const fiveHourDetail = data?.limits?.find(
					(l: any) => l?.window?.timeUnit === "TIME_UNIT_MINUTE" && l?.window?.duration === 300,
				)?.detail;
				kimiUsage = {
					token,
					fiveHour: fiveHourDetail ? windowOf(fiveHourDetail) : null,
					weekly: data?.usage ? windowOf(data.usage) : null,
					fetchedAt: Date.now(),
				};
			} catch {}
		},
	);
}

// A quota que interessa é a da conta que a sessão está GASTANDO — e a sessão gasta pela key do
// provider ativo, não pela do auth.json. As duas podem ser contas diferentes (ex: provider
// custom "kimi-jefferson" com $KIMI_API_KEY_2 enquanto o auth.json guarda outra key), e mostrar
// a quota da conta errada é pior que não mostrar nada. Por isso a key vem da MESMA fonte das
// chamadas do modelo: provider custom -> apiKey do models.json (resolvendo indireção $ENV);
// provider built-in "kimi-coding" (login do pi) -> auth.json, que é onde ele autentica.
// Não conseguiu resolver -> sem chip.
function kimiTokenFor(providerName: string | undefined): string {
	if (!providerName) return "";
	try {
		const models = JSON.parse(readFileSync(PI_MODELS_PATH, "utf8"));
		const prov = models?.providers?.[providerName];
		if (prov) {
			if (!/api\.kimi\.com/i.test(prov.baseUrl ?? "")) return "";
			const apiKey = String(prov.apiKey ?? "");
			return apiKey.startsWith("$") ? (process.env[apiKey.slice(1)] ?? "") : apiKey;
		}
	} catch {}
	if (providerName === "kimi-coding") {
		try {
			const auth = JSON.parse(readFileSync(PI_AUTH_PATH, "utf8"));
			return auth?.["kimi-coding"]?.access ?? auth?.["kimi-coding"]?.key ?? "";
		} catch {}
	}
	return "";
}

function kimiUsageSummary(token: string): string {
	kimiUsageRefresh(token); // não bloqueia: dispara a busca se vencida e formata o que já tem
	if (!kimiUsage || kimiUsage.token !== token) return "";
	const now = Date.now();
	const fmt = (w: KimiUsageWindow | null, label: string, empty: string): string => {
		if (!w) return empty;
		const reset = w.resetAt
			? ` ↻${formatDurationSeconds(Math.max(0, Math.floor((w.resetAt - now) / 1000)))}`
			: "";
		return `${label}:${w.pct}%${reset}`;
	};
	return `${fmt(kimiUsage.fiveHour, "⚡5h", "⚡5h:ok")} ${fmt(kimiUsage.weekly, "📅7d", "📅7d:?")}`;
}

function coloredBadge(text: string, color: string | undefined): string {
	const colors: Record<string, { bg: string; fg: string }> = {
		purple: { bg: "\x1b[105m", fg: "\x1b[97m" },
		blue: { bg: "\x1b[44m", fg: "\x1b[97m" },
		green: { bg: "\x1b[42m", fg: "\x1b[30m" },
		red: { bg: "\x1b[41m", fg: "\x1b[97m" },
		yellow: { bg: "\x1b[43m", fg: "\x1b[30m" },
	};
	const c = colors[color ?? "purple"] ?? colors.purple;
	return `${c.bg}${c.fg} ${text} \x1b[0m`;
}

function gitInfo(cwd: string): { branch: string | null; dirty: boolean } {
	try {
		const branch = execFileSync(
			"git",
			["-C", cwd, "--no-optional-locks", "rev-parse", "--abbrev-ref", "HEAD"],
			{ encoding: "utf8", timeout: 1000, stdio: ["ignore", "pipe", "ignore"] },
		).trim();
		if (!branch) return { branch: null, dirty: false };
		const dirty =
			execFileSync(
				"git",
				["-C", cwd, "--no-optional-locks", "status", "--porcelain"],
				{ encoding: "utf8", timeout: 1000, stdio: ["ignore", "pipe", "ignore"] },
			).trim().length > 0;
		return { branch, dirty };
	} catch {
		return { branch: null, dirty: false };
	}
}

function tmuxInfo(config: StatusLineConfig): { session: string | null; peers: string[] } {
	if (!process.env.TMUX || !process.env.TMUX_PANE) return { session: null, peers: [] };
	try {
		const session = execFileSync(
			"tmux",
			["display-message", "-p", "-t", process.env.TMUX_PANE, "#S"],
			{ encoding: "utf8", timeout: 1000, stdio: ["ignore", "pipe", "ignore"] },
		).trim();
		if (!session) return { session: null, peers: [] };

		let peers: string[] = [];
		try {
			const pair = JSON.parse(
				readFileSync(join(homedir(), ".pi/agent/.pi-pocket-pair", `${session}.json`), "utf8"),
			);
			peers = pair.peers || (pair.peer ? [pair.peer] : []);
		} catch {}
		if (peers.length === 0 && config.tmuxPeers) {
			peers = config.tmuxPeers;
		}
		return { session, peers };
	} catch {
		return { session: null, peers: [] };
	}
}

function kubectlContext(): { context: string; isProd: boolean } | null {
	try {
		const context = execFileSync(
			"kubectl",
			["config", "current-context"],
			{ encoding: "utf8", timeout: 1000, stdio: ["ignore", "pipe", "ignore"] },
		).trim();
		if (!context) return null;
		return { context, isProd: /prod/i.test(context) };
	} catch {
		return null;
	}
}

let clineUsageCache: { text: string; fetchedAt: number } | null = null;

function clineUsageSummary(): string {
	const now = Date.now();
	if (clineUsageCache && now - clineUsageCache.fetchedAt < 300_000) {
		return clineUsageCache.text;
	}

	try {
		const auth = JSON.parse(readFileSync(PI_AUTH_PATH, "utf8"));
		const token =
			auth?.clinepass?.access ??
			auth?.clinepass?.key ??
			auth?.cline?.access ??
			auth?.cline?.key;
		if (!token) return "";

		const curlJson = (url: string): any => {
			const body = execFileSync(
				"curl",
				["-sS", "-m", "4", "-H", `Authorization: Bearer ${token}`, url],
				{ encoding: "utf8", timeout: 4500, stdio: ["ignore", "pipe", "ignore"] },
			);
			return JSON.parse(body);
		};

		const me = curlJson("https://api.cline.bot/api/v1/users/me");
		const userId = me?.data?.id;
		if (!userId) return "";

		const plansResponse = curlJson("https://api.cline.bot/api/v1/plans");
		const plans = Array.isArray(plansResponse?.data)
			? plansResponse.data
			: Array.isArray(plansResponse?.data?.items)
				? plansResponse.data.items
				: [];
		const passPlan = plans.find(
			(p: any) =>
				p?.entitlements?.cline_pass?.enabled === true &&
				p?.entitlements?.cline_pass?.inferenceCapThreshold,
		);
		const thresholds = passPlan?.entitlements?.cline_pass?.inferenceCapThreshold;
		if (!thresholds) return "";

		const usagesResponse = curlJson(
			`https://api.cline.bot/api/v1/users/${userId}/usages?limit=200`,
		);
		const items = Array.isArray(usagesResponse?.data?.items)
			? usagesResponse.data.items
			: [];

		const sumSince = (seconds: number) => {
			const cutoff = now - seconds * 1000;
			return items.reduce((total: number, item: any) => {
				if (item?.operation !== "chat_completion") return total;
				if (item?.aiModelTypeName !== "cline-pass" && item?.metadata?.model_type !== "cline-pass") {
					return total;
				}
				const createdAt = Date.parse(item?.createdAt ?? "");
				if (!Number.isFinite(createdAt) || createdAt < cutoff) return total;
				return total + (Number(item?.costUsd) || 0);
			}, 0);
		};

		const formatWindow = (label: string, used: number, limit: number) => {
			if (!Number.isFinite(limit) || limit <= 0) return `${label}:?`;
			const percent = Math.min(999, Math.round((used / limit) * 100));
			return `${label}:${percent}%`;
		};

		const fiveHourUsed = sumSince(5 * 60 * 60);
		const weeklyUsed = sumSince(7 * 24 * 60 * 60);
		const monthlyUsed = sumSince(30 * 24 * 60 * 60);

		const text = [
			formatWindow("⚡5h", fiveHourUsed, Number(thresholds.last5HoursUsageCostUSDPerUser)),
			formatWindow("📅7d", weeklyUsed, Number(thresholds.last7daysUsageCostUSDPerUser)),
			formatWindow("🗓30d", monthlyUsed, Number(thresholds.last30daysUsageCostUSDPerUser)),
		].join(" ");

		clineUsageCache = { text, fetchedAt: now };
		return text;
	} catch {
		return clineUsageCache?.text ?? "";
	}
}

export default function (pi: ExtensionAPI) {
	let enabled = true;
	let sessionStartTime = Date.now();
	const config = loadConfig();

	function renderFooter(
		ctx: any,
		footerData: any,
		theme: any,
		width: number,
	): string[] {
		// Model + thinking level
		const model = ctx.model;
		const thinking = ctx.thinkingLevel ?? "off";
		const modelStr = model ? `${model.id ?? model.name} (${thinking})` : "no-model";
		const modelPart = theme.fg("accent", "🤖 " + modelStr);

		// Project + ticket + git branch
		const cwdParts = ctx.cwd.replace(/\\/g, "/").split("/").filter(Boolean);
		const folder = config.projectName ?? cwdParts[cwdParts.length - 1] ?? "project";
		const git = gitInfo(ctx.cwd);
		const ticket = config.ticket ? ` [${config.ticket}]` : "";
		let projectStr = "📁 " + folder + ticket;
		if (git.branch) {
			const branchColor = git.dirty ? "warning" : "success";
			projectStr += ` ${theme.fg(branchColor, `[${git.branch}${git.dirty ? "*" : ""}]`)}`;
		}
		const projectPart = theme.fg("text", projectStr);

		// Service
		const servicePart = config.service ? theme.fg("text", "🤝 " + config.service) : "";

		// Tmux session + peers
		const tmux = tmuxInfo(config);
		const tmuxPart = tmux.session
			? theme.fg(
				"text",
				`📟 ${tmux.session}` + (tmux.peers.length ? ` 🤝 ${tmux.peers.join(",")}` : ""),
			)
			: "";

		// Environment badge + kubectl context. Keep these opt-in because a global
		// kube context or static config can look like session-specific truth.
		const kctx = config.showKubectl ? kubectlContext() : null;
		const envParts: string[] = [];
		if (config.environment && config.environment !== kctx?.context) {
			const isProdEnv = /prod/i.test(config.environment);
			envParts.push(coloredBadge(config.environment, isProdEnv ? "red" : config.environmentColor));
		}
		if (kctx) {
			envParts.push(
				kctx.isProd
					? coloredBadge(`⚠ ${kctx.context}`, "red")
					: theme.fg("info", `⎈ ${kctx.context}`),
			);
		}

		// Metrics (session tokens + context usage)
		let input = 0;
		let output = 0;
		let cacheRead = 0;
		let reasoning = 0;
		let totalTokens = 0;
		let cost = 0;
		for (const e of ctx.sessionManager.getBranch()) {
			if (e.type === "message" && e.message.role === "assistant") {
				const m = e.message as AssistantMessage;
				const u = m.usage as any;
				input += u?.input ?? 0;
				output += u?.output ?? 0;
				cacheRead += u?.cacheRead ?? 0;
				reasoning += u?.reasoning ?? 0;
				totalTokens += u?.totalTokens ?? 0;
				cost += u?.cost?.total ?? 0;
			}
		}

		const usage = ctx.getContextUsage();
		const ctxWindow = usage?.contextWindow ?? ctx.model?.contextWindow;
		let ctxUsage = "";
		if (ctxWindow) {
			ctxUsage = usage?.tokens
				? ` ctx ${formatTokens(usage.tokens)}/${formatTokens(ctxWindow)}`
				: ` ctx ?/${formatTokens(ctxWindow)}`;
		}
		const tokensPart = theme.fg(
			"dim",
			`💬 sessão ${formatTokens(input)}in/${formatTokens(output)}out · cache ${formatTokens(cacheRead)} · total ${formatTokens(totalTokens)}${ctxUsage}`,
		);
		const codexLimitText = ctx.model?.provider === "openai-codex" || ctx.model?.id?.startsWith("gpt-")
			? codexUsageSummary()
			: "";
		const codexLimitPart = codexLimitText ? theme.fg("warning", codexLimitText) : "";

		const kimiToken = kimiTokenFor(ctx.model?.provider);
		const kimiLimitText = kimiToken ? kimiUsageSummary(kimiToken) : "";
		const kimiLimitPart = kimiLimitText ? theme.fg("warning", kimiLimitText) : "";

		const clineLimitText = ctx.model?.provider === "clinepass" ? clineUsageSummary() : "";
		const clineLimitPart = clineLimitText ? theme.fg("warning", clineLimitText) : "";

		// Cost
		const costPart = theme.fg("success", `💵 $${cost.toFixed(2)}`);

		// Session duration
		const elapsedMin = Math.floor((Date.now() - sessionStartTime) / 60000);
		const timePart = theme.fg("dim", "⏱ " + formatDurationMinutes(elapsedMin));

		// Clock
		const now = new Date();
		const clockPart = theme.fg(
			"dim",
			"🕐 " + now.toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" }),
		);

		// Compose a compact single-flow line. Avoid left/right padding because it
		// creates a large visual hole in wide terminals.
		const parts = [
			modelPart,
			projectPart,
			servicePart,
			tmuxPart,
			...envParts,
			tokensPart,
			codexLimitPart,
			kimiLimitPart,
			clineLimitPart,
			costPart,
			timePart,
			clockPart,
		].filter(Boolean);
		const full = parts.join(" " + theme.fg("border", "│") + " ");
		cpPublishStatus(ctx, full);      // versão inteira pro app, ANTES do corte por largura
		const line = truncateToWidth(full, width);
		return [line];
	}

	function attachFooter(ctx: any) {
		ctx.ui.setFooter((tui: any, theme: any, footerData: any) => {
			const unsubBranch = footerData.onBranchChange(() => tui.requestRender());

			const interval = setInterval(() => {
				// A busca de quota NÃO depende do render acontecer: o próprio timer dispara o refresh
				// em background; o render seguinte (quando quer que seja) já mostra dado fresco.
				const kimiToken = kimiTokenFor(ctx.model?.provider);
				if (kimiToken) kimiUsageRefresh(kimiToken);
				tui.requestRender();
			}, 30_000);

			return {
				dispose: () => {
					unsubBranch();
					clearInterval(interval);
				},
				invalidate() {},
				render(width: number): string[] {
					if (!enabled) return [];
					return renderFooter(ctx, footerData, theme, width);
				},
			};
		});
	}

	pi.on("session_start", async (_event, ctx) => {
		sessionStartTime = Date.now();
		if (enabled && ctx.mode === "tui") {
			// Some UI packages, especially sticky/Claude-style footer renderers, bind
			// their footer during reload/startup after other extensions. Re-attach a
			// couple of times so this custom statusline wins without needing the user
			// to run /statusline-on after every /reload.
			attachFooter(ctx);
			setTimeout(() => enabled && attachFooter(ctx), 50);
			setTimeout(() => enabled && attachFooter(ctx), 250);
		}
	});

	pi.registerCommand("statusline", {
		description: "Toggle the rich status line footer",
		handler: async (_args, ctx) => {
			enabled = !enabled;
			if (enabled) {
				attachFooter(ctx);
				ctx.ui.notify("Rich status line enabled", "info");
			} else {
				ctx.ui.setFooter(undefined);
				ctx.ui.notify("Default footer restored", "info");
			}
		},
	});

	pi.registerCommand("statusline-on", {
		description: "Force-enable the rich status line footer",
		handler: async (_args, ctx) => {
			enabled = true;
			attachFooter(ctx);
			ctx.ui.notify("Rich status line enabled", "info");
		},
	});

	pi.registerCommand("statusline-off", {
		description: "Force-disable the rich status line footer",
		handler: async (_args, ctx) => {
			enabled = false;
			ctx.ui.setFooter(undefined);
			ctx.ui.notify("Default footer restored", "info");
		},
	});

	pi.registerCommand("statusline-config", {
		description: "Show rich status line config path and current values",
		handler: async (_args, ctx) => {
			const lines = [
				`Config: ${CONFIG_PATH}`,
				`projectName: ${config.projectName ?? "(auto from cwd)"}`,
				`ticket: ${config.ticket ?? "(none)"}`,
				`service: ${config.service ?? "(none)"}`,
				`environment: ${config.environment ?? "(none)"}`,
				`environmentColor: ${config.environmentColor ?? "purple"}`,
				`showKubectl: ${config.showKubectl ?? false}`,
				`tmuxPeers: ${config.tmuxPeers?.join(", ") ?? "(none)"}`,
			];
			ctx.ui.notify(lines.join("\n"), "info");
		},
	});
}
