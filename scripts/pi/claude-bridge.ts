/**
 * pi-claude-bridge: usa a config do Claude Code (~/.claude) dentro do pi sem duplicar
 * nem editar nada na origem. Roda no load de cada sessão e materializa:
 *
 *  1. AGENTS  — ~/.claude/agents + ~/.claude/plugins/marketplaces/<x>/agents
 *               -> ~/.pi/agent/agents/claude-bridge/<fonte>/<nome>.md (pro pi-subagents).
 *               tools em array JSON viram lista pi (Read->read, Glob->find,
 *               WebSearch->web_search, WebFetch->fetch_content, Task->subagent,
 *               mcp__servidor__tool -> servidor_tool); model alias Anthropic
 *               (sonnet/opus/haiku/inherit/claude-*) é removido -> herda o modelo da
 *               sessão, qualquer provider; effort -> thinking.
 *  2. COMMANDS — ~/.claude/commands + marketplaces/<x>/commands
 *               -> ~/.pi/agent/prompts/<nome>.md (prompt template nativo do pi;
 *               $ARGUMENTS/$1 já funcionam). Nunca sobrescreve prompt que não é da
 *               ponte (manifest em ~/.pi/agent/claude-bridge-manifest.json).
 *  3. SKILLS de plugin versionado — ~/.claude/plugins/cache/<mkt>/<plugin>/<versão>/skills
 *               -> symlink ~/.pi/agent/skills-bridge/<plugin> sempre re-apontado pra
 *               versão mais nova (default: superpowers + ecc). O cache é de onde o
 *               Claude carrega de verdade — poda tipo ecc-slim.sh (move pra
 *               skills-disabled/) vale automaticamente pro pi. ecc commands idem
 *               (cacheCommands). Garante o dir em settings.skills e remove entradas legadas.
 *
 * Config opcional: ~/.pi/agent/claude-bridge.json
 *   { "enabled": true,
 *     "agents":   { "extraSources": [], "exclude": [], "modelMap": {"opus": "prov/id"} },
 *     "commands": { "extraSources": [], "exclude": [] },
 *     "skillPlugins": ["superpowers", "ecc"],
 *     "marketplaceSkills": [],
 *     "cacheCommands": ["ecc"],
 *     "disabledSources": { "agents": [], "commands": [], "skills": [] } }
 *
 * Comandos: /claude-bridge (status + resync) e /claude-bridge-config (liga/desliga
 * fontes num menu estilo /plugins do Claude; skills valem na próxima sessão).
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { parseFrontmatter } from "@earendil-works/pi-coding-agent";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const HOME = os.homedir();
const CLAUDE_DIR = path.join(HOME, ".claude");
const PI_AGENT_DIR = path.join(HOME, ".pi/agent");
const AGENTS_OUT = path.join(PI_AGENT_DIR, "agents/claude-bridge");
const PROMPTS_OUT = path.join(PI_AGENT_DIR, "prompts");
const SKILLS_OUT = path.join(PI_AGENT_DIR, "skills-bridge");
const CONFIG_PATH = path.join(PI_AGENT_DIR, "claude-bridge.json");
const MANIFEST_PATH = path.join(PI_AGENT_DIR, "claude-bridge-manifest.json");
const SETTINGS_PATH = path.join(PI_AGENT_DIR, "settings.json");
// entradas antigas que a ponte já usou e hoje substitui (symlink preso em versão; clone do
// marketplace que ignora a poda do ecc-slim — o Claude carrega do CACHE, não do clone)
const LEGACY_SKILLS_ENTRIES = ["~/.claude/skills-superpowers", "~/.claude/plugins/marketplaces/ecc/skills"];

type Config = {
	enabled?: boolean;
	agents?: { extraSources?: string[]; exclude?: string[]; modelMap?: Record<string, string> };
	commands?: { extraSources?: string[]; exclude?: string[] };
	skillPlugins?: string[];
	marketplaceSkills?: string[];
	/** Fontes de commands que vêm do cache versionado do plugin (respeita poda tipo ecc-slim). */
	cacheCommands?: string[];
	/** Fontes desligadas no /claude-bridge-config (labels de fonte; skills usa nome do plugin/marketplace). */
	disabledSources?: { agents?: string[]; commands?: string[]; skills?: string[] };
};

function loadConfig(): Config {
	try {
		return JSON.parse(fs.readFileSync(CONFIG_PATH, "utf8"));
	} catch {
		return {};
	}
}

function saveConfig(config: Config): void {
	fs.mkdirSync(path.dirname(CONFIG_PATH), { recursive: true });
	fs.writeFileSync(CONFIG_PATH, `${JSON.stringify(config, null, 2)}\n`);
}

function isDisabled(config: Config, kind: "agents" | "commands" | "skills", label: string): boolean {
	return (config.disabledSources?.[kind] ?? []).includes(label);
}

function expandHome(p: string): string {
	return p.replace(/^~(?=\/)/, HOME);
}

function listMd(dir: string): string[] {
	try {
		return fs.readdirSync(dir).filter((f) => f.endsWith(".md"));
	} catch {
		return [];
	}
}

/** Fontes <label, dir>: dir do usuário primeiro, depois marketplaces em ordem alfabética. */
function discoverSources(kind: "agents" | "commands", extra: string[]): { label: string; dir: string }[] {
	const sources: { label: string; dir: string }[] = [];
	const userDir = path.join(CLAUDE_DIR, kind);
	if (fs.existsSync(userDir)) sources.push({ label: "user", dir: userDir });
	const marketplaces = path.join(CLAUDE_DIR, "plugins/marketplaces");
	let entries: fs.Dirent[] = [];
	try {
		entries = fs.readdirSync(marketplaces, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name));
	} catch {}
	for (const entry of entries) {
		if (!entry.isDirectory() && !entry.isSymbolicLink()) continue;
		const dir = path.join(marketplaces, entry.name, kind);
		if (fs.existsSync(dir)) sources.push({ label: entry.name, dir });
	}
	for (const raw of extra) {
		const dir = expandHome(raw);
		if (fs.existsSync(dir)) sources.push({ label: path.basename(path.dirname(dir)), dir });
	}
	return sources;
}

// ---------------------------------------------------------------- agents

// Claude tool -> pi tool; null = sem equivalente, cai fora da lista
const TOOL_MAP: Record<string, string | null> = {
	read: "read",
	write: "write",
	edit: "edit",
	multiedit: "edit",
	bash: "bash",
	grep: "grep",
	glob: "find",
	ls: "ls",
	websearch: "web_search", // pi-web-access
	webfetch: "fetch_content", // pi-web-access
	task: "subagent", // pi-subagents
	agent: "subagent",
	todowrite: null,
	notebookedit: null,
	skill: null,
	askuserquestion: null,
};

const MODEL_ALIASES = new Set(["sonnet", "opus", "haiku", "inherit"]);
const THINKING_LEVELS = new Set(["off", "minimal", "low", "medium", "high", "xhigh", "max"]);

function convertTools(raw: unknown): { tools?: string[]; dropped: string[]; unusable: boolean } {
	if (raw === undefined) return { dropped: [], unusable: false };
	let items: unknown[];
	if (Array.isArray(raw)) items = raw;
	else if (typeof raw === "string") items = raw.split(",");
	else return { dropped: [], unusable: true };
	const tools: string[] = [];
	const dropped: string[] = [];
	for (const item of items) {
		if (typeof item !== "string") continue;
		const name = item.trim();
		if (!name) continue;
		// mcp__servidor__tool -> servidor_tool (formato do mcp.ts do pi-code)
		if (name.startsWith("mcp__")) {
			const piName = name.slice("mcp__".length).replace("__", "_").replaceAll("-", "_");
			if (!tools.includes(piName)) tools.push(piName);
			continue;
		}
		const mapped = TOOL_MAP[name.toLowerCase()];
		if (mapped === undefined || mapped === null) {
			dropped.push(name);
			continue;
		}
		if (!tools.includes(mapped)) tools.push(mapped);
	}
	// tinha restrição mas nenhuma tool sobrou -> não pode rodar irrestrito
	if (tools.length === 0) return { dropped, unusable: true };
	return { tools, dropped, unusable: false };
}

function convertModel(raw: unknown, modelMap: Record<string, string>): string | undefined {
	if (typeof raw !== "string" || !raw.trim()) return undefined;
	const model = raw.trim();
	const key = model.toLowerCase();
	if (modelMap[key]) return modelMap[key];
	// alias/id Anthropic sem provider -> remove, herda modelo da sessão
	if (MODEL_ALIASES.has(key) || /^claude-/.test(key)) return undefined;
	return model;
}

export function convertAgent(
	content: string,
	config: Config,
): { name: string; content: string; droppedTools: string[] } | null {
	let parsed: { frontmatter: Record<string, unknown>; body: string };
	try {
		parsed = parseFrontmatter(content);
	} catch {
		return null;
	}
	const fm = parsed.frontmatter;
	const name = typeof fm.name === "string" ? fm.name.trim() : "";
	const description = typeof fm.description === "string" ? fm.description : "";
	if (!name || !description) return null;
	if (config.agents?.exclude?.includes(name)) return null;

	const { tools, dropped, unusable } = convertTools(fm.tools);
	if (unusable) return null;
	const model = convertModel(fm.model, config.agents?.modelMap ?? {});
	const effort = typeof fm.effort === "string" && THINKING_LEVELS.has(fm.effort.trim().toLowerCase())
		? fm.effort.trim().toLowerCase()
		: undefined;

	const lines = ["---", `name: ${name}`, `description: ${description.replace(/\s*\n\s*/g, " ").trim()}`];
	if (tools) lines.push(`tools: ${tools.join(", ")}`);
	if (model) lines.push(`model: ${model}`);
	if (effort) lines.push(`thinking: ${effort}`);
	lines.push("---", "", parsed.body.trim(), "");
	return { name, content: lines.join("\n"), droppedTools: dropped };
}

// ---------------------------------------------------------------- commands

export function convertCommand(content: string, fileName: string, config: Config): { name: string; content: string } | null {
	const name = path.basename(fileName, ".md");
	if (config.commands?.exclude?.includes(name)) return null;
	let parsed: { frontmatter: Record<string, unknown>; body: string };
	try {
		parsed = parseFrontmatter(content);
	} catch {
		return null;
	}
	const body = parsed.body.trim();
	if (!body) return null;
	const description = typeof parsed.frontmatter.description === "string"
		? parsed.frontmatter.description.replace(/\s*\n\s*/g, " ").trim()
		: `Comando Claude ${name} (via claude-bridge)`;
	// allowed-tools/model/argument-hint do Claude não têm equivalente em prompt do pi; body vai
	// intacto ($ARGUMENTS e $1..$n funcionam nativo no pi)
	return { name, content: ["---", `description: ${description}`, "---", "", body, ""].join("\n") };
}

// ---------------------------------------------------------------- skills de plugin versionado

function latestVersionDir(pluginDir: string): string | null {
	let versions: string[] = [];
	try {
		versions = fs.readdirSync(pluginDir).filter((v) => /^\d+\.\d+\.\d+$/.test(v));
	} catch {
		return null;
	}
	if (!versions.length) return null;
	versions.sort((a, b) => {
		const pa = a.split(".").map(Number);
		const pb = b.split(".").map(Number);
		return pa[0] - pb[0] || pa[1] - pb[1] || pa[2] - pb[2];
	});
	return versions[versions.length - 1];
}

/** cache/<marketplace>/<plugin>/<maior versão>/<sub>, ou null se não existe. */
function latestCacheSubdir(plugin: string, sub: string): string | null {
	const cacheRoot = path.join(CLAUDE_DIR, "plugins/cache");
	let marketplaces: string[] = [];
	try {
		marketplaces = fs.readdirSync(cacheRoot);
	} catch {}
	let target: string | null = null;
	for (const marketplace of marketplaces) {
		const version = latestVersionDir(path.join(cacheRoot, marketplace, plugin));
		if (!version) continue;
		const dir = path.join(cacheRoot, marketplace, plugin, version, sub);
		if (fs.existsSync(dir)) target = dir;
	}
	return target;
}

/** skills-bridge/<plugin> -> cache/<marketplace>/<plugin>/<maior versão>/skills */
function syncSkillLinks(plugins: string[], disabled: string[]): { linked: string[]; missing: string[] } {
	const linked: string[] = [];
	const missing: string[] = [];
	for (const plugin of plugins) {
		if (disabled.includes(plugin)) {
			try {
				fs.rmSync(path.join(SKILLS_OUT, plugin), { force: true });
			} catch {}
			continue;
		}
		const target = latestCacheSubdir(plugin, "skills");
		if (!target) {
			missing.push(plugin);
			continue;
		}
		fs.mkdirSync(SKILLS_OUT, { recursive: true });
		const link = path.join(SKILLS_OUT, plugin);
		let current: string | undefined;
		try {
			current = fs.readlinkSync(link);
		} catch {}
		if (current !== target) {
			try {
				fs.rmSync(link, { force: true });
			} catch {}
			fs.symlinkSync(target, link);
		}
		linked.push(`${plugin} -> ${target}`);
	}
	return { linked, missing };
}

/**
 * Garante no settings.skills: skills-bridge + skills de marketplace (espelho do que está
 * ativo no Claude — ecc:config-gc poda no disco, então o dir JÁ é o conjunto ativo).
 * Tira a entrada legada do symlink preso em versão.
 */
function ensureSettingsSkills(marketplaceSkills: string[], disabled: string[]): boolean {
	let settings: Record<string, unknown>;
	try {
		settings = JSON.parse(fs.readFileSync(SETTINGS_PATH, "utf8"));
	} catch {
		return false;
	}
	const skills: unknown = settings.skills;
	if (!Array.isArray(skills)) return false;
	const wanted: string[] = ["~/.pi/agent/skills-bridge"];
	const unwanted: string[] = [];
	for (const marketplace of marketplaceSkills) {
		const entry = `~/.claude/plugins/marketplaces/${marketplace}/skills`;
		if (disabled.includes(marketplace)) unwanted.push(entry);
		else if (fs.existsSync(expandHome(entry))) wanted.push(entry);
	}
	unwanted.push(...LEGACY_SKILLS_ENTRIES);
	const next = skills.filter((s) => !unwanted.includes(s as string));
	for (const entry of wanted) {
		if (!next.includes(entry)) next.push(entry);
	}
	if (next.length === skills.length && next.every((s, i) => s === skills[i])) return false;
	settings.skills = next;
	fs.writeFileSync(SETTINGS_PATH, `${JSON.stringify(settings, null, 2)}\n`);
	return true;
}

// ---------------------------------------------------------------- sync

type SyncResult = {
	agents: { total: number; written: number; removed: number; dropped: Record<string, string[]> };
	commands: { total: number; written: number; removed: number; skipped: string[] };
	skills: { linked: string[]; missing: string[]; settingsChanged: boolean };
};

function readManifest(): string[] {
	try {
		const parsed = JSON.parse(fs.readFileSync(MANIFEST_PATH, "utf8"));
		return Array.isArray(parsed.prompts) ? parsed.prompts : [];
	} catch {
		return [];
	}
}

function writeIfChanged(target: string, content: string): boolean {
	let current: string | undefined;
	try {
		current = fs.readFileSync(target, "utf8");
	} catch {}
	if (current === content) return false;
	fs.mkdirSync(path.dirname(target), { recursive: true });
	fs.writeFileSync(target, content);
	return true;
}

export function sync(): SyncResult {
	const config = loadConfig();
	const result: SyncResult = {
		agents: { total: 0, written: 0, removed: 0, dropped: {} },
		commands: { total: 0, written: 0, removed: 0, skipped: [] },
		skills: { linked: [], missing: [], settingsChanged: false },
	};
	if (config.enabled === false) return result;

	// -- agents
	const expectedAgents = new Map<string, string>();
	for (const source of discoverSources("agents", config.agents?.extraSources ?? [])) {
		if (isDisabled(config, "agents", source.label)) continue;
		for (const file of listMd(source.dir)) {
			let content: string;
			try {
				content = fs.readFileSync(path.join(source.dir, file), "utf8");
			} catch {
				continue;
			}
			const converted = convertAgent(content, config);
			if (!converted) continue;
			expectedAgents.set(path.join(source.label, `${converted.name}.md`), converted.content);
			if (converted.droppedTools.length) result.agents.dropped[converted.name] = converted.droppedTools;
		}
	}
	result.agents.total = expectedAgents.size;
	for (const [relative, content] of expectedAgents) {
		if (writeIfChanged(path.join(AGENTS_OUT, relative), content)) result.agents.written++;
	}
	const staleAgents = (dir: string): void => {
		let entries: fs.Dirent[] = [];
		try {
			entries = fs.readdirSync(dir, { withFileTypes: true });
		} catch {
			return;
		}
		for (const entry of entries) {
			const full = path.join(dir, entry.name);
			if (entry.isDirectory()) {
				staleAgents(full);
				continue;
			}
			if (!expectedAgents.has(path.relative(AGENTS_OUT, full))) {
				try {
					fs.rmSync(full);
					result.agents.removed++;
				} catch {}
			}
		}
	};
	staleAgents(AGENTS_OUT);

	// -- commands -> prompts (flat; primeiro que registra o nome ganha)
	const owned = new Set(readManifest());
	const expectedPrompts = new Map<string, string>();
	const cacheCommands = config.cacheCommands ?? ["ecc"];
	const commandSources = discoverSources("commands", config.commands?.extraSources ?? []).map((source) => {
		// plugin com poda tipo ecc-slim: o conjunto ativo mora no cache, não no clone do marketplace
		if (!cacheCommands.includes(source.label)) return source;
		const cached = latestCacheSubdir(source.label, "commands");
		return cached ? { label: source.label, dir: cached } : source;
	});
	for (const source of commandSources) {
		if (isDisabled(config, "commands", source.label)) continue;
		for (const file of listMd(source.dir)) {
			let content: string;
			try {
				content = fs.readFileSync(path.join(source.dir, file), "utf8");
			} catch {
				continue;
			}
			const converted = convertCommand(content, file, config);
			if (!converted || expectedPrompts.has(`${converted.name}.md`)) continue;
			const target = path.join(PROMPTS_OUT, `${converted.name}.md`);
			// prompt pré-existente que não é da ponte: não sobrescrever
			if (!owned.has(`${converted.name}.md`) && fs.existsSync(target)) {
				result.commands.skipped.push(converted.name);
				continue;
			}
			expectedPrompts.set(`${converted.name}.md`, converted.content);
		}
	}
	result.commands.total = expectedPrompts.size;
	for (const [file, content] of expectedPrompts) {
		if (writeIfChanged(path.join(PROMPTS_OUT, file), content)) result.commands.written++;
	}
	for (const file of owned) {
		if (!expectedPrompts.has(file)) {
			try {
				fs.rmSync(path.join(PROMPTS_OUT, file));
				result.commands.removed++;
			} catch {}
		}
	}
	fs.mkdirSync(path.dirname(MANIFEST_PATH), { recursive: true });
	fs.writeFileSync(MANIFEST_PATH, `${JSON.stringify({ prompts: [...expectedPrompts.keys()] }, null, 2)}\n`);

	// -- skills de plugin versionado + settings
	const disabledSkills = config.disabledSources?.skills ?? [];
	const links = syncSkillLinks(config.skillPlugins ?? ["superpowers", "ecc"], disabledSkills);
	result.skills.linked = links.linked;
	result.skills.missing = links.missing;
	result.skills.settingsChanged = ensureSettingsSkills(config.marketplaceSkills ?? [], disabledSkills);

	return result;
}

/**
 * Memórias por projeto do Claude (~/.claude/projects/<cwd-sanitizado>/memory/MEMORY.md) — o
 * índice entra no system prompt do pi, igual o Claude Code faz. O corpo das memórias fica em
 * disco; o modelo lê sob demanda com a tool read (o índice traz os caminhos).
 */
function sanitizeCwd(cwd: string): string {
	return cwd.replace(/[^A-Za-z0-9]/g, "-");
}

function memoryIndex(cwd: string): string | null {
	const dir = path.join(CLAUDE_DIR, "projects", sanitizeCwd(cwd), "memory");
	try {
		const index = fs.readFileSync(path.join(dir, "MEMORY.md"), "utf8").trim();
		return index ? `\n\n# Memórias do projeto (Claude, índice — corpo em ${dir}/)\n${index}` : null;
	} catch {
		return null;
	}
}

export default function (pi: ExtensionAPI) {
	try {
		sync();
	} catch {
		// falha na ponte nunca derruba a sessão; /claude-bridge mostra o estado
	}

	pi.on("before_agent_start", async (event: any, ctx: any) => {
		try {
			const addition = memoryIndex(ctx?.cwd ?? process.cwd());
			if (addition) return { systemPrompt: event.systemPrompt + addition };
		} catch {}
	});

	pi.registerCommand("claude-bridge-config", {
		description: "Liga/desliga fontes da ponte Claude (agents, commands, skills) — estilo /plugins",
		handler: async (_args, ctx) => {
			const config = loadConfig();
			const disabled = {
				agents: [...(config.disabledSources?.agents ?? [])],
				commands: [...(config.disabledSources?.commands ?? [])],
				skills: [...(config.disabledSources?.skills ?? [])],
			};
			type Item = { kind: "agents" | "commands" | "skills"; label: string; detail: string };
			const items: Item[] = [];
			for (const source of discoverSources("agents", config.agents?.extraSources ?? [])) {
				const count = listMd(source.dir).length;
				if (count) items.push({ kind: "agents", label: source.label, detail: `${count} agents` });
			}
			const cacheCommands = config.cacheCommands ?? ["ecc"];
			for (const source of discoverSources("commands", config.commands?.extraSources ?? [])) {
				const dir = cacheCommands.includes(source.label) ? (latestCacheSubdir(source.label, "commands") ?? source.dir) : source.dir;
				const count = listMd(dir).length;
				if (count) items.push({ kind: "commands", label: source.label, detail: `${count} commands` });
			}
			for (const marketplace of config.marketplaceSkills ?? []) {
				const dir = path.join(CLAUDE_DIR, "plugins/marketplaces", marketplace, "skills");
				if (!fs.existsSync(dir)) continue;
				let count = 0;
				try {
					count = fs.readdirSync(dir).length;
				} catch {}
				items.push({ kind: "skills", label: marketplace, detail: `${count} skills (marketplace)` });
			}
			for (const plugin of config.skillPlugins ?? ["superpowers", "ecc"]) {
				const dir = latestCacheSubdir(plugin, "skills");
				let count = 0;
				try {
					count = dir ? fs.readdirSync(dir).length : 0;
				} catch {}
				items.push({ kind: "skills", label: plugin, detail: `${count} skills ativas (cache — segue poda do Claude)` });
			}

			const SAVE = "salvar e resync";
			const CANCEL = "cancelar";
			while (true) {
				const options = [
					...items.map((item) => {
						const on = !disabled[item.kind].includes(item.label);
						return `[${on ? "x" : " "}] ${item.kind}: ${item.label} — ${item.detail}`;
					}),
					SAVE,
					CANCEL,
				];
				const choice = await ctx.ui.select("claude-bridge — enter alterna, salvar aplica", options);
				if (choice === undefined || choice === CANCEL) return;
				if (choice === SAVE) break;
				const index = options.indexOf(choice);
				const item = items[index];
				if (!item) continue;
				const list = disabled[item.kind];
				const at = list.indexOf(item.label);
				if (at === -1) list.push(item.label);
				else list.splice(at, 1);
			}

			config.disabledSources = disabled;
			saveConfig(config);
			const r = sync();
			ctx.ui.notify(
				[
					`salvo em ${CONFIG_PATH}`,
					`agents: ${r.agents.total} · commands: ${r.commands.total} · skills: ${r.skills.linked.length ? r.skills.linked.map((l) => l.split(" -> ")[0]).join(", ") : "—"}`,
					"skills desligadas/religadas valem a partir da PRÓXIMA sessão (lista já carregada nesta).",
				].join("\n"),
				"info",
			);
		},
	});

	pi.registerCommand("claude-bridge", {
		description: "Resync Claude -> pi (agents, commands, skills) e mostra status",
		handler: async (_args, ctx) => {
			const r = sync();
			const droppedLines = Object.entries(r.agents.dropped)
				.slice(0, 10)
				.map(([agent, tools]) => `  ${agent}: sem equivalente pi -> ${tools.join(", ")}`);
			ctx.ui.notify(
				[
					`agents: ${r.agents.total} (alterados: ${r.agents.written}, removidos: ${r.agents.removed}) -> ${AGENTS_OUT}`,
					`commands: ${r.commands.total} (alterados: ${r.commands.written}, removidos: ${r.commands.removed}${r.commands.skipped.length ? `, pulados por conflito: ${r.commands.skipped.join(", ")}` : ""}) -> ${PROMPTS_OUT}`,
					`skills: ${r.skills.linked.join("; ") || "nenhum plugin"}${r.skills.missing.length ? ` | sem cache: ${r.skills.missing.join(", ")}` : ""}`,
					`config: ${CONFIG_PATH}`,
					...(droppedLines.length ? ["tools descartadas:", ...droppedLines] : []),
				].join("\n"),
				"info",
			);
		},
	});
}
