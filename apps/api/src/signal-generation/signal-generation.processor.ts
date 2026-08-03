import { Processor, WorkerHost } from '@nestjs/bullmq';
import { Logger } from '@nestjs/common';
import { Job } from 'bullmq';
import { SignalGenerationService } from './signal-generation.service';

@Processor('signal-generation')
export class SignalGenerationProcessor extends WorkerHost {
  private readonly logger = new Logger(SignalGenerationProcessor.name);

  constructor(private readonly service: SignalGenerationService) {
    super();
  }

  async process(job: Job<{ portfolioId?: string }>): Promise<string[]> {
    this.logger.log('Running signal generation');
    return this.service.generateForPortfolio(job.data.portfolioId);
  }
}
