//! Índice dos documentos do usuário (pastas escolhidas por ele): cada
//! documento vira trechos com texto (FTS5, sem acento) e, quando há modelo de
//! embeddings no Ollama, um vetor por trecho. A busca é decidida aqui, pelo
//! código: similaridade de cosseno + bônus para quem também casa por palavra;
//! sem vetores, só por palavra (BM25).

use super::{now_ms, Memory};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;

pub(crate) fn init(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS doc_files (
             path       TEXT PRIMARY KEY,
             folder     TEXT NOT NULL,
             mtime      INTEGER NOT NULL,
             size       INTEGER NOT NULL,
             chunks     INTEGER NOT NULL,
             embedded   INTEGER NOT NULL DEFAULT 0,
             indexed_ms INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_doc_files_folder ON doc_files(folder);
         CREATE TABLE IF NOT EXISTS doc_chunks (
             id        INTEGER PRIMARY KEY AUTOINCREMENT,
             path      TEXT NOT NULL,
             ord       INTEGER NOT NULL,
             text      TEXT NOT NULL,
             embedding BLOB
         );
         CREATE INDEX IF NOT EXISTS idx_doc_chunks_path ON doc_chunks(path);",
    );
    let _ = conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS doc_chunks_fts USING fts5(
             text, content='doc_chunks', content_rowid='id',
             tokenize='unicode61 remove_diacritics 2'
         );
         CREATE TRIGGER IF NOT EXISTS doc_chunks_ai AFTER INSERT ON doc_chunks BEGIN
             INSERT INTO doc_chunks_fts(rowid, text) VALUES (new.id, new.text);
         END;
         CREATE TRIGGER IF NOT EXISTS doc_chunks_ad AFTER DELETE ON doc_chunks BEGIN
             INSERT INTO doc_chunks_fts(doc_chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
         END;",
    );
}

/// O que o índice sabe de um arquivo (para reindexar só o que mudou).
#[derive(Debug, Clone, PartialEq)]
pub struct DocFile {
    pub path: String,
    pub mtime: i64,
    pub size: i64,
    /// Todos os trechos têm vetor.
    pub embedded: bool,
}

/// Um documento achado, com o trecho que mais casou.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DocHit {
    pub path: String,
    pub text: String,
    pub score: f32,
    /// Achado por significado (vetor), não só por palavra.
    pub semantic: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct DocStats {
    pub files: usize,
    pub chunks: usize,
    pub embedded_chunks: usize,
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Palavras que não ajudam a achar documento.
const STOPWORDS: &[&str] = &[
    "a", "o", "e", "de", "da", "do", "das", "dos", "que", "sobre", "para", "pra", "com", "uma", "um", "os", "as", "em",
    "no", "na", "nos", "nas", "por", "meu", "minha", "meus", "minhas", "fala", "falava", "falando", "documento",
    "documentos", "arquivo", "arquivos", "tem", "tinha", "algum", "alguma", "qual", "onde", "isso", "aquele", "aquela",
];

/// Consulta FTS5 tolerante: qualquer palavra relevante (OU), com prefixo para
/// pegar plural e variações ("contrato" acha "contratos").
fn doc_fts_query(q: &str) -> Option<String> {
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 2 && !STOPWORDS.contains(&w.as_str()))
        .map(|w| {
            let stem = if w.chars().count() >= 5 { w.trim_end_matches('s').to_string() } else { w };
            format!("\"{}\"*", stem.replace('"', ""))
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

/// Semelhança mínima para um trecho achado só pelo vetor contar.
const MIN_SEMANTIC: f32 = 0.35;
/// Bônus de quem também casa por palavra.
const KEYWORD_BONUS: f32 = 0.12;

impl Memory {
    pub fn doc_file(&self, path: &str) -> Option<DocFile> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT path, mtime, size, embedded FROM doc_files WHERE path = ?1",
            params![path],
            |r| Ok(DocFile { path: r.get(0)?, mtime: r.get(1)?, size: r.get(2)?, embedded: r.get::<_, i64>(3)? != 0 }),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Arquivos indexados a partir de uma pasta escolhida.
    pub fn doc_paths_in(&self, folder: &str) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let Ok(mut stmt) = conn.prepare("SELECT path FROM doc_files WHERE folder = ?1") else { return Vec::new() };
        stmt.query_map(params![folder], |r| r.get(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Troca os trechos de um documento (numa transação só). `embeddings`,
    /// se vier, tem um vetor por trecho.
    pub fn doc_store(
        &self,
        path: &str,
        folder: &str,
        mtime: i64,
        size: i64,
        chunks: &[String],
        embeddings: Option<&[Vec<f32>]>,
    ) -> bool {
        let embeddings = embeddings.filter(|e| e.len() == chunks.len());
        let mut conn = self.conn.lock().unwrap();
        let mut run = || -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM doc_chunks WHERE path = ?1", params![path])?;
            {
                let mut insert = tx.prepare("INSERT INTO doc_chunks (path, ord, text, embedding) VALUES (?1, ?2, ?3, ?4)")?;
                for (i, text) in chunks.iter().enumerate() {
                    let blob = embeddings.map(|e| to_blob(&e[i]));
                    insert.execute(params![path, i as i64, text, blob])?;
                }
            }
            tx.execute(
                "INSERT INTO doc_files (path, folder, mtime, size, chunks, embedded, indexed_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(path) DO UPDATE SET folder = excluded.folder, mtime = excluded.mtime,
                     size = excluded.size, chunks = excluded.chunks, embedded = excluded.embedded,
                     indexed_ms = excluded.indexed_ms",
                params![path, folder, mtime, size, chunks.len() as i64, embeddings.is_some() as i64, now_ms()],
            )?;
            tx.commit()
        };
        run().map_err(|e| tracing::warn!(error = %e, "não indexei o documento")).is_ok()
    }

    pub fn doc_remove(&self, path: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM doc_chunks WHERE path = ?1", params![path]);
        let _ = conn.execute("DELETE FROM doc_files WHERE path = ?1", params![path]);
    }

    /// Tira do índice tudo o que veio de uma pasta.
    pub fn doc_remove_folder(&self, folder: &str) {
        for path in self.doc_paths_in(folder) {
            self.doc_remove(&path);
        }
    }

    pub fn doc_stats(&self) -> DocStats {
        let conn = self.conn.lock().unwrap();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize;
        DocStats {
            files: count("SELECT count(*) FROM doc_files"),
            chunks: count("SELECT count(*) FROM doc_chunks"),
            embedded_chunks: count("SELECT count(*) FROM doc_chunks WHERE embedding IS NOT NULL"),
        }
    }

    /// Os documentos que mais respondem à pergunta, um trecho por documento.
    /// Com `query_vec`, ordena por significado (cosseno) e dá bônus a quem
    /// também tem as palavras; sem ele, só por palavra.
    pub fn doc_search(&self, query: &str, query_vec: Option<&[f32]>, limit: usize) -> Vec<DocHit> {
        const POOL: usize = 60;
        let limit = limit.clamp(1, 20);
        let conn = self.conn.lock().unwrap();
        // id → (pontuação, achado por vetor)
        let mut scored: HashMap<i64, (f32, bool)> = HashMap::new();
        if let Some(qv) = query_vec {
            if let Ok(mut stmt) = conn.prepare("SELECT id, embedding FROM doc_chunks WHERE embedding IS NOT NULL") {
                let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)));
                if let Ok(rows) = rows {
                    let mut all: Vec<(i64, f32)> =
                        rows.filter_map(|r| r.ok()).map(|(id, blob)| (id, cosine(qv, &from_blob(&blob)))).collect();
                    all.sort_by(|a, b| b.1.total_cmp(&a.1));
                    for (id, s) in all.into_iter().take(POOL) {
                        scored.insert(id, (s, true));
                    }
                }
            }
        }
        if let Some(fts) = doc_fts_query(query) {
            let ids: Vec<i64> = conn
                .prepare("SELECT rowid FROM doc_chunks_fts WHERE doc_chunks_fts MATCH ?1 ORDER BY bm25(doc_chunks_fts) LIMIT ?2")
                .and_then(|mut s| Ok(s.query_map(params![fts, POOL as i64], |r| r.get(0))?.filter_map(|r| r.ok()).collect()))
                .unwrap_or_default();
            let n = ids.len().max(1) as f32;
            for (rank, id) in ids.into_iter().enumerate() {
                let by_rank = 1.0 - rank as f32 / n;
                match scored.get_mut(&id) {
                    Some((s, _)) => *s += KEYWORD_BONUS * (0.5 + by_rank / 2.0),
                    // Só por palavra: com vetores na busca, fica abaixo dos bons
                    // achados por significado; sem vetores, a ordem do BM25.
                    None => {
                        let s = if query_vec.is_some() { 0.3 + KEYWORD_BONUS * by_rank } else { by_rank };
                        scored.insert(id, (s, false));
                    }
                }
            }
        }
        let mut ranked: Vec<(i64, f32, bool)> = scored
            .into_iter()
            .filter(|(_, (s, semantic))| !*semantic || *s >= MIN_SEMANTIC)
            .map(|(id, (s, sem))| (id, s, sem))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut out: Vec<DocHit> = Vec::new();
        for (id, score, semantic) in ranked {
            let row = conn.query_row("SELECT path, text FROM doc_chunks WHERE id = ?1", params![id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            });
            let Ok((path, text)) = row else { continue };
            if out.iter().any(|h| h.path == path) {
                continue;
            }
            out.push(DocHit { path, text, score, semantic });
            if out.len() >= limit {
                break;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keyword_search_without_vectors() {
        let m = Memory::in_memory().unwrap();
        assert!(m.doc_store("C:/d/contrato.pdf", "C:/d", 1, 10, &chunks(&["Contrato de aluguel do apartamento", "Cláusula de multa"]), None));
        assert!(m.doc_store("C:/d/receita.md", "C:/d", 1, 10, &chunks(&["Receita de bolo de cenoura"]), None));
        let hits = m.doc_search("contratos de aluguel", None, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "C:/d/contrato.pdf");
        assert!(!hits[0].semantic);
        // Sem acento casa com acento.
        assert_eq!(m.doc_search("clausula", None, 5)[0].path, "C:/d/contrato.pdf");
        assert!(m.doc_search("de da do", None, 5).is_empty(), "só palavras vazias");
        assert_eq!(m.doc_stats(), DocStats { files: 2, chunks: 3, embedded_chunks: 0 });
    }

    #[test]
    fn vectors_rank_by_meaning_and_reindex_replaces() {
        let m = Memory::in_memory().unwrap();
        let e = |v: [f32; 3]| v.to_vec();
        m.doc_store("a.txt", "f", 1, 1, &chunks(&["férias na praia"]), Some(&[e([1.0, 0.0, 0.0])]));
        m.doc_store("b.txt", "f", 1, 1, &chunks(&["planilha de gastos"]), Some(&[e([0.0, 1.0, 0.0])]));
        m.doc_store("c.txt", "f", 1, 1, &chunks(&["viagem ao litoral"]), Some(&[e([0.9, 0.1, 0.0])]));
        let hits = m.doc_search("descanso", Some(&[1.0, 0.0, 0.0]), 5);
        let paths: Vec<_> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "c.txt"], "o de gastos não parece");
        assert!(hits[0].semantic);
        // Casar também por palavra sobe na lista.
        let hits = m.doc_search("viagem", Some(&[1.0, 0.0, 0.0]), 5);
        assert_eq!(hits[0].path, "c.txt");

        assert_eq!(m.doc_file("a.txt").unwrap().embedded, true);
        m.doc_store("a.txt", "f", 2, 5, &chunks(&["outro texto", "mais"]), None);
        assert_eq!(m.doc_file("a.txt").unwrap(), DocFile { path: "a.txt".into(), mtime: 2, size: 5, embedded: false });
        assert_eq!(m.doc_stats().chunks, 4);
        m.doc_remove_folder("f");
        assert_eq!(m.doc_stats(), DocStats::default());
        assert!(m.doc_search("viagem", None, 5).is_empty(), "FTS também limpo");
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
    }
}
