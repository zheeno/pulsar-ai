import { NestFactory } from '@nestjs/core';
import { Logger } from '@nestjs/common';
import { AppModule } from './app.module';
import { LoggingInterceptor } from './common/logging.interceptor';
import { getAllowedOrigins, isOriginAllowed } from './cors.util';

async function bootstrap() {
  const logger = new Logger('Bootstrap');
  const allowedOrigins = getAllowedOrigins();

  const app = await NestFactory.create(AppModule);
  app.useGlobalInterceptors(new LoggingInterceptor());
  app.enableCors({
    origin: (origin, callback) => {
      if (isOriginAllowed(origin, allowedOrigins)) {
        callback(null, origin ?? true);
        return;
      }
      logger.warn(`Blocked CORS request from origin: ${origin}`);
      callback(null, false);
    },
    methods: ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'],
    allowedHeaders: ['Content-Type', 'Authorization', 'Accept'],
    credentials: false,
    optionsSuccessStatus: 204,
  });
  app.setGlobalPrefix('api');
  const port = process.env.API_PORT || 3001;
  await app.listen(port);
  logger.log(`API running on http://localhost:${port}`);
  logger.log(`CORS allowed origins: ${allowedOrigins.join(', ')}`);
}
bootstrap();
