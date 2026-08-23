-- Applied only when agent_memories CHECK does not yet allow dream_consolidate.
CREATE TABLE agent_memories_dream (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('symbol_lesson', 'freeform')),
  symbol TEXT,
  text TEXT NOT NULL,
  embedding BLOB,
  source TEXT NOT NULL CHECK (source IN ('agent_upsert', 'trade_outcome', 'dream_consolidate')),
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO agent_memories_dream (id, kind, symbol, text, embedding, source, created_at, updated_at)
SELECT id, kind, symbol, text, embedding, source, created_at, updated_at
FROM agent_memories;

DROP TABLE agent_memories;
ALTER TABLE agent_memories_dream RENAME TO agent_memories;

CREATE INDEX IF NOT EXISTS idx_agent_memories_symbol ON agent_memories(symbol);
CREATE INDEX IF NOT EXISTS idx_agent_memories_created ON agent_memories(created_at);
CREATE INDEX IF NOT EXISTS idx_agent_memories_source ON agent_memories(source);
