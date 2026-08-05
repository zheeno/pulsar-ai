import {
  CallHandler,
  ExecutionContext,
  Injectable,
  Logger,
  NestInterceptor,
} from '@nestjs/common';
import { Observable, tap } from 'rxjs';
import { Request, Response } from 'express';

@Injectable()
export class LoggingInterceptor implements NestInterceptor {
  private readonly logger = new Logger('HTTP');

  intercept(context: ExecutionContext, next: CallHandler): Observable<unknown> {
    const http = context.switchToHttp();
    const req = http.getRequest<Request>();
    const res = http.getResponse<Response>();
    const { method, originalUrl } = req;
    const started = Date.now();

    this.logger.log(`→ ${method} ${originalUrl}`);

    return next.handle().pipe(
      tap({
        next: () => {
          const ms = Date.now() - started;
          this.logger.log(`✓ ${method} ${originalUrl} ${res.statusCode} ${ms}ms`);
        },
        error: (err: Error) => {
          const ms = Date.now() - started;
          this.logger.error(`✗ ${method} ${originalUrl} ${res.statusCode} ${ms}ms — ${err.message}`);
        },
      }),
    );
  }
}
