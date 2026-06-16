# Pulse Chat — WebSocket demo

Chat en tiempo real sobre `pulse_core`, en **un solo binario y sin
infraestructura** (ni Postgres ni Redis).

## Qué demuestra

Tras desacoplar el motor, puedes consumir **piezas sueltas** del framework. Este
ejemplo usa SOLO `pulse_core::auth::jwt` y nada más:

- **Auth unificada HTTP↔WS:** el handshake del socket (`/ws?token=…`) se valida
  con el mismo JWT que emite el framework (`IdentityProvider::verify_token`).
- **À-la-carte:** sin `bootstrap`, sin base de datos. Prueba de que el core es
  modular.
- **Fan-out lock-free:** difusión en proceso con `tokio::sync::broadcast`. Para
  multi-nodo, ese `Sender` se cambia por `pulse_core::pulse` (PulseReactor sobre
  Redis) sin tocar la lógica de sesión.

## Ejecutar

```bash
cargo run        # http://127.0.0.1:8090
```

Abre dos pestañas, elige un nombre en cada una y chatea. (Opcional:
`JWT_SECRET=...` para fijar el secreto; por defecto usa uno de demo.)

## Endpoints

| Ruta             | Método | Descripción                                   |
| ---------------- | ------ | --------------------------------------------- |
| `/`              | GET    | Cliente web (vanilla JS).                     |
| `/login`         | POST   | `{username}` → `{token}` (JWT del framework). |
| `/ws?token=JWT`  | GET    | WebSocket; difunde mensajes a todos.          |
