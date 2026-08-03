import { Module } from '@nestjs/common';
import { BullModule } from '@nestjs/bullmq';
import { MarketDataModule, MARKET_DATA_QUEUE } from '../market-data/market-data.module';
import { SignalGenerationModule, SIGNAL_GENERATION_QUEUE } from '../signal-generation/signal-generation.module';
import { ExecutionModule, EXECUTION_QUEUE } from '../execution/execution.module';
import { OrchestratorService } from './orchestrator.service';
import { DailySnapshotService } from './daily-snapshot.service';

@Module({
  imports: [
    MarketDataModule,
    SignalGenerationModule,
    ExecutionModule,
    BullModule.registerQueue({ name: MARKET_DATA_QUEUE }),
    BullModule.registerQueue({ name: SIGNAL_GENERATION_QUEUE }),
    BullModule.registerQueue({ name: EXECUTION_QUEUE }),
  ],
  providers: [OrchestratorService, DailySnapshotService],
  exports: [OrchestratorService, DailySnapshotService],
})
export class OrchestratorModule {}
