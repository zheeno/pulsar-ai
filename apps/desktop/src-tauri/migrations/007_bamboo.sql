-- Bamboo NGX cache (separate from wealth_*).
CREATE TABLE IF NOT EXISTS bamboo_account (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  brokerage_balance REAL NOT NULL DEFAULT 0,
  stock_value REAL NOT NULL DEFAULT 0,
  profit REAL NOT NULL DEFAULT 0,
  synced_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS bamboo_positions (
  symbol TEXT PRIMARY KEY,
  stock_id INTEGER,
  quantity REAL NOT NULL DEFAULT 0,
  avg_cost REAL NOT NULL DEFAULT 0,
  last_price REAL NOT NULL DEFAULT 0,
  current_value REAL NOT NULL DEFAULT 0,
  synced_at TEXT NOT NULL DEFAULT (datetime('now'))
);
