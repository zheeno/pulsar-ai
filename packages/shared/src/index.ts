import { z } from 'zod';

export const SignalActionSchema = z.enum(['BUY', 'SELL', 'HOLD']);
export type SignalAction = z.infer<typeof SignalActionSchema>;

export const LlmSignalOutputSchema = z.object({
  action: SignalActionSchema,
  confidence: z.number().min(0).max(1),
  rationale: z.string().min(1),
});
export type LlmSignalOutput = z.infer<typeof LlmSignalOutputSchema>;

export const LlmPortfolioSignalOutputSchema = z.object({
  signals: z.array(
    z.object({
      symbol: z.string().min(1),
      action: SignalActionSchema,
      confidence: z.number().min(0).max(1),
      rationale: z.string().min(1),
    }),
  ),
});
export type LlmPortfolioSignalOutput = z.infer<typeof LlmPortfolioSignalOutputSchema>;

export const RiskPolicyResultSchema = z.enum([
  'APPROVED',
  'BLOCKED_EXPOSURE',
  'BLOCKED_STOPLOSS',
  'BLOCKED_OTHER',
  'BLOCKED_CONFIDENCE',
  'BLOCKED_DAILY_TRADES',
  'BLOCKED_DRAWDOWN',
  'BLOCKED_SYMBOL',
]);
export type RiskPolicyResult = z.infer<typeof RiskPolicyResultSchema>;

export const TradeSideSchema = z.enum(['BUY', 'SELL']);
export type TradeSide = z.infer<typeof TradeSideSchema>;

export const StrategyParamSetSchema = z.object({
  id: z.string().uuid().optional(),
  name: z.string().min(1),
  max_position_pct: z.number().min(0).max(1),
  max_daily_trades: z.number().int().positive(),
  stop_loss_pct: z.number().min(0).max(1),
  take_profit_pct: z.number().min(0).max(1).nullable().optional(),
  min_confidence_to_trade: z.number().min(0).max(1),
  max_daily_drawdown_pct: z.number().min(0).max(1),
  allowed_symbols: z.array(z.string()).nullable().optional(),
  position_size_pct: z.number().min(0).max(1).default(0.05),
  is_active: z.boolean().optional(),
});
export type StrategyParamSet = z.infer<typeof StrategyParamSetSchema>;

export const BacktestRequestSchema = z.object({
  strategy_param_set_id: z.string().uuid(),
  start_date: z.string(),
  end_date: z.string(),
});
export type BacktestRequest = z.infer<typeof BacktestRequestSchema>;

export const CURATED_SYMBOLS = [
  'DANGCEM', 'GTCO', 'ZENITHBANK', 'MTNN', 'BUACEMENT',
  'ACCESSCORP', 'UBA', 'FBNH', 'SEPLAT', 'NESTLE',
  'BUAFOODS', 'AIRTELAFRI', 'WAPCO', 'GUARANTY', 'STANBIC',
  'FLOURMILL', 'PRESCO', 'OKOMUOIL', 'NASCON', 'INTBREW',
];

export const NGX_TRADING_HOURS = { open: 9, close: 16 };
export const NGX_TIMEZONE = 'Africa/Lagos';

export const PROMPT_VERSION = 'v1.0.0';
export const PORTFOLIO_PROMPT_VERSION = 'v2.0.0';

export * from './types';
