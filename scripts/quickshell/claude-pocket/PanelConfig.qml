pragma Singleton
pragma ComponentBehavior: Bound

import Quickshell
import Quickshell.Io

// Config do painel em ~/.config/claude-pocket-panel.json (FileView + JsonAdapter, mesmo padrão
// do Config.qml do rice). Fica FORA do diretório do shell de propósito: aquele é um symlink pro
// repo, e config de máquina não tem por que virar arquivo versionado.
Singleton {
    id: root

    readonly property string filePath: Quickshell.env("HOME") + "/.config/claude-pocket-panel.json"
    property alias options: adapter

    // Abaixo de 0.79 o compositor PULA o blur (ignore_alpha do rice pra "quickshell:.*"), então o
    // painel vira vidro sujo em vez de fosco. Não é limite do slider — é o aviso que a UI mostra.
    readonly property real blurCutoff: 0.79

    FileView {
        path: root.filePath
        watchChanges: true
        onFileChanged: reload()
        onAdapterUpdated: writeAdapter()
        onLoadFailed: error => {
            // Primeira execução: grava os defaults em vez de rodar sem persistência.
            if (error === FileViewError.FileNotFound)
                writeAdapter();
        }

        JsonAdapter {
            id: adapter

            property real opacity: 0.88
        }
    }
}
