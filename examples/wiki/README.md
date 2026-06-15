# Pulse Wiki — demo app

Una **wiki funcional** construida sobre el framework `pulse_core` usado como
*crate* (path-dependency). Sirve para probar de punta a punta que el framework
vale como librería para una app real: auth, persistencia, paginación, caché,
métricas, salud y apagado limpio vienen del core; la wiki solo aporta su entidad,
su servicio y sus rutas.

## Qué demuestra

| Pieza del framework | Cómo la usa la wiki |
|---|---|
| `bootstrap()` | Arranca todo el server; la wiki solo le pasa su `configure` + OpenAPI |
| JWT / `Claims` extractor | **Lectura pública, escritura autenticada** (crear/editar/borrar exigen token) |
| `AppError` | Mapeo limpio a 400 / 401 / 404 / 409 / 500 |
| `core::query` | Listado paginado con el mismo `PaginatedResult` del core |
| `core::transaction::AtomicFlow` | Página + revisión se guardan en una transacción |
| `HybridStore` (caché L1/L2) | `GET /pages/{slug}` es read-through; las escrituras invalidan |
| sea-orm entities propias | Tablas `pages` y `page_revisions` con `ActiveModelBehavior` |
| Swagger UI | OpenAPI de la wiki **fusionada** con la del core en `/swagger-ui/` |

### Funcionalidades

- **Historial de revisiones:** cada guardado crea una versión inmutable. Desde una
  página, *History* lista las versiones; puedes ver cualquiera y **restaurarla**
  (lo que genera una nueva revisión, sin reescribir el historial).
- **Markdown** (renderizado en cliente, XSS-safe): encabezados `#`..`######`,
  `**negrita**`, `*cursiva*`, `~~tachado~~`, `` `código` ``, bloques ```` ``` ````,
  listas `-`/`1.`, citas `>`, reglas `---`, enlaces, imágenes `![](...)` y **tablas**.

## Arrancar (recomendado: Docker)

```bash
cd examples/wiki
./run.sh
```

Levanta Postgres + compila y corre la wiki dentro de la imagen `pulse-test`
(la misma de la suite de certificación, con Rust + openssl). Luego abre:

- **App:** http://localhost:8080
- **Swagger:** http://localhost:8080/swagger-ui/
- **Login de demo:** usuario `admin`, password `Str0ng-Pass1`

`Ctrl-C` para y limpia el Postgres (usa `KEEP_DB=1 ./run.sh` para conservarlo).

## Arrancar sin Docker

Necesitas Postgres accesible y `pkg-config` + `libssl-dev` en el host:

```bash
cp .env.example .env          # ajusta DATABASE_URL
cargo run --release
```

El binario crea el esquema (idempotente) y siembra el usuario admin + la página
`welcome` en el primer arranque.

## Rendimiento a escala

La app crea índices al arrancar (best-effort; ver `OPTIMIZATIONS` en `main.rs`) y
evita el coste O(n) de la paginación. Medido con **100.000 páginas** (latencia HTTP p50):

| Operación | Sin índices | Con los fixes | Cómo |
|---|---:|---:|---|
| Lectura por slug | 3.4 ms | 3.4 ms | el `UNIQUE` de slug ya es índice |
| Listado paginado | 22.7 ms | **5.4 ms** | índice `updated_at` (sin sort) + total O(1) vía `pg_class.reltuples` |
| Búsqueda selectiva | 74 ms | **~11 ms** | índice GIN `pg_trgm` + CTE `MATERIALIZED` (fuerza uso del índice) |
| Búsqueda amplia | 74 ms | **~18 ms** | candidatos acotados (`LIMIT` interno) |

Trade-offs honestos: el `total` del listado es **aproximado** por encima de 50k filas
(estimación de Postgres, refrescada por `ANALYZE`/autovacuum), y una búsqueda cuyo
término aparezca en **>1000 páginas** devuelve un subconjunto (cualquier búsqueda
realista, ≤1000 coincidencias, es exacta). Concurrencia bajo carga es otra historia
(esto es latencia secuencial).

## API

| Método | Ruta | Auth | Descripción |
|---|---|---|---|
| `GET`  | `/api/wiki/pages?page=&size=` | — | Listado paginado |
| `GET`  | `/api/wiki/pages/{slug}` | — | Ver una página (cacheada) |
| `POST` | `/api/wiki/pages` | ✅ | Crear (`{title, content, slug?}`) |
| `PUT`  | `/api/wiki/pages/{slug}` | ✅ | Editar (`{title?, content?}`) |
| `DELETE` | `/api/wiki/pages/{slug}` | ✅ | Borrar |
| `GET`  | `/api/wiki/pages/{slug}/revisions` | — | Historial de versiones |
| `GET`  | `/api/wiki/pages/{slug}/revisions/{n}` | — | Ver una versión |
| `POST` | `/api/wiki/pages/{slug}/revisions/{n}/restore` | ✅ | Restaurar una versión |
| `GET`  | `/api/wiki/search?q=` | — | Búsqueda case-insensitive |
| `POST` | `/api/v1/auth/login` | — | Obtener token (`{username, password}`) — del core |

Ejemplo por cURL:

```bash
TOKEN=$(curl -s localhost:8080/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"Str0ng-Pass1"}' | grep -o '"access_token":"[^"]*"' | cut -d'"' -f4)

curl -s localhost:8080/api/wiki/pages \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"title":"My Page","content":"# Hola\n\nDesde **cURL**."}'
```
