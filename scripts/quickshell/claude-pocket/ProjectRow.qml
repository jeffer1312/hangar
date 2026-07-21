pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Io

// Um projeto do launcher (projects.json): bolinha de estado + nome + play/stop + log.
// O tail do log expande embaixo da linha (clique no ícone de log) e se atualiza a cada poll
// enquanto aberto — falha ao subir (state=failed) é exatamente quando ele importa.
Rectangle {
    id: row

    required property var project
    signal changed

    // Última ação falhou (texto do cp-panel-action). Some na próxima ação.
    property string actionError: ""
    // Aberto/fechado vive no singleton (Sessions.openLogs): o delegate é destruído quando o
    // modelo troca (play/stop) e um bool local fecharia o tail sozinho nessas horas.
    // FALHOU abre o tail por padrão — o motivo da queda tem que estar na cara, sem clique
    // (fechável no ícone mesmo assim: o mapa guarda a escolha explícita).
    readonly property bool logOpen: Sessions.openLogs[project.name] ?? (project.state === "failed")
    property string logText: ""

    // Delegate recriado com o tail já aberto (troca de modelo) precisa repopular o texto.
    Component.onCompleted: if (logOpen)
        fetchLog()
    onLogOpenChanged: if (logOpen)
        fetchLog()

    readonly property bool alive: project.state === "running" || project.state === "starting"
    // Rodando fora do launcher (porta aberta sem pane nosso): sem log/attach — o processo não
    // é nosso — e stop só se o config tiver stop_command.
    readonly property bool external: project.state === "external"
    readonly property color stateColor: project.state === "running" ? "#8fce9b"
        : project.state === "starting" ? "#f9c784"
        : project.state === "failed" ? "#f28b82"
        : project.state === "external" ? "#8ab4f8" : "#6e7079"
    readonly property string stateLabel: {
        if (project.state === "running")
            return project.port ? `rodando · :${project.port}` : "rodando";
        if (project.state === "starting")
            return `subindo · :${project.port} ainda fechada`;
        if (project.state === "failed")
            return project.exit_status !== null && project.exit_status !== undefined ? `falhou (exit ${project.exit_status})` : "falhou";
        if (project.state === "external")
            return `rodando fora do launcher · :${project.port}`;
        return "parado";
    }

    // Poll novo troca o objeto project inteiro — refetch do tail só com ele aberto.
    onProjectChanged: if (logOpen)
        row.fetchLog()

    function act(action: string): void {
        if (actProc.running)
            return;
        row.actionError = "";
        actProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", row.project.name, "project", action];
        actProc.running = true;
    }

    function fetchLog(): void {
        if (paneProc.running)
            return;
        paneProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", row.project.name, "project", "pane"];
        paneProc.running = true;
    }

    Process {
        id: actProc
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
                // Falha vira texto na linha (mesmo contrato do toggle de peer): stop_command
                // quebrado ou API fora não podem morrer mudos num botão de play.
                row.actionError = r.ok ? "" : (r.message ?? "falhou");
                row.changed();
                if (row.logOpen)
                    row.fetchLog();
            }
        }
    }

    Process {
        id: paneProc
        stdout: StdioCollector {
            onStreamFinished: {
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    r = {};
                }
                // Últimas 14 linhas não-vazias: tail de painel, não viewer — o completo é o
                // botão de terminal (attach na sessão do runner).
                const lines = String(r.pane ?? "").split("\n").filter(l => l.trim() !== "");
                row.logText = lines.slice(-14).join("\n") || "(log vazio)";
            }
        }
    }

    implicitHeight: col.implicitHeight + 16
    radius: 14
    color: "#14ffffff"

    ColumnLayout {
        id: col
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.leftMargin: 14
        anchors.rightMargin: 10
        anchors.topMargin: 8
        spacing: 4

        RowLayout {
            Layout.fillWidth: true
            spacing: 10

            Rectangle {
                implicitWidth: 10
                implicitHeight: 10
                radius: 5
                color: row.stateColor
                Layout.alignment: Qt.AlignVCenter
            }

            ColumnLayout {
                spacing: 1
                Layout.fillWidth: true

                Text {
                    Layout.fillWidth: true
                    text: row.project.name
                    color: "#e3e2e6"
                    font.pixelSize: 13
                    elide: Text.ElideMiddle
                }

                Text {
                    Layout.fillWidth: true
                    text: row.actionError !== "" ? row.actionError : row.stateLabel
                    color: row.actionError !== "" ? "#f28b82" : (row.project.state === "failed" ? "#f28b82" : "#a0a0a8")
                    font.pixelSize: 10
                    elide: Text.ElideRight
                }
            }

            // Play/stop. Enquanto a ação roda, ícone de espera — evita clique duplo.
            // Externo: stop SÓ com stop_command (não há pane pra matar); play escondido — subir
            // por cima de porta ocupada só fabricaria um "failed" de porta em uso.
            Text {
                visible: !row.external || row.project.has_stop_command
                text: actProc.running ? "hourglass_empty" : (row.alive || row.external ? "stop_circle" : "play_circle")
                color: actProc.running ? "#6e7079" : (row.alive || row.external ? "#f2b8b5" : "#8fce9b")
                font.family: "Material Symbols Rounded"
                font.pixelSize: 20
                renderType: Text.NativeRendering

                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -6
                    cursorShape: Qt.PointingHandCursor
                    enabled: !actProc.running
                    onClicked: row.act(row.alive || row.external ? "stop" : "start")
                }
            }

            // Tail do log embutido no painel. Externo não tem pane — não há log a mostrar.
            Text {
                visible: !row.external
                text: "receipt_long"
                color: row.logOpen ? "#e3e2e6" : "#6e7079"
                font.family: "Material Symbols Rounded"
                font.pixelSize: 17
                renderType: Text.NativeRendering

                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -6
                    cursorShape: Qt.PointingHandCursor
                    onClicked: Sessions.setLog(row.project.name, !row.logOpen)
                }
            }

            // Log completo: terminal attachado na sessão do runner (funciona até com o
            // processo morto — remain-on-exit guarda o pane pra inspeção).
            Text {
                text: "terminal"
                color: "#6e7079"
                font.family: "Material Symbols Rounded"
                font.pixelSize: 17
                renderType: Text.NativeRendering
                visible: row.project.state !== "stopped" && !row.external

                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -6
                    cursorShape: Qt.PointingHandCursor
                    onClicked: row.act("open")
                }
            }
        }

        Rectangle {
            visible: row.logOpen
            Layout.fillWidth: true
            Layout.bottomMargin: 4
            implicitHeight: logBody.implicitHeight + 12
            radius: 8
            color: "#33000000"

            Text {
                id: logBody
                anchors.fill: parent
                anchors.margins: 6
                text: row.logText || "carregando…"
                color: "#c8c6cf"
                font.family: "monospace"
                font.pixelSize: 9
                wrapMode: Text.WrapAnywhere
                textFormat: Text.PlainText
            }
        }
    }
}
