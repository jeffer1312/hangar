import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const CLAUDE_SETTINGS_PATH = join(homedir(), ".claude/settings.json");
const ADAPTER_CONFIG_PATH = join(homedir(), ".pi/agent/claude-hooks-adapter.json");

type ClaudeHook = {
	type?: string;
	command?: string;
	statusMessage?: string;
};

type ClaudeHookGroup = {
	matcher?: string;
	hooks?: ClaudeHook[];
};

type AdapterConfig = {
	enabled?: boolean;
	timeoutMs?: number;
	allowPatterns?: string[];
	skipPatterns?: string[];
	events?: Record<string, boolean>;
};

function readJson(path: string): any {
	return JSON.parse(readFileSync(path, "utf8"));
}

function loadConfig(): Required<AdapterConfig> {
	const defaults: Required<AdapterConfig> = {
		enabled: true,
		timeoutMs: 15_000,
		// Default: only the user's own hooks dir. Hooks that live elsewhere go in
		// ~/.pi/agent/claude-hooks-adapter.json (allowPatterns REPLACES this list).
		allowPatterns: [
			"/.claude/hooks/",
		],
		skipPatterns: [
			// The Hangar's Claude-only hooks: the Pi has its own extension for state/preview,
			// and AskUserQuestion capture reads a Claude payload that never exists here.
			"/backend/hooks/state_hook.py",
			"/backend/hooks/askq_capture.py",
			"/backend/hooks/preview_hook.py",
			"/backend/hooks/subagent_hook.py",
		],
		events: {
			SessionStart: true,
			PreToolUse: true,
			PostToolUse: true,
			UserPromptSubmit: true,
			Stop: true,
		},
	};
	if (!existsSync(ADAPTER_CONFIG_PATH)) return defaults;
	try {
		const user = readJson(ADAPTER_CONFIG_PATH) as AdapterConfig;
		return {
			...defaults,
			...user,
			allowPatterns: user.allowPatterns ?? defaults.allowPatterns,
			skipPatterns: user.skipPatterns ?? defaults.skipPatterns,
			events: { ...defaults.events, ...(user.events ?? {}) },
		};
	} catch {
		return defaults;
	}
}

function loadClaudeHooks(): Record<string, ClaudeHookGroup[]> {
	if (!existsSync(CLAUDE_SETTINGS_PATH)) return {};
	try {
		return readJson(CLAUDE_SETTINGS_PATH)?.hooks ?? {};
	} catch {
		return {};
	}
}

function shouldRunCommand(command: string, config: Required<AdapterConfig>): boolean {
	// The Hangar writes Windows hooks with backslashes; the patterns are written with slashes.
	const cmd = command.replaceAll("\\", "/");
	if (config.allowPatterns.length > 0 && !config.allowPatterns.some((p) => cmd.includes(p))) {
		return false;
	}
	if (config.skipPatterns.some((p) => cmd.includes(p))) return false;
	return true;
}

function piToolToClaudeName(name: string | undefined): string {
	if (!name) return "";
	const map: Record<string, string> = {
		bash: "Bash",
		read: "Read",
		write: "Write",
		edit: "Edit",
		grep: "Grep",
		find: "Find",
		ls: "LS",
	};
	return map[name] ?? name;
}

function matcherMatches(matcher: string | undefined, toolName: string | undefined): boolean {
	if (!matcher) return true;
	const claudeName = piToolToClaudeName(toolName).toLowerCase();
	const piName = (toolName ?? "").toLowerCase();
	return matcher
		.split("|")
		.map((x) => x.trim().toLowerCase())
		.some((x) => x === claudeName || x === piName || x === "*");
}

type HookEffects = { deny: string | null; contexts: string[] };

// Lê o protocolo Claude da saída do hook: `permissionDecision: "deny"` bloqueia (PreToolUse) e
// `additionalContext` vira mensagem pro agente. No UserPromptSubmit o Claude também trata stdout
// puro (sem JSON) como contexto — é o caso do skill-suggester, que imprime texto direto. Nos
// outros eventos stdout solto é só log e não injeta.
function extractEffects(eventName: string, stdout: string, acc: HookEffects): void {
	const plain: string[] = [];
	for (const line of stdout.split("\n")) {
		const t = line.trim();
		if (!t) continue;
		if (!t.startsWith("{")) {
			plain.push(line);
			continue;
		}
		let out: any;
		try {
			out = JSON.parse(t);
		} catch {
			plain.push(line);
			continue;
		}
		const specific = out?.hookSpecificOutput;
		if (specific?.permissionDecision === "deny" && !acc.deny) {
			acc.deny = String(specific.permissionDecisionReason || "Bloqueado por hook do Claude");
		}
		if (typeof specific?.additionalContext === "string" && specific.additionalContext.trim()) {
			acc.contexts.push(specific.additionalContext);
		}
	}
	if (eventName === "UserPromptSubmit" && plain.length) {
		acc.contexts.push(plain.join("\n"));
	}
}

function runClaudeHooks(
	eventName: string,
	ctx: any,
	payload: Record<string, any>,
	toolName?: string,
): HookEffects {
	const effects: HookEffects = { deny: null, contexts: [] };
	const config = loadConfig();
	if (!config.enabled || !config.events[eventName]) return effects;
	const groups = loadClaudeHooks()[eventName] ?? [];
	if (groups.length === 0) return effects;

	const envFromClaude = (() => {
		try {
			return readJson(CLAUDE_SETTINGS_PATH)?.env ?? {};
		} catch {
			return {};
		}
	})();

	for (const group of groups) {
		if (!matcherMatches(group.matcher, toolName)) continue;
		for (const hook of group.hooks ?? []) {
			if (hook.type && hook.type !== "command") continue;
			if (!hook.command || !shouldRunCommand(hook.command, config)) continue;

			const input = JSON.stringify({
				hook_event_name: eventName,
				cwd: ctx.cwd,
				session_id: process.env.PI_SESSION_ID,
				transcript_path: process.env.PI_SESSION_FILE,
				tool_name: toolName ? piToolToClaudeName(toolName) : undefined,
				...payload,
			}) + "\n";

			const result = spawnSync("bash", ["-lc", hook.command], {
				cwd: ctx.cwd,
				input,
				encoding: "utf8",
				timeout: config.timeoutMs,
				env: { ...process.env, ...envFromClaude, PI_CLAUDE_HOOK_ADAPTER: "1" },
			});

			extractEffects(eventName, result.stdout ?? "", effects);

			if (result.error || result.status !== 0) {
				const reason = result.error?.message || result.stderr || `exit ${result.status}`;
				ctx.ui?.notify?.(`Claude hook adapter: ${hook.command}\n${reason}`.slice(0, 1200), "warning");
			}
		}
	}
	return effects;
}

export default function (pi: ExtensionAPI) {
	pi.on("session_start", async (event, ctx) => {
		runClaudeHooks("SessionStart", ctx, { reason: event.reason });
	});

	pi.on("before_agent_start", async (event, ctx) => {
		const fx = runClaudeHooks("UserPromptSubmit", ctx, {
			prompt: event.prompt,
			images: event.images,
		});
		if (fx.contexts.length) {
			return {
				message: {
					customType: "claude-hook-context",
					content: fx.contexts.join("\n\n"),
					display: true,
				},
			};
		}
	});

	pi.on("tool_call", async (event, ctx) => {
		const fx = runClaudeHooks("PreToolUse", ctx, {
			tool_input: event.input,
			tool_call_id: event.toolCallId,
		}, event.toolName);
		if (fx.deny) return { block: true, reason: fx.deny };
	});

	pi.on("tool_result", async (event, ctx) => {
		const fx = runClaudeHooks("PostToolUse", ctx, {
			tool_input: event.input,
			tool_response: event.content,
			is_error: event.isError,
			tool_call_id: event.toolCallId,
		}, event.toolName);
		// additionalContext do hook vira um bloco de texto extra no resultado da tool — é o canal
		// dos lembretes (plano-adversarial, plano-html) chegarem ao agente como chegam no Claude.
		if (fx.contexts.length && Array.isArray(event.content)) {
			return {
				content: [...event.content, { type: "text", text: fx.contexts.join("\n\n") }],
			};
		}
	});

	pi.on("agent_settled", async (_event, ctx) => {
		runClaudeHooks("Stop", ctx, {});
	});

	pi.registerCommand("claude-hooks-adapter", {
		description: "Show Claude hooks adapter status",
		handler: async (_args, ctx) => {
			const config = loadConfig();
			const hooks = loadClaudeHooks();
			const counts = Object.fromEntries(
				Object.entries(hooks).map(([name, groups]) => [
					name,
					(groups as ClaudeHookGroup[]).flatMap((g) => g.hooks ?? []).filter((h) => h.command && shouldRunCommand(h.command, config)).length,
				]),
			);
			ctx.ui.notify(
				[
					`enabled: ${config.enabled}`,
					`config: ${ADAPTER_CONFIG_PATH}`,
					`claude settings: ${CLAUDE_SETTINGS_PATH}`,
					`mapped hooks: ${JSON.stringify(counts)}`,
				].join("\n"),
				"info",
			);
		},
	});
}
