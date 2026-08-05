import { getAllowedOrigins, isOriginAllowed } from './cors.util';

describe('cors.util', () => {
  const originalEnv = process.env;

  beforeEach(() => {
    process.env = { ...originalEnv };
    delete process.env.CORS_ORIGIN;
    delete process.env.WEB_DOMAIN;
  });

  afterAll(() => {
    process.env = originalEnv;
  });

  it('includes CORS_ORIGIN entries', () => {
    process.env.CORS_ORIGIN = 'https://app.example.com, https://staging.example.com/';
    expect(getAllowedOrigins()).toEqual([
      'https://app.example.com',
      'https://staging.example.com',
    ]);
  });

  it('adds WEB_DOMAIN http/https and www variants', () => {
    process.env.WEB_DOMAIN = 'pulsar.antimony.com.ng';
    const origins = getAllowedOrigins();
    expect(origins).toContain('https://pulsar.antimony.com.ng');
    expect(origins).toContain('http://pulsar.antimony.com.ng');
    expect(origins).toContain('https://www.pulsar.antimony.com.ng');
    expect(origins).toContain('http://www.pulsar.antimony.com.ng');
  });

  it('defaults to localhost when unset', () => {
    expect(getAllowedOrigins()).toEqual(['http://localhost:3000']);
  });

  it('allows matching origins', () => {
    const allowed = ['https://pulsar.antimony.com.ng'];
    expect(isOriginAllowed('https://pulsar.antimony.com.ng', allowed)).toBe(true);
    expect(isOriginAllowed('https://pulsar.antimony.com.ng/', allowed)).toBe(true);
    expect(isOriginAllowed('https://evil.example.com', allowed)).toBe(false);
    expect(isOriginAllowed(undefined, allowed)).toBe(true);
  });
});
