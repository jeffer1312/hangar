// A ponte converte lacunas do Claude; plugins nativos continuam com o instalador do harness.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { AgentContext } from "./lib/agent-context";
import type { FrontmatterParser, ParsedFrontmatter } from "./lib/frontmatter";
import { getAgentContext } from "./lib/agent-context";
import { loadFrontmatterParser } from "./lib/frontmatter";
import * as fs from "node:fs";
import { homedir } from "node:os";
import * as path from "node:path";
import { randomUUID } from "node:crypto";

export type Config = {
	enabled?: boolean;
	agents?: { extraSources?: string[]; exclude?: string[]; modelMap?: Record<string, string> };
	commands?: { extraSources?: string[]; exclude?: string[] };
	skillPlugins?: string[];
	marketplaceSkills?: string[];
	cacheCommands?: string[];
	disabledSources?: { agents?: string[]; commands?: string[]; skills?: string[] };
};

const PI_TOOLS: Record<string, string | null> = {
	read: "read", write: "write", edit: "edit", multiedit: "edit", bash: "bash", grep: "grep",
	glob: "find", ls: "ls", websearch: "web_search", webfetch: "fetch_content",
	task: "subagent", agent: "subagent", todowrite: null, notebookedit: null, skill: null, askuserquestion: null,
};
const OMP_TOOLS: Record<string, string | null> = {
	...PI_TOOLS, glob: "glob", ls: "read", webfetch: "read", task: "task", agent: "task",
	todowrite: "todo", notebookedit: "edit", askuserquestion: "ask",
};
const MODEL_ALIASES: Record<string, true> = { sonnet: true, opus: true, haiku: true, fable: true, inherit: true };
const THINKING_LEVELS: Record<string, true> = { off: true, minimal: true, low: true, medium: true, high: true, xhigh: true, max: true };

function convertTools(raw: unknown, harness: AgentContext["harness"]): { tools?: string[]; dropped: string[]; unusable: boolean } {
	if (raw === undefined) return { dropped: [], unusable: false };
	const items = Array.isArray(raw) ? raw : typeof raw === "string" ? raw.split(",") : [];
	const tools: string[] = [];
	const dropped: string[] = [];
	const mapping = harness === "omp" ? OMP_TOOLS : PI_TOOLS;
	for (const item of items) {
		if (typeof item !== "string" || !item.trim()) continue;
		const name = item.trim();
		const mapped = name.startsWith("mcp__")
			? harness === "omp" ? name : name.slice(5).replace("__", "_").replaceAll("-", "_")
			: Object.hasOwn(mapping, name.toLowerCase()) ? mapping[name.toLowerCase()] : undefined;
		if (!mapped) dropped.push(name);
		else if (!tools.includes(mapped)) tools.push(mapped);
	}
	// Restrição vazia ou incompatível nunca vira permissão irrestrita.
	return { tools, dropped, unusable: tools.length === 0 };
}

function convertModel(raw: unknown, modelMap: Record<string, string>): string | undefined {
	if (typeof raw !== "string" || !raw.trim()) return undefined;
	const model = raw.trim();
	const key = model.toLowerCase();
	if (Object.hasOwn(modelMap, key)) return modelMap[key];
	if (Object.hasOwn(MODEL_ALIASES, key) || /^claude-/.test(key)) return undefined;
	return model;
}

export function convertAgent(content: string, config: Config, harness: AgentContext["harness"], parse: FrontmatterParser): { name: string; content: string; droppedTools: string[] } | null {
	let parsed: ParsedFrontmatter;
	try { parsed = parse(content); } catch { return null; }
	return convertParsedAgent(parsed, config, harness);
}

function convertParsedAgent(parsed: ParsedFrontmatter, config: Config, harness: AgentContext["harness"]) {
	const fm = parsed.frontmatter;
	const name = typeof fm.name === "string" ? fm.name.trim() : "";
	const description = typeof fm.description === "string" ? fm.description : "";
	if (!/^[\p{L}\p{N}_-][\p{L}\p{N}_.-]*$/u.test(name) || !description.trim()) return null;
	if (harness === "omp" && ["main", "sub"].includes(name.toLowerCase())) return null;
	if (config.agents?.exclude?.includes(name)) return null;
	const { tools: allowed, dropped, unusable } = convertTools(fm.tools, harness);
	if (unusable) return null;
	let tools = allowed;
	if (fm.disallowedTools !== undefined) {
		const deniedItems: unknown[] | null = Array.isArray(fm.disallowedTools) ? fm.disallowedTools
			: typeof fm.disallowedTools === "string" ? fm.disallowedTools.split(",") : null;
		if (!deniedItems || !deniedItems.every((item): item is string => typeof item === "string" &&
			(!item.trim().startsWith("mcp__") || /^mcp__[\w-]+__[\w-]+$/.test(item.trim())))) return null;
		if (deniedItems.some(item => item.trim())) {
			const denied = convertTools(fm.disallowedTools, harness);
			// Sem inventário herdado ou negação representável, recusar é mais seguro que ampliar acesso.
			if (!tools || denied.unusable || denied.dropped.length) return null;
			tools = tools.filter(tool => !denied.tools?.includes(tool));
			if (!tools.length) return null;
		}
	}
	const model = convertModel(fm.model, config.agents?.modelMap ?? {});
	const effort = typeof fm.effort === "string" ? fm.effort.trim().toLowerCase() : "";
	const lines = ["---", `name: ${JSON.stringify(name)}`, `description: ${JSON.stringify(description.replace(/\s*\n\s*/g, " ").trim())}`];
	if (tools) lines.push(`tools: ${harness === "omp" ? JSON.stringify(tools) : JSON.stringify(tools.join(", "))}`);
	if (model) lines.push(`model: ${JSON.stringify(model)}`);
	if (Object.hasOwn(THINKING_LEVELS, effort)) lines.push(`thinking: ${effort}`);
	lines.push("---", "", parsed.body.trim(), "");
	return { name, content: lines.join("\n"), droppedTools: dropped };
}

export function convertCommand(content: string, fileName: string, config: Config, parse: FrontmatterParser): { name: string; content: string } | null {
	let parsed: ParsedFrontmatter;
	try { parsed = parse(content); } catch { return null; }
	return convertParsedCommand(parsed, fileName, config);
}

function convertParsedCommand(parsed: ParsedFrontmatter, fileName: string, config: Config) {
	const name = path.basename(fileName, ".md");
	if (config.commands?.exclude?.includes(name)) return null;
	const body = parsed.body.trim();
	if (!body) return null;
	const description = typeof parsed.frontmatter.description === "string"
		? parsed.frontmatter.description.replace(/\s*\n\s*/g, " ").trim()
		: `Comando Claude ${name} (via claude-bridge)`;
	return { name, content: ["---", `description: ${JSON.stringify(description)}`, "---", "", body, ""].join("\n") };
}

type Kind = "agents" | "commands";
type OwnedFile = { kind: Kind; content: string };
type Manifest = Record<string, OwnedFile>;
export type SyncResult = {
	agents: { total: number; written: number; removed: number; dropped: Record<string, string[]>; skipped: string[] };
	commands: { total: number; written: number; removed: number; skipped: string[] };
	skills: { linked: string[]; missing: string[]; settingsChanged: boolean };
};

function readOptional(file: string): string | undefined {
	try { return fs.readFileSync(file, "utf8"); }
	catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
		throw error;
	}
}

function atomicWrite(file: string, content: string): void {
	fs.mkdirSync(path.dirname(file), { recursive: true });
	const temporary = `${file}.${process.pid}.${randomUUID()}.tmp`;
	try {
		fs.writeFileSync(temporary, content, { flag: "wx" });
		fs.renameSync(temporary, file);
	} finally {
		fs.rmSync(temporary, { force: true });
	}
}

function listMd(dir: string): string[] {
	try { return fs.readdirSync(dir).filter(file => file.endsWith(".md")).sort(); }
	catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
		throw error;
	}
}

function listMdDeep(dir: string, base = dir): string[] {
	let entries: fs.Dirent[];
	try { entries = fs.readdirSync(dir, { withFileTypes: true }); }
	catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
		throw error;
	}
	const files: string[] = [];
	for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
		const full = path.join(dir, entry.name);
		if (entry.isDirectory()) files.push(...listMdDeep(full, base));
		else if (entry.isFile() && entry.name.endsWith(".md")) files.push(path.relative(base, full));
	}
	return files;
}

function latestVersionDir(pluginDir: string): string | null {
	let versions: string[];
	try { versions = fs.readdirSync(pluginDir).filter(v => /^\d+\.\d+\.\d+$/.test(v)); }
	catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
		throw error;
	}
	versions.sort((a, b) => {
		const pa = a.split(".").map(Number), pb = b.split(".").map(Number);
		return pa[0] - pb[0] || pa[1] - pb[1] || pa[2] - pb[2];
	});
	return versions.at(-1) ?? null;
}

export async function createBridge(pi: ExtensionAPI, context: AgentContext = getAgentContext()) {
	const parse = await loadFrontmatterParser(context);
	const { harness, agentDir, claudeDir } = context;
	const agentsOut = path.join(agentDir, harness === "omp" ? "agents" : "agents/claude-bridge");
	const promptsOut = path.join(agentDir, "prompts");
	const skillsOut = path.join(agentDir, "skills-bridge");
	const configPath = path.join(agentDir, "claude-bridge.json");
	const manifestPath = path.join(agentDir, "claude-bridge-manifest.json");
	const settingsPath = path.join(agentDir, "settings.json");
	const expandHome = (value: string) => path.resolve(value.replace(/^~(?=$|[\\/])/, homedir()));

	function loadConfig(): Config {
		const content = readOptional(configPath);
		if (content === undefined) return {};
		const parsed: unknown = JSON.parse(content);
		if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("Configuração da ponte inválida");
		return parsed as Config;
	}

	function discoverSources(kind: Kind, extra: string[]): { label: string; dir: string }[] {
		const sources = [{ label: "user", dir: path.join(claudeDir, kind) }];
		// No OMP, os plugins já têm descoberta nativa; a lacuna são agents pessoais/extras.
		if (harness === "pi") {
			const marketplaces = path.join(claudeDir, "plugins/marketplaces");
			let entries: fs.Dirent[] = [];
			try { entries = fs.readdirSync(marketplaces, { withFileTypes: true }); }
			catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
			for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
				if (entry.isDirectory() || entry.isSymbolicLink()) sources.push({ label: entry.name, dir: path.join(marketplaces, entry.name, kind) });
			}
		}
		for (const raw of extra) {
			const dir = expandHome(raw);
			sources.push({ label: path.basename(path.dirname(dir)), dir });
		}
		return sources;
	}

	function latestCacheSubdir(plugin: string, sub: string): string | null {
		const root = path.join(claudeDir, "plugins/cache");
		let marketplaces: string[];
		try { marketplaces = fs.readdirSync(root); }
		catch (error) {
			if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
			throw error;
		}
		let target: string | null = null;
		for (const marketplace of marketplaces) {
			const version = latestVersionDir(path.join(root, marketplace, plugin));
			if (!version) continue;
			const dir = path.join(root, marketplace, plugin, version, sub);
			if (fs.existsSync(dir)) target = dir;
		}
		return target;
	}

	// O manifesto v1 só listava nomes de prompts, e agents/claude-bridge/ era inteira da ponte —
	// os dois já eram sobrescritos e apagados por ela. Adotar uma vez, lendo o disco, é o que
	// impede toda instalação existente de virar conflito permanente na primeira rodada v2.
	function adoptLegacy(prompts: unknown[]): Manifest {
		const owned: Manifest = {};
		const adopt = (relative: string, kind: Kind) => {
			const full = path.join(agentDir, relative);
			let stat: fs.Stats;
			try { stat = fs.lstatSync(full); }
			catch (error) {
				if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
				throw error;
			}
			if (stat.isFile()) owned[relative] = { kind, content: fs.readFileSync(full, "utf8") };
		};
		for (const name of prompts) {
			if (typeof name === "string" && /^[^\\/]+\.md$/.test(name)) adopt(path.join("prompts", name), "commands");
		}
		const legacyAgents = path.join("agents", "claude-bridge");
		for (const file of listMdDeep(path.join(agentDir, legacyAgents))) adopt(path.join(legacyAgents, file), "agents");
		return owned;
	}

	function readManifest(): Manifest {
		const content = readOptional(manifestPath);
		if (content === undefined) return {};
		const parsed = JSON.parse(content);
		if (parsed && Array.isArray(parsed.prompts) && parsed.version === undefined) return adoptLegacy(parsed.prompts);
		if (parsed?.version !== 2 || !parsed.files || typeof parsed.files !== "object" || Array.isArray(parsed.files)) throw new Error("Manifesto da ponte inválido");
		for (const [relative, record] of Object.entries(parsed.files)) {
			const entry = record as OwnedFile;
			if (!entry || !["agents", "commands"].includes(entry.kind) || typeof entry.content !== "string" ||
				path.isAbsolute(relative) || relative.split(/[\\/]/).some(part => part === ".." || !part) ||
				!relative.startsWith((entry.kind === "agents" ? "agents" : "prompts") + path.sep)) throw new Error("Entrada inválida no manifesto da ponte");
		}
		return parsed.files;
	}

	function nativeNames(owned: Manifest): Set<string> {
		const names = new Set<string>();
		if (harness !== "omp") return names;
		for (const file of listMd(agentsOut)) {
			const full = path.join(agentsOut, file);
			const content = fs.readFileSync(full, "utf8");
			if (owned[path.relative(agentDir, full)]?.content === content) continue;
			let name: unknown;
			try { name = parse(content).frontmatter.name; }
			catch (error) {
				console.error(`[claude-bridge] agent nativo ilegível, ignorado na checagem de nomes: ${full} (${error instanceof Error ? error.message : String(error)})`);
				continue;
			}
			if (typeof name === "string") names.add(name);
		}
		return names;
	}

	// Com uma fonte ilegível não se sabe qual cópia gerenciada ela geraria: a ponte segue
	// criando e atualizando, mas não remove nada até a fonte voltar a ser lida.
	function reconcile(expected: Manifest, owned: Manifest, result: SyncResult, remove = true): Manifest {
		const next = { ...owned };
		for (const relative of new Set([...Object.keys(owned), ...Object.keys(expected)])) {
			const wanted = expected[relative];
			const previous = owned[relative];
			const target = path.join(agentDir, relative);
			const kind = (wanted ?? previous).kind;
			// Um link pessoal, inclusive em diretório ancestral, não é arquivo da ponte.
			let linked = false;
			for (let current = target; current !== agentDir; current = path.dirname(current)) {
				try { if (fs.lstatSync(current).isSymbolicLink()) { linked = true; break; } }
				catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
			}
			const current = linked ? undefined : readOptional(target);
			if (linked || (current !== undefined && (!previous || previous.content !== current))) {
				result[kind].skipped.push(relative);
				continue;
			}
			if (wanted) {
				if (current !== wanted.content) { atomicWrite(target, wanted.content); result[kind].written++; }
				next[relative] = wanted;
			} else if (!remove) {
				continue;
			} else {
				if (current !== undefined) { fs.unlinkSync(target); result[kind].removed++; }
				delete next[relative];
			}
		}
		return next;
	}

	function syncSkills(config: Config, result: SyncResult): void {
		const disabled = config.disabledSources?.skills ?? [];
		for (const plugin of config.skillPlugins ?? ["superpowers", "ecc"]) {
			if (!/^[\w.-]+$/.test(plugin) || plugin === "..") throw new Error("Nome de plugin inválido");
			const link = path.join(skillsOut, plugin);
			const target = latestCacheSubdir(plugin, "skills");
			let current: string | undefined;
			try { current = fs.readlinkSync(link); }
			catch (error) { if (!["ENOENT", "EINVAL"].includes((error as NodeJS.ErrnoException).code ?? "")) throw error; }
			const managed = current?.startsWith(path.join(claudeDir, "plugins/cache") + path.sep);
			if (disabled.includes(plugin)) { if (managed) fs.unlinkSync(link); continue; }
			if (!target) { result.skills.missing.push(plugin); continue; }
			if (current !== target) {
				if (fs.existsSync(link) && !managed) { result.skills.missing.push(`${plugin} (conflito)`); continue; }
				if (managed) fs.unlinkSync(link);
				fs.mkdirSync(skillsOut, { recursive: true });
				fs.symlinkSync(target, link);
			}
			result.skills.linked.push(`${plugin} -> ${target}`);
		}
		const content = readOptional(settingsPath);
		if (content === undefined) return;
		const settings = JSON.parse(content);
		if (!Array.isArray(settings.skills)) return;
		const wanted = [skillsOut];
		const unwanted = ["~/.claude/skills-superpowers", "~/.claude/plugins/marketplaces/ecc/skills"];
		for (const marketplace of config.marketplaceSkills ?? []) {
			const entry = path.join(claudeDir, "plugins/marketplaces", marketplace, "skills");
			if (disabled.includes(marketplace)) unwanted.push(entry);
			else if (fs.existsSync(entry)) wanted.push(entry);
		}
		const next = settings.skills.filter((entry: unknown) => typeof entry !== "string" || !unwanted.some(value => expandHome(value) === expandHome(entry)));
		for (const entry of wanted) if (!next.some((value: unknown) => typeof value === "string" && expandHome(value) === entry)) next.push(entry);
		if (JSON.stringify(next) !== JSON.stringify(settings.skills)) {
			settings.skills = next;
			atomicWrite(settingsPath, `${JSON.stringify(settings, null, 2)}\n`);
			result.skills.settingsChanged = true;
		}
	}

	function sync(): SyncResult {
		const config = loadConfig();
		const result: SyncResult = {
			agents: { total: 0, written: 0, removed: 0, dropped: {}, skipped: [] },
			commands: { total: 0, written: 0, removed: 0, skipped: [] },
			skills: { linked: [], missing: [], settingsChanged: false },
		};
		const owned = readManifest();
		const expected: Manifest = {};
		let broken = 0;
		const parseSource = (dir: string, file: string, kind: Kind, label: string): ParsedFrontmatter | null => {
			try { return parse(fs.readFileSync(path.join(dir, file), "utf8")); }
			catch (error) {
				broken++;
				result[kind].skipped.push(`${label}/${file} (frontmatter inválida: ${error instanceof Error ? error.message : String(error)})`);
				return null;
			}
		};
		if (config.enabled !== false) {
			const native = nativeNames(owned);
			const seen = new Set<string>();
			for (const source of discoverSources("agents", config.agents?.extraSources ?? [])) {
				if (config.disabledSources?.agents?.includes(source.label)) continue;
				for (const file of listMd(source.dir)) {
					const parsed = parseSource(source.dir, file, "agents", source.label);
					if (!parsed) continue;
					const converted = convertParsedAgent(parsed, config, harness);
					if (!converted) {
						result.agents.skipped.push(`${source.label}/${file} (definição incompatível ou excluída)`);
						continue;
					}
					if (native.has(converted.name)) { result.agents.skipped.push(converted.name); continue; }
					const relative = path.relative(agentDir, path.join(agentsOut, harness === "omp"
						? `claude-bridge-${converted.name}.md` : path.join(encodeURIComponent(source.label), `${converted.name}.md`)));
					if (seen.has(relative)) { result.agents.skipped.push(`${source.label}/${file} (nome ${converted.name} já usado por outra fonte)`); continue; }
					seen.add(relative);
					expected[relative] = { kind: "agents", content: converted.content };
					result.agents.total++;
					if (converted.droppedTools.length) result.agents.dropped[converted.name] = converted.droppedTools;
				}
			}
			if (harness === "pi") {
				const cacheCommands = config.cacheCommands ?? ["ecc"];
				for (const source of discoverSources("commands", config.commands?.extraSources ?? [])) {
					if (config.disabledSources?.commands?.includes(source.label)) continue;
					const dir = cacheCommands.includes(source.label) ? latestCacheSubdir(source.label, "commands") ?? source.dir : source.dir;
					for (const file of listMd(dir)) {
						const parsed = parseSource(dir, file, "commands", source.label);
						if (!parsed) continue;
						const converted = convertParsedCommand(parsed, file, config);
						if (!converted) continue;
						const relative = path.relative(agentDir, path.join(promptsOut, `${converted.name}.md`));
						if (expected[relative]) continue;
						expected[relative] = { kind: "commands", content: converted.content };
						result.commands.total++;
					}
				}
			}
		}
		const files = reconcile(expected, owned, result, broken === 0);
		atomicWrite(manifestPath, `${JSON.stringify({ version: 2, files }, null, 2)}\n`);
		if (harness === "pi" && config.enabled !== false) syncSkills(config, result);
		return result;
	}

	function memoryIndex(cwd: string): string | null {
		const dir = path.join(claudeDir, "projects", cwd.replace(/[^A-Za-z0-9]/g, "-"), "memory");
		const index = readOptional(path.join(dir, "MEMORY.md"))?.trim();
		return index ? `\n\n# Memórias do projeto (Claude, índice — corpo em ${dir}/)\n${index}` : null;
	}

	try { sync(); }
	catch (error) { console.error(`[claude-bridge] Falha na sincronização: ${String(error)}`); }

	let avisouConfig = false;
	pi.on("before_agent_start", async (event: { systemPrompt: string | string[] }, ctx: { cwd: string }) => {
		let config: Config;
		try { config = loadConfig(); avisouConfig = false; }
		catch (error) {
			// Uma vez por quebra, não a cada prompt: o arquivo só muda quando alguém mexe nele.
			if (!avisouConfig) console.error(`[claude-bridge] ${configPath} ilegível, memória do projeto desligada até consertar: ${String(error)}`);
			avisouConfig = true;
			return;
		}
		if (config.enabled === false) return;
		const addition = memoryIndex(ctx.cwd);
		if (!addition) return;
		const present = Array.isArray(event.systemPrompt)
			? event.systemPrompt.some(block => block.includes(addition))
			: event.systemPrompt.includes(addition);
		if (present) return;
		return { systemPrompt: Array.isArray(event.systemPrompt) ? [...event.systemPrompt, addition] : `${event.systemPrompt}${addition}` };
	});

	pi.registerCommand("claude-bridge-config", {
		description: harness === "omp" ? "Configura fontes de agents pessoais do Claude" : "Configura fontes da ponte Claude (agents, commands, skills)",
		handler: async (_args, ctx) => {
			const config = loadConfig();
			const disabled = { agents: [...(config.disabledSources?.agents ?? [])], commands: [...(config.disabledSources?.commands ?? [])], skills: [...(config.disabledSources?.skills ?? [])] };
			const items: { kind: Kind | "skills"; label: string; detail: string }[] = [];
			for (const kind of harness === "omp" ? ["agents"] as const : ["agents", "commands"] as const) {
				for (const source of discoverSources(kind, config[kind]?.extraSources ?? [])) {
					const dir = kind === "commands" && (config.cacheCommands ?? ["ecc"]).includes(source.label) ? latestCacheSubdir(source.label, "commands") ?? source.dir : source.dir;
					const count = listMd(dir).length;
					if (count) items.push({ kind, label: source.label, detail: `${count} ${kind}` });
				}
			}
			if (harness === "pi") {
				for (const label of [...(config.marketplaceSkills ?? []), ...(config.skillPlugins ?? ["superpowers", "ecc"])]) items.push({ kind: "skills", label, detail: "skills" });
			}
			while (true) {
				const options = [...items.map(item => `[${disabled[item.kind].includes(item.label) ? " " : "x"}] ${item.kind}: ${item.label} — ${item.detail}`), "salvar e resync", "cancelar"];
				const choice = await ctx.ui.select("claude-bridge — enter alterna, salvar aplica", options);
				if (choice === undefined || choice === "cancelar") return;
				if (choice === "salvar e resync") break;
				const item = items[options.indexOf(choice)];
				if (!item) continue;
				const list = disabled[item.kind], at = list.indexOf(item.label);
				if (at === -1) list.push(item.label); else list.splice(at, 1);
			}
			config.disabledSources = disabled;
			atomicWrite(configPath, `${JSON.stringify(config, null, 2)}\n`);
			const result = sync();
			ctx.ui.notify(`Salvo em ${configPath}\nAgents: ${result.agents.total}. Recursos são recarregados na próxima sessão.`, "info");
		},
	});
	pi.registerCommand("claude-bridge", {
		description: `Sincroniza a ponte Claude no ${harness} e mostra o estado`,
		handler: async (_args, ctx) => {
			const result = sync();
			const lines = [`Agents: ${result.agents.total} (alterados: ${result.agents.written}, removidos: ${result.agents.removed}) → ${agentsOut}`, `Configuração: ${configPath}`];
			if (harness === "pi") lines.push(`Commands: ${result.commands.total} → ${promptsOut}`, `Skills: ${result.skills.linked.join("; ") || "nenhum plugin"}`, `Sem cache: ${result.skills.missing.join(", ")}`);
			for (const kind of ["agents", "commands"] as const) if (result[kind].skipped.length) lines.push(`Conflitos preservados (${kind}): ${result[kind].skipped.join(", ")}`);
			for (const [name, tools] of Object.entries(result.agents.dropped)) lines.push(`${name}: sem equivalente ${harness} → ${tools.join(", ")}`);
			ctx.ui.notify(lines.join("\n"), "info");
		},
	});
	return { sync };
}

export default async function (pi: ExtensionAPI) {
	return createBridge(pi, getAgentContext());
}
