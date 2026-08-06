import { Injectable, Logger } from '@nestjs/common';
import { logStart } from '../common/log.util';

const USER_AGENT =
  'Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36';

const TOKEN_REFRESH_BUFFER_SEC = 300;

interface SessionTokens {
  accessToken: string;
  refreshToken: string;
  expiresAt: number;
}

interface SupabaseAuthResponse {
  access_token: string;
  refresh_token: string;
  expires_at: number;
  expires_in?: number;
}

@Injectable()
export class NgxPulseSessionService {
  private readonly logger = new Logger(NgxPulseSessionService.name);
  private readonly supabaseUrl: string;
  private readonly anonKey: string;
  private readonly email: string;
  private readonly password: string;
  private readonly configured: boolean;

  private tokens: SessionTokens | null = null;
  private loginPromise: Promise<string> | null = null;

  constructor() {
    this.supabaseUrl = process.env.NGX_PULSE_SUPABASE_URL || '';
    this.anonKey = process.env.NGX_PULSE_SUPABASE_ANON_KEY || '';
    this.email = process.env.NGX_PULSE_EMAIL || '';
    this.password = process.env.NGX_PULSE_PASSWORD || '';
    this.configured = Boolean(
      this.supabaseUrl &&
        this.anonKey &&
        this.email &&
        this.password &&
        this.anonKey !== 'your-ngx-pulse-supabase-anon-key',
    );
  }

  isConfigured(): boolean {
    return this.configured;
  }

  async getAccessToken(): Promise<string> {
    if (!this.configured) {
      throw new Error('NGX Pulse session credentials are not configured');
    }

    if (this.tokens && !this.isExpiringSoon(this.tokens.expiresAt)) {
      return this.tokens.accessToken;
    }

    if (this.loginPromise) {
      return this.loginPromise;
    }

    this.loginPromise = this.ensureSession().finally(() => {
      this.loginPromise = null;
    });
    return this.loginPromise;
  }

  async invalidate(): Promise<void> {
    this.tokens = null;
  }

  private async ensureSession(): Promise<string> {
    if (this.tokens && !this.isExpiringSoon(this.tokens.expiresAt)) {
      return this.tokens.accessToken;
    }

    if (this.tokens?.refreshToken) {
      try {
        await this.refreshSession(this.tokens.refreshToken);
        return this.tokens.accessToken;
      } catch (err) {
        this.logger.warn(
          `NGX Pulse token refresh failed, re-logging in: ${err instanceof Error ? err.message : err}`,
        );
        this.tokens = null;
      }
    }

    await this.login();
    if (!this.tokens) {
      throw new Error('NGX Pulse login did not return a session');
    }
    return this.tokens.accessToken;
  }

  private isExpiringSoon(expiresAt: number): boolean {
    const now = Math.floor(Date.now() / 1000);
    return expiresAt - now <= TOKEN_REFRESH_BUFFER_SEC;
  }

  private supabaseHeaders(): Record<string, string> {
    return {
      accept: '*/*',
      'accept-language': 'en-US,en;q=0.9,ha;q=0.8',
      apikey: this.anonKey,
      authorization: `Bearer ${this.anonKey}`,
      'content-type': 'application/json;charset=UTF-8',
      origin: 'https://ngxpulse.ng',
      referer: 'https://ngxpulse.ng/',
      'user-agent': USER_AGENT,
      'x-client-info': 'supabase-js/2.112.1; runtime=web',
      'x-supabase-api-version': '2024-01-01',
    };
  }

  private async login(): Promise<void> {
    const log = logStart(this.logger, 'login', { email: this.email });
    const url = `${this.supabaseUrl.replace(/\/$/, '')}/auth/v1/token?grant_type=password`;
    const res = await fetch(url, {
      method: 'POST',
      headers: this.supabaseHeaders(),
      body: JSON.stringify({
        email: this.email,
        password: this.password,
        gotrue_meta_security: {},
      }),
    });

    const body = await this.parseAuthResponse(res);
    this.tokens = this.toSessionTokens(body);
    log.done({ expiresAt: this.tokens.expiresAt });
  }

  private async refreshSession(refreshToken: string): Promise<void> {
    const log = logStart(this.logger, 'refreshSession');
    const url = `${this.supabaseUrl.replace(/\/$/, '')}/auth/v1/token?grant_type=refresh_token`;
    const res = await fetch(url, {
      method: 'POST',
      headers: this.supabaseHeaders(),
      body: JSON.stringify({ refresh_token: refreshToken }),
    });

    const body = await this.parseAuthResponse(res);
    this.tokens = this.toSessionTokens(body);
    log.done({ expiresAt: this.tokens.expiresAt });
  }

  private async parseAuthResponse(res: Response): Promise<SupabaseAuthResponse> {
    const text = await res.text();
    let body: SupabaseAuthResponse & { error_description?: string; msg?: string; message?: string };
    try {
      body = JSON.parse(text);
    } catch {
      throw new Error(`NGX Pulse auth returned non-JSON (${res.status})`);
    }

    if (!res.ok) {
      const message = body.error_description || body.msg || body.message || text;
      throw new Error(`NGX Pulse auth failed (${res.status}): ${message}`);
    }

    if (!body.access_token || !body.refresh_token) {
      throw new Error('NGX Pulse auth response missing tokens');
    }

    return body;
  }

  private toSessionTokens(body: SupabaseAuthResponse): SessionTokens {
    const expiresAt =
      body.expires_at ??
      Math.floor(Date.now() / 1000) + (body.expires_in ?? 3600);

    return {
      accessToken: body.access_token,
      refreshToken: body.refresh_token,
      expiresAt,
    };
  }
}
