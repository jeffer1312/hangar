/// Linha real do Claude Code 2.1.296 (sem terminal, caminhos encurtados): três mensagens que chegaram durante o
/// turno viraram UMA mensagem `user` com um bloco de texto por entrada (e as imagens no meio).
pub(crate) fn merged_inputs_line(session:&str,timestamp:&str) -> serde_json::Value {
    serde_json::json!({"type":"user","uuid":"17729883-e589-4998-b1ee-45d3e73dcd56","timestamp":timestamp,"sessionId":session,
        "promptSource":"sdk","message":{"role":"user","content":[
            {"type":"text","text":"aqui precisava saber oque esta rodando, ou ja terminou — \u{1f4ce} imagem: /up/1791899344-c98f30.png"},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":""}},
            {"type":"text","text":"precisa melhorar essa tela de ativiidades, talvez ate mudar esse nome, pq nao faz mujito sentido faz? "},
            {"type":"text","text":"e ele ainda ocme um pouco ali da barra, daria pra conseguir mandar msg nesses subagnts? — \u{1f4ce} imagem: /up/1791899408-46912e.png \u{1f4ce} imagem: /up/1791899409-aa.png"},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":""}},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":""}}]}})
}
pub(crate) const MERGED_INPUTS:[&str;3] = [
    "aqui precisava saber oque esta rodando, ou ja terminou — \u{1f4ce} imagem: /up/1791899344-c98f30.png",
    "precisa melhorar essa tela de ativiidades, talvez ate mudar esse nome, pq nao faz mujito sentido faz? ",
    "e ele ainda ocme um pouco ali da barra, daria pra conseguir mandar msg nesses subagnts? — \u{1f4ce} imagem: /up/1791899408-46912e.png \u{1f4ce} imagem: /up/1791899409-aa.png",
];
