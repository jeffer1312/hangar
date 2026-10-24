#!/usr/bin/env node
// Confere o scripts/i18n-compile.mjs com o Paraglide de verdade, numa cópia do projeto: a saída é
// a mesma da compilação direta, a segunda chamada não regrava nada, e mudar uma mensagem, a saída
// ou os argumentos compila de novo.
// Uso (da raiz, com node_modules instalado): node scripts/test-i18n-compile.mjs
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const raiz = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const wrapper = path.join(raiz, "scripts/i18n-compile.mjs");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "i18n-compile-"));
let falhou = false;
const falha = (msg) => { console.log(`FALHOU ${msg}`); falhou = true; };

fs.cpSync(path.join(raiz, "project.inlang"), path.join(tmp, "project.inlang"), { recursive: true });
fs.cpSync(path.join(raiz, "messages"), path.join(tmp, "messages"), { recursive: true });
const args = (out, ...extra) => ["--project", path.join(tmp, "project.inlang"), "--outdir", out,
    "--strategy", "baseLocale", "--output-structure", "locale-modules", ...extra];
const roda = (out, ...extra) => execFileSync("node", [wrapper, "compile", ...args(out, ...extra)], { cwd: raiz, stdio: "pipe" });
const direto = (out) => execFileSync(path.join(raiz, "node_modules/.bin/paraglide-js"), ["compile", ...args(out)], { cwd: raiz, stdio: "pipe" });

// Conteúdo e caminho de cada arquivo, menos o carimbo do wrapper.
function arvore(dir) {
    const h = createHash("sha256");
    const andar = (d) => {
        for (const n of fs.readdirSync(d).sort()) {
            const p = path.join(d, n);
            if (fs.statSync(p).isDirectory()) andar(p);
            else if (n !== ".compile-stamp.json") h.update(path.relative(dir, p)).update("\0").update(fs.readFileSync(p));
        }
    };
    andar(dir);
    return h.digest("hex");
}
const mtimes = (dir) => fs.readdirSync(path.join(dir, "messages")).sort()
    .map((n) => fs.statSync(path.join(dir, "messages", n)).mtimeMs).join(",");

try {
    const outW = path.join(tmp, "out-wrapper"), outD = path.join(tmp, "out-direto");
    roda(outW);
    direto(outD);
    if (arvore(outW) !== arvore(outD)) falha("saída do wrapper difere da compilação direta");

    const antes = mtimes(outW);
    const saida = roda(outW).toString();
    if (mtimes(outW) !== antes) falha("segunda chamada sem mudança regravou a saída");
    if (!saida.includes("sem mudança")) falha(`segunda chamada não avisou que pulou: "${saida.trim()}"`);

    const en = path.join(tmp, "messages/en.json");
    const msgs = JSON.parse(fs.readFileSync(en, "utf8"));
    msgs.zz_teste_carimbo = "carimbo";
    fs.writeFileSync(en, JSON.stringify(msgs, null, 2));
    roda(outW);
    if (!fs.readFileSync(path.join(outW, "messages/en.js"), "utf8").includes("carimbo")) falha("mensagem nova não recompilou");

    fs.appendFileSync(path.join(outW, "messages/pt.js"), "\n// mexido\n");
    roda(outW);
    if (fs.readFileSync(path.join(outW, "messages/pt.js"), "utf8").includes("// mexido")) falha("saída alterada à mão não recompilou");

    fs.rmSync(path.join(outW, "messages/_index.js"));
    roda(outW);
    if (!fs.existsSync(path.join(outW, "messages/_index.js"))) falha("saída apagada não recompilou");

    const comOutro = path.join(tmp, "out-args");
    roda(comOutro);
    roda(comOutro, "--emit-ts-declarations");
    if (!fs.readdirSync(path.join(comOutro, "messages")).some((n) => n.endsWith(".d.ts"))) falha("argumento novo não recompilou");
} finally {
    fs.rmSync(tmp, { recursive: true, force: true });
}
if (falhou) process.exit(1);
console.log("ok: i18n-compile");
