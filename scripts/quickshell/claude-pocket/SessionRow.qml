import QtQuick
import QtQuick.Layouts

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
    // Erro da última ação de start/stop e o "em voo" moram no singleton Sessions (não aqui): o
    // delegate da lista é recriado a cada poll que muda sessions[] (status_line de uma sessão
    // working), e um Process/estado local morreria em voo — o "falhou" sumiria calado.
    readonly property string projError: row.project ? Sessions.projErr(row.project.name) : ""
    readonly property bool projBusy: row.project && Sessions.togglePending === row.project.name

    readonly property bool pushConfirming: Sessions.pushConfirm === row.session.address
    readonly property bool pushing: Sessions.pushPending === row.session.address
    readonly property string pushError: Sessions.pushErr(row.session.address)
    // Um erro só no subtítulo: dev server tem precedência sobre push (raro os dois juntos).
    readonly property string rowError: row.projError !== "" ? row.projError : row.pushError

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
                text: row.rowError !== "" ? row.rowError
                    : (row.awaiting && row.session.question ? String(row.session.question) : (row.session.cwd ?? ""))
                color: row.rowError !== "" ? "#f28b82"
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

        // Badge de git: não-commitado (●N âmbar) e não-pushado (↑N azul), ou ✓ sincronizado.
        // Campos vêm do /api/sessions; null (não-repo/sem-upstream) → some. Glyphs unicode na fonte
        // padrão (pixelSize casando o chip de branch), não Material Symbols — ●↑✓ pesam certo aqui.
        Text {
            visible: (row.session.git_dirty ?? 0) > 0
            text: "●" + row.session.git_dirty
            color: "#f9c784"
            font.pixelSize: 10
            renderType: Text.NativeRendering
        }

        // ↑N não-pushado: 1º toque arma o confirm inline; em voo vira ↑…; confirmando vira
        // "push N em <branch>? ✓ ✕". Estado no singleton (sobrevive à recria do delegate).
        Text {
            visible: (row.session.git_ahead ?? 0) > 0 && !row.pushConfirming && !row.pushing
            text: "↑" + row.session.git_ahead
            color: "#8ab4f8"
            font.pixelSize: 10
            renderType: Text.NativeRendering

            MouseArea {
                anchors.fill: parent
                anchors.margins: -6
                cursorShape: Qt.PointingHandCursor
                onClicked: Sessions.pushConfirmToggle(row.session.address)
            }
        }

        Text {
            visible: row.pushing
            text: "↑…"
            color: "#6e7079"
            font.pixelSize: 10
            renderType: Text.NativeRendering
        }

        Row {
            visible: row.pushConfirming
            spacing: 6
            Layout.alignment: Qt.AlignVCenter

            Text {
                text: "push " + (row.session.git_ahead ?? 0) + " em " + (row.session.branch ?? "?") + "?"
                color: "#e3e2e6"
                font.pixelSize: 10
                elide: Text.ElideRight
                width: Math.min(implicitWidth, 150)
            }

            Text {
                text: "✓"
                color: "#8fce9b"
                font.pixelSize: 13
                renderType: Text.NativeRendering
                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -6
                    cursorShape: Qt.PointingHandCursor
                    onClicked: Sessions.pushSession(row.session.address)
                }
            }

            Text {
                text: "✕"
                color: "#f28b82"
                font.pixelSize: 13
                renderType: Text.NativeRendering
                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -6
                    cursorShape: Qt.PointingHandCursor
                    onClicked: Sessions.pushCancel()
                }
            }
        }

        Text {
            // Sincronizado: repo com upstream, sem sujo e sem commits à frente. git_ahead null (sem
            // upstream) NÃO conta — aí não há "sincronizado" a afirmar.
            visible: (row.session.git_ahead ?? null) === 0 && (row.session.git_dirty ?? 0) === 0
            text: "✓"
            color: "#6e7079"
            font.pixelSize: 10
            renderType: Text.NativeRendering
        }

        // Play/stop do dev server do projeto casado (projects.json). Só aparece se a sessão bate
        // num projeto; externo só mostra stop com stop_command (sem pane pra matar). Z-order já
        // isola o clique — o RowLayout renderiza sobre o MouseArea da linha, então este não dispara
        // o activated(). Sem hoverEnabled: roubaria o containsMouse que a linha usa pro realce.
        Text {
            visible: row.project && (!row.projExternal || row.project.has_stop_command)
            text: row.projBusy ? "hourglass_empty" : (row.projAlive || row.projExternal ? "stop_circle" : "play_circle")
            color: row.projBusy ? "#6e7079" : (row.projAlive || row.projExternal ? "#f2b8b5" : "#8fce9b")
            font.family: "Material Symbols Rounded"
            font.pixelSize: 18
            renderType: Text.NativeRendering
            Layout.alignment: Qt.AlignVCenter

            MouseArea {
                anchors.fill: parent
                anchors.margins: -6
                cursorShape: Qt.PointingHandCursor
                enabled: !row.projBusy
                onClicked: Sessions.toggleProject(row.project.name, row.projAlive || row.projExternal ? "stop" : "start")
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
