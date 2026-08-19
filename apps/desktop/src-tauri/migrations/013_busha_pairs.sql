CREATE TABLE IF NOT EXISTS busha_pairs (
  symbol TEXT PRIMARY KEY,
  pair_id TEXT NOT NULL,
  buy_price REAL NOT NULL DEFAULT 0,
  sell_price REAL NOT NULL DEFAULT 0,
  min_buy_ngn REAL NOT NULL DEFAULT 0,
  min_buy_base REAL NOT NULL DEFAULT 0,
  min_sell_ngn REAL NOT NULL DEFAULT 0,
  min_sell_base REAL NOT NULL DEFAULT 0,
  max_buy_ngn REAL NOT NULL DEFAULT 0,
  max_buy_base REAL NOT NULL DEFAULT 0,
  max_sell_ngn REAL NOT NULL DEFAULT 0,
  max_sell_base REAL NOT NULL DEFAULT 0,
  base_decimal INTEGER NOT NULL DEFAULT 8,
  counter_decimal INTEGER NOT NULL DEFAULT 2,
  synced_at TEXT NOT NULL DEFAULT (datetime('now'))
);
