DROP TABLE IF EXISTS backtest_runs;

CREATE TABLE IF NOT EXISTS agent_memories (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('symbol_lesson', 'freeform')),
  symbol TEXT,
  text TEXT NOT NULL,
  embedding BLOB,
  source TEXT NOT NULL CHECK (source IN ('agent_upsert', 'trade_outcome')),
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_agent_memories_symbol ON agent_memories(symbol);
CREATE INDEX IF NOT EXISTS idx_agent_memories_created ON agent_memories(created_at);
