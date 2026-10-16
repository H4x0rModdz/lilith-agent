//! Texto de documentos (txt, md, pdf, docx) para a busca nos documentos e
//! para `file.read`. Extração em Rust puro e leve: PDF pelo `lopdf`, DOCX
//! abrindo o zip e lendo os parágrafos do `word/document.xml`.

use crate::WinError;
use regex::Regex;
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

/// Tipos que entram no índice de documentos.
pub const DOC_EXTS: &[&str] = &["txt", "md", "markdown", "pdf", "docx"];
/// Maior arquivo aceito para extrair texto.
pub const MAX_DOC_BYTES: u64 = 40 * 1024 * 1024;
/// Texto devolvido por documento, no máximo.
const MAX_TEXT_CHARS: usize = 400_000;

fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

/// É um documento que precisa de extração (não é texto puro)?
pub fn needs_extraction(path: &Path) -> bool {
    matches!(ext_of(path).as_str(), "pdf" | "docx")
}

/// PDF ou DOCX de verdade (assinatura no começo do arquivo), não um texto
/// com a extensão trocada.
pub fn has_document_signature(path: &Path) -> bool {
    let mut head = [0u8; 4];
    let ok = std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok();
    ok && match ext_of(path).as_str() {
        "pdf" => &head == b"%PDF",
        "docx" => &head[..2] == b"PK",
        _ => false,
    }
}

pub fn is_document(path: &Path) -> bool {
    DOC_EXTS.contains(&ext_of(path).as_str())
}

/// O texto do documento, com parágrafos separados por linha.
pub fn extract_text(path: &Path) -> Result<String, WinError> {
    let meta = std::fs::metadata(path)?;
    if meta.len() > MAX_DOC_BYTES {
        return Err(WinError::InvalidParams(format!("documento grande demais ({} bytes)", meta.len())));
    }
    let text = match ext_of(path).as_str() {
        "pdf" => pdf_text(path)?,
        "docx" => docx_text(path)?,
        _ => decode_text(&std::fs::read(path)?),
    };
    Ok(text.chars().take(MAX_TEXT_CHARS).collect())
}

/// UTF-8 (com ou sem BOM) ou UTF-16 com BOM, como o Bloco de Notas salva.
fn decode_text(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        }
        [0xFE, 0xFF, rest @ ..] => {
            let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn pdf_text(path: &Path) -> Result<String, WinError> {
    let path = path.to_path_buf();
    // Um PDF malformado não pode derrubar quem pediu (em build de debug o
    // pânico é pego aqui; ver o aviso no relatório sobre `panic = "abort"`).
    let run = move || -> Result<String, WinError> {
        let doc = lopdf::Document::load(&path).map_err(|e| WinError::Failed(format!("PDF ilegível: {e}")))?;
        if doc.is_encrypted() {
            return Err(WinError::Unavailable("PDF protegido por senha".into()));
        }
        let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
        let mut out = String::new();
        // Página por página: uma página ruim não perde o documento inteiro.
        for p in pages {
            if let Ok(t) = doc.extract_text(&[p]) {
                out.push_str(&t);
                out.push('\n');
            }
            if out.len() > MAX_TEXT_CHARS * 4 {
                break;
            }
        }
        Ok(out)
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(r) => r,
        Err(_) => Err(WinError::Failed("PDF ilegível".into())),
    }
}

fn docx_text(path: &Path) -> Result<String, WinError> {
    const MAX_XML: u64 = 50 * 1024 * 1024;
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| WinError::Failed(format!("DOCX ilegível: {e}")))?;
    let entry = zip
        .by_name("word/document.xml")
        .map_err(|e| WinError::Failed(format!("DOCX sem texto: {e}")))?;
    let mut xml = String::new();
    entry
        .take(MAX_XML)
        .read_to_string(&mut xml)
        .map_err(|e| WinError::Failed(format!("DOCX ilegível: {e}")))?;
    Ok(docx_xml_text(&xml))
}

/// Texto dos `<w:t>`, com quebra de linha no fim de cada parágrafo.
pub(crate) fn docx_xml_text(xml: &str) -> String {
    static TOKENS: OnceLock<Regex> = OnceLock::new();
    let re = TOKENS.get_or_init(|| {
        Regex::new(r"<w:t(?:\s[^>]*)?>([^<]*)</w:t>|</w:p>|<w:tab\s*/>|<w:br\s*/>").expect("regex do docx")
    });
    let mut out = String::new();
    for c in re.captures_iter(xml) {
        match c.get(1) {
            Some(t) => out.push_str(&unescape_xml(t.as_str())),
            None if c[0].starts_with("<w:tab") => out.push('\t'),
            None => out.push('\n'),
        }
    }
    out
}

fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(end) = after.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Divide o texto em trechos de ~`size` caracteres com `overlap` de
/// sobreposição, cortando de preferência em fim de parágrafo, frase ou
/// palavra. Espaços repetidos viram um só.
pub fn chunks(text: &str, size: usize, overlap: usize, max_chunks: usize) -> Vec<String> {
    let clean: String = text
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let chars: Vec<char> = clean.chars().collect();
    let size = size.max(50);
    let overlap = overlap.min(size / 2);
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() && out.len() < max_chunks {
        let hard_end = (start + size).min(chars.len());
        let mut end = hard_end;
        if hard_end < chars.len() {
            // Melhor ponto de corte na segunda metade do trecho.
            let window = &chars[start + size / 2..hard_end];
            let cut = window
                .iter()
                .rposition(|c| *c == '\n')
                .or_else(|| window.windows(2).rposition(|w| matches!(w[0], '.' | '!' | '?') && w[1] == ' ').map(|i| i + 1))
                .or_else(|| window.iter().rposition(|c| *c == ' '));
            if let Some(i) = cut {
                end = start + size / 2 + i + 1;
            }
        }
        let piece: String = chars[start..end].iter().collect::<String>().trim().to_string();
        if !piece.is_empty() {
            out.push(piece);
        }
        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(overlap).max(start + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn docx_paragraphs_and_entities() {
        let xml = r#"<w:document><w:body><w:p><w:r><w:t>Olá</w:t></w:r><w:r><w:t xml:space="preserve"> mundo &amp; cia</w:t></w:r></w:p><w:p><w:r><w:tab/><w:t>Segundo &#233; &#xE7;</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(docx_xml_text(xml), "Olá mundo & cia\n\tSegundo é ç\n");
    }

    #[test]
    fn extracts_docx_txt_and_utf16() {
        let dir = tempfile::tempdir().unwrap();
        let docx = dir.path().join("a.docx");
        {
            let f = std::fs::File::create(&docx).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            z.start_file("word/document.xml", opts).unwrap();
            z.write_all(br#"<w:document><w:body><w:p><w:r><w:t>Relatorio de ferias</w:t></w:r></w:p></w:body></w:document>"#).unwrap();
            z.finish().unwrap();
        }
        assert_eq!(extract_text(&docx).unwrap().trim(), "Relatorio de ferias");

        let txt = dir.path().join("b.txt");
        std::fs::write(&txt, "\u{feff}linha um\nlinha dois").unwrap();
        assert_eq!(extract_text(&txt).unwrap(), "linha um\nlinha dois");

        let utf16 = dir.path().join("c.txt");
        let mut bytes = vec![0xFF, 0xFE];
        for u in "ação".encode_utf16() {
            bytes.extend(u.to_le_bytes());
        }
        std::fs::write(&utf16, bytes).unwrap();
        assert_eq!(extract_text(&utf16).unwrap(), "ação");

        let bad = dir.path().join("d.pdf");
        std::fs::write(&bad, "não é pdf").unwrap();
        assert!(extract_text(&bad).is_err());
    }

    #[test]
    fn extracts_a_simple_pdf() {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document, Object, Stream};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier" });
        let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 24.into()]),
                Operation::new("Td", vec![100.into(), 600.into()]),
                Operation::new("Tj", vec![Object::string_literal("Contrato de aluguel")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
                "Resources" => resources_id, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contrato.pdf");
        doc.save(&path).unwrap();
        assert!(extract_text(&path).unwrap().contains("Contrato de aluguel"));
    }

    #[test]
    fn chunks_cut_at_boundaries_with_overlap() {
        let text = "Primeira frase do texto. Segunda frase vem aqui. Terceira frase fecha o parágrafo.\n\nOutro parágrafo   com    espaços.";
        let c = chunks(text, 60, 10, 100);
        assert!(c.len() >= 2, "{c:?}");
        assert!(c.iter().all(|p| p.chars().count() <= 60), "{c:?}");
        assert!(c[0].ends_with('.'), "corta em fim de frase: {c:?}");
        assert!(c.last().unwrap().contains("Outro parágrafo com espaços."));
        assert_eq!(chunks("", 100, 10, 10), Vec::<String>::new());
        assert_eq!(chunks(&"x ".repeat(5000), 100, 10, 3).len(), 3, "teto de trechos");
    }
}
