import { NgxPulseClient } from './ngx-pulse.client';

describe('NgxPulseClient response normalization', () => {
  const rateLimit = {
    canMakeRequest: jest.fn().mockResolvedValue(true),
    recordRequest: jest.fn().mockResolvedValue(undefined),
  };
  const session = {
    isConfigured: jest.fn().mockReturnValue(true),
    getAccessToken: jest.fn().mockResolvedValue('session-token'),
    invalidate: jest.fn().mockResolvedValue(undefined),
  };

  const originalEnv = process.env;
  const fetchMock = jest.fn();

  beforeEach(() => {
    jest.resetAllMocks();
    process.env = {
      ...originalEnv,
      NGX_PULSE_SUPABASE_URL: 'https://example.supabase.co',
      NGX_PULSE_SUPABASE_ANON_KEY: 'anon-key',
      NGX_PULSE_EMAIL: 'user@example.com',
      NGX_PULSE_PASSWORD: 'secret',
      NGX_PULSE_API_KEY: '',
    };
    global.fetch = fetchMock as typeof fetch;
    session.isConfigured.mockReturnValue(true);
    session.getAccessToken.mockResolvedValue('session-token');
    rateLimit.canMakeRequest.mockResolvedValue(true);
  });

  afterAll(() => {
    process.env = originalEnv;
  });

  function createClient() {
    return new NgxPulseClient(rateLimit as never, session as never);
  }

  it('normalizes stocks payload from web session API', async () => {
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        stocks: [
          {
            symbol: 'DANGCEM',
            name: 'Dangote Cement',
            current_price: 1015,
            change_percent: 1.2,
            volume: 1000,
            sector: 'Industrial Goods',
          },
        ],
      }),
    });

    const client = createClient();
    const stocks = await client.getStocks();

    expect(stocks).toEqual([
      expect.objectContaining({
        symbol: 'DANGCEM',
        price: 1015,
        change_percent: 1.2,
        volume: 1000,
        sector: 'Industrial Goods',
      }),
    ]);
  });

  it('normalizes market payload with flat asi value', async () => {
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        success: true,
        data: {
          asi: 244912.24,
          pct_change: 1.99,
          market_cap: 158086501465893.53,
        },
      }),
    });

    const client = createClient();
    const market = await client.getMarket();

    expect(market.asi).toEqual({ value: 244912.24, change_percent: 1.99 });
    expect(market.market_cap).toBe(158086501465893.53);
  });

  it('normalizes indices and prices payloads', async () => {
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        data: [
          {
            code: 'ASI',
            currentPrice: 244912.24,
            points: 7563,
            weekChange: -1.28,
          },
        ],
      }),
    });
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        prices: [
          {
            trade_date: '2026-07-07',
            close_price: 1015,
            volume: 875147,
          },
        ],
      }),
    });

    const client = createClient();
    const indices = await client.getIndices();
    const prices = await client.getSymbolPrice('DANGCEM');

    expect(indices[0]).toEqual(
      expect.objectContaining({ code: 'ASI', value: 244912.24, points: 7563, week_change: -1.28 }),
    );
    expect(prices[0]).toEqual({ date: '2026-07-07', price: 1015, volume: 875147 });
  });
});
