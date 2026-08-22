CREATE TABLE IF NOT EXISTS busha_ohlc_points (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  symbol TEXT NOT NULL,
  period TEXT NOT NULL,
  ts TEXT NOT NULL,
  price REAL NOT NULL,
  ingested_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE(symbol, period, ts)
);
CREATE INDEX IF NOT EXISTS idx_busha_ohlc_symbol_period_ts
  ON busha_ohlc_points(symbol, period, ts DESC);

CREATE TABLE IF NOT EXISTS busha_ohlc_meta (
  symbol TEXT NOT NULL,
  period TEXT NOT NULL,
  snapshot_price REAL,
  change_pct REAL,
  high REAL,
  low REAL,
  market_cap REAL,
  fetched_at TEXT NOT NULL,
  PRIMARY KEY (symbol, period)
);
