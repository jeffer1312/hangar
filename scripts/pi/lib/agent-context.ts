import { homedir } from "node:os";
import { join, resolve } from "node:path";

export interface AgentContext {
	harness: "pi" | "omp";
	agentDir: string;
	claudeDir: string;
}

export function getAgentContext(): AgentContext {
	const harness = /(^|[\\/])omp(\.exe)?$/.test(process.execPath) ? "omp" : "pi";
	const home = homedir();
	const normalize = (value: string) => resolve(value.replace(/^~(?=$|[\\/])/, home));
	return {
		harness,
		agentDir: normalize(process.env.PI_CODING_AGENT_DIR || join(home, harness === "omp" ? ".omp/agent" : ".pi/agent")),
		claudeDir: normalize(process.env.CLAUDE_CONFIG_DIR || join(home, ".claude")),
	};
}
