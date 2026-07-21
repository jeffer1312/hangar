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

    // Grava o estado EFETIVO desejado (não um flip cego): o card falhado abre o tail por
    // padrão sem entrada no mapa, então o toggle precisa receber o valor visível atual.
    function setLog(name: string, open: bool): void {
        const m = Object.assign({}, openLogs);
        m[name] = open;
        openLogs = m;
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

    Timer {
        running: true
        repeat: true
        triggeredOnStart: true
        interval: root.fast ? 2000 : 10000
        onTriggered: root.refresh()
    }
}
