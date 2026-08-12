-- Pulsar AI SQLite schema (local-first desktop)

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS instruments (
  symbol TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  sector TEXT,
  is_active INTEGER NOT NULL DEFAULT 1,
  added_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS price_history (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  symbol TEXT NOT NULL REFERENCES instruments(symbol),
  trade_date TEXT NOT NULL,
  price REAL NOT NULL,
  change_percent REAL DEFAULT 0,
  volume INTEGER DEFAULT 0,
  market_cap REAL,
  pe_ratio REAL,
  source_updated_at TEXT,
  ingested_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE(symbol, trade_date)
);
CREATE INDEX IF NOT EXISTS idx_price_history_symbol_date ON price_history(symbol, trade_date DESC);

CREATE TABLE IF NOT EXISTS index_history (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  index_code TEXT NOT NULL,
  trade_date TEXT NOT NULL,
  value REAL NOT NULL,
  points REAL,
  week_change REAL,
  month_change REAL,
  year_change REAL,
  UNIQUE(index_code, trade_date)
);

CREATE TABLE IF NOT EXISTS strategy_param_sets (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  max_position_pct REAL NOT NULL DEFAULT 0.1,
  max_daily_trades INTEGER NOT NULL DEFAULT 5,
  stop_loss_pct REAL NOT NULL DEFAULT 0.05,
  take_profit_pct REAL,
  min_confidence_to_trade REAL NOT NULL DEFAULT 0.65,
  max_daily_drawdown_pct REAL NOT NULL DEFAULT 0.03,
  allowed_symbols TEXT,
  position_size_pct REAL NOT NULL DEFAULT 0.05,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  is_active INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS sandbox_portfolios (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  starting_capital REAL NOT NULL,
  cash_balance REAL NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  strategy_param_set_id TEXT NOT NULL REFERENCES strategy_param_sets(id)
);

CREATE TABLE IF NOT EXISTS signals (
  id TEXT PRIMARY KEY,
  symbol TEXT NOT NULL REFERENCES instruments(symbol),
  generated_at TEXT NOT NULL DEFAULT (datetime('now')),
  action TEXT NOT NULL CHECK (action IN ('BUY', 'SELL', 'HOLD')),
  confidence REAL NOT NULL,
  rationale TEXT NOT NULL,
  technical_snapshot TEXT NOT NULL DEFAULT '{}',
  fundamental_snapshot TEXT,
  model_name TEXT NOT NULL,
  prompt_version TEXT NOT NULL,
  risk_policy_result TEXT NOT NULL DEFAULT 'BLOCKED_OTHER',
  executed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_signals_generated_at ON signals(generated_at DESC);

CREATE TABLE IF NOT EXISTS signal_llm_logs (
  id TEXT PRIMARY KEY,
  signal_id TEXT NOT NULL REFERENCES signals(id),
  prompt TEXT NOT NULL,
  raw_response TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS sandbox_positions (
  id TEXT PRIMARY KEY,
  portfolio_id TEXT NOT NULL REFERENCES sandbox_portfolios(id),
  symbol TEXT NOT NULL REFERENCES instruments(symbol),
  quantity REAL NOT NULL DEFAULT 0,
  avg_cost REAL NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE(portfolio_id, symbol)
);

CREATE TABLE IF NOT EXISTS sandbox_trades (
  id TEXT PRIMARY KEY,
  portfolio_id TEXT NOT NULL REFERENCES sandbox_portfolios(id),
  signal_id TEXT REFERENCES signals(id),
  symbol TEXT NOT NULL,
  side TEXT NOT NULL CHECK (side IN ('BUY', 'SELL')),
  quantity REAL NOT NULL,
  fill_price REAL NOT NULL,
  simulated_fee REAL NOT NULL DEFAULT 0,
  simulated_slippage_bps REAL NOT NULL DEFAULT 0,
  executed_at TEXT NOT NULL DEFAULT (datetime('now')),
  resulting_cash_balance REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS daily_performance_snapshot (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  portfolio_id TEXT NOT NULL REFERENCES sandbox_portfolios(id),
  snapshot_date TEXT NOT NULL,
  total_equity REAL NOT NULL,
  pnl_daily REAL NOT NULL DEFAULT 0,
  pnl_cumulative REAL NOT NULL DEFAULT 0,
  benchmark_asi_change_pct REAL DEFAULT 0,
  drawdown_pct REAL DEFAULT 0,
  UNIQUE(portfolio_id, snapshot_date)
);

CREATE TABLE IF NOT EXISTS backfill_state (
  symbol TEXT PRIMARY KEY REFERENCES instruments(symbol),
  earliest_date_fetched TEXT,
  last_run_at TEXT
);

CREATE TABLE IF NOT EXISTS ngx_pulse_usage_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  endpoint TEXT NOT NULL,
  called_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS backtest_runs (
  id TEXT PRIMARY KEY,
  strategy_param_set_id TEXT NOT NULL REFERENCES strategy_param_sets(id),
  start_date TEXT NOT NULL,
  end_date TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  results TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  completed_at TEXT
);

CREATE TABLE IF NOT EXISTS rate_limit_counters (
  key TEXT PRIMARY KEY,
  count INTEGER NOT NULL DEFAULT 0,
  expires_at TEXT
);
