-- Uma linha por falha (ação + código + versão). number = 0 enquanto a issue
-- está sendo criada; -1 no modo DRY_RUN.
CREATE TABLE IF NOT EXISTS issues (
  fingerprint TEXT PRIMARY KEY,
  number INTEGER NOT NULL,
  count INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL
);

-- Contadores diários (limites). Apagados pelo cron no dia seguinte.
CREATE TABLE IF NOT EXISTS counters (
  day TEXT NOT NULL,
  key TEXT NOT NULL,
  n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, key)
);
