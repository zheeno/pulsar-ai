import { Module } from '@nestjs/common';
import { BullModule } from '@nestjs/bullmq';
import { IngestionService } from './ingestion.service';
import { NgxPulseClient } from './ngx-pulse.client';
import { RateLimitService } from './rate-limit.service';
import { TradingCalendarService } from './trading-calendar.service';
import { IngestionProcessor } from './ingestion.processor';

export const MARKET_DATA_QUEUE = 'market-data-ingest';

@Module({
  imports: [
    BullModule.registerQueue({ name: MARKET_DATA_QUEUE }),
  ],
  providers: [
    IngestionService,
    NgxPulseClient,
    RateLimitService,
    TradingCalendarService,
    IngestionProcessor,
  ],
  exports: [IngestionService, NgxPulseClient, RateLimitService, TradingCalendarService, BullModule],
})
export class MarketDataModule {}
