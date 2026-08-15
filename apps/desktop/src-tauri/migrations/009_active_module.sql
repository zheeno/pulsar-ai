INSERT OR IGNORE INTO settings (key, value) VALUES ('active_module', 'stocks');
CREATE UNIQUE INDEX IF NOT EXISTS idx_instruments_module_symbol ON instruments(module, symbol);
