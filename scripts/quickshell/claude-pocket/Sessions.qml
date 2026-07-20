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
    property bool everLoaded: false
    // Poll rápido só com o painel aberto: fechado é passivo (badge/notificação), não precisa de 2s.
    property bool fast: false
    readonly property int awaitingCount: sessions.filter(s => s.state === "awaiting_input").length
    readonly property int workingCount: sessions.filter(s => s.state === "working").length

    // Endereços em awaiting_input no ciclo anterior — a notificação dispara só na TRANSIÇÃO
    // pra awaiting (sem isto, todo poll re-notificaria a mesma sessão em loop).
    property var _notified: ({})

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
                    root.sessions = data.sessions ?? [];
                    root.errors = data.errors ?? [];
                    root.servers = data.servers ?? [];
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
