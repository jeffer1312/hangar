import type { AgentContext } from "./agent-context";

export interface ParsedFrontmatter {
	frontmatter: Record<string, unknown>;
	body: string;
}

export type FrontmatterParser = (content: string) => ParsedFrontmatter;

export async function loadFrontmatterParser(context: AgentContext): Promise<FrontmatterParser> {
	if (context.harness === "pi") {
		// O pacote legado só existe no Pi; carregá-lo no OMP quebra a extensão.
		const { parseFrontmatter } = await import("@earendil-works/pi-coding-agent");
		return parseFrontmatter;
	}
	return (content) => {
		const normalized = content.replace(/^\ufeff/, "").replace(/\r\n/g, "\n");
		if (!normalized.startsWith("---\n")) return { frontmatter: {}, body: normalized };
		const match = /^---\n([\s\S]*?)\n---(?:\n|$)/.exec(normalized);
		if (!match) throw new Error("Cabeçalho YAML sem fechamento");
		const frontmatter: unknown = Bun.YAML.parse(match[1]);
		if (!frontmatter || typeof frontmatter !== "object" || Array.isArray(frontmatter)) {
			throw new Error("Cabeçalho YAML precisa ser um objeto");
		}
		return { frontmatter: frontmatter as Record<string, unknown>, body: normalized.slice(match[0].length) };
	};
}
