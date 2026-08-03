import { PositionSizingService, RiskPolicyService } from './risk-policy.service';

describe('PositionSizingService', () => {
  const service = new PositionSizingService();
  const paramSet = {
    max_position_pct: 0.1,
    max_daily_trades: 5,
    stop_loss_pct: 0.05,
    min_confidence_to_trade: 0.65,
    max_daily_drawdown_pct: 0.03,
    allowed_symbols: ['DANGCEM'],
    position_size_pct: 0.05,
  };

  it('should calculate position size based on equity percentage', () => {
    const qty = service.calculatePositionSize(10000000, 100, paramSet, 0);
    expect(qty).toBe(5000); // 5% of 10M = 500k / 100 = 5000 shares
  });

  it('should cap position size at max_position_pct', () => {
    const qty = service.calculatePositionSize(10000000, 100, paramSet, 900000);
    expect(qty).toBe(1000); // remaining room: 1M / 100 = 10000, but target is 5000... actually 10% - 9% = 1% = 100k/100 = 1000
  });

  it('should return 0 when price is 0', () => {
    expect(service.calculatePositionSize(10000000, 0, paramSet, 0)).toBe(0);
  });
});

describe('RiskPolicyService', () => {
  const sizing = new PositionSizingService();
  const service = new RiskPolicyService(sizing);
  const paramSet = {
    max_position_pct: 0.1,
    max_daily_trades: 5,
    stop_loss_pct: 0.05,
    min_confidence_to_trade: 0.65,
    max_daily_drawdown_pct: 0.03,
    allowed_symbols: ['DANGCEM'],
    position_size_pct: 0.05,
  };

  it('should block HOLD signals', async () => {
    const result = await service.evaluate(
      { id: '1', symbol: 'DANGCEM', action: 'HOLD', confidence: 0.9 },
      paramSet, { cash_balance: 10000000, id: 'p1' }, [], { DANGCEM: 100 }, 0, 0,
    );
    expect(result.result).toBe('BLOCKED_OTHER');
  });

  it('should block low confidence signals', async () => {
    const result = await service.evaluate(
      { id: '1', symbol: 'DANGCEM', action: 'BUY', confidence: 0.5 },
      paramSet, { cash_balance: 10000000, id: 'p1' }, [], { DANGCEM: 100 }, 0, 0,
    );
    expect(result.result).toBe('BLOCKED_CONFIDENCE');
  });

  it('should approve high confidence BUY', async () => {
    const result = await service.evaluate(
      { id: '1', symbol: 'DANGCEM', action: 'BUY', confidence: 0.75 },
      paramSet, { cash_balance: 10000000, id: 'p1' }, [], { DANGCEM: 100 }, 0, 0,
    );
    expect(result.result).toBe('APPROVED');
    expect(result.quantity).toBeGreaterThan(0);
  });

  it('should block when daily trade cap reached', async () => {
    const result = await service.evaluate(
      { id: '1', symbol: 'DANGCEM', action: 'BUY', confidence: 0.75 },
      paramSet, { cash_balance: 10000000, id: 'p1' }, [], { DANGCEM: 100 }, 5, 0,
    );
    expect(result.result).toBe('BLOCKED_DAILY_TRADES');
  });

  it('should block BUY when drawdown circuit breaker tripped', async () => {
    const result = await service.evaluate(
      { id: '1', symbol: 'DANGCEM', action: 'BUY', confidence: 0.75 },
      paramSet, { cash_balance: 10000000, id: 'p1' }, [], { DANGCEM: 100 }, 0, 0.05,
    );
    expect(result.result).toBe('BLOCKED_DRAWDOWN');
  });

  it('should block symbols not in allowlist', async () => {
    const result = await service.evaluate(
      { id: '1', symbol: 'UNKNOWN', action: 'BUY', confidence: 0.75 },
      paramSet, { cash_balance: 10000000, id: 'p1' }, [], { UNKNOWN: 100 }, 0, 0,
    );
    expect(result.result).toBe('BLOCKED_SYMBOL');
  });
});
