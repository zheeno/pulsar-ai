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

  it('should pick symbols from the full universe in portfolio mode', async () => {
    const result = await service.generatePortfolioSignals({
      maxPicks: 3,
      universe: [
        { symbol: 'DANGCEM', price: 1015, change_percent: 2.5, volume: 100000 },
        { symbol: 'MTNN', price: 845, change_percent: -1.2, volume: 50000 },
        { symbol: 'GTCO', price: 46, change_percent: 0.1, volume: 20000 },
      ],
      positions: [],
    });
    expect(result.output.signals.length).toBeGreaterThan(0);
    expect(result.output.signals[0].symbol).toBeTruthy();
  });
});
