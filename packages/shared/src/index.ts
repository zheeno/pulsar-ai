import { z } from 'zod';

export const SignalActionSchema = z.enum(['BUY', 'SELL', 'HOLD']);
export type SignalAction = z.infer<typeof SignalActionSchema>;

export const LlmSignalOutputSchema = z.object({
  action: SignalActionSchema,
  confidence: z.number().min(0).max(1),
  rationale: z.string().min(1),
});
export type LlmSignalOutput = z.infer<typeof LlmSignalOutputSchema>;

export const TICKER_PATTERN = /^[A-Z0-9.-]{1,16}$/;

export const LlmPortfolioSignalOutputSchema = z.object({
  signals: z.array(
    z.object({
      symbol: z.string().regex(TICKER_PATTERN),
      action: SignalActionSchema,
      confidence: z.number().min(0).max(1),
      rationale: z.string().min(1).max(2000),
    }),
  ).max(40),
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
  'BLOCKED_NO_POSITION',
  'BLOCKED_NOT_EXECUTED',
  'BLOCKED_MARKET_CLOSED',
  'BLOCKED_AMBIGUOUS_ORDERS',
  'BLOCKED_LIVE_DISABLED',
  'BLOCKED_PENDING_CONFIRM',
  'BLOCKED_QUOTE_DEVIATION',
  'BLOCKED_CASH',
  'BLOCKED_PIN_MISSING',
  'BLOCKED_BROKER',
  'BLOCKED_NOTIONAL',
]);
export type RiskPolicyResult = z.infer<typeof RiskPolicyResultSchema>;

export const TradeSideSchema = z.enum(['BUY', 'SELL']);
export type TradeSide = z.infer<typeof TradeSideSchema>;

export const StrategyParamSetSchema = z.object({
  id: z.string().uuid().optional(),
  name: z.string().min(1),
  max_position_pct: z.number().min(0).max(1),
  max_daily_trades: z.number().int().positive().optional(), // max new BUY fills per calendar day
  stop_loss_pct: z.number().min(0).max(1),
  take_profit_pct: z.number().min(0).max(1).nullable().optional(),
  min_confidence_to_trade: z.number().min(0).max(1),
  max_daily_drawdown_pct: z.number().min(0).max(1),
  allowed_symbols: z.array(z.string()).nullable().optional(), // deprecated: unused; universe is all active instruments
  position_size_pct: z.number().min(0).max(1).default(0.05),
  cycle_budget_pct: z.number().min(0.05).max(1).default(0.2),
  is_active: z.boolean().optional(),
});
export type StrategyParamSet = z.infer<typeof StrategyParamSetSchema>;

export const StrategyCoachPatchSchema = z.object({
  max_position_pct: z.number().min(0).max(1).optional(),
  cycle_budget_pct: z.number().min(0).max(1).optional(),
  min_confidence_to_trade: z.number().min(0).max(1).optional(),
  max_daily_drawdown_pct: z.number().min(0).max(1).optional(),
  stop_loss_pct: z.number().min(0).max(1).optional(),
  take_profit_pct: z.number().min(0).max(1).optional(),
});
export type StrategyCoachPatch = z.infer<typeof StrategyCoachPatchSchema>;

export const LlmStrategyCoachOutputSchema = z.object({
  needMoreContext: z.boolean().default(false),
  clarifyingQuestions: z.array(z.string()).default([]),
  summary: z.string(),
  patch: StrategyCoachPatchSchema.default({}),
  rationale: z.record(z.string()).default({}),
  warnings: z.array(z.string()).default([]),
});
export type LlmStrategyCoachOutput = z.infer<typeof LlmStrategyCoachOutputSchema>;

export const CURATED_SYMBOLS = [
  'DANGCEM', 'GTCO', 'ZENITHBANK', 'MTNN', 'BUACEMENT',
  'ACCESSCORP', 'UBA', 'FBNH', 'SEPLAT', 'NESTLE',
  'BUAFOODS', 'AIRTELAFRI', 'WAPCO', 'GUARANTY', 'STANBIC',
  'FLOURMILL', 'PRESCO', 'OKOMUOIL', 'NASCON', 'INTBREW',
];

export const NGX_TRADING_HOURS = { open: 9, close: 16 };
export const NGX_TIMEZONE = 'Africa/Lagos';

export const PROMPT_VERSION = 'v1.0.0';
export const PORTFOLIO_PROMPT_VERSION = 'v2.5.3';
export const STRATEGY_COACH_PROMPT_VERSION = 'v4.1.1';

export * from './types';
export * from './desktop';
export * from './coach-format';
