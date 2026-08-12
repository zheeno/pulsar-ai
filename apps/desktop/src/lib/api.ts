import { invoke } from '@tauri-apps/api/core';
import { httpInvoke } from './http-bridge';

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** Map frontend camelCase args to Tauri command parameter names. */
function tauriArgs(command: string, args?: Record<string, unknown>): Record<string, unknown> | undefined {
  if (!args) return undefined;

  if (command === 'settings_set') {
    return { payload: args };
  }

  if (command === 'wealth_login' || command === 'wealth_verify_2fa') {
    return { payload: args };
  }

  if (command === 'start_backtest') {
    return {
      strategy_param_set_id: args.strategyParamSetId,
      start_date: args.startDate,
      end_date: args.endDate,
    };
  }

  if (command === 'get_backtest') {
    return { run_id: args.runId };
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
  pulseConfigured: boolean;
  llmConfigured: boolean;
  onboardingComplete: boolean;
  defaultStartingCapital: number;
  simulatedSlippageBps: number;
  simulatedFeePct: number;
  autoCycleEnabled: boolean;
  autoCycleIntervalMinutes: number;
  wealthEmail?: string | null;
  wealthConnected?: boolean;
}

export interface PortfolioData {
  portfolio: { id: string; cash_balance: number; starting_capital: number };
  positions: { symbol: string; quantity: number; avg_cost: number; current_price: number; market_value: number }[];
  total_equity: number;
  market_value: number;
  pnl_today: number;
  tradingMode?: 'sandbox' | 'live' | string;
  tradingVerified?: boolean;
  wealthStatus?: {
    connected?: boolean;
    tradingVerified?: boolean;
    message?: string;
  } | null;
  wealthError?: string | null;
}
