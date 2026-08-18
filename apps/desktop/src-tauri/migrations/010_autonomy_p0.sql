-- Outcome labels for fills/closes (no double-count: unique signal_id + event).
CREATE TABLE IF NOT EXISTS signal_outcomes (
  id TEXT PRIMARY KEY,
  signal_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  side TEXT NOT NULL,
  event TEXT NOT NULL CHECK (event IN ('fill', 'close')),
  quantity REAL NOT NULL,
  fill_price REAL NOT NULL,
  fee REAL NOT NULL DEFAULT 0,
  pnl REAL,
  horizon_return_pct REAL,
  confidence REAL,
  venue TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE(signal_id, event)
);
CREATE INDEX IF NOT EXISTS idx_signal_outcomes_symbol ON signal_outcomes(symbol, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_signal_outcomes_signal ON signal_outcomes(signal_id);
