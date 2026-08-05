import { IngestionService } from './ingestion.service';

describe('IngestionService error handling', () => {
  const db = { query: jest.fn() };
  const redis = { set: jest.fn() };
  const ngx = {
    getStocks: jest.fn(),
    getMarket: jest.fn(),
    getIndices: jest.fn(),
  };
  const calendar = {
    isMarketOpen: jest.fn().mockReturnValue(true),
    isTradingDay: jest.fn().mockReturnValue(true),
    todayWAT: jest.fn().mockReturnValue('2026-08-05'),
    toWAT: jest.fn(),
  };

  const service = new IngestionService(
    db as never,
    redis as never,
    ngx as never,
    calendar as never,
  );

  beforeEach(() => {
    jest.clearAllMocks();
  });

  it('should return 0 when NGX stock fetch fails instead of throwing', async () => {
    ngx.getStocks.mockRejectedValue(new Error('NGX Pulse error: 401 Unauthorized'));
    const count = await service.ingestStocks({ force: true });
    expect(count).toBe(0);
  });

  it('should not throw when NGX market fetch fails', async () => {
    ngx.getMarket.mockRejectedValue(new Error('NGX Pulse error: 401 Unauthorized'));
    await expect(service.ingestMarket({ force: true })).resolves.toBeUndefined();
  });

  it('should not throw when NGX indices fetch fails', async () => {
    ngx.getIndices.mockRejectedValue(new Error('NGX Pulse error: 401 Unauthorized'));
    await expect(service.ingestIndices({ force: true })).resolves.toBeUndefined();
  });
});
