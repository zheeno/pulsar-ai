import { Injectable } from '@nestjs/common';

@Injectable()
export class FillSimulatorService {
  private readonly slippageBps = Number(process.env.SIMULATED_SLIPPAGE_BPS || 10);
  private readonly feePct = Number(process.env.SIMULATED_FEE_PCT || 0.0015);

  simulateFill(side: 'BUY' | 'SELL', price: number): { fillPrice: number; slippageBps: number; fee: number; quantity: number } {
    const slippageMultiplier = side === 'BUY'
      ? 1 + this.slippageBps / 10000
      : 1 - this.slippageBps / 10000;
    const fillPrice = price * slippageMultiplier;
    return { fillPrice, slippageBps: this.slippageBps, fee: 0, quantity: 0 };
  }

  calculateFee(notional: number): number {
    return notional * this.feePct;
  }
}
