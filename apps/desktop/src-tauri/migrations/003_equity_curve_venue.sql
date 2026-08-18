CREATE TABLE IF NOT EXISTS equity_curve_points (
  venue TEXT NOT NULL,
  snapshot_date TEXT NOT NULL,
  total_equity REAL NOT NULL,
  cash_balance REAL NOT NULL,
  market_value REAL NOT NULL,
  pnl_daily REAL NOT NULL DEFAULT 0,
  PRIMARY KEY (venue, snapshot_date)
);

CREATE INDEX IF NOT EXISTS idx_equity_curve_venue_date
  ON equity_curve_points(venue, snapshot_date);
