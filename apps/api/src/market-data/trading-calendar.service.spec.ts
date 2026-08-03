import { Test, TestingModule } from '@nestjs/testing';
import { TradingCalendarService } from './trading-calendar.service';

describe('TradingCalendarService', () => {
  let service: TradingCalendarService;

  beforeEach(async () => {
    const module: TestingModule = await Test.createTestingModule({
      providers: [TradingCalendarService],
    }).compile();
    service = module.get(TradingCalendarService);
  });

  it('should identify weekends as non-trading days', () => {
    const saturday = new Date('2026-01-03T12:00:00Z');
    expect(service.isTradingDay(saturday)).toBe(false);
  });

  it('should identify weekdays as trading days (non-holiday)', () => {
    const tuesday = new Date('2026-01-06T12:00:00Z');
    expect(service.isTradingDay(tuesday)).toBe(true);
  });

  it('should identify holidays as non-trading days', () => {
    const newYear = new Date('2026-01-01T12:00:00Z');
    expect(service.isTradingDay(newYear)).toBe(false);
  });
});
