pragma Singleton
pragma ComponentBehavior: Bound

import Quickshell
import Quickshell.Io
import QtQuick

// Estado das sessões de TODOS os servidores. Toda a lógica suja (token, peers.json, merge,
// ordenação) vive no scripts/cp-panel-data — aqui só roda e faz parse, pra este QML não
// duplicar nada do backend nem carregar segredo.
Singleton {
    id: root

    property var sessions: []
    property var errors: []
    // Servidores do peers.json com {id, enabled, ok, error} — inclui os DESLIGADOS, que não
    // aparecem em sessions[] mas precisam de linha na UI pra ter como religar.
    property var servers: []
    // Launcher de projetos (projects.json, só local): {name, cwd, command, port, state, ...}.
    property var projects: []
    property bool everLoaded: false
    // Poll rápido só com o painel aberto: fechado é passivo (badge/notificação), não precisa de 2s.
    property bool fast: false
    readonly property int awaitingCount: sessions.filter(s => s.state === "awaiting_input").length
    readonly property int workingCount: sessions.filter(s => s.state === "working").length

    // Endereços em awaiting_input no ciclo anterior — a notificação dispara só na TRANSIÇÃO
    // pra awaiting (sem isto, todo poll re-notificaria a mesma sessão em loop).
    property var _notified: ({})

    // Tail de log aberto por projeto (chave = nome). Mora AQUI e não no ProjectRow porque o
    // delegate morre quando o modelo troca — o estado sobrevive no singleton.
    property var openLogs: ({})

    // Erro da última ação de start/stop de dev server por projeto (chave = nome; vazio = sem
    // erro) e o nome do projeto com toggle em voo. Moram AQUI pelo mesmo motivo do openLogs: o
    // delegate SessionRow é recriado a cada poll que muda sessions[], e um Process/erro local
    // morreria em voo — o onStreamFinished nunca dispararia e o "falhou" sumiria calado.
    property var projErrors: ({})
    property string togglePending: ""
    property string togglePendingAction: ""

    // Grava o estado EFETIVO desejado (não um flip cego): o card falhado abre o tail por
    // padrão sem entrada no mapa, então o toggle precisa receber o valor visível atual.
    function setLog(name: string, open: bool): void {
        const m = Object.assign({}, openLogs);
        m[name] = open;
        openLogs = m;
    }

    function setProjError(name: string, msg: string, action: string): void {
        const m = Object.assign({}, projErrors);
        if (msg)
            m[name] = {
                msg: msg,
                action: action
            };
        else
            delete m[name];
        projErrors = m;
    }

    function projErr(name: string): string {
        const e = projErrors[name];
        return e ? e.msg : "";
    }

    // Start/stop do dev server disparado pela linha de sessão. O Process (toggler) mora no
    // singleton pra sobreviver à recriação do delegate. Um toggle por vez basta no painel
    // single-user; clique num segundo enquanto um roda é ignorado.
    function toggleProject(name: string, action: string): void {
        if (toggler.running)
            return;
        root.togglePending = name;
        root.togglePendingAction = action;
        setProjError(name, "", "");
        toggler.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", name, "project", action];
        toggler.running = true;
    }

    // Limpa o erro só quando a ação que falhou VISIVELMENTE deu certo depois: start -> projeto no
    // ar (running/starting/external); stop -> parado. Enquanto não deu (start em failed/stopped,
    // stop com o server ainda de pé), o texto FICA — é o único aviso de falha na linha da sessão,
    // e limpar por estado cru sumiria com o erro de um stop que falhou (o server segue running).
    // Projeto sumido do launcher também limpa.
    function _pruneProjErrors(): void {
        if (Object.keys(root.projErrors).length === 0)
            return;
        const st = {};
        for (const p of root.projects)
            st[p.name] = p.state;
        const m = {};
        for (const name in root.projErrors) {
            const e = root.projErrors[name];
            const s = st[name];
            const succeeded = e.action === "start"
                ? (s === "running" || s === "starting" || s === "external")
                : (s === "stopped");
            if (s !== undefined && !succeeded)
                m[name] = e;
        }
        if (JSON.stringify(m) !== JSON.stringify(root.projErrors))
            root.projErrors = m;
    }

    // Só reatribui quando MUDOU de verdade: cada reassign destrói os delegates da ListView
    // (scroll volta pro topo, tail de log aberto fecha). Poll sem mudança — a imensa maioria
    // com o painel aberto a 2s — vira no-op. Stringify é estável aqui: o backend serializa os
    // mesmos campos na mesma ordem a cada poll.
    function _assign(prop: string, val: var): void {
        if (JSON.stringify(root[prop]) !== JSON.stringify(val))
            root[prop] = val;
    }

    function refresh(): void {
        // Guarda: com um servidor lento, um novo poll antes do anterior terminar empilharia
        // processos indefinidamente.
        if (fetcher.running)
            return;
        fetcher.running = true;
    }

    function _notify(list: var): void {
        const seen = {};
        for (const s of list) {
            if (s.state !== "awaiting_input")
                continue;
            seen[s.address] = true;
            if (root._notified[s.address])
                continue;
            Quickshell.execDetached(["notify-send", "-a", "claude-pocket", "-u", "critical",
                "-i", "dialog-question", `Claude precisa de você — ${s.address}`,
                s.question ? String(s.question) : (s.cwd ?? "")]);
        }
        root._notified = seen;
    }

    Process {
        id: fetcher
        // Symlink do installer (mesmo padrão do cp-send). Caminho absoluto de propósito: o
        // quickshell sobe pelo compositor, sem garantia de ~/.local/bin no PATH.
        command: [Quickshell.env("HOME") + "/.local/bin/cp-panel-data"]
        stdout: StdioCollector {
            onStreamFinished: {
                try {
                    const data = JSON.parse(text);
                    root._assign("sessions", data.sessions ?? []);
                    root._assign("errors", data.errors ?? []);
                    root._assign("servers", data.servers ?? []);
                    root._assign("projects", data.projects ?? []);
                    root._pruneProjErrors();
                    root.everLoaded = true;
                    root._notify(root.sessions);
                } catch (e) {
                    // Saída ilegível = script quebrado/ausente. Vira erro VISÍVEL no painel;
                    // engolir deixaria a lista velha na tela parecendo atual.
                    root.errors = [`cp-panel-data: saída inválida (${e.message})`];
                    root.everLoaded = true;
                }
            }
        }
    }

    Process {
        id: toggler
        stdout: StdioCollector {
            onStreamFinished: {
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    r = {
                        ok: false,
                        message: "resposta inválida do cp-panel-action"
                    };
                }
                // Falha vira texto na linha da sessão; não pode morrer muda num botão de play.
                root.setProjError(root.togglePending, r.ok ? "" : (r.message ?? "falhou"), root.togglePendingAction);
                root.togglePending = "";
                root.togglePendingAction = "";
                // Reflete o novo estado do projeto sem esperar o próximo tick do Timer.
                root.refresh();
            }
        }
    }

    Timer {
        running: true
        repeat: true
        triggeredOnStart: true
        interval: root.fast ? 2000 : 10000
        onTriggered: root.refresh()
    }
}
