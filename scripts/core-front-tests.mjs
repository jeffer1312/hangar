#!/usr/bin/env node
// Arquivos do front que usam o que mudou no core, para o `vitest related` do front. O
// `vitest related` sobre o arquivo do core puxa quase todo teste do front, porque o index.ts
// reexporta tudo; aqui a escolha é por nome: as declarações de topo que o diff tocou (sem --base,
// todas as do arquivo mudado), as que as usam no mesmo arquivo e nos outros do core, até fechar;
// o que disso o index publica, contra os nomes que cada arquivo do front importa.
// Uso (da raiz): node scripts/core-front-tests.mjs [--base <ref>] packages/core/src/<mudado>.ts...
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const raiz = process.cwd();
const core = path.join(raiz, "packages/core/src");
const indice = path.join(core, "index.ts");
const args = process.argv.slice(2);
let base = null;
if (args[0] === "--base") base = args.splice(0, 2)[1];

const ehTeste = (p) => /\.test\.(ts|tsx)$|\.test\.svelte\.ts$/.test(p);
const andar = (dir, ok) => fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "node_modules" || e.name === "paraglide" ? [] : andar(p, ok);
    return ok(e.name) ? [p] : [];
});
const ler = (p) => fs.readFileSync(p, "utf8");
const usa = (txt, nome) => new RegExp(`(?<![\\w$])${nome.replace(/\$/g, "\\$")}(?![\\w$])`).test(txt);

function resolver(de, spec) {
    if (!spec.startsWith(".")) return null;
    const b = path.resolve(path.dirname(de), spec.replace(/\.js$/, ""));
    for (const c of [b, `${b}.ts`, `${b}.tsx`, `${b}.svelte.ts`, path.join(b, "index.ts")]) {
        if (fs.existsSync(c) && fs.statSync(c).isFile()) return c;
    }
    return null;
}

const lista = (txt) => txt.split(",").map((s) => s.replace(/\btype\s+/, "").trim()).filter(Boolean)
    .map((s) => { const [a, b] = s.split(/\s+as\s+/); return [a.trim(), (b ?? a).trim()]; });

// Declarações de topo (começam na coluna 0) com o trecho de cada uma, os nomes que declaram, o
// que importam e o que exportam.
const INICIO = /^(?:import\b|export\b|(?:declare\s+)?(?:async\s+)?(?:abstract\s+)?(?:function\*?|const|let|var|class|interface|type|enum|namespace)\s)/;
const DECL = /^(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:async\s+)?(?:abstract\s+)?(?:function\*?|const|let|var|class|interface|type|enum|namespace)\s+([A-Za-z_$][\w$]*)/;
function analisar(arq) {
    const linhas = ler(arq).split("\n");
    const decls = [];
    linhas.forEach((l, i) => { if (INICIO.test(l)) decls.push({ inicio: i + 1 }); });
    decls.forEach((d, i) => {
        d.fim = i + 1 < decls.length ? decls[i + 1].inicio - 1 : linhas.length;
        d.texto = linhas.slice(d.inicio - 1, d.fim).join("\n");
        d.nomes = [];
        d.imports = [];   // [local, arquivo, nome importado ("*" = namespace)]
        d.exporta = [];   // [exportado, local] ou [exportado, arquivo, nome] ou ["*", arquivo]
        const m = DECL.exec(d.texto);
        if (m) {
            d.nomes.push(m[1]);
            if (/^export\s/.test(d.texto)) d.exporta.push([m[1], m[1]]);
            return;
        }
        let r;
        if ((r = /^import\s+(?:type\s+)?([\s\S]*?)\s*from\s*['"]([^'"]+)['"]/.exec(d.texto))) {
            const alvo = resolver(arq, r[2]);
            if (!alvo) return;
            const [, cl] = r;
            const ns = /\*\s+as\s+([A-Za-z_$][\w$]*)/.exec(cl);
            if (ns) d.imports.push([ns[1], alvo, "*"]);
            const ch = /\{([^}]*)\}/.exec(cl);
            if (ch) for (const [a, b] of lista(ch[1])) d.imports.push([b, alvo, a]);
            const def = /^([A-Za-z_$][\w$]*)/.exec(cl);
            if (def) d.imports.push([def[1], alvo, "default"]);
            d.nomes.push(...d.imports.map(([l]) => l));
        } else if ((r = /^export\s+\*\s+(?:as\s+([A-Za-z_$][\w$]*)\s+)?from\s*['"]([^'"]+)['"]/.exec(d.texto))) {
            const alvo = resolver(arq, r[2]);
            if (alvo) d.exporta.push(r[1] ? [r[1], alvo, "*"] : ["*", alvo]);
        } else if ((r = /^export\s+(?:type\s+)?\{([^}]*)\}\s*(?:from\s*['"]([^'"]+)['"])?/.exec(d.texto))) {
            const alvo = r[2] ? resolver(arq, r[2]) : null;
            if (r[2] && !alvo) return;
            for (const [a, b] of lista(r[1])) d.exporta.push(alvo ? [b, alvo, a] : [b, a]);
        }
    });
    return decls;
}

// Linhas do arquivo atual que o diff desde a base tocou; null = arquivo novo (tudo mudou).
function linhasMudadas(arq) {
    const rel = path.relative(raiz, arq);
    try { execFileSync("git", ["cat-file", "-e", `${base}:${rel}`], { stdio: "ignore" }); } catch { return null; }
    const diff = execFileSync("git", ["diff", "-U0", base, "--", rel]).toString();
    const set = new Set();
    for (const m of diff.matchAll(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@/gm)) {
        const ini = Number(m[1]), n = m[2] === undefined ? 1 : Number(m[2]);
        // Remoção pura (n = 0) fica entre a linha ini e a seguinte.
        if (n === 0) { set.add(ini); set.add(ini + 1); } else for (let i = ini; i < ini + n; i++) set.add(i);
    }
    return set;
}

const arquivosCore = andar(core, (n) => /\.(ts|tsx)$/.test(n) && !ehTeste(n));
const info = new Map(arquivosCore.map((f) => [f, analisar(f)]));
const mudados = args.map((p) => path.resolve(raiz, p)).filter((p) => p.startsWith(core + path.sep) && !ehTeste(p));

// Nomes do front: arquivo -> nomes do core que importa; null = não dá para saber quais.
const usos = new Map();
for (const f of andar(path.join(raiz, "frontend/src"), (n) => /\.(ts|tsx|svelte)$/.test(n))) {
    const txt = ler(f);
    if (!txt.includes("@hangar/core")) continue;
    const nomes = new Set();
    // Import dinâmico e `export *` do core não dizem o nome: o arquivo entra sempre.
    const dinamico = /(?<!typeof\s*)import\(\s*['"]@hangar\/core['"]\s*\)|export\s+\*\s+from\s*['"]@hangar\/core['"]/.test(txt);
    // Reexportar conta como usar: quem importa do arquivo do front recebe o nome do core.
    for (const m of txt.matchAll(/(?:import|export)\s+(?:type\s+)?\{([^}]*)\}\s*from\s*['"]@hangar\/core['"]/g)) lista(m[1]).forEach(([a]) => nomes.add(a));
    for (const m of txt.matchAll(/import\s+(?:type\s+)?\*\s+as\s+([A-Za-z_$][\w$]*)\s+from\s*['"]@hangar\/core['"]/g)) {
        for (const u of txt.matchAll(new RegExp(`(?<![\\w$])${m[1].replace(/\$/g, "\\$")}\\.([A-Za-z_$][\\w$]*)`, "g"))) nomes.add(u[1]);
    }
    usos.set(f, dinamico ? null : nomes);
}

let saida;
if (!mudados.length) saida = [];
else if (mudados.some((p) => !info.has(p))) saida = [...usos.keys()];   // apagado: não se sabe o que publicava
else {
    // Por arquivo: nomes locais mudados e nomes exportados mudados.
    const locais = new Map(arquivosCore.map((f) => [f, new Set()]));
    const exportados = new Map(arquivosCore.map((f) => [f, new Set()]));
    const todosDe = (f) => info.get(f).flatMap((d) => d.nomes.concat(d.exporta.filter((e) => e.length === 2 && e[0] !== "*").map((e) => e[1])));
    const exportaTudo = new Set();   // declaração de export tocada pelo diff publica tudo o que lista
    for (const f of mudados) {
        const linhas = base ? linhasMudadas(f) : null;
        for (const d of info.get(f)) {
            let tocada = linhas === null;
            for (let i = d.inicio; !tocada && i <= d.fim; i++) tocada = linhas.has(i);
            if (!tocada) continue;
            d.nomes.forEach((n) => locais.get(f).add(n));
            if (d.exporta.length) exportaTudo.add(d);
        }
        if (linhas === null) todosDe(f).forEach((n) => locais.get(f).add(n));
    }
    for (let cresceu = true; cresceu;) {
        cresceu = false;
        const add = (set, n) => { if (!set.has(n)) { set.add(n); cresceu = true; } };
        for (const [f, decls] of info) {
            const loc = locais.get(f), exp = exportados.get(f);
            for (const d of decls) {
                for (const [local, alvo, nome] of d.imports) {
                    if (nome === "*" ? exportados.get(alvo)?.size : exportados.get(alvo)?.has(nome)) add(loc, local);
                }
                if (!d.nomes.some((n) => loc.has(n)) && d.nomes.length && [...loc].some((n) => usa(d.texto, n))) d.nomes.forEach((n) => add(loc, n));
                for (const e of d.exporta) {
                    const tudo = exportaTudo.has(d);
                    if (e[0] === "*") (tudo ? exportadosDe(e[1]) : exportados.get(e[1]) ?? []).forEach((n) => add(exp, n));
                    else if (e.length === 2) { if (tudo || loc.has(e[1])) add(exp, e[0]); }
                    else if (tudo || (e[2] === "*" ? exportados.get(e[1])?.size : exportados.get(e[1])?.has(e[2]))) add(exp, e[0]);
                }
            }
        }
    }
    function exportadosDe(f, visto = new Set()) {
        if (visto.has(f) || !info.has(f)) return [];
        visto.add(f);
        return info.get(f).flatMap((d) => d.exporta.flatMap((e) => (e[0] === "*" ? exportadosDe(e[1], visto) : [e[0]])));
    }
    const publicos = exportados.get(indice) ?? new Set();
    saida = [...usos].filter(([, n]) => n === null || [...n].some((x) => publicos.has(x))).map(([f]) => f);
}
if (saida.length) console.log(saida.sort().join("\n"));
