-- NGX AI Trading Assistant Schema

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- 4.1 instruments
CREATE TABLE IF NOT EXISTS instruments (
  symbol TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  sector TEXT,
  is_active BOOLEAN NOT NULL DEFAULT true,
  added_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 4.2 price_history
CREATE TABLE IF NOT EXISTS price_history (
  id BIGSERIAL PRIMARY KEY,
  symbol TEXT NOT NULL REFERENCES instruments(symbol),
  trade_date DATE NOT NULL,
  price NUMERIC NOT NULL,
  change_percent NUMERIC DEFAULT 0,
  volume BIGINT DEFAULT 0,
  market_cap NUMERIC,
  pe_ratio NUMERIC,
  source_updated_at TIMESTAMPTZ,
  ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(symbol, trade_date)
);
CREATE INDEX IF NOT EXISTS idx_price_history_symbol_date ON price_history(symbol, trade_date DESC);

-- 4.3 index_history
CREATE TABLE IF NOT EXISTS index_history (
  id BIGSERIAL PRIMARY KEY,
  index_code TEXT NOT NULL,
  trade_date DATE NOT NULL,
  value NUMERIC NOT NULL,
  points NUMERIC,
  week_change NUMERIC,
  month_change NUMERIC,
  year_change NUMERIC,
  UNIQUE(index_code, trade_date)
);

-- 4.4 fundamentals_snapshot
CREATE TABLE IF NOT EXISTS fundamentals_snapshot (
  id BIGSERIAL PRIMARY KEY,
  symbol TEXT NOT NULL REFERENCES instruments(symbol),
  snapshot_date DATE NOT NULL,
  eps NUMERIC, dividend_per_share NUMERIC, dividend_yield NUMERIC,
  roe NUMERIC, roa NUMERIC, pb_ratio NUMERIC, debt_equity NUMERIC,
  beta NUMERIC, profit_margin NUMERIC,
  extra JSONB DEFAULT '{}',
  UNIQUE(symbol, snapshot_date)
);

-- 4.5 disclosures / news
CREATE TABLE IF NOT EXISTS news (
  id BIGSERIAL PRIMARY KEY,
  symbol TEXT REFERENCES instruments(symbol),
  headline TEXT NOT NULL,
  body_summary TEXT,
  source TEXT,
  published_at TIMESTAMPTZ NOT NULL,
  category TEXT
);

-- 4.10 strategy_param_sets (before portfolios due to FK)
CREATE TABLE IF NOT EXISTS strategy_param_sets (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  max_position_pct NUMERIC NOT NULL DEFAULT 0.1,
  max_daily_trades INTEGER NOT NULL DEFAULT 5,
  stop_loss_pct NUMERIC NOT NULL DEFAULT 0.05,
  take_profit_pct NUMERIC,
  min_confidence_to_trade NUMERIC NOT NULL DEFAULT 0.65,
  max_daily_drawdown_pct NUMERIC NOT NULL DEFAULT 0.03,
  allowed_symbols TEXT[],
  position_size_pct NUMERIC NOT NULL DEFAULT 0.05,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  is_active BOOLEAN NOT NULL DEFAULT false
);

-- 4.7 sandbox_portfolios
CREATE TABLE IF NOT EXISTS sandbox_portfolios (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  starting_capital NUMERIC NOT NULL,
  cash_balance NUMERIC NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  strategy_param_set_id UUID NOT NULL REFERENCES strategy_param_sets(id)
);

-- 4.6 signals
CREATE TABLE IF NOT EXISTS signals (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  symbol TEXT NOT NULL REFERENCES instruments(symbol),
  generated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  action TEXT NOT NULL CHECK (action IN ('BUY', 'SELL', 'HOLD')),
  confidence NUMERIC(3,2) NOT NULL,
  rationale TEXT NOT NULL,
  technical_snapshot JSONB NOT NULL DEFAULT '{}',
  fundamental_snapshot JSONB,
  model_name TEXT NOT NULL,
  prompt_version TEXT NOT NULL,
  risk_policy_result TEXT NOT NULL DEFAULT 'BLOCKED_OTHER',
  executed BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_signals_generated_at ON signals(generated_at DESC);
CREATE INDEX IF NOT EXISTS idx_signals_symbol ON signals(symbol);

-- signal_llm_logs
CREATE TABLE IF NOT EXISTS signal_llm_logs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  signal_id UUID NOT NULL REFERENCES signals(id),
  prompt TEXT NOT NULL,
  raw_response TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 4.8 sandbox_positions
CREATE TABLE IF NOT EXISTS sandbox_positions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  portfolio_id UUID NOT NULL REFERENCES sandbox_portfolios(id),
  symbol TEXT NOT NULL REFERENCES instruments(symbol),
  quantity NUMERIC NOT NULL DEFAULT 0,
  avg_cost NUMERIC NOT NULL DEFAULT 0,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(portfolio_id, symbol)
);

-- 4.9 sandbox_trades
CREATE TABLE IF NOT EXISTS sandbox_trades (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  portfolio_id UUID NOT NULL REFERENCES sandbox_portfolios(id),
  signal_id UUID REFERENCES signals(id),
  symbol TEXT NOT NULL,
  side TEXT NOT NULL CHECK (side IN ('BUY', 'SELL')),
  quantity NUMERIC NOT NULL,
  fill_price NUMERIC NOT NULL,
  simulated_fee NUMERIC NOT NULL DEFAULT 0,
  simulated_slippage_bps NUMERIC NOT NULL DEFAULT 0,
  executed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  resulting_cash_balance NUMERIC NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sandbox_trades_executed_at ON sandbox_trades(executed_at DESC);

-- 4.11 daily_performance_snapshot
CREATE TABLE IF NOT EXISTS daily_performance_snapshot (
  id BIGSERIAL PRIMARY KEY,
  portfolio_id UUID NOT NULL REFERENCES sandbox_portfolios(id),
  snapshot_date DATE NOT NULL,
  total_equity NUMERIC NOT NULL,
  pnl_daily NUMERIC NOT NULL DEFAULT 0,
  pnl_cumulative NUMERIC NOT NULL DEFAULT 0,
  benchmark_asi_change_pct NUMERIC DEFAULT 0,
  drawdown_pct NUMERIC DEFAULT 0,
  UNIQUE(portfolio_id, snapshot_date)
);

-- backfill_state
CREATE TABLE IF NOT EXISTS backfill_state (
  symbol TEXT PRIMARY KEY REFERENCES instruments(symbol),
  earliest_date_fetched DATE,
  last_run_at TIMESTAMPTZ
);

-- ngx_pulse_usage_log
CREATE TABLE IF NOT EXISTS ngx_pulse_usage_log (
  id BIGSERIAL PRIMARY KEY,
  endpoint TEXT NOT NULL,
  called_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_ngx_usage_called_at ON ngx_pulse_usage_log(called_at);

-- backtest_runs
CREATE TABLE IF NOT EXISTS backtest_runs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  strategy_param_set_id UUID NOT NULL REFERENCES strategy_param_sets(id),
  start_date DATE NOT NULL,
  end_date DATE NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  results JSONB,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ
);

-- users for local auth (sandbox)
CREATE TABLE IF NOT EXISTS app_users (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  email TEXT UNIQUE NOT NULL,
  password_hash TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
