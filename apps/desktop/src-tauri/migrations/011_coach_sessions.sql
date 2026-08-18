-- Durable Coach sessions, transcripts, and confirm-gated trade proposals.
CREATE TABLE IF NOT EXISTS coach_sessions (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_coach_sessions_updated ON coach_sessions(updated_at DESC);

CREATE TABLE IF NOT EXISTS coach_messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES coach_sessions(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
  text TEXT NOT NULL,
  payload TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_coach_messages_session ON coach_messages(session_id, created_at);

CREATE TABLE IF NOT EXISTS coach_trade_proposals (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES coach_sessions(id) ON DELETE CASCADE,
  message_id TEXT,
  symbol TEXT NOT NULL,
  side TEXT NOT NULL,
  quantity REAL,
  notional REAL,
  rationale TEXT,
  preview TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('proposed', 'cancelled', 'executed', 'blocked')),
  result TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_coach_proposals_session ON coach_trade_proposals(session_id, created_at DESC);
