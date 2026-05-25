# Pulse — Guía de despliegue / endurecimiento

## Variables de entorno

### Obligatorias
| Variable | Descripción |
|---|---|
| `DATABASE_URL` | Cadena de conexión Postgres. |
| `JWT_SECRET` | Secreto de firma JWT. **≥16 caracteres** o el proceso no arranca. Usa un valor aleatorio (`openssl rand -base64 48`). |

### Recomendadas
| Variable | Default | Descripción |
|---|---|---|
| `REDIS_URL` | — | Activa cola/caché/pub-sub/orquestador distribuidos y revocación de tokens compartida. Sin esto, modo local de un solo nodo. |
| `PULSE_ADMIN_USERS` | (vacío) | Allowlist de usernames con rol `admin`, separada por comas. Sin esto **nadie** es admin (los endpoints `/admin/*` quedan inaccesibles). |
| `PULSE_CORS_ORIGINS` | (vacío) | Orígenes permitidos para CORS, separados por comas. Vacío = se rechazan peticiones cross-origin. |
| `PULSE_ACCESS_TTL_SECS` | `3600` | Vida del access token. |
| `PULSE_REFRESH_TTL_SECS` | `604800` | Vida del refresh token (7 días). |
| `PULSE_RATE_LIMIT_MAX` | `10` | Peticiones permitidas por ventana en `/auth/login` y registro. |
| `PULSE_RATE_LIMIT_WINDOW_SECS` | `60` | Tamaño de la ventana del rate limiter. |
| `RUST_LOG` | — | Filtro de tracing, p.ej. `info,pulse_core=debug`. |

## Gestión de secretos
- **Nunca** pongas `JWT_SECRET` en el repo ni en imágenes. Inyéctalo en runtime (Docker secrets, Kubernetes Secret, Vault, SSM Parameter Store).
- El proceso falla rápido si `JWT_SECRET` falta o es débil: es intencional.
- Rota el secreto invalidando sesiones (cambiarlo invalida todos los tokens existentes).

## TLS
El framework sirve **HTTP en claro**; termina TLS en un reverse proxy delante (patrón estándar):

- **Nginx / Caddy / Traefik**: TLS + reenvío a `127.0.0.1:8080`.
- En la nube: ALB/Cloud Load Balancer con certificado gestionado.
- Asegúrate de propagar `X-Forwarded-For` para que el rate limiter vea la IP real
  (Pulse usa `connection_info().realip_remote_addr()`).

Ejemplo mínimo Caddy:
```
api.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

## Observabilidad
- `GET /api/v1/health` — estado de DB y Redis (503 si algo está caído). Úsalo como liveness/readiness probe.
- `GET /api/v1/metrics` — formato de exposición Prometheus. **Restríngelo a la red interna** (no lo expongas públicamente vía el proxy).

## Apagado
El binario hace shutdown graceful con SIGINT/SIGTERM: deja de aceptar conexiones, espera hasta 30s a que terminen, y drena el worker de jobs (termina y hace ack del job en curso). Los jobs in-flight no completados se recuperan al reiniciar (`recover_stale`).

## Pendiente conocido (no apto aún para internet sin esto)
- Rate limiter es **por proceso**; para límite global multi-nodo, respaldarlo en Redis.
- La rotación del blackbox en disco conserva solo el segmento actual para `tail`/`replay`.
- Sin export OTLP de trazas (solo logs locales). Integrar `opentelemetry` si se requiere tracing distribuido.
