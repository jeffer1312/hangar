pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Io

// Uma sessão na lista: bolinha de estado + nome + subtítulo. Clique = ação do painel
// (attach local / abrir web remota), sinalizada pro pai.
Rectangle {
    id: row

    required property var session
    signal activated
    // Coordenadas de CENA (não locais): o menu flutua numa camada que cobre a janela toda,
    // então quem posiciona precisa do ponto no espaço dela, não no da linha.
    signal menuRequested(real sceneX, real sceneY)

    readonly property bool awaiting: session.state === "awaiting_input"
    readonly property bool working: session.state === "working"
    readonly property color stateColor: awaiting ? "#f2b8b5" : working ? "#f9c784" : "#8e9099"

    // Projeto do launcher casado com esta sessão, por cwd resolvido (realpath, campo cwd_real do
    // cp-panel-data). Só sessão local: dev server e projects.json são local-only. null = sem botão.
    readonly property var project: session.local
        ? (Sessions.projects.find(p => p.cwd_real && p.cwd_real === session.cwd_real) ?? null)
        : null
    readonly property bool projAlive: project && (project.state === "running" || project.state === "starting")
    readonly property bool projExternal: project && project.state === "external"
    // Última ação de dev server falhou (texto do cp-panel-action). Some na próxima ação.
    property string projError: ""

    function projAct(action: string): void {
        if (projProc.running || !project)
            return;
        row.projError = "";
        projProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", row.project.name, "project", action];
        projProc.running = true;
    }

    Process {
        id: projProc
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
                // Falha vira texto na linha (mesmo contrato do ProjectRow): stop_command quebrado
                // ou API fora não podem morrer mudos num botão de play.
                row.projError = r.ok ? "" : (r.message ?? "falhou");
            }
        }
    }

    implicitHeight: 56
    radius: 14
    color: mouse.containsMouse ? "#33ffffff" : "#14ffffff"

    Behavior on color {
        ColorAnimation {
            duration: 120
        }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onClicked: mouseEvent => {
            if (mouseEvent.button === Qt.RightButton) {
                const p = row.mapToItem(null, mouseEvent.x, mouseEvent.y);
                row.menuRequested(p.x, p.y);
            } else {
                row.activated();
            }
        }
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 14
        anchors.rightMargin: 14
        spacing: 12

        Rectangle {
            implicitWidth: 10
            implicitHeight: 10
            radius: 5
            color: row.stateColor
            Layout.alignment: Qt.AlignVCenter

            // Só o awaiting pulsa — é o único estado que exige ação humana. Pulsar o working
            // também tiraria o valor de olhar rápido pro painel.
            SequentialAnimation on opacity {
                running: row.awaiting
                loops: Animation.Infinite
                NumberAnimation {
                    to: 0.25
                    duration: 700
                    easing.type: Easing.InOutQuad
                }
                NumberAnimation {
                    to: 1
                    duration: 700
                    easing.type: Easing.InOutQuad
                }
            }
            onVisibleChanged: if (!row.awaiting)
                opacity = 1
        }

        ColumnLayout {
            spacing: 1
            Layout.fillWidth: true

            Text {
                Layout.fillWidth: true
                // Só o nome: o cabeçalho da seção já diz o servidor, então repetir o prefixo
                // "servidor::" em cada linha só roubaria largura do que importa.
                text: row.session.name
                color: "#e3e2e6"
                font.pixelSize: 14
                font.weight: row.awaiting ? Font.DemiBold : Font.Normal
                elide: Text.ElideMiddle
            }

            Text {
                Layout.fillWidth: true
                // Subtítulo carrega o que importa naquele estado: erro de dev server > pergunta
                // pendente > pasta.
                text: row.projError !== "" ? row.projError
                    : (row.awaiting && row.session.question ? String(row.session.question) : (row.session.cwd ?? ""))
                color: row.projError !== "" ? "#f28b82"
                    : (row.awaiting ? "#f2b8b5" : "#a0a0a8")
                font.pixelSize: 11
                elide: Text.ElideMiddle
            }
        }

        Text {
            visible: row.session.branch
            text: row.session.branch ?? ""
            color: "#8e9099"
            font.pixelSize: 10
        }

        // Play/stop do dev server do projeto casado (projects.json). Só aparece se a sessão bate
        // num projeto; externo só mostra stop com stop_command (sem pane pra matar). Z-order já
        // isola o clique — o RowLayout renderiza sobre o MouseArea da linha, então este não dispara
        // o activated(). Sem hoverEnabled: roubaria o containsMouse que a linha usa pro realce.
        Text {
            visible: row.project && (!row.projExternal || row.project.has_stop_command)
            text: projProc.running ? "hourglass_empty" : (row.projAlive || row.projExternal ? "stop_circle" : "play_circle")
            color: projProc.running ? "#6e7079" : (row.projAlive || row.projExternal ? "#f2b8b5" : "#8fce9b")
            font.family: "Material Symbols Rounded"
            font.pixelSize: 18
            renderType: Text.NativeRendering
            Layout.alignment: Qt.AlignVCenter

            MouseArea {
                anchors.fill: parent
                anchors.margins: -6
                cursorShape: Qt.PointingHandCursor
                enabled: !projProc.running
                onClicked: row.projAct(row.projAlive || row.projExternal ? "stop" : "start")
            }
        }

        // Diz o que o clique vai fazer: focar terminal já aberto, abrir um novo, ou abrir a web
        // UI (remota não attacha nesta máquina). Material Symbols (a mesma fonte de ícone do
        // rice) no lugar de emoji: pesa igual ao resto da UI e alinha na baseline do texto.
        Text {
            text: !row.session.local ? "language" : row.session.attached ? "terminal" : "add"
            color: !row.session.local ? "#8ab4f8" : row.session.attached ? "#a8d5a2" : "#6e7079"
            font.family: "Material Symbols Rounded"
            font.pixelSize: 18
            renderType: Text.NativeRendering
        }
    }
}
