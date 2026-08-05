import { NestFactory } from '@nestjs/core';
import { Logger } from '@nestjs/common';
import { AppModule } from './app.module';
import { LoggingInterceptor } from './common/logging.interceptor';

function getAllowedOrigins(): string[] {
  const raw = process.env.CORS_ORIGIN || 'http://localhost:3000';
  return raw.split(',').map((o) => o.trim()).filter(Boolean);
}

async function bootstrap() {
  const logger = new Logger('Bootstrap');
  const allowedOrigins = getAllowedOrigins();

  const app = await NestFactory.create(AppModule);
  app.useGlobalInterceptors(new LoggingInterceptor());
  app.enableCors({
    origin: (origin, callback) => {
      // Allow non-browser clients (no Origin header) and whitelisted web origins
      if (!origin || allowedOrigins.includes(origin)) {
        callback(null, origin ?? allowedOrigins[0]);
        return;
      }
      logger.warn(`Blocked CORS request from origin: ${origin}`);
      callback(null, false);
    },
    methods: ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'],
    allowedHeaders: ['Content-Type', 'Authorization', 'Accept'],
    credentials: false,
  });
  app.setGlobalPrefix('api');
  const port = process.env.API_PORT || 3001;
  await app.listen(port);
  logger.log(`API running on http://localhost:${port}`);
  logger.log(`CORS allowed origins: ${allowedOrigins.join(', ')}`);
}
bootstrap();
