import { invoke } from '@tauri-apps/api/core';
import { httpInvoke } from './http-bridge';

export function isTauri(): boolean {
  if (typeof window === 'undefined') return false;
  if ('__TAURI_INTERNALS__' in window) return true;
  if ('__TAURI__' in window) return true;
  return window.location.protocol === 'tauri:' || window.location.hostname === 'tauri.localhost';
}

/**
 * Tauri 2 IPC deserializes command args as camelCase (`session_id` → `sessionId`).
 * Wrap payload-style commands; pass other keys in camelCase.
 */
function tauriArgs(command: string, args?: Record<string, unknown>): Record<string, unknown> | undefined {
  if (!args) return undefined;

  if (command === 'settings_set') {
    return { payload: args };
  }

  if (command === 'set_selected_broker' || command === 'wealth_login' || command === 'wealth_verify_2fa' || command === 'bamboo_login') {
    return { payload: args };
  }

  if (command === 'cycle_run') {
    return {
      allowBulkLiquidation: args.allowBulkLiquidation,
    };
  }

  if (command === 'memory_delete') {
    return { id: args.id };
  }

  if (command === 'memory_search') {
    return { query: args.query, symbol: args.symbol, k: args.k };
  }

  if (command === 'strategy_coach_propose') {
    return { message: args.message, history: args.history };
  }

  if (command === 'coach_turn') {
    return { sessionId: args.sessionId, message: args.message };
  }

  if (command === 'coach_get_session' || command === 'coach_delete_session') {
    return { id: args.id };
  }

  if (command === 'coach_execute_trade' || command === 'coach_cancel_trade') {
    return { proposalId: args.proposalId };
  }

  if (command === 'strategy_coach_apply') {
    return {
      selected: args.selected,
      rationale: args.rationale,
      summary: args.summary,
      chatExcerpt: args.chatExcerpt,
    };
  }

  return args;
}

export async function api<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri()) {
    return invoke<T>(command, tauriArgs(command, args));
  }
  return httpInvoke<T>(command, args);
}

export interface AppSettings {
  pulseSupabaseUrl?: string;
  pulseSupabaseAnonKey?: string;
  pulseEmail?: string;
  pulseBaseUrl: string;
  llmProvider: string;
  llmModel: string;
  llmBaseUrl?: string;
  llmTemperature?: number | null;
  pulseConfigured: boolean;
  llmConfigured: boolean;
  onboardingComplete: boolean;
  defaultStartingCapital: number;
  simulatedSlippageBps: number;
  simulatedFeePct: number;
  autoCycleEnabled: boolean;
  autoCycleIntervalMinutes: number;
  selectedBroker?: string;
  wealthEmail?: string | null;
  wealthConnected?: boolean;
  bambooPhone?: string | null;
  bambooConnected?: boolean;
  liveTradingEnabled?: boolean;
  maxLiveNotional?: number;
  maxLiveActions?: number;
  retainRawLlmLogs?: boolean;
  haltNewBuys?: boolean;
  flattenOnDrawdownArmed?: boolean;
  launchAtLogin?: boolean;
  assetClass?: 'stocks' | 'crypto' | string;
  cryptoSession?: 'connected' | 'expiredNeedsReconnect' | 'disconnected' | string;
}

export interface PortfolioData {
  portfolio: { id: string; cash_balance: number; starting_capital: number };
  positions: { symbol: string; quantity: number; avg_cost: number; current_price: number; market_value: number }[];
  total_equity: number;
  market_value: number;
  pnl_today: number;
  unrealized_pnl?: number;
  quotesAsOf?: string | null;
  stale?: boolean;
  tradingMode?: 'sandbox' | 'live' | string;
  assetClass?: 'stocks' | 'crypto' | string;
  cryptoSession?: 'connected' | 'expiredNeedsReconnect' | 'disconnected' | string;
  brokerId?: string | null;
  brokerName?: string | null;
  tradingVerified?: boolean;
  wealthStatus?: {
    connected?: boolean;
    tradingVerified?: boolean;
    message?: string;
  } | null;
  wealthError?: string | null;
}
