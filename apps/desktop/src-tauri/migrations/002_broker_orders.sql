-- Live broker order history (Coronation Wealth). Sandbox trades stay in sandbox_trades.
CREATE TABLE IF NOT EXISTS broker_orders (
  id TEXT PRIMARY KEY,
  signal_id TEXT,
  symbol TEXT NOT NULL,
  side TEXT NOT NULL,
  quantity REAL NOT NULL,
  external_order_id INTEGER,
  stock_id INTEGER,
  status TEXT NOT NULL,
  fill_price REAL,
  fee REAL,
  rejection_reason TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_broker_orders_created ON broker_orders(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_broker_orders_symbol ON broker_orders(symbol);
