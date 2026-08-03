import { FillSimulatorService } from './fill-simulator.service';

describe('FillSimulatorService', () => {
  const service = new FillSimulatorService();

  it('should apply slippage against buyer', () => {
    const result = service.simulateFill('BUY', 100);
    expect(result.fillPrice).toBeGreaterThan(100);
    expect(result.slippageBps).toBe(10);
  });

  it('should apply slippage against seller', () => {
    const result = service.simulateFill('SELL', 100);
    expect(result.fillPrice).toBeLessThan(100);
  });

  it('should calculate fee as percentage of notional', () => {
    const fee = service.calculateFee(1000000);
    expect(fee).toBe(1500); // 0.15%
  });
});
