import { Injectable, Logger } from '@nestjs/common';
import { RateLimitService } from './rate-limit.service';
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

@Injectable()
export class NgxPulseClient {
  private readonly logger = new Logger(NgxPulseClient.name);
  private readonly baseUrl: string;
  private readonly apiKey: string;
  private readonly useMock: boolean;

  constructor(private readonly rateLimit: RateLimitService) {
    this.baseUrl = process.env.NGX_PULSE_BASE_URL || 'https://www.ngxpulse.ng/api';
    this.apiKey = process.env.NGX_PULSE_API_KEY || '';
    this.useMock = !this.apiKey || this.apiKey === 'your-ngx-pulse-api-key';
    if (this.useMock) {
      this.logger.warn('NGX Pulse API key not set — using mock data');
    }
  }

  async getStocks(): Promise<NgxStock[]> {
    const log = logStart(this.logger, 'getStocks', { mock: this.useMock });
    const result = this.useMock ? this.mockStocks() : await this.fetch<NgxStock[]>('/ngxdata/stocks', 'stocks');
    log.done({ count: result.length });
    return result;
  }

  async getMarket(): Promise<NgxMarketOverview> {
    const log = logStart(this.logger, 'getMarket', { mock: this.useMock });
    const result = this.useMock
      ? { asi: { value: 98000, change_percent: 0.5 } }
      : await this.fetch<NgxMarketOverview>('/ngxdata/market', 'market');
    log.done({ asi: result.asi?.value });
    return result;
  }

  async getIndices(): Promise<NgxIndex[]> {
    const log = logStart(this.logger, 'getIndices', { mock: this.useMock });
    const result = this.useMock
      ? [{ code: 'ASI', value: 98000, points: 120 }]
      : await this.fetch<NgxIndex[]>('/ngxdata/indices', 'indices');
    log.done({ count: result.length });
    return result;
  }

  async getSymbolPrice(symbol: string, from?: string, to?: string): Promise<{ date: string; price: number; volume?: number }[]> {
    const log = logStart(this.logger, 'getSymbolPrice', { symbol, from, to, mock: this.useMock });
    if (this.useMock) {
      const result = this.mockHistorical(symbol);
      log.done({ count: result.length });
      return result;
    }
    const params = new URLSearchParams();
    if (from) params.set('from', from);
    if (to) params.set('to', to);
    const qs = params.toString();
    const result = await this.fetch<{ date: string; price: number; volume?: number }[]>(
      `/ngxdata/prices/${symbol}${qs ? `?${qs}` : ''}`,
      `prices/${symbol}`,
    );
    log.done({ count: result.length });
    return result;
  }

  private async fetch<T>(path: string, endpoint: string): Promise<T> {
    const log = logStart(this.logger, 'fetch', { endpoint });
    if (!(await this.rateLimit.canMakeRequest())) {
      log.fail(new Error('NGX Pulse rate limit exceeded'));
      throw new Error('NGX Pulse rate limit exceeded');
    }
    const url = `${this.baseUrl}${path}`;
    const res = await fetch(url, {
      headers: { Authorization: `Bearer ${this.apiKey}`, Accept: 'application/json' },
    });
    await this.rateLimit.recordRequest(endpoint);
    if (!res.ok) {
      log.fail(new Error(`NGX Pulse error: ${res.status} ${res.statusText}`));
      throw new Error(`NGX Pulse error: ${res.status} ${res.statusText}`);
    }
    const data = await res.json();
    log.done({ status: res.status });
    return (data.data ?? data) as T;
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
