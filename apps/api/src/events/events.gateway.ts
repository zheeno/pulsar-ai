import { WebSocketGateway, WebSocketServer } from '@nestjs/websockets';
import { Server } from 'socket.io';

@WebSocketGateway({ cors: { origin: '*' } })
export class EventsGateway {
  @WebSocketServer()
  server!: Server;

  broadcastSignal(data: unknown) {
    this.server?.emit('signal:new', data);
  }

  broadcastTrade(data: unknown) {
    this.server?.emit('trade:new', data);
  }

  broadcastPortfolio(data: unknown) {
    this.server?.emit('portfolio:update', data);
  }
}
