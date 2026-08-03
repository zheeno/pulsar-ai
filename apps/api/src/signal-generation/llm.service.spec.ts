import { LlmService } from './llm.service';

describe('LlmService', () => {
  const service = new LlmService();

  it('should return valid mock signal for oversold conditions', async () => {
    const result = await service.generateSignal({
      symbol: 'DANGCEM',
      technical: { rsi14: 30, momentum: 2, sma50: 100, currentPrice: 105 },
    });
    expect(result.output.action).toBe('BUY');
    expect(result.output.confidence).toBeGreaterThan(0);
    expect(result.output.rationale).toContain('RSI');
  });

  it('should return SELL for overbought conditions', async () => {
    const result = await service.generateSignal({
      symbol: 'DANGCEM',
      technical: { rsi14: 75, momentum: -1, sma50: 100, currentPrice: 95 },
    });
    expect(result.output.action).toBe('SELL');
  });

  it('should return HOLD for mixed signals', async () => {
    const result = await service.generateSignal({
      symbol: 'DANGCEM',
      technical: { rsi14: 55, momentum: 0.2, sma50: 100, currentPrice: 99 },
    });
    expect(result.output.action).toBe('HOLD');
  });
});
