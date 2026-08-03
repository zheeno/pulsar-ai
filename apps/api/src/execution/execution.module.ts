import { Module } from '@nestjs/common';
import { BullModule } from '@nestjs/bullmq';
import { StrategyModule } from '../strategy/strategy.module';
import { FillSimulatorService } from './fill-simulator.service';
import { ExecutionService } from './execution.service';
import { ExecutionProcessor } from './execution.processor';
import { EventsGateway } from '../events/events.gateway';

export const EXECUTION_QUEUE = 'sandbox-execution';

@Module({
  imports: [
    StrategyModule,
    BullModule.registerQueue({ name: EXECUTION_QUEUE }),
  ],
  providers: [FillSimulatorService, ExecutionService, ExecutionProcessor, EventsGateway],
  exports: [ExecutionService, FillSimulatorService, BullModule],
})
export class ExecutionModule {}
