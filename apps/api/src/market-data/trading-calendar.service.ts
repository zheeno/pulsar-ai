import { Injectable } from '@nestjs/common';

// NGX public holidays (sample - extend as needed)
const NGX_HOLIDAYS_2025_2026 = new Set([
  '2025-01-01', '2025-04-18', '2025-04-21', '2025-05-01', '2025-06-12',
  '2025-10-01', '2025-12-25', '2025-12-26',
  '2026-01-01', '2026-04-03', '2026-04-06', '2026-05-01', '2026-06-12',
  '2026-10-01', '2026-12-25', '2026-12-26',
]);

@Injectable()
export class TradingCalendarService {
  isTradingDay(date: Date = new Date()): boolean {
    const wat = this.toWAT(date);
    const day = wat.getDay();
    if (day === 0 || day === 6) return false;
    const dateStr = this.formatDate(wat);
    return !NGX_HOLIDAYS_2025_2026.has(dateStr);
  }

  isMarketOpen(date: Date = new Date()): boolean {
    if (!this.isTradingDay(date)) return false;
    const wat = this.toWAT(date);
    const hour = wat.getHours();
    const minute = wat.getMinutes();
    const timeMinutes = hour * 60 + minute;
    return timeMinutes >= 9 * 60 && timeMinutes < 16 * 60;
  }

  toWAT(date: Date): Date {
    return new Date(date.toLocaleString('en-US', { timeZone: 'Africa/Lagos' }));
  }

  formatDate(date: Date): string {
    return date.toISOString().split('T')[0];
  }

  todayWAT(): string {
    return this.formatDate(this.toWAT(new Date()));
  }
}
