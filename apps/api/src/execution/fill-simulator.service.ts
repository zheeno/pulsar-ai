import { Injectable, Logger } from '@nestjs/common';
import { logStart } from '../common/log.util';

@Injectable()
export class FillSimulatorService {
  private readonly logger = new Logger(FillSimulatorService.name);
  private readonly slippageBps = Number(process.env.SIMULATED_SLIPPAGE_BPS || 10);
  private readonly feePct = Number(process.env.SIMULATED_FEE_PCT || 0.0015);

  simulateFill(side: 'BUY' | 'SELL', price: number): { fillPrice: number; slippageBps: number; fee: number; quantity: number } {
    const log = logStart(this.logger, 'simulateFill', { side, price });
    const slippageMultiplier = side === 'BUY'
      ? 1 + this.slippageBps / 10000
      : 1 - this.slippageBps / 10000;
    const fillPrice = price * slippageMultiplier;
    const result = { fillPrice, slippageBps: this.slippageBps, fee: 0, quantity: 0 };
    log.done({ fillPrice, slippageBps: this.slippageBps });
    return result;
  }

  calculateFee(notional: number): number {
    const log = logStart(this.logger, 'calculateFee', { notional });
    const fee = notional * this.feePct;
    log.done({ fee });
    return fee;
  }
}
