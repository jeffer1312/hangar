#!/usr/bin/env node
// `paraglide-js compile` que pula quando nada mudou: entrada (mensagens, settings do projeto,
// versão do Paraglide, argumentos) e saída iguais às do carimbo gravado na última compilação.
// Regravar os mesmos ~11 MB invalidava o cache de transformação do Vite e do vitest a cada rodada.
// Uso: node scripts/i18n-compile.mjs compile <os mesmos argumentos do paraglide-js compile>
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);
const opcao = (nome) => {
    const i = args.indexOf(nome);
    if (i < 0 || !args[i + 1]) throw new Error(`i18n-compile: falta ${nome}`);
    return path.resolve(args[i + 1]);
};
const projeto = opcao("--project");
const saida = opcao("--outdir");
const carimbo = path.join(saida, ".compile-stamp.json");

// O Paraglide que o npm script usaria: o node_modules mais próximo do cwd (o pacote não exporta
// o package.json, então a busca é a da resolução do Node, à mão).
let pacote = null;
for (let d = process.cwd(); !pacote; d = path.dirname(d)) {
    const p = path.join(d, "node_modules/@inlang/paraglide-js/package.json");
    if (fs.existsSync(p)) pacote = p;
    else if (d === path.dirname(d)) throw new Error("i18n-compile: @inlang/paraglide-js não instalado");
}
const { version, bin } = JSON.parse(fs.readFileSync(pacote, "utf8"));

function entrada() {
    const h = createHash("sha256").update(version).update("\0").update(JSON.stringify(args)).update("\0");
    const settings = fs.readFileSync(path.join(projeto, "settings.json"));
    h.update(settings);
    // As mensagens moram onde o pathPattern do settings aponta (./messages/{locale}.json).
    const padrao = JSON.parse(settings)["plugin.inlang.messageFormat"].pathPattern;
    const pasta = path.resolve(path.dirname(projeto), path.dirname(padrao));
    for (const n of fs.readdirSync(pasta).sort()) h.update(n).update("\0").update(fs.readFileSync(path.join(pasta, n)));
    return h.digest("hex");
}

function arvore() {
    if (!fs.existsSync(saida)) return null;
    const h = createHash("sha256");
    const andar = (d) => {
        for (const n of fs.readdirSync(d).sort()) {
            const p = path.join(d, n);
            if (fs.statSync(p).isDirectory()) andar(p);
            else if (p !== carimbo) h.update(path.relative(saida, p)).update("\0").update(fs.readFileSync(p));
        }
    };
    andar(saida);
    return h.digest("hex");
}

const atual = entrada();
let gravado = null;
try { gravado = JSON.parse(fs.readFileSync(carimbo, "utf8")); } catch {}
if (gravado?.entrada === atual && gravado.saida === arvore()) {
    console.log(`i18n-compile: ${path.relative(process.cwd(), saida) || "."} sem mudança, compilação pulada`);
    process.exit(0);
}
fs.rmSync(carimbo, { force: true });
execFileSync(process.execPath, [path.resolve(path.dirname(pacote), bin["paraglide-js"]), ...args], { stdio: "inherit" });
fs.writeFileSync(carimbo, JSON.stringify({ entrada: atual, saida: arvore() }) + "\n");
