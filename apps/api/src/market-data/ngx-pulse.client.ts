import { Injectable, Logger } from '@nestjs/common';
import { RateLimitService } from './rate-limit.service';

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
    if (this.useMock) return this.mockStocks();
    return this.fetch<NgxStock[]>('/ngxdata/stocks', 'stocks');
  }

  async getMarket(): Promise<NgxMarketOverview> {
    if (this.useMock) return { asi: { value: 98000, change_percent: 0.5 } };
    return this.fetch<NgxMarketOverview>('/ngxdata/market', 'market');
  }

  async getIndices(): Promise<NgxIndex[]> {
    if (this.useMock) return [{ code: 'ASI', value: 98000, points: 120 }];
    return this.fetch<NgxIndex[]>('/ngxdata/indices', 'indices');
  }

  async getSymbolPrice(symbol: string, from?: string, to?: string): Promise<{ date: string; price: number; volume?: number }[]> {
    if (this.useMock) return this.mockHistorical(symbol);
    const params = new URLSearchParams();
    if (from) params.set('from', from);
    if (to) params.set('to', to);
    const qs = params.toString();
    return this.fetch(`/ngxdata/prices/${symbol}${qs ? `?${qs}` : ''}`, `prices/${symbol}`);
  }

  private async fetch<T>(path: string, endpoint: string): Promise<T> {
    if (!(await this.rateLimit.canMakeRequest())) {
      throw new Error('NGX Pulse rate limit exceeded');
    }
    const url = `${this.baseUrl}${path}`;
    const res = await fetch(url, {
      headers: { Authorization: `Bearer ${this.apiKey}`, Accept: 'application/json' },
    });
    await this.rateLimit.recordRequest(endpoint);
    if (!res.ok) throw new Error(`NGX Pulse error: ${res.status} ${res.statusText}`);
    const data = await res.json();
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
