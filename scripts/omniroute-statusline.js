#!/usr/bin/env node
// OmniRoute Status Line para Claude Code
// Mostra billing/rate limits das contas Claude via OmniRoute

const path = require('path');
const os = require('os');
const fs = require('fs');

// ─── Quota das sessões de motor ──────────────────────────────────────────────
// O Claude Code só preenche `rate_limits` falando com a Anthropic; num motor ele vem vazio e os
// chips ⚡5h/📅7d somem. Cada provedor expõe a própria quota, mas por HTTP — e a statusline roda a
// cada render, então NUNCA pode esperar rede aqui. Contrato: o render lê um cache e desenha; se o
// cache estiver velho, dispara este mesmo script destacado (`--refresh-usage`) e segue com o que tem.
const USAGE_TTL_S = 60;
const usageCache = eng => path.join(os.tmpdir(), `cp-engine-usage-${eng.replace(/[^\w.-]/g, '_')}.json`);

// {limit,used,resetTime} do provedor -> {pct, reset} que os chips já sabem desenhar.
const janela = (lim, used, resetTime) => {
  const l = Number(lim), u = Number(used), r = Date.parse(resetTime);
  if (!(l > 0) || !Number.isFinite(u)) return null;
  return { pct: (u / l) * 100, reset: Number.isFinite(r) ? Math.floor(r / 1000) : null };
};

async function refreshUsage(name) {
  const eng = JSON.parse(fs.readFileSync(path.join(os.homedir(), '.claude', 'engines.json'), 'utf8'))[name];
  if (!eng?.base_url || !eng?.api_key) return;
  const base = eng.base_url.replace(/\/+$/, '');
  const h = { Authorization: 'Bearer ' + eng.api_key, Accept: 'application/json' };
  let erro = null;
  // Timeout obrigatório: o refresher roda destacado com stdio ignorado, então um provedor que não
  // responde deixaria um node órfão pendurado pra sempre — um novo a cada ciclo de TTL.
  const get = async p => {
    const r = await fetch(base + p, { headers: h, signal: AbortSignal.timeout(8000) });
    return r.ok ? r.json() : null;
  };
  let janelas = null;

  // Kimi Code: GET /v1/usages -> `usage` (semanal) + `limits[]` (a de duration 300min = 5h).
  try {
    const j = await get('/v1/usages');
    if (j?.usage) janelas = [
      ...(j.limits || []).map(w => janela(w.detail?.limit, w.detail?.used, w.detail?.resetTime)),
      janela(j.usage.limit, j.usage.used, j.usage.resetTime),
    ];
  } catch (e) { erro = [erro, String(e)].filter(Boolean).join(' | '); }

  // OmniRoute: a quota é a do provedor UPSTREAM, então precisa da connection que atende esta key.
  // Descoberta sem id chumbado: `keyPrefix` casa a nossa chave em /api/keys, e o call-log mais
  // recente dessa key aponta a connection viva.
  // Sem casar a NOSSA key não dá pra seguir: pegar o call-log de qualquer key mostraria a quota de
  // outra conta como se fosse desta sessão — mentira pior do que chip nenhum.
  if (!janelas) try {
    const keys = (await get('/api/keys'))?.keys || [];
    const eu = keys.find(k => k.keyPrefix && eng.api_key.startsWith(k.keyPrefix));
    const logs = eu ? (await get('/api/usage/call-logs?limit=50')) || [] : [];
    // Ordem do array e detalhe de implementacao da API: ordena pelo timestamp do proprio log, senao
    // uma mudanca de paginacao la resolveria uma connection velha sem erro nenhum aqui.
    const log = logs
      .filter(l => l.connectionId && l.apiKeyId === eu.id)
      .sort((a, b) => Date.parse(b.timestamp || 0) - Date.parse(a.timestamp || 0))[0];
    if (log) {
      const q = (await get('/api/usage/' + log.connectionId))?.quotas || {};
      janelas = Object.values(q).filter(x => x && !x.unlimited).map(x => janela(x.total, x.used, x.resetAt));
    }
  } catch (e) { erro = [erro, String(e)].filter(Boolean).join(' | '); }

  // Grava sempre: a falha fica NO cache (campo `erro`), senão não dá pra distinguir no disco
  // "dentro do TTL" de "quebrado faz uma semana". Falha transitória não derruba a quota boa.
  janelas = (janelas || []).filter(Boolean);
  let anterior = null;
  try { anterior = JSON.parse(fs.readFileSync(usageCache(name), 'utf8')); } catch {}
  fs.writeFileSync(usageCache(name), JSON.stringify({
    ts: Math.floor(Date.now() / 1000),
    janelas: janelas.length ? janelas : (Array.isArray(anterior?.janelas) ? anterior.janelas : []),
    ...(erro && !janelas.length ? { erro } : {}),
  }));
}

if (process.argv[2] === '--refresh-usage' && process.argv[3]) {
  refreshUsage(process.argv[3]).catch(() => {}).finally(() => process.exit(0));
  return;
}

const stdinTimeout = setTimeout(() => process.exit(0), 3000);
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => input += chunk);
process.stdin.on('end', () => {
  clearTimeout(stdinTimeout);
  try {
    const data = JSON.parse(input);

    // Sessão rodando em motor de terceiro (engines.json, via cp-engine --exec). O que o Claude Code
    // calcula com PREÇO da Anthropic deixa de valer aqui — e no Kimi Code, que é assinatura de valor
    // fixo, qualquer valor em dólar é ficção. Número errado é pior que número nenhum.
    const engine = process.env.CP_ENGINE || '';

    // Expoe o custo REAL da sessao (Claude Code passa cost.total_cost_usd aqui) num arquivo temp que
    // o cost-tracker do ecc le como fonte autoritativa -> ele mostra ~$30 real em vez da estimativa
    // por soma de tokens (que infla em sessao longa). Contrato: {ts:<unix s>, cost_usd} valido <=300s.
    if (!engine) try {
      const sid = data.session_id;
      const costUsd = data.cost?.total_cost_usd;
      if (sid && typeof costUsd === 'number') {
        fs.writeFileSync(
          path.join(os.tmpdir(), `harness-cost-${sid}.json`),
          JSON.stringify({ ts: Math.floor(Date.now() / 1000), cost_usd: costUsd })
        );
      }
    } catch {}

    // Modelo compacto: "Opus 4.8 (1M context)" -> "Opus4.8·1M"
    const model = (data.model?.display_name || 'Claude')
      .replace(/\s*\(1M context\)/i, '·1M')
      .replace(/Opus (\d)/, 'Opus$1');
    // Workspace atual (pasta sendo trabalhada)
    const dir = path.basename(data.workspace?.current_dir || process.cwd());
    // Diretório de config do Claude (CLAUDE_CONFIG_DIR ou ~/.claude)
    const claudeDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
    const claudeDirShort = claudeDir.replace(os.homedir(), '~').replace(/^~\//, '');
    // Email da conta Claude (oauthAccount.emailAddress em ~/.claude.json)
    let account = '';
    try {
      // Conta do perfil ativo: usa CLAUDE_CONFIG_DIR se setado (cc=.claude-clean,
      // claude=.claude-work), senão o ~/.claude.json da raiz (perfil default).
      const cfgPath = process.env.CLAUDE_CONFIG_DIR
        ? path.join(process.env.CLAUDE_CONFIG_DIR, '.claude.json')
        : path.join(os.homedir(), '.claude.json');
      const cfg = JSON.parse(fs.readFileSync(cfgPath, 'utf8'));
      const email = cfg.oauthAccount?.emailAddress;
      if (email) account = ' \x1b[97m👤 ' + email.split('@')[0] + '\x1b[0m';
    } catch {}
    const remaining = data.context_window?.remaining_percentage;
    const totalIn = data.context_window?.total_input_tokens ?? null;
    const totalOut = data.context_window?.total_output_tokens ?? null;
    const ctxSize = data.context_window?.context_window_size ?? null;
    // Branch git (verde se limpo, amarelo se dirty)
    let gitBranch = '';
    const gitDir = data.workspace?.project_dir || data.workspace?.current_dir || '';
    if (gitDir && fs.existsSync(path.join(gitDir, '.git'))) {
      try {
        const { execFileSync } = require('child_process');
        const branch = execFileSync('git', ['-C', gitDir, '--no-optional-locks', 'rev-parse', '--abbrev-ref', 'HEAD'],
          { encoding: 'utf8', timeout: 2000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
        if (branch) {
          const dirty = execFileSync('git', ['-C', gitDir, '--no-optional-locks', 'status', '--porcelain'],
            { encoding: 'utf8', timeout: 2000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
          gitBranch = dirty
            ? ' \x1b[33m[' + branch + '*]\x1b[0m'
            : ' \x1b[32m[' + branch + ']\x1b[0m';
        }
      } catch {}
    }

    const fiveHourPct = data.rate_limits?.five_hour?.used_percentage ?? null;
    const fiveHourResets = data.rate_limits?.five_hour?.resets_at ?? null;
    const sevenDayPct = data.rate_limits?.seven_day?.used_percentage ?? null;
    const sevenDayResets = data.rate_limits?.seven_day?.resets_at ?? null;

    const fmtTok = n => {
      if (n >= 1000000) return (n / 1000000).toFixed(1).replace(/\.0$/, '') + 'M';
      if (n >= 1000) return Math.round(n / 1000) + 'k';
      return String(n);
    };

    // Tokens
    let tokens = '';
    if (totalIn != null && totalOut != null) {
      let ctxUsage = '';
      if (ctxSize != null && remaining != null) {
        const used = Math.round((1 - remaining / 100) * ctxSize);
        ctxUsage = ' ' + fmtTok(used) + '/' + fmtTok(ctxSize);
      }
      tokens = ' \x1b[97m💬 ' + fmtTok(totalIn) + '/' + fmtTok(totalOut) + '\x1b[0m\x1b[36m' + ctxUsage + '\x1b[0m';
    }

    // Cache do provedor (só em motor). Um turno relido do cache custa ~10% do preço de input novo,
    // então o que interessa saber é se o ÚLTIMO turno acertou o cache ou pagou re-prefill inteiro.
    // Não dá pra mostrar contagem regressiva de expiração: a statusline não redesenha com a sessão
    // parada (o relógio congela), então um cronômetro ficaria mentindo até o próximo turno. O que
    // aparece é fato consumado — taxa de acerto do último turno e o intervalo que o antecedeu.
    let cacheChip = '';
    if (engine && data.transcript_path) try {
      const fd = fs.openSync(data.transcript_path, 'r');
      let buf;
      try {
        const { size } = fs.fstatSync(fd);
        const len = Math.min(size, 262144);         // só a cauda: transcript de sessão longa é enorme
        buf = Buffer.alloc(len);
        fs.readSync(fd, buf, 0, len, size - len);
      } finally { fs.closeSync(fd); }
      const turnos = [];                            // [{id, ts, in, read, novo}], do fim pro começo
      const linhas = buf.toString('utf8').split('\n');
      for (let k = linhas.length - 1; k >= 0 && turnos.length < 2; k--) {
        if (!linhas[k].startsWith('{')) continue;
        let d; try { d = JSON.parse(linhas[k]); } catch { continue; }
        const u = d.message?.usage;
        // Turno sintético (hook barrou a continuação) tem usage todo zerado e id próprio: passaria
        // pelo dedup e entraria como turno de verdade, encurtando o intervalo medido — 52min viram
        // 12min sem nada indicar erro. Fora daqui, na coleta, senão contamina `anterior` também.
        if (!u || !(u.cache_read_input_tokens || u.input_tokens || u.cache_creation_input_tokens)) continue;
        const id = d.message?.id || d.uuid;
        if (turnos.some(t => t.id === id)) continue;  // o transcript repete o mesmo turno 2-4x
        turnos.push({ id, ts: Date.parse(d.timestamp),
                      read: u.cache_read_input_tokens || 0,
                      novo: (u.input_tokens || 0) + (u.cache_creation_input_tokens || 0) });
      }
      const [ultimo, anterior] = turnos;
      if (ultimo && ultimo.read + ultimo.novo > 0) {
        const pct = Math.round(ultimo.read / (ultimo.read + ultimo.novo) * 100);
        // Verde: pagou 10%. Vermelho: re-prefill — contexto inteiro cobrado como input novo.
        cacheChip = ' ' + (pct >= 80 ? '\x1b[32m' : pct >= 40 ? '\x1b[33m' : '\x1b[91m') + '♻' + pct + '%';
        const gap = anterior && Number.isFinite(ultimo.ts) && Number.isFinite(anterior.ts)
          ? Math.round((ultimo.ts - anterior.ts) / 60000) : 0;
        if (gap >= 5) cacheChip += ' ⏳' + (gap >= 60
          ? Math.floor(gap / 60) + 'h' + (gap % 60 ? (gap % 60) + 'm' : '')
          : gap + 'm');
        cacheChip += '\x1b[0m';
      }
    } catch {}

    // Rate limits nativos
    let quotaAviso = '';  // '⚠' quando a ultima leitura de quota falhou: o valor exibido e velho
    let rateLimit = '';
    let sevenDay = '';
    let fh = fiveHourPct;
    let sd = sevenDayPct;
    let fhReset = fiveHourResets;
    let sdReset = sevenDayResets;

    // Motor: os nativos vêm vazios (só a Anthropic os manda). Preenche com a quota do provedor,
    // cacheada pelo --refresh-usage. Cada janela vira o chip pela DISTÂNCIA do reset — ⚡ pra curta,
    // 📅 pra longa — porque nem todo provedor tem exatamente 5h/7d (o Kimi tem; o OmniRoute reporta
    // só a do upstream). Cache vazio = sem chip: melhor faltar do que inventar número.
    // O try de fora isola o chip como todo recurso opcional deste arquivo já faz (git, tmux,
    // kubectl): cache num formato inesperado derruba a quota, nunca a statusline inteira.
    if (engine) try {
      const agora = Math.floor(Date.now() / 1000);
      let cache = null;
      try { cache = JSON.parse(fs.readFileSync(usageCache(engine), 'utf8')); } catch {}
      const janelas = Array.isArray(cache?.janelas) ? cache.janelas : [];
      if (!cache || !(agora - cache.ts <= USAGE_TTL_S)) {
        // Carimba SEMPRE (sem isto cada render, ~300ms, subiria um refresher novo), mas o carimbo
        // leva junto a falha do spawn: senao um spawn barrado (EMFILE, sandbox) congelaria o chip
        // pra sempre parecendo fresco, e o `erro` do refresher nunca chegaria a ser escrito.
        let erroSpawn = null;
        if (!process.env.CP_STATUSLINE_NO_REFRESH) {
          try {
            require('child_process')
              .spawn(process.execPath, [__filename, '--refresh-usage', engine],
                     { detached: true, stdio: 'ignore' })
              .unref();
          } catch (e) { erroSpawn = 'spawn: ' + e; }
        }
        try {
          fs.writeFileSync(usageCache(engine), JSON.stringify(
            { ts: agora, janelas, ...(erroSpawn ? { erro: erroSpawn } : {}) }));
        } catch {}
      }
      if (cache?.erro) quotaAviso = '⚠';
      for (const j of janelas) {
        if (typeof j?.pct !== 'number') continue;
        const curta = j.reset == null || j.reset - agora < 86400;
        if (curta && fh == null) { fh = j.pct; fhReset = j.reset; }
        else if (!curta && sd == null) { sd = j.pct; sdReset = j.reset; }
      }
    } catch {}

    if (fh != null) {
      let resetStr = '';
      if (fhReset) {
        const diff = Math.max(0, fhReset - Math.floor(Date.now() / 1000));
        const m = Math.floor(diff / 60);
        if (m >= 60) { resetStr = ' ↺' + Math.floor(m/60) + 'h' + (m%60 > 0 ? m%60 + 'm' : ''); }
        else if (m > 0) { resetStr = ' ↺' + m + 'm'; }
      }
      const c = fh < 50 ? '\x1b[32m' : fh < 75 ? '\x1b[33m' : '\x1b[91m';
      rateLimit = ' ' + c + '⚡5h:' + Math.round(fh) + '%' + quotaAviso + resetStr + '\x1b[0m';
    }
    if (sd != null) {
      let resetStr = '';
      if (sdReset) {
        // Hora absoluta de quando a janela de 7d vence: ↺sex 14h·2d3h
        const d = new Date(sdReset * 1000);
        const dias = ['dom','seg','ter','qua','qui','sex','sab'];
        const diff = Math.max(0, sdReset - Math.floor(Date.now() / 1000));
        const dd = Math.floor(diff / 86400);
        const hh = Math.floor((diff % 86400) / 3600);
        const left = (dd > 0 ? dd + 'd' : '') + hh + 'h';
        resetStr = ' ↺' + dias[d.getDay()] + ' ' + d.getHours() + 'h·' + left;
      }
      const c = sd < 50 ? '\x1b[32m' : sd < 75 ? '\x1b[33m' : '\x1b[91m';
      sevenDay = ' ' + c + '📅7d:' + Math.round(sd) + '%' + quotaAviso + resetStr + '\x1b[0m';
    }

    // Nome da sessão tmux (= endereço da sessão no cp-send / claude-pocket)
    let tmuxSess = '';
    if (process.env.TMUX && process.env.TMUX_PANE) {
      try {
        const { execFileSync } = require('child_process');
        const s = execFileSync('tmux', ['display-message', '-p', '-t', process.env.TMUX_PANE, '#S'],
          { encoding: 'utf8', timeout: 1000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
        if (s) {
          tmuxSess = ' \x1b[95m📟 ' + s + '\x1b[0m';
          // Pareamento (claude-pocket): sidecar <config>/.claude-pocket-pair/<sessao>.json -> chip 🤝.
          // Grupo = {peers: [...]} (legado 1:1 = {peer}); 1 par mostra o nome, N mostra "a,b".
          try {
            const pair = JSON.parse(fs.readFileSync(
              path.join(claudeDir, '.claude-pocket-pair', s + '.json'), 'utf8'));
            const peers = pair.peers || (pair.peer ? [pair.peer] : []);
            if (peers.length) tmuxSess += ' \x1b[93m🤝 ' + peers.join(',') + '\x1b[0m';
          } catch {}
        }
      } catch {}
    }

    // kubectl current-context (vermelho piscando se prod)
    let kctx = '';
    try {
      const { execFileSync } = require('child_process');
      const ctxName = execFileSync('kubectl', ['config', 'current-context'],
        { encoding: 'utf8', timeout: 1000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
      if (ctxName) {
        const isProd = /prod/i.test(ctxName);
        const color = isProd ? '\x1b[5;1;97;41m' : '\x1b[36m';
        const prefix = isProd ? '⚠ ' : '⎈ ';
        kctx = ' ' + color + prefix + ctxName + '\x1b[0m';
      }
    } catch {}

    // Effort + thinking como sufixo do modelo: "Opus4.8·1M (high✦)"
    let effortSuffix = '';
    const effortLvl = data.effort?.level;
    const thinkingOn = data.thinking?.enabled;
    if (effortLvl) {
      effortSuffix = ' (' + effortLvl + (thinkingOn ? '✦' : '') + ')';
    } else if (thinkingOn) {
      effortSuffix = ' (thinking)';
    }

    // Custo da sessão (built-in do Claude Code). Em motor NÃO aparece: vem da tabela de preço da
    // Anthropic e não corresponde ao provedor. Consultar o painel dele.
    let cost = '';
    const usd = data.cost?.total_cost_usd;
    if (!engine && usd != null) {
      cost = ' \x1b[32m💵 $' + usd.toFixed(2) + '\x1b[0m';
    }

    // Hora local HH:MM + tempo de sessão (relógio de parede)
    const now = new Date();
    const hhmm = String(now.getHours()).padStart(2, '0') + ':' + String(now.getMinutes()).padStart(2, '0');
    let sessStr = '';
    const durMs = data.cost?.total_duration_ms;
    if (durMs != null) {
      const m = Math.floor(durMs / 60000);
      sessStr = ' ⏱ ' + (m >= 60 ? Math.floor(m / 60) + 'h' + (m % 60 ? (m % 60) + 'm' : '') : m + 'm');
    }
    const clock = ' \x1b[1;97m🕐 ' + hhmm + sessStr + '\x1b[0m';

    // Segmentos lógicos (trim remove o espaço inicial que cada um trazia)
    const segs = [
      '\x1b[1;35m🤖 ' + model + effortSuffix + '\x1b[0m',
      '\x1b[97m📁 ' + dir + '\x1b[0m' + gitBranch,
      tmuxSess, kctx, tokens, cacheChip, cost, rateLimit, sevenDay, clock
    ].map(s => s.trim()).filter(Boolean);

    const sep = ' │ ';
    // Largura visível: ignora códigos ANSI ao medir
    const visLen = s => s.replace(/\x1b\[[0-9;]*m/g, '').length;
    // COLUMNS setado pelo Claude Code (v2.1.153+); 0 = sem wrap (fallback antigo)
    const cols = parseInt(process.env.COLUMNS, 10) || 0;

    const lines = [];
    let cur = '', curLen = 0;
    for (const seg of segs) {
      const segLen = visLen(seg);
      if (cur === '') { cur = seg; curLen = segLen; continue; }
      const add = visLen(sep) + segLen;
      if (cols > 0 && curLen + add > cols) {
        lines.push(cur);        // não cabe → quebra linha
        cur = seg; curLen = segLen;
      } else {
        cur += sep + seg; curLen += add;
      }
    }
    if (cur) lines.push(cur);

    // claude-cockpit: publica a linha INTEIRA (sem ANSI, sem quebra) num sidecar que o backend lê
    // preferindo-a ao pane. O que sai no terminal é limitado pela largura — e quando a quebra cai em
    // cima do par de contexto, o Claude Code corta com "…" ("💬 769k/238 770k…") e o app fica sem
    // como medir contexto, exibindo "medição indisponível" só por causa do tamanho da janela.
    // Chave = session_id, que é o stem do .jsonl (mesma chave dos outros marcadores do cockpit).
    try {
      const sid = data.session_id;
      if (sid) {
        const sidecarDir = path.join(claudeDir, '.claude-pocket-status');
        fs.mkdirSync(sidecarDir, { recursive: true });
        const alvo = path.join(sidecarDir, sid + '.json');
        // pid no tmp: este script roda a CADA render e ainda faz 4 execFileSync (git×2, tmux,
        // kubectl) com timeout de 1-2s, entao duas invocacoes da MESMA sessao se sobrepoem na
        // pratica. Com nome fixo as duas abririam o mesmo caminho em truncate e o rename podia
        // promover bytes entrelacados — mesmo furo que scripts/cp_panel_common.py ja corrigiu
        // usando nome unico.
        const tmp = alvo + '.' + process.pid + '.tmp';
        fs.writeFileSync(tmp, JSON.stringify({
          line: segs.join(sep).replace(/\x1b\[[0-9;:?]*[ -/]*[@-~]/g, ''),
          ts: Math.floor(Date.now() / 1000),
        }));
        fs.renameSync(tmp, alvo);   // atômico: o backend pode ler no meio da escrita
      }
    } catch {}   // sidecar é conveniência: falhar aqui não pode sujar o statusline

    process.stdout.write(lines.join('\n'));
  } catch {}
});
