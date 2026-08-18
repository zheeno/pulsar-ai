CREATE TABLE IF NOT EXISTS order_intents (
  id TEXT PRIMARY KEY,
  client_order_id TEXT NOT NULL UNIQUE,
  external_order_id INTEGER UNIQUE,
  cycle_id TEXT,
  signal_id TEXT,
  symbol TEXT NOT NULL,
  side TEXT NOT NULL,
  requested_qty REAL NOT NULL,
  requested_quote REAL,
  requested_notional REAL,
  venue TEXT NOT NULL DEFAULT 'wealth',
  state TEXT NOT NULL,
  last_error TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_order_intents_state ON order_intents(state);
CREATE INDEX IF NOT EXISTS idx_order_intents_created ON order_intents(created_at DESC);

CREATE TABLE IF NOT EXISTS cycle_audits (
  id TEXT PRIMARY KEY,
  cycle_id TEXT,
  summary TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  expires_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cycle_audits_expires ON cycle_audits(expires_at);
