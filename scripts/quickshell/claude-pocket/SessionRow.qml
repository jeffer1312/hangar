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
                // Subtítulo carrega o que importa naquele estado: pergunta pendente > pasta.
                text: row.awaiting && row.session.question ? String(row.session.question) : (row.session.cwd ?? "")
                color: row.awaiting ? "#f2b8b5" : "#a0a0a8"
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
