export interface Instrument {
  symbol: string;
  name: string;
  sector: string;
  is_active: boolean;
  added_at: string;
}

export interface PriceHistory {
  id?: number;
  symbol: string;
  trade_date: string;
  price: number;
  change_percent: number;
  volume: number;
  market_cap?: number;
  pe_ratio?: number;
  source_updated_at?: string;
  ingested_at?: string;
}

export interface Signal {
  id: string;
  symbol: string;
  generated_at: string;
  action: 'BUY' | 'SELL' | 'HOLD';
  confidence: number;
  rationale: string;
  technical_snapshot: Record<string, unknown>;
  fundamental_snapshot?: Record<string, unknown> | null;
  model_name: string;
  prompt_version: string;
  risk_policy_result: string;
  executed: boolean;
}

export interface SandboxPortfolio {
  id: string;
  name: string;
  starting_capital: number;
  cash_balance: number;
  created_at: string;
  strategy_param_set_id: string;
}

export interface SandboxPosition {
  id: string;
  portfolio_id: string;
  symbol: string;
  quantity: number;
  avg_cost: number;
  updated_at: string;
}

export interface SandboxTrade {
  id: string;
  portfolio_id: string;
  signal_id: string | null;
  symbol: string;
  side: 'BUY' | 'SELL';
  quantity: number;
  fill_price: number;
  simulated_fee: number;
  simulated_slippage_bps: number;
  executed_at: string;
  resulting_cash_balance: number;
}

export interface DailyPerformanceSnapshot {
  id?: number;
  portfolio_id: string;
  snapshot_date: string;
  total_equity: number;
  pnl_daily: number;
  pnl_cumulative: number;
  benchmark_asi_change_pct: number;
  drawdown_pct: number;
}

export interface PortfolioSummary {
  portfolio: SandboxPortfolio;
  positions: SandboxPosition[];
  total_equity: number;
  market_value: number;
  pnl_today: number;
}

export interface TechnicalSnapshot {
  sma50: number | null;
  sma200: number | null;
  rsi14: number | null;
  momentum: number | null;
  volumeAnomaly: number | null;
  currentPrice: number;
}
