//! Memória de longo prazo da Lilith, num único SQLite
//! (`%APPDATA%\Lilith\lilith.db`):
//!
//! - **conversas** (`conversations` + `messages`): o histórico do chat, com
//!   índice FTS5 para ela procurar em *todas* as conversas
//!   (`memory.search`). Busca portada de `jarvis-os/ai/lilith/src/turn_store.rs`.
//! - **fatos** (`facts`): o que ela sabe sobre o usuário — configurado por
//!   ele no painel ou aprendido nas conversas.
//! - **configurações** (`settings`): comportamento escolhido no painel.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

pub const DEFAULT_TITLE: &str = "Nova conversa";
const MAX_TITLE: usize = 60;

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error(e.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// "user" ou "lilith".
    pub role: String,
    pub text: String,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub renamed: bool,
    pub created_ms: i64,
    pub updated_ms: i64,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub messages: usize,
}

/// Um trecho achado por `search`, com a conversa de onde veio.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub conversation: String,
    pub title: String,
    pub role: String,
    pub text: String,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fact {
    pub key: String,
    pub value: String,
    /// "user" (configurado no painel / pedido explícito) ou "learned".
    pub source: String,
    pub updated_ms: i64,
}

/// Id vindo do frontend: curto e sem nada que vire caminho ou SQL estranho.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

pub struct Memory {
    conn: Mutex<Connection>,
}

impl Memory {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error(e.to_string()))?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS conversations (
                 id         TEXT PRIMARY KEY,
                 title      TEXT NOT NULL,
                 renamed    INTEGER NOT NULL DEFAULT 0,
                 created_ms INTEGER NOT NULL,
                 updated_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages (
                 id           INTEGER PRIMARY KEY AUTOINCREMENT,
                 conversation TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                 role         TEXT NOT NULL,
                 text         TEXT NOT NULL,
                 ts           INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation, id);
             CREATE TABLE IF NOT EXISTS facts (
                 key        TEXT PRIMARY KEY COLLATE NOCASE,
                 value      TEXT NOT NULL,
                 source     TEXT NOT NULL,
                 updated_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS settings (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;
        // Índice FTS5 "external content" sobre messages (como no jarvis-os):
        // só o índice invertido; os triggers mantêm sincronizado. Sem acento.
        let _ = conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                 text, content='messages', content_rowid='id',
                 tokenize='unicode61 remove_diacritics 2'
             );
             CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                 INSERT INTO messages_fts(rowid, text) VALUES (new.id, new.text);
             END;
             CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                 INSERT INTO messages_fts(messages_fts, rowid, text) VALUES ('delete', old.id, old.text);
             END;",
        );
        Ok(Self { conn: Mutex::new(conn) })
    }

    // ── conversas ─────────────────────────────────────────────────────────

    pub fn list(&self) -> Vec<Summary> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT c.id, c.title, c.created_ms, c.updated_ms,
                    (SELECT count(*) FROM messages m WHERE m.conversation = c.id)
             FROM conversations c ORDER BY c.updated_ms DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |r| {
            Ok(Summary {
                id: r.get(0)?,
                title: r.get(1)?,
                created_ms: r.get(2)?,
                updated_ms: r.get(3)?,
                messages: r.get::<_, i64>(4)? as usize,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<Conversation> {
        let conn = self.conn.lock().unwrap();
        let mut c: Conversation = conn
            .query_row(
                "SELECT id, title, renamed, created_ms, updated_ms FROM conversations WHERE id = ?1",
                params![id],
                |r| {
                    Ok(Conversation {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        renamed: r.get::<_, i64>(2)? != 0,
                        created_ms: r.get(3)?,
                        updated_ms: r.get(4)?,
                        messages: Vec::new(),
                    })
                },
            )
            .optional()
            .ok()
            .flatten()?;
        let mut stmt = conn
            .prepare("SELECT role, text, ts FROM messages WHERE conversation = ?1 ORDER BY id")
            .ok()?;
        c.messages = stmt
            .query_map(params![id], |r| Ok(Message { role: r.get(0)?, text: r.get(1)?, ts: r.get(2)? }))
            .ok()?
            .filter_map(|r| r.ok())
            .collect();
        Some(c)
    }

    pub fn title(&self, id: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT title FROM conversations WHERE id = ?1", params![id], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    fn ensure(conn: &Connection, id: &str, ts: i64) -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO conversations (id, title, renamed, created_ms, updated_ms)
             VALUES (?1, ?2, 0, ?3, ?3)",
            params![id, DEFAULT_TITLE, ts],
        )?;
        Ok(())
    }

    /// Acrescenta uma fala (cria a conversa se preciso). Na primeira fala do
    /// usuário, o título automático vira o começo dela (se não foi
    /// renomeada). Devolve o novo título se mudou.
    pub fn append(&self, id: &str, role: &str, text: &str) -> Option<String> {
        let ts = now_ms();
        let conn = self.conn.lock().unwrap();
        let run = || -> Result<Option<String>> {
            Self::ensure(&conn, id, ts)?;
            let had_user: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation = ?1 AND role = 'user')",
                params![id],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT INTO messages (conversation, role, text, ts) VALUES (?1, ?2, ?3, ?4)",
                params![id, role, text, ts],
            )?;
            conn.execute("UPDATE conversations SET updated_ms = ?2 WHERE id = ?1", params![id, ts])?;
            if role == "user" && !had_user {
                let renamed: bool =
                    conn.query_row("SELECT renamed FROM conversations WHERE id = ?1", params![id], |r| {
                        r.get::<_, i64>(0).map(|v| v != 0)
                    })?;
                if !renamed {
                    let title = auto_title(text);
                    conn.execute("UPDATE conversations SET title = ?2 WHERE id = ?1", params![id, title])?;
                    return Ok(Some(title));
                }
            }
            Ok(None)
        };
        run().unwrap_or_else(|e| {
            tracing::warn!(error = %e, id, "não salvei a fala");
            None
        })
    }

    /// Renomeia (cria vazia se ainda não existir). Devolve o título final.
    pub fn rename(&self, id: &str, title: &str) -> String {
        let title = clean_title(title);
        let conn = self.conn.lock().unwrap();
        let _ = Self::ensure(&conn, id, now_ms());
        let _ = conn.execute(
            "UPDATE conversations SET title = ?2, renamed = 1 WHERE id = ?1",
            params![id, title],
        );
        title
    }

    pub fn delete(&self, id: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM messages WHERE conversation = ?1", params![id]);
        let _ = conn.execute("DELETE FROM conversations WHERE id = ?1", params![id]);
    }

    /// Pares (fala do usuário, resposta) para reabrir a memória da Lilith.
    pub fn turns(&self, id: &str) -> Vec<(String, String)> {
        let Some(c) = self.get(id) else { return Vec::new() };
        let mut out = Vec::new();
        let mut pending: Option<String> = None;
        for m in c.messages {
            match m.role.as_str() {
                "user" => pending = Some(m.text),
                "lilith" => {
                    if let Some(u) = pending.take() {
                        out.push((u, m.text));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Procura em todas as conversas. FTS5 (BM25, sem acento, todas as
    /// palavras) quando disponível; LIKE como reserva. Mais relevante
    /// primeiro. `exclude` deixa de fora a conversa atual.
    pub fn search(&self, query: &str, limit: usize, exclude: Option<&str>) -> Vec<Hit> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }
        let conn = self.conn.lock().unwrap();
        let exclude = exclude.unwrap_or("");
        let limit = limit.clamp(1, 50) as i64;
        let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Hit> {
            Ok(Hit {
                conversation: r.get(0)?,
                title: r.get(1)?,
                role: r.get(2)?,
                text: r.get(3)?,
                ts: r.get(4)?,
            })
        };
        let fts = conn
            .prepare(
                "SELECT m.conversation, c.title, m.role, m.text, m.ts
                 FROM messages_fts f
                 JOIN messages m ON m.id = f.rowid
                 JOIN conversations c ON c.id = m.conversation
                 WHERE messages_fts MATCH ?1 AND m.conversation != ?2
                 ORDER BY bm25(messages_fts), m.id DESC
                 LIMIT ?3",
            )
            .and_then(|mut s| {
                Ok(s.query_map(params![fts5_query(q), exclude, limit], map)?
                    .filter_map(|r| r.ok())
                    .collect::<Vec<_>>())
            });
        if let Ok(hits) = fts {
            return hits;
        }
        let pattern = format!("%{}%", q.to_lowercase());
        conn.prepare(
            "SELECT m.conversation, c.title, m.role, m.text, m.ts
             FROM messages m JOIN conversations c ON c.id = m.conversation
             WHERE lower(m.text) LIKE ?1 AND m.conversation != ?2
             ORDER BY m.id DESC LIMIT ?3",
        )
        .and_then(|mut s| {
            Ok(s.query_map(params![pattern, exclude, limit], map)?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>())
        })
        .unwrap_or_default()
    }

    /// Importa as conversas salvas em JSON pela versão anterior. Devolve
    /// quantas entraram; conversas já existentes são ignoradas.
    pub fn import_json_dir(&self, dir: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
        let mut imported = 0;
        for e in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let Ok(c) = serde_json::from_str::<Conversation>(&text) else { continue };
            if !valid_id(&c.id) || self.title(&c.id).is_some() {
                continue;
            }
            let conn = self.conn.lock().unwrap();
            let ok = conn
                .execute(
                    "INSERT INTO conversations (id, title, renamed, created_ms, updated_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![c.id, c.title, c.renamed as i64, c.created_ms, c.updated_ms],
                )
                .is_ok();
            if ok {
                for m in &c.messages {
                    let _ = conn.execute(
                        "INSERT INTO messages (conversation, role, text, ts) VALUES (?1, ?2, ?3, ?4)",
                        params![c.id, m.role, m.text, m.ts],
                    );
                }
                imported += 1;
            }
        }
        imported
    }

    // ── fatos ─────────────────────────────────────────────────────────────

    /// Guarda/atualiza um fato. Um fato configurado pelo usuário ("user")
    /// não é sobrescrito por um aprendido ("learned").
    pub fn remember(&self, key: &str, value: &str, source: &str) -> bool {
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || value.is_empty() {
            return false;
        }
        let conn = self.conn.lock().unwrap();
        if source == "learned" {
            let user_owned: bool = conn
                .query_row("SELECT source = 'user' FROM facts WHERE key = ?1", params![key], |r| r.get(0))
                .optional()
                .ok()
                .flatten()
                .unwrap_or(false);
            if user_owned {
                return false;
            }
        }
        conn.execute(
            "INSERT INTO facts (key, value, source, updated_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, source = excluded.source,
                                            updated_ms = excluded.updated_ms",
            params![key, value, source, now_ms()],
        )
        .is_ok()
    }

    pub fn forget(&self, key: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM facts WHERE key = ?1", params![key.trim()]).map(|n| n > 0).unwrap_or(false)
    }

    pub fn facts(&self) -> Vec<Fact> {
        let conn = self.conn.lock().unwrap();
        conn.prepare("SELECT key, value, source, updated_ms FROM facts ORDER BY source DESC, updated_ms DESC")
            .and_then(|mut s| {
                Ok(s.query_map([], |r| {
                    Ok(Fact { key: r.get(0)?, value: r.get(1)?, source: r.get(2)?, updated_ms: r.get(3)? })
                })?
                .filter_map(|r| r.ok())
                .collect())
            })
            .unwrap_or_default()
    }

    // ── configurações ─────────────────────────────────────────────────────

    pub fn setting(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }
}

/// Cada palavra vira um termo FTS5 entre aspas (literal, sem operadores) e
/// todas precisam aparecer — "conta netflix" acha "a conta da Netflix".
fn fts5_query(q: &str) -> String {
    q.split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_title(raw: &str) -> String {
    let t: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(MAX_TITLE).collect();
    if t.is_empty() {
        DEFAULT_TITLE.into()
    } else {
        t
    }
}

fn auto_title(first_message: &str) -> String {
    let t = clean_title(first_message);
    if t.chars().count() > 32 {
        format!("{}…", t.chars().take(32).collect::<String>())
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversations_titles_and_turns() {
        let m = Memory::in_memory().unwrap();
        assert_eq!(m.append("c1", "user", "abre   o spotify pra mim"), Some("abre o spotify pra mim".into()));
        assert_eq!(m.append("c1", "lilith", "Pronto!"), None);
        assert_eq!(m.append("c1", "user", "valeu"), None, "só a primeira fala dá título");
        assert_eq!(m.turns("c1"), vec![("abre   o spotify pra mim".into(), "Pronto!".into())]);
        assert_eq!(m.rename("c1", "Música"), "Música");
        m.rename("c2", "Projeto X");
        assert_eq!(m.append("c2", "user", "primeira fala"), None, "não sobrescreve nome escolhido");
        assert_eq!(m.list().len(), 2);
        assert_eq!(m.get("c1").unwrap().messages.len(), 3);
        m.delete("c1");
        assert!(m.get("c1").is_none());
        assert!(!valid_id("../x") && !valid_id("") && valid_id("c1a-2"));
    }

    #[test]
    fn search_across_conversations_ignores_accents_and_current_chat() {
        let m = Memory::in_memory().unwrap();
        m.append("a", "user", "a senha do wifi da casa é example-pass");
        m.append("b", "user", "lembra da conta da Netflix?");
        m.append("c", "user", "o wifi caiu de novo");
        let hits = m.search("WIFI", 10, None);
        assert_eq!(hits.len(), 2);
        let hits = m.search("netflix conta", 10, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].conversation, "b");
        assert!(m.search("wifi", 10, Some("c")).iter().all(|h| h.conversation != "c"));
        assert!(m.search("  ", 10, None).is_empty());
        // Aspas e operadores do usuário não quebram a busca.
        assert!(m.search("wifi\" OR \"x", 10, None).is_empty());
    }

    #[test]
    fn user_facts_win_over_learned_ones() {
        let m = Memory::in_memory().unwrap();
        assert!(m.remember("nome", "Alex", "user"));
        assert!(!m.remember("Nome", "Lucas", "learned"), "aprendido não sobrescreve o configurado");
        assert!(m.remember("time", "Corinthians", "learned"));
        assert!(m.remember("time", "Palmeiras", "learned"));
        let facts = m.facts();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts.iter().find(|f| f.key == "nome").unwrap().value, "Alex");
        assert_eq!(facts.iter().find(|f| f.key == "time").unwrap().value, "Palmeiras");
        assert!(m.forget("TIME"));
        assert!(!m.remember(" ", "x", "user"));
    }

    #[test]
    fn settings_round_trip_and_json_import() {
        let m = Memory::in_memory().unwrap();
        assert_eq!(m.setting("user_name"), None);
        m.set_setting("user_name", "Alex");
        m.set_setting("user_name", "Lobão");
        assert_eq!(m.setting("user_name").as_deref(), Some("Lobão"));

        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"id":"old-1","title":"Antiga","renamed":true,"created_ms":1,"updated_ms":2,
                      "messages":[{"role":"user","text":"oi","ts":1},{"role":"lilith","text":"olá","ts":2}]}"#;
        std::fs::write(dir.path().join("old-1.json"), json).unwrap();
        std::fs::write(dir.path().join("lixo.json"), "{").unwrap();
        assert_eq!(m.import_json_dir(dir.path()), 1);
        assert_eq!(m.import_json_dir(dir.path()), 0, "não duplica");
        assert_eq!(m.get("old-1").unwrap().messages.len(), 2);
        assert_eq!(m.search("olá", 5, None).len(), 1, "importadas entram na busca");
    }
}
