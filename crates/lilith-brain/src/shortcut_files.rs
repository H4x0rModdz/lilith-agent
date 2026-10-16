//! Atalhos sem IA para arquivos, documentos e área de transferência:
//! "organiza meus downloads", "desfaz a organização", "acha arquivos
//! duplicados", "quais os maiores arquivos", "procura nos meus documentos
//! sobre X", "traduz o que eu copiei". Como em `shortcut`, a frase inteira
//! precisa casar; o que não casar vai para o modelo. As respostas são montadas
//! aqui a partir do resultado.

use crate::shortcut::Shortcut;
use crate::tools::ToolCall;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex de atalho válida"))
}

fn one(action: &str, params: Value) -> Shortcut {
    Shortcut { calls: vec![ToolCall { action: action.into(), params }], reply: "Pronto!".into() }
}

/// "meus downloads", "a pasta de documentos", "área de trabalho", "pc todo"
/// → nome que o `lilith-win` resolve. `None` = não é uma pasta conhecida.
pub fn folder_of(phrase: &str) -> Option<String> {
    let mut p = phrase.trim().to_lowercase();
    loop {
        let before = p.clone();
        for lead in ["minha ", "minhas ", "meu ", "meus ", "a ", "o ", "as ", "os ", "pasta ", "de ", "da ", "do ", "dos ", "das ", "todo ", "toda "] {
            if let Some(rest) = p.strip_prefix(lead) {
                p = rest.trim().to_string();
            }
        }
        if p == before {
            break;
        }
    }
    let p = p.trim_end_matches(" todo").trim_end_matches(" inteiro").trim();
    let name = match p {
        "downloads" | "download" | "transferências" | "transferencias" => "Downloads",
        "área de trabalho" | "area de trabalho" | "desktop" => "Área de Trabalho",
        "documentos" => "Documentos",
        "imagens" | "fotos" => "Imagens",
        "vídeos" | "videos" => "Vídeos",
        "músicas" | "musicas" => "Músicas",
        "pc" | "computador" | "disco" | "usuário" | "usuario" => "home",
        _ => return None,
    };
    Some(name.into())
}

/// O que o usuário copiou (Ctrl+C), em várias formas de dizer.
const COPIED: &str = r"(?:o\s+)?(?:que\s+(?:eu\s+)?copiei|texto\s+(?:que\s+(?:eu\s+)?copiei|copiado)|(?:meu\s+)?(?:clipboard|ctrl\s*\+?\s*c)|(?:da\s+|a\s+)?[aá]rea\s+de\s+transfer[eê]ncia)";

pub fn parse(t: &str) -> Option<Shortcut> {
    // ── organizar pastas ──────────────────────────────────────────────────
    static ORGANIZE: OnceLock<Regex> = OnceLock::new();
    static UNDO: OnceLock<Regex> = OnceLock::new();
    if re(&UNDO, r"^(?:desfaz|desfazer|desfa[çc]a|reverte|cancela)\s+(?:a\s+)?(?:[uú]ltima\s+)?(?:organiza[çc][aã]o|arruma[çc][aã]o)(?:\s+d[aeo]s?\s+.+)?$")
        .is_match(t)
    {
        return Some(one("file.organize_undo", json!({})));
    }
    if let Some(c) = re(&ORGANIZE, r"^(?:organiza|organize|arruma|arrume)\s+(.+)$").captures(t) {
        if let Some(folder) = folder_of(&c[1]).filter(|f| f != "home") {
            return Some(one("file.organize", json!({ "folder": folder })));
        }
    }

    // ── duplicados ────────────────────────────────────────────────────────
    const DUPS: &str = r"(?:(?:os\s+|as\s+)?(?:arquivos\s+)?(?:duplicados|duplicadas|duplicatas|repetidos|c[oó]pias\s+repetidas))";
    const WHERE: &str = r"(?:\s+(?:n[oa]s?|em|d[oa]s?)\s+(.+))?";
    static DEDUPE: OnceLock<Regex> = OnceLock::new();
    static DUPLICATES: OnceLock<Regex> = OnceLock::new();
    let target = |c: &regex::Captures, i: usize| -> Option<Option<String>> {
        match c.get(i) {
            None => Some(None),
            Some(m) => folder_of(m.as_str()).map(Some),
        }
    };
    if let Some(c) = re(
        &DEDUPE,
        &format!(r"^(?:apaga|apague|remove|exclui|deleta|limpa|joga\s+fora|manda\s+pra\s+lixeira)\s+{DUPS}{WHERE}$"),
    )
    .captures(t)
    {
        let folder = target(&c, 1)?;
        return Some(one("file.dedupe", folder.map(|f| json!({ "folder": f })).unwrap_or_else(|| json!({}))));
    }
    if let Some(c) = re(
        &DUPLICATES,
        &format!(r"^(?:(?:acha|ache|procura|procure|encontra|mostra|lista|tem|quais\s+s[aã]o)\s+)?(?:algum\s+|alguns\s+)?{DUPS}{WHERE}$"),
    )
    .captures(t)
    {
        let folder = target(&c, 1)?;
        return Some(one("file.duplicates", folder.map(|f| json!({ "folder": f })).unwrap_or_else(|| json!({}))));
    }

    // ── maiores arquivos ──────────────────────────────────────────────────
    static BIG: OnceLock<Regex> = OnceLock::new();
    static SPACE: OnceLock<Regex> = OnceLock::new();
    if let Some(c) = re(
        &BIG,
        &format!(r"^(?:(?:quais|mostra|lista|acha|procura|me\s+mostra)\s+)?(?:s[aã]o\s+)?(?:os\s+)?(\d{{1,2}}\s+)?(?:maiores\s+arquivos|arquivos\s+(?:mais\s+)?(?:grandes|pesados)){WHERE}$"),
    )
    .captures(t)
    {
        let folder = target(&c, 2)?;
        let mut params = json!({});
        if let Some(f) = folder {
            params["folder"] = json!(f);
        }
        if let Some(n) = c.get(1).and_then(|m| m.as_str().trim().parse::<u64>().ok()) {
            params["limit"] = json!(n.clamp(1, 50));
        }
        return Some(one("file.big", params));
    }
    if let Some(c) = re(&SPACE, r"^(?:o\s+)?que\s+(?:t[aá]|est[aá])\s+(?:ocupando|comendo|enchendo)\s+(?:mais\s+)?(?:espa[çc]o|o\s+disco)(?:\s+(?:n[oa]s?|em)\s+(.+))?$").captures(t) {
        let folder = target(&c, 1)?;
        return Some(one("file.big", folder.map(|f| json!({ "folder": f })).unwrap_or_else(|| json!({}))));
    }

    // ── documentos ────────────────────────────────────────────────────────
    static DOC_SEARCH: OnceLock<Regex> = OnceLock::new();
    static DOC_WHICH: OnceLock<Regex> = OnceLock::new();
    static DOC_INDEX: OnceLock<Regex> = OnceLock::new();
    if let Some(c) = re(
        &DOC_SEARCH,
        r"^(?:procura|procure|busca|busque|acha|ache|pesquisa)\s+(?:nos|em|entre\s+os)\s+(?:meus\s+)?(?:documentos|arquivos)\s+(?:(?:algo\s+)?(?:sobre|por|de|que\s+fal[ae]\s+(?:de|sobre))\s+)?(.{2,})$",
    )
    .captures(t)
    {
        return Some(one("docs.search", json!({ "query": c[1].trim() })));
    }
    if let Some(c) = re(&DOC_WHICH, r"^(?:qual|que|em\s+qual|algum)\s+(?:documento|arquivo)\s+(?:que\s+)?(?:fala|falava|tem|tinha|menciona|mencionava|cita|citava)\s+(?:sobre\s+|de\s+|do\s+|da\s+)?(.{2,})$").captures(t) {
        return Some(one("docs.search", json!({ "query": c[1].trim() })));
    }
    if let Some(c) = re(&DOC_INDEX, r"^(?:indexa|indexe|indexar)\s+(?:a\s+pasta\s+|os\s+|as\s+|meus\s+|minhas\s+)?(.+)$").captures(t) {
        let folder = folder_of(&c[1]).filter(|f| f != "home")?;
        return Some(one("docs.index", json!({ "folder": folder })));
    }

    // ── o que foi copiado ─────────────────────────────────────────────────
    static TRANSLATE: OnceLock<Regex> = OnceLock::new();
    static SUMMARIZE: OnceLock<Regex> = OnceLock::new();
    static FIX: OnceLock<Regex> = OnceLock::new();
    const BACK: &str = r"(\s+e\s+(?:copia|cola|coloca|p[oõ]e|joga|bota)(?:\s+(?:de\s+volta|no\s+lugar))?(?:\s+n[oa]\s+(?:clipboard|[aá]rea\s+de\s+transfer[eê]ncia))?)?";
    if let Some(c) = re(
        &TRANSLATE,
        &format!(r"^(?:traduz|traduza|traduzir|translate)\s+{COPIED}(?:\s+(?:pro|pra|para\s+o|para|em)\s+(\p{{L}}+))?{BACK}$"),
    )
    .captures(t)
    {
        let lang = c.get(1).map(|m| m.as_str().to_string()).unwrap_or_else(|| "auto".into());
        return Some(one("clipboard.transform", json!({ "op": "translate", "lang": lang, "copy_back": c.get(2).is_some() })));
    }
    if let Some(c) = re(&SUMMARIZE, &format!(r"^(?:resume|resuma|resumir|faz\s+um\s+resumo\s+d)\s*{COPIED}{BACK}$")).captures(t) {
        return Some(one("clipboard.transform", json!({ "op": "summarize", "copy_back": c.get(1).is_some() })));
    }
    if let Some(c) = re(&FIX, &format!(r"^(?:corrige|corrija|corrigir|revisa|revise|arruma\s+o\s+portugu[eê]s\s+d)\s*{COPIED}{BACK}$")).captures(t) {
        return Some(one("clipboard.transform", json!({ "op": "fix", "copy_back": c.get(1).is_some() })));
    }
    None
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Nome curto da pasta do resultado ("Downloads").
fn folder_label(result: &Value) -> String {
    result["folder"].as_str().map(|f| file_name(f.trim_end_matches(['\\', '/'])).to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "pasta".into())
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// Resposta montada pelo código a partir do resultado (sem modelo).
pub fn render(action: &str, result: &Value) -> Option<String> {
    let n = |k: &str| result[k].as_u64().unwrap_or(0);
    let s = |k: &str| result[k].as_str().unwrap_or("").to_string();
    match action {
        "file.organize" => {
            let folder = folder_label(result);
            if result["nothing"] == true || n("moved") == 0 {
                let waiting = result["skipped"].as_array().map(|a| a.len() as u64).unwrap_or(0);
                return Some(if waiting > 0 {
                    format!("Nada para organizar em {folder} agora: {} baixando, aberto(s) ou mexido(s) há pouco.", plural(waiting, "arquivo está", "arquivos estão"))
                } else {
                    format!("{folder} já está organizada: não tem arquivo solto.")
                });
            }
            let parts: Vec<String> = result["categories"]
                .as_array()
                .map(|a| a.iter().map(|c| format!("{} em {}", c["count"], c["category"].as_str().unwrap_or(""))).collect())
                .unwrap_or_default();
            let mut out = format!("Pronto! Organizei {} de {folder}: {}.", plural(n("moved"), "arquivo", "arquivos"), parts.join(", "));
            let skipped = result["skipped"].as_array().map(|a| a.len() as u64).unwrap_or(0);
            if skipped > 0 {
                out.push_str(&format!(" {} ficaram onde estavam (baixando, abertos ou mudaram).", plural(skipped, "arquivo", "arquivos")));
            }
            out.push_str(" Se não gostar, é só dizer “desfaz a organização”.");
            Some(out)
        }
        "file.organize_undo" => Some(if result["nothing"] == true {
            "Não tem nenhuma organização recente para desfazer.".into()
        } else {
            let mut out = format!("Desfeito! Devolvi {} para {}.", plural(n("restored"), "arquivo", "arquivos"), folder_label(result));
            if n("missing") > 0 {
                out.push_str(&format!(" {} não estava(m) mais na pasta organizada.", plural(n("missing"), "arquivo", "arquivos")));
            }
            out
        }),
        "file.duplicates" => {
            let folder = folder_label(result);
            let groups = result["groups"].as_array().cloned().unwrap_or_default();
            if groups.is_empty() {
                return Some(format!("Não achei arquivos duplicados em {folder}."));
            }
            let mut out = format!(
                "Achei {} em {folder} — dá para liberar {}:",
                plural(n("group_count"), "arquivo repetido", "arquivos repetidos"),
                s("reclaimable_text")
            );
            for g in groups.iter().take(5) {
                let files: Vec<&str> = g["files"].as_array().map(|f| f.iter().filter_map(|p| p.as_str()).collect()).unwrap_or_default();
                let keep = g["keep"].as_str().unwrap_or("");
                out.push_str(&format!("\n- {} ({} cópias, {} cada)", file_name(keep), g["copies"], g["size_text"].as_str().unwrap_or("")));
                for f in files.iter().filter(|f| **f != keep).take(3) {
                    out.push_str(&format!("\n    cópia: {f}"));
                }
            }
            if n("group_count") > 5 {
                out.push_str(&format!("\n… e mais {}.", n("group_count") - 5));
            }
            out.push_str("\nQuer que eu mande as cópias extras para a Lixeira? É só dizer “apaga os duplicados” (fica uma de cada, e eu mostro antes).");
            Some(out)
        }
        "file.dedupe" => Some(if result["nothing"] == true {
            format!("Não achei arquivos duplicados em {}.", folder_label(result))
        } else {
            let mut out = format!("Pronto! Mandei {} para a Lixeira e liberei {}.", plural(n("trashed"), "cópia", "cópias"), s("freed_text"));
            if n("kept_back") > 0 {
                out.push_str(&format!(" {} ficou(aram) porque mudou(aram) desde a prévia.", plural(n("kept_back"), "arquivo", "arquivos")));
            }
            out
        }),
        "file.big" => {
            let files = result["files"].as_array().cloned().unwrap_or_default();
            let folder = folder_label(result);
            if files.is_empty() {
                return Some(format!("Não achei arquivos em {folder}."));
            }
            let mut out = format!("Os maiores arquivos em {folder}:");
            for (i, f) in files.iter().enumerate() {
                let path = f["path"].as_str().unwrap_or("");
                out.push_str(&format!("\n{}. {} — {}\n    {path}", i + 1, file_name(path), f["size_text"].as_str().unwrap_or("")));
            }
            if result["truncated"] == true {
                out.push_str("\n(A pasta é enorme: olhei só uma parte.)");
            }
            Some(out)
        }
        "docs.search" => {
            let hits = result["results"].as_array().cloned().unwrap_or_default();
            if hits.is_empty() {
                return Some(if n("indexed_files") == 0 {
                    "Ainda não indexei nenhum documento. Escolha as pastas em Configurações → Documentos, ou diga “indexa a pasta Documentos”.".into()
                } else {
                    "Não achei nada sobre isso nos seus documentos.".into()
                });
            }
            let mut out = String::from("Achei isto nos seus documentos:");
            for (i, h) in hits.iter().enumerate() {
                out.push_str(&format!(
                    "\n{}. {} — “{}”\n    {}",
                    i + 1,
                    h["name"].as_str().unwrap_or(""),
                    h["snippet"].as_str().unwrap_or(""),
                    h["path"].as_str().unwrap_or("")
                ));
            }
            out.push_str("\nQuer um resumo? Diga “resume o primeiro” (ou o nome do documento).");
            if result["mode"] == "keyword" {
                out.push_str("\n(Busca por palavras: com o modelo de embeddings ela também acha pelo sentido — dá para baixar em Configurações → Documentos.)");
            }
            Some(out)
        }
        "docs.index" => {
            let mut out = if result["already_running"] == true {
                "Já estou indexando seus documentos; dá para acompanhar em Configurações → Documentos.".to_string()
            } else {
                format!(
                    "Comecei a indexar {}. Vou lendo os documentos em segundo plano; dá para acompanhar em Configurações → Documentos.",
                    result["folders"].as_array().map(|f| f.iter().filter_map(|x| x.as_str()).map(|x| file_name(x.trim_end_matches(['\\', '/']))).collect::<Vec<_>>().join(", ")).unwrap_or_default()
                )
            };
            if result["semantic"] == false {
                out.push_str(" Sem o modelo de embeddings a busca vai ser por palavras (dá para baixar o modelo lá também).");
            }
            Some(out)
        }
        "clipboard.transform" => {
            let text = s("text");
            let head = match result["op"].as_str().unwrap_or("") {
                "translate" => "Tradução:",
                "summarize" => "Resumo do que você copiou:",
                "fix" => "Texto corrigido:",
                _ => "Pronto:",
            };
            let mut out = format!("{head}\n\n{text}");
            if result["copied_back"] == true {
                out.push_str("\n\n(Já deixei na área de transferência: é só colar.)");
            } else if result["op"] != "summarize" {
                out.push_str("\n\n(Se quiser isso na área de transferência, peça de novo com “e copia de volta”.)");
            }
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_call(text: &str) -> Option<(String, Value)> {
        crate::shortcut::parse(text).map(|s| (s.calls[0].action.clone(), s.calls[0].params.clone()))
    }

    #[test]
    fn organizing_folders() {
        assert_eq!(one_call("organiza meus downloads"), Some(("file.organize".into(), json!({ "folder": "Downloads" }))));
        assert_eq!(one_call("arruma a pasta de downloads por favor").unwrap().1, json!({ "folder": "Downloads" }));
        assert_eq!(one_call("organiza minha área de trabalho").unwrap().1, json!({ "folder": "Área de Trabalho" }));
        assert_eq!(one_call("desfaz a organização").unwrap().0, "file.organize_undo");
        assert_eq!(one_call("desfaz a organização dos downloads").unwrap().0, "file.organize_undo");
        // Janelas continuam com o modelo / outros atalhos.
        assert_eq!(one_call("organiza as janelas"), None);
        assert_eq!(one_call("arruma o pc todo"), None, "organizar a pasta do usuário inteira não");
    }

    #[test]
    fn duplicates_and_big_files() {
        assert_eq!(one_call("acha arquivos duplicados"), Some(("file.duplicates".into(), json!({}))));
        assert_eq!(one_call("tem arquivos repetidos nos downloads?").unwrap().1, json!({ "folder": "Downloads" }));
        assert_eq!(one_call("arquivos duplicados no pc todo").unwrap().1, json!({ "folder": "home" }));
        assert_eq!(one_call("apaga os duplicados"), Some(("file.dedupe".into(), json!({}))));
        assert_eq!(one_call("remove os arquivos duplicados da pasta documentos").unwrap().1, json!({ "folder": "Documentos" }));
        assert_eq!(one_call("quais os maiores arquivos do pc?"), Some(("file.big".into(), json!({ "folder": "home" }))));
        assert_eq!(one_call("mostra os 5 maiores arquivos nos downloads").unwrap().1, json!({ "folder": "Downloads", "limit": 5 }));
        assert_eq!(one_call("o que tá ocupando espaço").unwrap().0, "file.big");
        assert_eq!(one_call("acha duplicados no projeto x"), None, "pasta desconhecida vai pro modelo");
    }

    #[test]
    fn documents_and_clipboard() {
        assert_eq!(one_call("procura nos meus documentos sobre contrato de aluguel"), Some(("docs.search".into(), json!({ "query": "contrato de aluguel" }))));
        assert_eq!(one_call("qual documento fala sobre férias?").unwrap().1, json!({ "query": "férias" }));
        assert_eq!(one_call("indexa a pasta documentos"), Some(("docs.index".into(), json!({ "folder": "Documentos" }))));
        assert_eq!(
            one_call("traduz o que eu copiei"),
            Some(("clipboard.transform".into(), json!({ "op": "translate", "lang": "auto", "copy_back": false })))
        );
        assert_eq!(
            one_call("traduz o texto copiado pro inglês e copia de volta").unwrap().1,
            json!({ "op": "translate", "lang": "inglês", "copy_back": true })
        );
        assert_eq!(one_call("resume o que eu copiei").unwrap().1, json!({ "op": "summarize", "copy_back": false }));
        assert_eq!(
            one_call("corrige o texto que copiei e copia de volta").unwrap().1,
            json!({ "op": "fix", "copy_back": true })
        );
        assert_eq!(one_call("corrige o que eu copiei").unwrap().1, json!({ "op": "fix", "copy_back": false }));
        assert_eq!(one_call("o que eu copiei?"), None, "só ler fica com o modelo (clipboard.get)");
    }

    #[test]
    fn replies_are_rendered_by_code() {
        let r = json!({ "folder": "C:\\x\\Downloads", "moved": 3, "categories": [{ "category": "Imagens", "count": 2 }, { "category": "Outros", "count": 1 }], "skipped": [] });
        assert_eq!(
            render("file.organize", &r).unwrap(),
            "Pronto! Organizei 3 arquivos de Downloads: 2 em Imagens, 1 em Outros. Se não gostar, é só dizer “desfaz a organização”."
        );
        assert_eq!(render("file.organize", &json!({ "folder": "C:\\x\\Downloads", "nothing": true, "skipped": [] })).unwrap(), "Downloads já está organizada: não tem arquivo solto.");
        assert_eq!(render("file.organize_undo", &json!({ "folder": "C:\\x\\Downloads", "restored": 3, "missing": 0 })).unwrap(), "Desfeito! Devolvi 3 arquivos para Downloads.");
        assert_eq!(render("file.duplicates", &json!({ "folder": "C:\\d", "groups": [] })).unwrap(), "Não achei arquivos duplicados em d.");
        let dup = json!({ "folder": "C:\\d", "group_count": 1, "reclaimable_text": "2,0 MB", "groups": [{ "keep": "C:\\d\\a.jpg", "copies": 2, "size_text": "2,0 MB", "files": ["C:\\d\\a.jpg", "C:\\d\\a (1).jpg"] }] });
        let text = render("file.duplicates", &dup).unwrap();
        assert!(text.starts_with("Achei 1 arquivo repetido em d — dá para liberar 2,0 MB:\n- a.jpg (2 cópias, 2,0 MB cada)\n    cópia: C:\\d\\a (1).jpg"), "{text}");
        assert_eq!(render("file.dedupe", &json!({ "trashed": 2, "freed_text": "4,0 MB", "kept_back": 0 })).unwrap(), "Pronto! Mandei 2 cópias para a Lixeira e liberei 4,0 MB.");
        let tr = json!({ "op": "translate", "text": "Hello", "copied_back": true });
        assert_eq!(render("clipboard.transform", &tr).unwrap(), "Tradução:\n\nHello\n\n(Já deixei na área de transferência: é só colar.)");
        assert!(render("docs.search", &json!({ "results": [], "indexed_files": 0 })).unwrap().starts_with("Ainda não indexei"));
    }
}
