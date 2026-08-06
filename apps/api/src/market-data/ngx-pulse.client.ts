import { Injectable, Logger } from '@nestjs/common';
import { RateLimitService } from './rate-limit.service';
import { NgxPulseSessionService } from './ngx-pulse-session.service';
import { logStart } from '../common/log.util';

export interface NgxStock {
  symbol: string;
  name?: string;
  price: number;
  change_percent?: number;
  volume?: number;
  market_cap?: number;
  pe_ratio?: number;
  sector?: string;
}

export interface NgxMarketOverview {
  asi?: { value: number; change_percent?: number };
  market_cap?: number;
}

export interface NgxIndex {
  code: string;
  value: number;
  points?: number;
  week_change?: number;
  month_change?: number;
  year_change?: number;
}

export interface NgxMarketStatus {
  status: string;
  is_open: boolean;
  raw_status?: string;
}

type AuthMode = 'session' | 'api_key' | 'mock';

const USER_AGENT =
  'Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36';

@Injectable()
export class NgxPulseClient {
  private readonly logger = new Logger(NgxPulseClient.name);
  private readonly baseUrl: string;
  private readonly apiKey: string;
  private readonly authMode: AuthMode;

  constructor(
    private readonly rateLimit: RateLimitService,
    private readonly session: NgxPulseSessionService,
  ) {
    this.baseUrl = process.env.NGX_PULSE_BASE_URL || 'https://ngxpulse.ng/api';
    this.apiKey = process.env.NGX_PULSE_API_KEY || '';

    if (this.session.isConfigured()) {
      this.authMode = 'session';
      this.logger.log('NGX Pulse using web session auth');
    } else if (this.apiKey && this.apiKey !== 'your-ngx-pulse-api-key') {
      this.authMode = 'api_key';
      this.logger.log('NGX Pulse using API key auth');
    } else {
      this.authMode = 'mock';
      this.logger.warn('NGX Pulse credentials not set — using mock data');
    }
  }

  getAuthMode(): AuthMode {
    return this.authMode;
  }

  async getMarketStatus(): Promise<NgxMarketStatus> {
    const log = logStart(this.logger, 'getMarketStatus', { mock: this.authMode === 'mock' });
    if (this.authMode === 'mock') {
      const result = { status: 'Open', is_open: true };
      log.done(result);
      return result;
    }
    const raw = await this.fetchRaw('/ngxdata/market-status', 'market-status');
    const data = this.unwrap(raw) as {
      status?: string;
      is_open?: boolean;
      raw_status?: string;
    };
    const result = {
      status: data.status || 'Unknown',
      is_open: Boolean(data.is_open),
      raw_status: data.raw_status,
    };
    log.done(result);
    return result;
  }

  async getStocks(): Promise<NgxStock[]> {
    const log = logStart(this.logger, 'getStocks', { mock: this.authMode === 'mock' });
    const result = this.authMode === 'mock'
      ? this.mockStocks()
      : this.normalizeStocks(await this.fetchRaw('/ngxdata/stocks', 'stocks'));
    log.done({ count: result.length });
    return result;
  }

  async getMarket(): Promise<NgxMarketOverview> {
    const log = logStart(this.logger, 'getMarket', { mock: this.authMode === 'mock' });
    const result = this.authMode === 'mock'
      ? { asi: { value: 98000, change_percent: 0.5 } }
      : this.normalizeMarket(await this.fetchRaw('/ngxdata/market', 'market'));
    log.done({ asi: result.asi?.value });
    return result;
  }

  async getIndices(): Promise<NgxIndex[]> {
    const log = logStart(this.logger, 'getIndices', { mock: this.authMode === 'mock' });
    const result = this.authMode === 'mock'
      ? [{ code: 'ASI', value: 98000, points: 120 }]
      : this.normalizeIndices(await this.fetchRaw('/ngxdata/indices', 'indices'));
    log.done({ count: result.length });
    return result;
  }

  async getSymbolPrice(
    symbol: string,
    from?: string,
    to?: string,
  ): Promise<{ date: string; price: number; volume?: number }[]> {
    const log = logStart(this.logger, 'getSymbolPrice', {
      symbol,
      from,
      to,
      mock: this.authMode === 'mock',
    });
    if (this.authMode === 'mock') {
      const result = this.mockHistorical(symbol);
      log.done({ count: result.length });
      return result;
    }
    const params = new URLSearchParams();
    if (from) params.set('from', from);
    if (to) params.set('to', to);
    const qs = params.toString();
    const result = this.normalizePrices(
      await this.fetchRaw(`/ngxdata/prices/${symbol}${qs ? `?${qs}` : ''}`, `prices/${symbol}`),
    );
    log.done({ count: result.length });
    return result;
  }

  private async fetchRaw(path: string, endpoint: string): Promise<unknown> {
    const log = logStart(this.logger, 'fetch', { endpoint, authMode: this.authMode });
    if (this.authMode === 'api_key' && !(await this.rateLimit.canMakeRequest())) {
      log.fail(new Error('NGX Pulse rate limit exceeded'));
      throw new Error('NGX Pulse rate limit exceeded');
    }

    const url = `${this.baseUrl.replace(/\/$/, '')}${path}`;
    const token = this.authMode === 'session'
      ? await this.session.getAccessToken()
      : this.apiKey;

    let res = await fetch(url, {
      headers: this.requestHeaders(token),
    });

    if (res.status === 401 && this.authMode === 'session') {
      await this.session.invalidate();
      const retryToken = await this.session.getAccessToken();
      res = await fetch(url, {
        headers: this.requestHeaders(retryToken),
      });
    }

    await this.rateLimit.recordRequest(endpoint, this.authMode);

    if (!res.ok) {
      log.fail(new Error(`NGX Pulse error: ${res.status} ${res.statusText}`));
      throw new Error(`NGX Pulse error: ${res.status} ${res.statusText}`);
    }

    const data = await res.json();
    log.done({ status: res.status });
    return data;
  }

  private requestHeaders(token: string): Record<string, string> {
    return {
      Accept: 'application/json',
      Authorization: `Bearer ${token}`,
      Referer: 'https://ngxpulse.ng/',
      'User-Agent': USER_AGENT,
    };
  }

  private unwrap(payload: unknown): Record<string, unknown> {
    if (!payload || typeof payload !== 'object') return {};
    const record = payload as Record<string, unknown>;
    if (record.data && typeof record.data === 'object') {
      return record.data as Record<string, unknown>;
    }
    return record;
  }

  private normalizeStocks(payload: unknown): NgxStock[] {
    const data = this.unwrap(payload);
    const stocks = (data.stocks ?? payload) as Array<Record<string, unknown>>;
    if (!Array.isArray(stocks)) return [];

    return stocks.map((stock) => ({
      symbol: String(stock.symbol),
      name: stock.name ? String(stock.name) : undefined,
      price: Number(stock.current_price ?? stock.price ?? 0),
      change_percent: Number(stock.change_percent ?? stock.official_change_percent ?? 0),
      volume: stock.volume != null ? Number(stock.volume) : undefined,
      market_cap: stock.market_cap != null ? Number(stock.market_cap) : undefined,
      pe_ratio: stock.pe_ratio != null ? Number(stock.pe_ratio) : undefined,
      sector: stock.sector ? String(stock.sector) : undefined,
    }));
  }

  private normalizeMarket(payload: unknown): NgxMarketOverview {
    const data = this.unwrap(payload);
    const asiValue = typeof data.asi === 'object' && data.asi
      ? Number((data.asi as { value?: number }).value ?? 0)
      : Number(data.asi ?? 0);
    const changePercent = typeof data.asi === 'object' && data.asi
      ? Number((data.asi as { change_percent?: number }).change_percent ?? 0)
      : Number(data.pct_change ?? data.change_percent ?? 0);

    return {
      asi: asiValue ? { value: asiValue, change_percent: changePercent } : undefined,
      market_cap: data.market_cap != null ? Number(data.market_cap) : undefined,
    };
  }

  private normalizeIndices(payload: unknown): NgxIndex[] {
    const root = payload as Record<string, unknown>;
    const indices = (root.data ?? this.unwrap(payload)) as Array<Record<string, unknown>>;
    if (!Array.isArray(indices)) return [];

    return indices.map((idx) => ({
      code: String(idx.code),
      value: Number(idx.currentPrice ?? idx.value ?? 0),
      points: idx.points != null ? Number(idx.points) : undefined,
      week_change: idx.weekChange != null ? Number(idx.weekChange) : undefined,
      month_change: idx.monthChange != null ? Number(idx.monthChange) : undefined,
      year_change: idx.yearChange != null ? Number(idx.yearChange) : undefined,
    }));
  }

  private normalizePrices(payload: unknown): { date: string; price: number; volume?: number }[] {
    const root = payload as Record<string, unknown>;
    const prices = (root.prices ?? this.unwrap(payload)) as Array<Record<string, unknown>>;
    if (!Array.isArray(prices)) return [];

    return prices.map((row) => ({
      date: String(row.trade_date ?? row.date).split('T')[0],
      price: Number(row.close_price ?? row.price ?? 0),
      volume: row.volume != null ? Number(row.volume) : undefined,
    }));
  }

  private mockStocks(): NgxStock[] {
    const symbols = [
      'DANGCEM', 'GTCO', 'ZENITHBANK', 'MTNN', 'BUACEMENT',
      'ACCESSCORP', 'UBA', 'FBNH', 'SEPLAT', 'NESTLE',
    ];
    const prices: Record<string, number> = {
      DANGCEM: 285, GTCO: 46, ZENITHBANK: 39, MTNN: 225, BUACEMENT: 98,
      ACCESSCORP: 23, UBA: 29, FBNH: 19, SEPLAT: 3250, NESTLE: 1210,
    };
    return symbols.map((symbol) => ({
      symbol,
      price: prices[symbol] || 100,
      change_percent: (Math.random() - 0.5) * 2,
      volume: Math.floor(Math.random() * 1000000),
    }));
  }

  private mockHistorical(symbol: string): { date: string; price: number; volume?: number }[] {
    const rows = [];
    const today = new Date();
    let price = 100;
    for (let i = 30; i >= 0; i--) {
      const d = new Date(today);
      d.setDate(d.getDate() - i);
      price *= 1 + (Math.random() - 0.48) * 0.02;
      rows.push({ date: d.toISOString().split('T')[0], price: Math.round(price * 100) / 100 });
    }
    return rows;
  }
}
