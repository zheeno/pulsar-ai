import { Global, Module } from '@nestjs/common';
import { Pool } from 'pg';
import { DatabaseService } from './database.service';

@Global()
@Module({
  providers: [
    {
      provide: 'PG_POOL',
      useFactory: () => {
        const connectionString = process.env.DATABASE_URL || 'postgresql://postgres:postgres@localhost:5432/ngx_trading';
        return new Pool({ connectionString });
      },
    },
    DatabaseService,
  ],
  exports: ['PG_POOL', DatabaseService],
})
export class DatabaseModule {}
