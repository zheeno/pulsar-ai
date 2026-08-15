-- Cycle cash budget: fraction of available cash allocated across BUYs in a cycle.
ALTER TABLE strategy_param_sets ADD COLUMN cycle_budget_pct REAL NOT NULL DEFAULT 0.20;
