import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join } from "node:path";

// The same source is linked into Pi and Oh My Pi. Use the process identity to
// keep each agent's opt-in state in its own config directory.
const IS_OMP = /(^|[\\/])omp(\.exe)?$/.test(process.execPath);
const AGENT_DIR =
	process.env.PI_CODING_AGENT_DIR || join(homedir(), IS_OMP ? ".omp/agent" : ".pi/agent");
const CONFIG_PATH = join(AGENT_DIR, "fullscreen-tui.json");
const SETTINGS_PATH = join(AGENT_DIR, "settings.json");
const EM_SUBAGENTE = Number(process.env.PI_SUBAGENT_DEPTH ?? "0") > 0;
const STEM_RE = /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_[0-9a-fA-F-]{36}$/;
type FullscreenState = {
	active: boolean;
	exitHandler?: () => void;
};

function isFullscreenState(value: unknown): value is FullscreenState {
	return value !== null && typeof value === "object" && "active" in value && typeof value.active === "boolean";
}

const STATE_KEY = Symbol.for("hangar.fullscreen-tui.state");
const persistedState = Reflect.get(globalThis, STATE_KEY);
const STATE: FullscreenState = isFullscreenState(persistedState) ? persistedState : { active: false };
Reflect.set(globalThis, STATE_KEY, STATE);

function ehSubagente(ctx: ExtensionContext): boolean {
	if (EM_SUBAGENTE) return true;
	const file = ctx.sessionManager.getSessionFile?.() ?? "";
	return !!file && STEM_RE.test(basename(dirname(file)));
}

function loadEnabled(): boolean {
	try {
		if (!existsSync(CONFIG_PATH)) return false;
		return JSON.parse(readFileSync(CONFIG_PATH, "utf8"))?.enabled === true;
	} catch {
		return false;
	}
}

function saveEnabled(enabled: boolean): boolean {
	try {
		mkdirSync(dirname(CONFIG_PATH), { recursive: true });
		writeFileSync(CONFIG_PATH, JSON.stringify({ enabled }, null, "\t") + "\n");
		return true;
	} catch {
		return false;
	}
}

function nativeFullscreenEnabled(cwd = process.cwd(), includeCliOverride = false): boolean {
	if (IS_OMP) return false;
	if (includeCliOverride) {
		const modeIndex = process.argv.indexOf("--tui-mode");
		if (modeIndex >= 0) return process.argv[modeIndex + 1] === "fullscreen";
	}
	for (const settingsPath of [join(cwd, ".pi", "settings.json"), SETTINGS_PATH]) {
		try {
			const settings = JSON.parse(readFileSync(settingsPath, "utf8"));
			if (settings && typeof settings === "object" && "tuiMode" in settings) {
				return settings.tuiMode === "fullscreen";
			}
		} catch {
			// Try the next settings scope.
		}
	}
	return false;
}

function enterAltScreen(cwd: string, force = false): void {
	if (nativeFullscreenEnabled(cwd) || !process.stdout.isTTY) return;
	if (STATE.active && !force) return;
	// Alternate screen buffer keeps the live TUI and composer on-screen while
	// transcript scrolling stays inside the terminal viewport.
	process.stdout.write("\x1b[?1049h\x1b[H\x1b[2J");
	STATE.active = true;
}

// O omp pode sair do alternate screen sozinho depois da largada (reset da TUI): o pane volta pro
// buffer normal, mas STATE.active continua verdadeiro e o enterAltScreen vira no-op — foi assim que
// `/fullscreen-on` respondeu "enabled" com a tela ainda rolando. Reafirmar só o `?1049h`, sem
// limpar: dentro do tmux, entrar no alternate screen já estando nele é no-op (não repinta nada),
// então isto pode rodar a cada início de turno sem custo visível.
function reassertAltScreen(cwd: string): void {
	if (!STATE.active || nativeFullscreenEnabled(cwd) || !process.stdout.isTTY) return;
	process.stdout.write("\x1b[?1049h");
}

function leaveAltScreen(): void {
	if (!STATE.active) return;
	STATE.active = false;
	if (!process.stdout.isTTY) return;
	process.stdout.write("\x1b[?1049l");
}

function registerExitCleanup(): void {
	if (typeof STATE.exitHandler === "function") {
		process.removeListener("exit", STATE.exitHandler);
	}
	const exitHandler = () => leaveAltScreen();
	STATE.exitHandler = exitHandler;
	process.once("exit", exitHandler);
}

export default function (pi: ExtensionAPI) {
	if (EM_SUBAGENTE) return;
	registerExitCleanup();

	pi.on("session_start", async (_event, ctx) => {
		if (ehSubagente(ctx) || nativeFullscreenEnabled(ctx.cwd, true)) return;
		if (ctx.mode === "tui" && loadEnabled()) {
			enterAltScreen(ctx.cwd);
			// A TUI do omp termina de subir depois deste evento; reafirma quando ela já está de pé.
			const t = setTimeout(() => reassertAltScreen(ctx.cwd), 1500);
			t.unref?.();
		}
	});

	pi.on("agent_start", async (_event, ctx) => {
		if (!ehSubagente(ctx) && ctx.mode === "tui") reassertAltScreen(ctx.cwd);
	});

	pi.on("session_shutdown", async (_event, ctx) => {
		if (!ehSubagente(ctx)) leaveAltScreen();
	});

	pi.registerCommand("fullscreen-on", {
		description: "Use terminal alternate screen so the composer stays on-screen while scrolling",
		handler: async (_args, ctx) => {
			if (ehSubagente(ctx) || ctx.mode !== "tui") return;
			const native = nativeFullscreenEnabled(ctx.cwd);
			const saved = saveEnabled(true);
			enterAltScreen(ctx.cwd, true);
			ctx.ui.notify(
				saved
					? native
						? "Pi native fullscreen is active; fullscreen-tui preference enabled"
						: `Fullscreen TUI enabled. Config: ${CONFIG_PATH}`
					: "Fullscreen TUI enabled for this session; config could not be saved",
				saved ? "info" : "warning",
			);
		},
	});

	pi.registerCommand("fullscreen-off", {
		description: "Leave terminal alternate screen and restore normal scrollback mode",
		handler: async (_args, ctx) => {
			if (ehSubagente(ctx) || ctx.mode !== "tui") return;
			leaveAltScreen();
			const native = nativeFullscreenEnabled(ctx.cwd);
			const saved = saveEnabled(false);
			ctx.ui.notify(
				saved
					? native
						? "Fullscreen-tui preference disabled; Pi native fullscreen remains active"
						: `Fullscreen TUI disabled. Config: ${CONFIG_PATH}`
					: "Fullscreen TUI disabled for this session; config could not be saved",
				saved ? "info" : "warning",
			);
		},
	});
}
