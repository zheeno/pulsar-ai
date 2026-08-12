import { Schema, SchemaFactory } from '@nestjs/mongoose';
import { HydratedDocument } from 'mongoose';
import { v4 as uuidv4 } from 'uuid';

function uuidDefault() {
  return uuidv4();
}

@Schema({ collection: 'users', timestamps: { createdAt: 'created_at', updatedAt: false } })
export class User {
  _id!: string;
  email!: string;
  password_hash!: string;
}
export const UserSchema = SchemaFactory.createForClass(User);
UserSchema.add({ _id: { type: String, default: uuidDefault } });
UserSchema.index({ email: 1 }, { unique: true });

@Schema({ collection: 'instruments', timestamps: { createdAt: 'added_at', updatedAt: false } })
export class Instrument {
  _id!: string;
  symbol!: string;
  name!: string;
  sector?: string;
  is_active!: boolean;
}
export const InstrumentSchema = SchemaFactory.createForClass(Instrument);
InstrumentSchema.add({ _id: { type: String, default: uuidDefault } });
InstrumentSchema.index({ symbol: 1 }, { unique: true });

@Schema({ collection: 'price_history', timestamps: false })
export class PriceHistory {
  _id!: string;
  symbol!: string;
  trade_date!: string;
  price!: number;
  change_percent?: number;
  volume?: number;
  market_cap?: number;
  pe_ratio?: number;
  ingested_at?: Date;
}
export const PriceHistorySchema = SchemaFactory.createForClass(PriceHistory);
PriceHistorySchema.add({ _id: { type: String, default: uuidDefault } });
PriceHistorySchema.index({ symbol: 1, trade_date: -1 }, { unique: true });

@Schema({ collection: 'index_history', timestamps: false })
export class IndexHistory {
  _id!: string;
  index_code!: string;
  trade_date!: string;
  value!: number;
  points?: number;
  week_change?: number;
  month_change?: number;
  year_change?: number;
}
export const IndexHistorySchema = SchemaFactory.createForClass(IndexHistory);
IndexHistorySchema.add({ _id: { type: String, default: uuidDefault } });
IndexHistorySchema.index({ index_code: 1, trade_date: 1 }, { unique: true });

@Schema({ collection: 'fundamentals_snapshots', timestamps: false })
export class FundamentalsSnapshot {
  _id!: string;
  symbol!: string;
  snapshot_date!: string;
  eps?: number;
  dividend_per_share?: number;
  dividend_yield?: number;
  roe?: number;
  roa?: number;
  pb_ratio?: number;
  debt_equity?: number;
  beta?: number;
  profit_margin?: number;
  extra?: Record<string, unknown>;
}
export const FundamentalsSnapshotSchema = SchemaFactory.createForClass(FundamentalsSnapshot);
FundamentalsSnapshotSchema.add({ _id: { type: String, default: uuidDefault } });
FundamentalsSnapshotSchema.index({ symbol: 1, snapshot_date: -1 });

@Schema({ collection: 'news', timestamps: false })
export class News {
  _id!: string;
  symbol?: string;
  headline!: string;
  body_summary?: string;
  source?: string;
  published_at!: Date;
  category?: string;
}
export const NewsSchema = SchemaFactory.createForClass(News);
NewsSchema.add({ _id: { type: String, default: uuidDefault } });

@Schema({ collection: 'strategy_param_sets', timestamps: { createdAt: 'created_at', updatedAt: false } })
export class StrategyParamSet {
  _id!: string;
  name!: string;
  max_position_pct!: number;
  max_daily_trades!: number;
  stop_loss_pct!: number;
  take_profit_pct?: number | null;
  min_confidence_to_trade!: number;
  max_daily_drawdown_pct!: number;
  allowed_symbols?: string[] | null;
  position_size_pct!: number;
  is_active!: boolean;
}
export const StrategyParamSetSchema = SchemaFactory.createForClass(StrategyParamSet);
StrategyParamSetSchema.add({ _id: { type: String, default: uuidDefault } });

@Schema({ collection: 'sandbox_portfolios', timestamps: { createdAt: 'created_at', updatedAt: false } })
export class SandboxPortfolio {
  _id!: string;
  name!: string;
  starting_capital!: number;
  cash_balance!: number;
  strategy_param_set_id!: string;
}
export const SandboxPortfolioSchema = SchemaFactory.createForClass(SandboxPortfolio);
SandboxPortfolioSchema.add({ _id: { type: String, default: uuidDefault } });

@Schema({ collection: 'sandbox_positions', timestamps: { createdAt: false, updatedAt: 'updated_at' } })
export class SandboxPosition {
  _id!: string;
  portfolio_id!: string;
  symbol!: string;
  quantity!: number;
  avg_cost!: number;
}
export const SandboxPositionSchema = SchemaFactory.createForClass(SandboxPosition);
SandboxPositionSchema.add({ _id: { type: String, default: uuidDefault } });
SandboxPositionSchema.index({ portfolio_id: 1, symbol: 1 }, { unique: true });

@Schema({ collection: 'signals', timestamps: { createdAt: 'generated_at', updatedAt: false } })
export class Signal {
  _id!: string;
  symbol!: string;
  action!: string;
  confidence!: number;
  rationale!: string;
  technical_snapshot!: Record<string, unknown>;
  fundamental_snapshot?: Record<string, unknown> | null;
  model_name!: string;
  prompt_version!: string;
  risk_policy_result!: string;
  executed!: boolean;
}
export const SignalSchema = SchemaFactory.createForClass(Signal);
SignalSchema.add({ _id: { type: String, default: uuidDefault } });
SignalSchema.index({ generated_at: -1 });
SignalSchema.index({ symbol: 1 });

@Schema({ collection: 'signal_llm_logs', timestamps: { createdAt: 'created_at', updatedAt: false } })
export class SignalLlmLog {
  _id!: string;
  signal_id!: string;
  prompt!: string;
  raw_response!: string;
}
export const SignalLlmLogSchema = SchemaFactory.createForClass(SignalLlmLog);
SignalLlmLogSchema.add({ _id: { type: String, default: uuidDefault } });

@Schema({ collection: 'sandbox_trades', timestamps: { createdAt: 'executed_at', updatedAt: false } })
export class SandboxTrade {
  _id!: string;
  portfolio_id!: string;
  signal_id?: string;
  symbol!: string;
  side!: string;
  quantity!: number;
  fill_price!: number;
  simulated_fee!: number;
  simulated_slippage_bps!: number;
  resulting_cash_balance!: number;
}
export const SandboxTradeSchema = SchemaFactory.createForClass(SandboxTrade);
SandboxTradeSchema.add({ _id: { type: String, default: uuidDefault } });
SandboxTradeSchema.index({ executed_at: -1 });

@Schema({ collection: 'daily_performance_snapshots', timestamps: false })
export class DailyPerformanceSnapshot {
  _id!: string;
  portfolio_id!: string;
  snapshot_date!: string;
  total_equity!: number;
  pnl_daily!: number;
  pnl_cumulative!: number;
  benchmark_asi_change_pct?: number;
  drawdown_pct?: number;
}
export const DailyPerformanceSnapshotSchema = SchemaFactory.createForClass(DailyPerformanceSnapshot);
DailyPerformanceSnapshotSchema.add({ _id: { type: String, default: uuidDefault } });
DailyPerformanceSnapshotSchema.index({ portfolio_id: 1, snapshot_date: 1 }, { unique: true });

@Schema({ collection: 'backfill_state', timestamps: false })
export class BackfillState {
  _id!: string;
  symbol!: string;
  earliest_date_fetched?: string;
  last_run_at?: Date;
}
export const BackfillStateSchema = SchemaFactory.createForClass(BackfillState);
BackfillStateSchema.add({ _id: { type: String, default: uuidDefault } });
BackfillStateSchema.index({ symbol: 1 }, { unique: true });

@Schema({ collection: 'ngx_pulse_usage_logs', timestamps: { createdAt: 'called_at', updatedAt: false } })
export class NgxPulseUsageLog {
  _id!: string;
  endpoint!: string;
}
export const NgxPulseUsageLogSchema = SchemaFactory.createForClass(NgxPulseUsageLog);
NgxPulseUsageLogSchema.add({ _id: { type: String, default: uuidDefault } });

@Schema({ collection: 'backtest_runs', timestamps: { createdAt: 'created_at', updatedAt: false } })
export class BacktestRun {
  _id!: string;
  strategy_param_set_id!: string;
  start_date!: string;
  end_date!: string;
  status!: string;
  results?: Record<string, unknown>;
  completed_at?: Date;
}
export const BacktestRunSchema = SchemaFactory.createForClass(BacktestRun);
BacktestRunSchema.add({ _id: { type: String, default: uuidDefault } });

export type UserDocument = HydratedDocument<User>;
export type InstrumentDocument = HydratedDocument<Instrument>;
export type PriceHistoryDocument = HydratedDocument<PriceHistory>;
export type IndexHistoryDocument = HydratedDocument<IndexHistory>;
export type FundamentalsSnapshotDocument = HydratedDocument<FundamentalsSnapshot>;
export type NewsDocument = HydratedDocument<News>;
export type StrategyParamSetDocument = HydratedDocument<StrategyParamSet>;
export type SandboxPortfolioDocument = HydratedDocument<SandboxPortfolio>;
export type SandboxPositionDocument = HydratedDocument<SandboxPosition>;
export type SignalDocument = HydratedDocument<Signal>;
export type SignalLlmLogDocument = HydratedDocument<SignalLlmLog>;
export type SandboxTradeDocument = HydratedDocument<SandboxTrade>;
export type DailyPerformanceSnapshotDocument = HydratedDocument<DailyPerformanceSnapshot>;
export type BackfillStateDocument = HydratedDocument<BackfillState>;
export type NgxPulseUsageLogDocument = HydratedDocument<NgxPulseUsageLog>;
export type BacktestRunDocument = HydratedDocument<BacktestRun>;
