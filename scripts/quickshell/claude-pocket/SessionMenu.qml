import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io

// Menu de contexto FLUTUANTE de uma sessão (botão direito na linha): abre no cursor, por cima
// da lista, e some ao escolher/dispensar — como menu de contexto de sistema. Não empurra a
// lista (versão anterior fazia isso e reordenava tudo sob o cursor).
// As operações passam pelo cp-panel-action, que fala com a API do servidor DONO da sessão,
// então valem também pras remotas: só o attach no terminal é exclusivo da máquina local.
Rectangle {
    id: menu

    required property var session
    signal closed
    signal changed   // rename/kill mudam a lista -> pai dá refresh

    property string mode: ""      // "" | "rename" | "commit" | "kill"
    property string info: "…"
    property bool hasGit: false
    property string result: ""
    property bool busy: false

    implicitWidth: 236
    implicitHeight: inner.implicitHeight + 12
    radius: 14
    color: "#f21f2128"
    border.width: 1
    border.color: "#2effffff"

    Component.onCompleted: runner.run("info")

    // Engole cliques: sem isto o clique no menu vazava pra área de dispensa e fechava tudo.
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.RightButton
    }

    QtObject {
        id: runner

        function run(...args): void {
            if (menu.busy)
                return;
            menu.busy = true;
            menu.result = "";
            proc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action",
                menu.session.address, ...args];
            proc.running = true;
        }
    }

    Process {
        id: proc
        stdout: StdioCollector {
            onStreamFinished: {
                menu.busy = false;
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    menu.result = "resposta inválida do cp-panel-action";
                    menu.mode = "";
                    return;
                }
                if (r.branch !== undefined) {          // resposta do "info"
                    menu.info = r.message;
                    menu.hasGit = !!r.git;
                } else {
                    menu.result = r.message ?? "";
                    if (r.ok)
                        menu.changed();
                }
                menu.mode = "";
            }
        }
    }

    ColumnLayout {
        id: inner
        anchors.fill: parent
        anchors.margins: 6
        spacing: 0

        // Cabeçalho: nome + estado do git da sessão.
        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 8
            Layout.rightMargin: 8
            Layout.topMargin: 4
            Layout.bottomMargin: 4
            spacing: 0

            Text {
                Layout.fillWidth: true
                text: menu.session.name
                color: "#e3e2e6"
                font.pixelSize: 12
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }

            Text {
                Layout.fillWidth: true
                text: menu.busy && menu.result === "" ? "…" : menu.info
                color: "#8e9099"
                font.pixelSize: 9
                elide: Text.ElideRight
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.topMargin: 2
            Layout.bottomMargin: 2
            implicitHeight: 1
            color: "#1affffff"
        }

        // Itens do menu. Git só entra quando a sessão ESTÁ num repo — item que só pode falhar
        // é ruído. Pull/fetch executam direto (reversíveis); os demais pedem texto/confirmação.
        Repeater {
            model: menu.mode !== "" ? [] : [
                {id: "rename", icon: "edit", label: "Renomear…", git: false, danger: false},
                {id: "pull", icon: "download", label: "Git pull", git: true, danger: false},
                {id: "fetch", icon: "sync", label: "Git fetch", git: true, danger: false},
                {id: "commit", icon: "commit", label: "Commit + push…", git: true, danger: false},
                {id: "kill", icon: "power_settings_new", label: "Encerrar sessão", git: false, danger: true}
            ]

            Rectangle {
                required property var modelData
                visible: !modelData.git || menu.hasGit
                Layout.fillWidth: true
                implicitHeight: visible ? 30 : 0
                radius: 8
                color: itemMouse.containsMouse ? (modelData.danger ? "#33f2b8b5" : "#1fffffff") : "transparent"

                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 8
                    anchors.rightMargin: 8
                    spacing: 8

                    Text {
                        text: parent.parent.modelData.icon
                        font.family: "Material Symbols Rounded"
                        font.pixelSize: 15
                        color: parent.parent.modelData.danger ? "#f2b8b5" : "#c8c6cf"
                    }

                    Text {
                        Layout.fillWidth: true
                        text: parent.parent.modelData.label
                        font.pixelSize: 11
                        color: parent.parent.modelData.danger ? "#f2b8b5" : "#e3e2e6"
                    }
                }

                MouseArea {
                    id: itemMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        const op = parent.modelData.id;
                        if (op === "rename" || op === "commit" || op === "kill")
                            menu.mode = op;
                        else
                            runner.run("git", op);
                    }
                }
            }
        }

        // Passo 2 de rename/commit: campo de texto.
        ColumnLayout {
            Layout.fillWidth: true
            Layout.margins: 4
            spacing: 4
            visible: menu.mode === "rename" || menu.mode === "commit"

            TextField {
                id: field
                Layout.fillWidth: true
                placeholderText: menu.mode === "rename" ? "novo nome" : "mensagem do commit"
                text: menu.mode === "rename" ? menu.session.name : ""
                font.pixelSize: 11
                color: "#e3e2e6"
                onVisibleChanged: if (visible) {
                    selectAll();
                    forceActiveFocus();
                }
                onAccepted: menu.submit()
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 4

                MenuAction {
                    label: "Cancelar"
                    onTriggered: menu.mode = ""
                }

                MenuAction {
                    label: menu.mode === "rename" ? "Renomear" : "Commit + push"
                    accent: "#a8d5a2"
                    onTriggered: menu.submit()
                }
            }
        }

        // Passo 2 de encerrar: confirmação. Destrutivo não sai num clique só.
        ColumnLayout {
            Layout.fillWidth: true
            Layout.margins: 4
            spacing: 4
            visible: menu.mode === "kill"

            Text {
                Layout.fillWidth: true
                text: "Encerrar esta sessão?"
                color: "#f2b8b5"
                font.pixelSize: 11
                wrapMode: Text.Wrap
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 4

                MenuAction {
                    label: "Cancelar"
                    onTriggered: menu.mode = ""
                }

                MenuAction {
                    label: "Encerrar"
                    accent: "#f2b8b5"
                    onTriggered: runner.run("kill")
                }
            }
        }

        // Resultado da última operação — inclusive falha (pull com conflito, push rejeitado):
        // o cp-panel-action devolve ok=false com a saída real em vez de fingir sucesso.
        Text {
            Layout.fillWidth: true
            Layout.margins: 8
            Layout.topMargin: 2
            visible: menu.result !== ""
            text: menu.result
            color: "#c8c6cf"
            font.pixelSize: 9
            wrapMode: Text.Wrap
        }
    }

    function submit(): void {
        if (!field.text.trim())
            return;
        if (menu.mode === "rename")
            runner.run("rename", field.text.trim());
        else
            runner.run("commit-push", field.text.trim());
    }
}
