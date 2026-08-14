-- Per-cycle equity points: timestamp PK instead of one row per calendar day.
CREATE TABLE IF NOT EXISTS equity_curve_points_v2 (
  venue TEXT NOT NULL,
  recorded_at TEXT NOT NULL,
  snapshot_date TEXT NOT NULL,
  total_equity REAL NOT NULL,
  cash_balance REAL NOT NULL,
  market_value REAL NOT NULL,
  pnl_daily REAL NOT NULL DEFAULT 0,
  PRIMARY KEY (venue, recorded_at)
);

INSERT OR IGNORE INTO equity_curve_points_v2
  (venue, recorded_at, snapshot_date, total_equity, cash_balance, market_value, pnl_daily)
SELECT
  venue,
  CASE
    WHEN length(snapshot_date) >= 19 THEN snapshot_date
    ELSE snapshot_date || 'T00:00:00Z'
  END,
  substr(snapshot_date, 1, 10),
  total_equity,
  cash_balance,
  market_value,
  pnl_daily
FROM equity_curve_points;

DROP TABLE equity_curve_points;
ALTER TABLE equity_curve_points_v2 RENAME TO equity_curve_points;

CREATE INDEX IF NOT EXISTS idx_equity_curve_venue_recorded
  ON equity_curve_points(venue, recorded_at);
CREATE INDEX IF NOT EXISTS idx_equity_curve_venue_date
  ON equity_curve_points(venue, snapshot_date);
