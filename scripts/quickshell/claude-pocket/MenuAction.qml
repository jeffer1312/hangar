import QtQuick
import QtQuick.Layouts

// Botãozinho dos passos de confirmação do menu (Cancelar / Renomear / Encerrar).
Rectangle {
    id: btn

    property string label: ""
    property string accent: "#c8c6cf"
    signal triggered

    Layout.fillWidth: true
    implicitHeight: 26
    radius: 8
    color: mouse.containsMouse ? Qt.rgba(1, 1, 1, 0.18) : Qt.rgba(1, 1, 1, 0.08)

    Text {
        anchors.centerIn: parent
        text: btn.label
        font.pixelSize: 10
        font.weight: Font.DemiBold
        color: btn.accent
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: btn.triggered()
    }
}
