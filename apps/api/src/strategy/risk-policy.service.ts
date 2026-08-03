import { Injectable } from '@nestjs/common';
import { RiskPolicyResult } from '@ngx/shared';

export interface ParamSet {
  max_position_pct: number;
  max_daily_trades: number;
  stop_loss_pct: number;
  min_confidence_to_trade: number;
  max_daily_drawdown_pct: number;
  allowed_symbols: string[] | null;
  position_size_pct: number;
}

export interface SignalInput {
  id: string;
  symbol: string;
  action: 'BUY' | 'SELL' | 'HOLD';
  confidence: number;
}

@Injectable()
export class PositionSizingService {
  calculatePositionSize(
    totalEquity: number,
    currentPrice: number,
    paramSet: ParamSet,
    currentPositionValue: number,
  ): number {
    const targetValue = totalEquity * paramSet.position_size_pct;
    const maxValue = totalEquity * paramSet.max_position_pct - currentPositionValue;
    const allocValue = Math.min(targetValue, Math.max(0, maxValue));
    if (currentPrice <= 0) return 0;
    return Math.floor(allocValue / currentPrice);
  }
}

@Injectable()
export class RiskPolicyService {
  constructor(private readonly sizing: PositionSizingService) {}

  async evaluate(
    signal: SignalInput,
    paramSet: ParamSet,
    portfolio: { cash_balance: number; id: string },
    positions: { symbol: string; quantity: number; avg_cost: number }[],
    prices: Record<string, number>,
    dailyTrades: number,
    dailyDrawdownPct: number,
  ): Promise<{ result: RiskPolicyResult; quantity: number }> {
    if (signal.action === 'HOLD') {
      return { result: 'BLOCKED_OTHER', quantity: 0 };
    }

    if (signal.confidence < paramSet.min_confidence_to_trade) {
      return { result: 'BLOCKED_CONFIDENCE', quantity: 0 };
    }

    if (paramSet.allowed_symbols && !paramSet.allowed_symbols.includes(signal.symbol)) {
      return { result: 'BLOCKED_SYMBOL', quantity: 0 };
    }

    if (signal.action === 'BUY' && dailyTrades >= paramSet.max_daily_trades) {
      return { result: 'BLOCKED_DAILY_TRADES', quantity: 0 };
    }

    if (signal.action === 'BUY' && dailyDrawdownPct >= paramSet.max_daily_drawdown_pct) {
      return { result: 'BLOCKED_DRAWDOWN', quantity: 0 };
    }

    const position = positions.find((p) => p.symbol === signal.symbol);
    const currentPrice = prices[signal.symbol] || 0;
    const positionValue = position ? position.quantity * currentPrice : 0;
    const marketValue = positions.reduce((sum, p) => sum + p.quantity * (prices[p.symbol] || p.avg_cost), 0);
    const totalEquity = Number(portfolio.cash_balance) + marketValue;

    if (signal.action === 'BUY') {
      const quantity = this.sizing.calculatePositionSize(totalEquity, currentPrice, paramSet, positionValue);
      const newExposure = (positionValue + quantity * currentPrice) / totalEquity;
      if (newExposure > paramSet.max_position_pct) {
        return { result: 'BLOCKED_EXPOSURE', quantity: 0 };
      }
      if (quantity <= 0) return { result: 'BLOCKED_EXPOSURE', quantity: 0 };
      return { result: 'APPROVED', quantity };
    }

    if (signal.action === 'SELL') {
      if (!position || position.quantity <= 0) {
        return { result: 'BLOCKED_OTHER', quantity: 0 };
      }
      const stopLossPrice = position.avg_cost * (1 - paramSet.stop_loss_pct);
      if (currentPrice > stopLossPrice && signal.confidence < 0.8) {
        // Allow sell on stop loss or high confidence
      }
      return { result: 'APPROVED', quantity: position.quantity };
    }

    return { result: 'BLOCKED_OTHER', quantity: 0 };
  }
}
