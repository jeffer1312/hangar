#!/usr/bin/env node
// Confere o scripts/core-front-tests.mjs num repositório falso: o arquivo do front entra quando
// importa um nome que o arquivo mudado do core publica, direto ou por quem o importa dentro do core.
// Uso: node scripts/test-core-front-tests.mjs
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "core-front-tests.mjs");
const raiz = fs.mkdtempSync(path.join(os.tmpdir(), "core-front-"));
const escreve = (rel, txt) => { fs.mkdirSync(path.dirname(path.join(raiz, rel)), { recursive: true }); fs.writeFileSync(path.join(raiz, rel), txt); };

escreve("packages/core/src/index.ts", [
    "export * from './a';",
    "export * from './b';",
    "export { fc as fcPublico, outra } from './c';",
    "export type { Tipo } from './d';",
].join("\n"));
escreve("packages/core/src/a.ts", "export function fa() { return 1 }\nexport const ka = 2;\n");
escreve("packages/core/src/b.ts", "import { fa } from './a';\nexport async function fb() { return fa() }\n");
escreve("packages/core/src/c.ts", "export const fc = 3;\nconst x = 1;\nexport { x as outra };\n");
escreve("packages/core/src/d.ts", "export interface Tipo { a: number }\n");
escreve("packages/core/src/a.test.ts", "import { fa } from './a';\n");
escreve("frontend/src/x.ts", "import {\n  ka,\n  type Tipo,\n} from '@hangar/core';\n");
escreve("frontend/src/y.svelte", "<script>\nimport { fcPublico } from '@hangar/core';\n</script>\n");
escreve("frontend/src/z.test.ts", "import * as core from '@hangar/core';\ncore.fb();\n");
escreve("frontend/src/w.test.ts", "import { outra } from \"@hangar/core\";\n");
escreve("frontend/src/v.test.ts", "vi.mock('@hangar/core', async (o) => ({ ...(await o()) }));\nimport { Tipo } from '@hangar/core';\n");
escreve("frontend/src/u.ts", "const m = await import('@hangar/core');\n");

let falhou = false;
const caso = (mudados, esperado) => {
    const saida = execFileSync("node", [script, ...mudados], { cwd: raiz }).toString().trim().split("\n").filter(Boolean)
        .map((p) => path.relative(raiz, p)).sort().join(" ");
    if (saida !== esperado.split(" ").filter(Boolean).sort().join(" ")) {
        console.log(`FALHOU ${mudados.join(" ")}: "${saida}" (esperado "${esperado}")`);
        falhou = true;
    }
};
// a.ts: ka direto, fb porque b.ts importa a.ts; import dinâmico do core não diz o nome: entra sempre.
caso(["packages/core/src/a.ts"], "frontend/src/x.ts frontend/src/z.test.ts frontend/src/u.ts");
caso(["packages/core/src/c.ts"], "frontend/src/y.svelte frontend/src/w.test.ts frontend/src/u.ts");
caso(["packages/core/src/d.ts"], "frontend/src/x.ts frontend/src/v.test.ts frontend/src/u.ts");
// Teste do core não muda o front; índice mudado vale para todos os que importam o core.
caso(["packages/core/src/a.test.ts"], "");
caso(["packages/core/src/index.ts"],
    "frontend/src/u.ts frontend/src/v.test.ts frontend/src/w.test.ts frontend/src/x.ts frontend/src/y.svelte frontend/src/z.test.ts");
// Arquivo do core apagado: não dá para saber o que ele publicava, vale para todos.
caso(["packages/core/src/sumiu.ts"],
    "frontend/src/u.ts frontend/src/v.test.ts frontend/src/w.test.ts frontend/src/x.ts frontend/src/y.svelte frontend/src/z.test.ts");

// Com --base: só as declarações que o diff tocou, e quem as usa.
const git = (...a) => execFileSync("git", a, { cwd: raiz, env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" } });
escreve("packages/core/src/a.ts", "function interno() { return 0 }\n\nexport function fa() { return interno() }\n\nexport const ka = 2;\n");
fs.rmSync(path.join(raiz, "packages/core/src/c.ts"));
git("init", "-q");
git("add", "-A");
git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "base");
const base = git("rev-parse", "HEAD").toString().trim();
const comBase = (mudados, esperado) => caso(["--base", base, ...mudados], esperado);
const muda = (de, para) => escreve("packages/core/src/a.ts", ler("packages/core/src/a.ts").replace(de, para));
const ler = (rel) => fs.readFileSync(path.join(raiz, rel), "utf8");

muda("ka = 2", "ka = 5");
comBase(["packages/core/src/a.ts"], "frontend/src/x.ts frontend/src/u.ts");
muda("ka = 5", "ka = 2");
muda("return interno()", "return interno() + 1");
comBase(["packages/core/src/a.ts"], "frontend/src/z.test.ts frontend/src/u.ts");
muda("return interno() + 1", "return interno()");
muda("return 0", "return 9");
comBase(["packages/core/src/a.ts"], "frontend/src/z.test.ts frontend/src/u.ts");
// Arquivo novo no core (fora da base): tudo o que ele publica conta como mudado.
escreve("packages/core/src/c.ts", "export const fc = 3;\nconst x = 1;\nexport { x as outra };\n");
comBase(["packages/core/src/a.ts", "packages/core/src/c.ts"],
    "frontend/src/z.test.ts frontend/src/u.ts frontend/src/y.svelte frontend/src/w.test.ts");

fs.rmSync(raiz, { recursive: true, force: true });
if (falhou) process.exit(1);
console.log("ok: core-front-tests");
