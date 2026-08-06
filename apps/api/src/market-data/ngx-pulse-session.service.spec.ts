import { NgxPulseSessionService } from './ngx-pulse-session.service';

describe('NgxPulseSessionService', () => {
  const originalEnv = process.env;
  const fetchMock = jest.fn();

  beforeEach(() => {
    jest.resetAllMocks();
    process.env = { ...originalEnv };
    global.fetch = fetchMock as typeof fetch;
  });

  afterAll(() => {
    process.env = originalEnv;
  });

  function mockAuthResponse(overrides: Record<string, unknown> = {}) {
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: async () =>
        JSON.stringify({
          access_token: 'access-1',
          refresh_token: 'refresh-1',
          expires_at: Math.floor(Date.now() / 1000) + 3600,
          ...overrides,
        }),
    });
  }

  it('reports not configured when session env vars are missing', () => {
    delete process.env.NGX_PULSE_SUPABASE_URL;
    delete process.env.NGX_PULSE_SUPABASE_ANON_KEY;
    delete process.env.NGX_PULSE_EMAIL;
    delete process.env.NGX_PULSE_PASSWORD;

    const service = new NgxPulseSessionService();
    expect(service.isConfigured()).toBe(false);
  });

  it('logs in and returns a cached access token', async () => {
    process.env.NGX_PULSE_SUPABASE_URL = 'https://example.supabase.co';
    process.env.NGX_PULSE_SUPABASE_ANON_KEY = 'anon-key';
    process.env.NGX_PULSE_EMAIL = 'user@example.com';
    process.env.NGX_PULSE_PASSWORD = 'secret';

    mockAuthResponse();

    const service = new NgxPulseSessionService();
    const token = await service.getAccessToken();

    expect(token).toBe('access-1');
    expect(fetchMock).toHaveBeenCalledTimes(1);

    const cached = await service.getAccessToken();
    expect(cached).toBe('access-1');
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('refreshes an expiring token', async () => {
    process.env.NGX_PULSE_SUPABASE_URL = 'https://example.supabase.co';
    process.env.NGX_PULSE_SUPABASE_ANON_KEY = 'anon-key';
    process.env.NGX_PULSE_EMAIL = 'user@example.com';
    process.env.NGX_PULSE_PASSWORD = 'secret';

    mockAuthResponse({ expires_at: Math.floor(Date.now() / 1000) + 60 });

    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: async () =>
        JSON.stringify({
          access_token: 'access-2',
          refresh_token: 'refresh-2',
          expires_at: Math.floor(Date.now() / 1000) + 3600,
        }),
    });

    const service = new NgxPulseSessionService();
    await service.getAccessToken();
    const token = await service.getAccessToken();

    expect(token).toBe('access-2');
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(fetchMock.mock.calls[1][0]).toContain('grant_type=refresh_token');
  });
});
