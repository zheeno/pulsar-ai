import { Logger } from '@nestjs/common';
import { WebSocketGateway, WebSocketServer } from '@nestjs/websockets';
import { Server } from 'socket.io';
import { logStart } from '../common/log.util';

@WebSocketGateway({ cors: { origin: '*' } })
export class EventsGateway {
  private readonly logger = new Logger(EventsGateway.name);

  @WebSocketServer()
  server!: Server;

  broadcastSignal(data: unknown) {
    const log = logStart(this.logger, 'broadcastSignal', { data });
    this.server?.emit('signal:new', data);
    log.done();
  }

  broadcastTrade(data: unknown) {
    const log = logStart(this.logger, 'broadcastTrade', { data });
    this.server?.emit('trade:new', data);
    log.done();
  }

  broadcastPortfolio(data: unknown) {
    const log = logStart(this.logger, 'broadcastPortfolio', { data });
    this.server?.emit('portfolio:update', data);
    log.done();
  }
}
