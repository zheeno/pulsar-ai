import { Inject, Injectable, Logger, OnModuleDestroy } from '@nestjs/common';
import { Pool, PoolClient, QueryResult, QueryResultRow } from 'pg';
import { logStart } from '../common/log.util';

@Injectable()
export class DatabaseService implements OnModuleDestroy {
  private readonly logger = new Logger(DatabaseService.name);

  constructor(@Inject('PG_POOL') private readonly pool: Pool) {}

  async query<T extends QueryResultRow = QueryResultRow>(text: string, params?: unknown[]): Promise<QueryResult<T>> {
    return this.pool.query<T>(text, params);
  }

  async getClient(): Promise<PoolClient> {
    return this.pool.connect();
  }

  async transaction<T>(fn: (client: PoolClient) => Promise<T>): Promise<T> {
    const log = logStart(this.logger, 'transaction');
    const client = await this.getClient();
    try {
      await client.query('BEGIN');
      const result = await fn(client);
      await client.query('COMMIT');
      log.done({ committed: true });
      return result;
    } catch (err) {
      await client.query('ROLLBACK');
      log.fail(err);
      throw err;
    } finally {
      client.release();
    }
  }

  async onModuleDestroy() {
    this.logger.log('Closing database pool');
    await this.pool.end();
  }
}
