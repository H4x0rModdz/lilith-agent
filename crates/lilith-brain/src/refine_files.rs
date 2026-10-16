//! Regras do plano para arquivos, documentos e área de transferência, quando
//! a frase não é exata o bastante para um atalho ("dá uma geral nos meus
//! downloads", "me traduz pro espanhol o texto que tá no ctrl c"). Olham só o
//! texto do usuário; na dúvida, `None` (o plano do intérprete segue).

fn has(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

const COPIED: &[&str] = &[
    "copiei", "copiado", "copiada", "clipboard", "área de transferência", "area de transferencia", "ctrl c", "ctrl+c",
    "ctrl + c", "control c",
];
const DUPLICATE: &[&str] = &["duplicad", "duplicata", "repetid", "cópias iguais", "copias iguais", "arquivos iguais"];
const REMOVE: &[&str] = &["apaga", "apague", "remove", "exclui", "deleta", "lixeira", "joga fora", "limpa", "limpe"];
const FOLDERS: &[&str] = &["download", "transferências", "transferencias", "área de trabalho", "area de trabalho", "desktop", "pasta"];

/// Plano decidido pelo código, ou `None` para deixar como veio. `recent`:
/// (pedido, resposta) dos turnos anteriores.
pub fn plan(t: &str, recent: &[(String, String)]) -> Option<Vec<String>> {
    let one = |a: &str| Some(vec![a.to_string()]);
    // O que foi copiado: traduzir/resumir/corrigir/explicar é uma ação só
    // (lê, transforma e, se pedido, copia de volta).
    if has(t, COPIED) && has(t, &["traduz", "tradução", "traducao", "translate", "resum", "corrig", "revis", "explica", "melhora", "reescrev"]) {
        return one("clipboard.transform");
    }
    // "Desfaz a organização" vem antes de "organiza".
    if has(t, &["desfaz", "desfazer", "desfaça", "reverte", "volta como estava", "volta como tava"]) && has(t, &["organiza", "arruma"]) {
        return one("file.organize_undo");
    }
    if has(t, &["organiza", "arruma", "dá uma geral", "da uma geral", "faxina"]) && has(t, FOLDERS) && !has(t, &["janela", "tela", "monitor"]) {
        return one("file.organize");
    }
    if has(t, DUPLICATE) {
        return one(if has(t, REMOVE) { "file.dedupe" } else { "file.duplicates" });
    }
    if has(t, &["maiores arquivos", "arquivos grandes", "arquivos mais pesados", "arquivos pesados", "ocupando espaço", "ocupando espaco", "comendo espaço", "enchendo o disco", "liberar espaço", "liberar espaco"]) {
        return one("file.big");
    }
    if has(t, &["indexa", "indexar"]) {
        return one("docs.index");
    }
    let searches_docs = has(
        t,
        &[
            "nos meus documentos", "nos documentos", "em meus documentos", "qual documento", "que documento",
            "algum documento", "qual arquivo fala", "que arquivo fala", "meus documentos dizem", "meus arquivos sobre",
        ],
    ) && !has(t, &["abre a pasta", "abra a pasta", "lista", "abre os documentos"]);
    if searches_docs && !has(t, &["resum"]) {
        return one("docs.search");
    }
    // "Resume esse documento" / "resume o primeiro" (depois de uma busca):
    // lê o arquivo; o modelo resume o que foi lido.
    let after_search = recent.iter().any(|(_, r)| r.contains("docs.search") || r.contains("nos seus documentos"));
    if has(t, &["resum", "do que se trata", "sobre o que é", "sobre o que e"])
        && (has(t, &["documento", "arquivo", "pdf", "docx", ".txt", ".md"])
            || (after_search && has(t, &["primeiro", "segundo", "terceiro", "esse", "este", "último", "ultimo"])))
    {
        return one("file.read");
    }
    // "Copia de volta" depois de traduzir/corrigir: grava o texto da resposta.
    if has(t, &["copia de volta", "copia isso", "coloca na área de transferência", "coloca no clipboard"]) {
        return one("clipboard.set");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(t: &str) -> Option<Vec<String>> {
        plan(t, &[])
    }

    fn v(a: &str) -> Option<Vec<String>> {
        Some(vec![a.to_string()])
    }

    #[test]
    fn files_and_clipboard_plans() {
        assert_eq!(p("me traduz pro espanhol o texto que tá no ctrl c"), v("clipboard.transform"));
        assert_eq!(p("faz um resumo bem curto do que eu copiei agora há pouco"), v("clipboard.transform"));
        assert_eq!(p("dá uma geral na minha pasta de downloads"), v("file.organize"));
        assert_eq!(p("desfaz aquela organização que você fez"), v("file.organize_undo"));
        assert_eq!(p("tem coisa repetida? procura arquivos duplicados nas fotos"), v("file.duplicates"));
        assert_eq!(p("apaga as fotos duplicadas"), v("file.dedupe"));
        assert_eq!(p("preciso liberar espaço, o que dá pra apagar?"), v("file.big"));
        assert_eq!(p("tem algum documento que fala do reajuste do aluguel?"), v("docs.search"));
        assert_eq!(p("resume esse documento pra mim"), v("file.read"));
        let recent = vec![("procura nos documentos x".to_string(), "Achei isto nos seus documentos:\n1. a.pdf".to_string())];
        assert_eq!(plan("resume o primeiro", &recent), v("file.read"));
        assert_eq!(p("resume o primeiro"), None, "sem busca antes, não sabe o que é");
        assert_eq!(p("organiza as janelas na tela"), None);
        assert_eq!(p("abre a pasta documentos"), None);
        assert_eq!(p("o que eu copiei?"), None);
    }
}
