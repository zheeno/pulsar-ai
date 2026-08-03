import { Module, forwardRef } from '@nestjs/common';
import { BullModule } from '@nestjs/bullmq';
import { IndicatorService } from './indicator.service';
import { LlmService } from './llm.service';
import { SignalGenerationService } from './signal-generation.service';
import { SignalGenerationProcessor } from './signal-generation.processor';
import { EventsGateway } from '../events/events.gateway';

export const SIGNAL_GENERATION_QUEUE = 'signal-generation';

@Module({
  imports: [BullModule.registerQueue({ name: SIGNAL_GENERATION_QUEUE })],
  providers: [
    IndicatorService,
    LlmService,
    SignalGenerationService,
    SignalGenerationProcessor,
    EventsGateway,
  ],
  exports: [SignalGenerationService, IndicatorService, LlmService, BullModule],
})
export class SignalGenerationModule {}
