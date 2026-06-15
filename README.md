# ⚡ Pulse Framework

**A high-performance, fault-tolerant, and chaos-hardened web framework built in Rust.**

Pulse Framework is designed for mission-critical applications where latency, resource efficiency, and reliability are non-negotiable. Built on top of `actix-web`, `tokio`, and `sea-orm`, it provides a production-ready foundation with built-in resilience patterns.

---

## 🚀 Performance & Resilience: The "Gauntlet" Benchmarks

Pulse Framework focuses on **efficiency** and **graceful degradation under failure**. The following numbers come from the **System Gauntlet** load/chaos suite on a local dev machine — treat them as a baseline for that setup, not a universal benchmark.

### 📊 **Throughput & Latency** *(local, single node)*
* ~**4,500 req/s** sustained on a local environment.
* ~**47 ms** average latency under 200 concurrent connections.
* ~**45,000 requests in 10 s** with a high success rate.

> Auth endpoints are intentionally bounded by **bcrypt** (CPU-bound hashing runs off the async executor via `spawn_blocking`); throughput there is dominated by the configured bcrypt cost, by design.

### 🍃 **Low Resource Footprint**
* **RAM:** ~**33 MB** resident under the load above.
* **CPU:** peaks around **25%**, leaving headroom for background work.

### 🛡️ **Chaos Engineering Ready**
* **Survival Mode:** dependency failures (e.g. a Redis crash) degrade gracefully or recover without taking down the main process.
* *Observed in the suite:* `Chaos: Killing Redis... -> SUCCESS`.

---

## ✨ Key Features

* **Modular Workspace:** Organized into core, cli, migration, and dashboard crates.
* **Production Optimized:** `Cargo.toml` configured with `lto = "fat"`, `codegen-units = 1`, and `panic = "abort"` for smallest binary size and max speed.
* **Database Agnostic:** Powered by **SeaORM** (Async ORM) for SQL databases.
* **Caching & Queues:** Built-in **Redis** support for high-speed caching and background jobs.
* **Security First:** JWT Authentication, secure headers, and rigorous input validation.
* **Developer Experience:**
    * Create migrations effortlessly.
    * Integrated **CLI** for management.
    * Automated Test Suite (Logic, Security, Recovery, Chaos).

---

## 🛠️ Getting Started

### Prerequisites
* [Rust](https://www.rust-lang.org/tools/install) (latest stable)
* Docker & Docker Compose (for Postgres & Redis)

### 1. Clone & Setup
```bash
git clone https://github.com/lear94/pulse-framework.git
cd pulse-framework

```

### 2. Environment Configuration

Copy the example environment file and configure your secrets:

Bash

```
cp .env.example .env

```

> **Note:** Ensure your `.env` contains valid credentials for Postgres and Redis.

### 3. Start Infrastructure

Start your database and cache services:

Bash

```
docker-compose up -d

```

### 4. Run Migrations

Initialize the database schema:

Bash

```
cargo run --bin migration up

```

> **Safety lock:** destructive commands (`fresh`, `refresh`, `reset`) drop tables
> and are **blocked by default**. To run one intentionally, set
> `PULSE_ALLOW_DESTRUCTIVE_MIGRATIONS=yes-i-understand-this-drops-tables`
> (and confirm interactively if on a terminal). Prefer `up`; roll back with
> `down -n N`.

### 5. Run the Server

Bash

```
cargo run --release

```

_API will be available at: `http://127.0.0.1:8080/api/v1`_

----------

## 🧪 Testing Suite: "The Certification"

Pulse Framework comes with a comprehensive shell-based test runner that validates the system from logic to chaos.

To run the full suite:

Bash

```
./run_tests.sh

```

**What is tested?**

1.  **Logical Integrity:** Unit tests & logic verification.
    
2.  **CLI Operations:** Verifies command-line tools.
    
3.  **Migrations:** Tests database schema evolution.
    
4.  **Security:** Penetration testing scripts.
    
5.  **Recovery:** System behavior after crashes.
    
6.  **The Gauntlet:** Load and Chaos testing (Stress tests).
    
7.  **Idempotency:** Ensures safe retries.
    
8.  **Toxic Scenarios:** Handling malformed data.
    

----------

## 📂 Project Structure

```
pulse-framework/
├── cli/             # Command Line Interface tool
├── dashboard/       # Admin Dashboard (WASM/Frontend)
├── migration/       # SeaORM Migrations
├── src/             # Core API Logic
│   ├── auth/        # JWT & Security
│   ├── core/        # Orchestrator, Monitoring, Queues
│   ├── models/      # Database Entities
│   └── services/    # Business Logic
├── tests/           # Integration & Shell scripts
└── Cargo.toml       # Workspace definition

```

## 📜 License

This project is licensed under the MIT License - see the [LICENSE](https://www.google.com/search?q=LICENSE) file for details.
