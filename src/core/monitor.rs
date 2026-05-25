use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{MemoryRefreshKind, Pid, ProcessesToUpdate, RefreshKind, System};

#[derive(Debug)]
pub struct SystemMonitor {
    pub requests_total: AtomicUsize,
    pub active_connections: AtomicUsize,
    pub failures_total: AtomicUsize,
    pub start_time: std::time::Instant,
    // Métricas internas
    sys_ram_mb: AtomicU64,
    app_ram_mb: AtomicU64,
    cpu_usage_bits: AtomicU32,
}

impl SystemMonitor {
    pub fn new() -> Arc<Self> {
        let monitor = Arc::new(Self {
            requests_total: AtomicUsize::new(0),
            active_connections: AtomicUsize::new(0),
            failures_total: AtomicUsize::new(0),
            start_time: std::time::Instant::now(),
            sys_ram_mb: AtomicU64::new(0),
            app_ram_mb: AtomicU64::new(0),
            cpu_usage_bits: AtomicU32::new(0),
        });

        let mon_clone = monitor.clone();
        tokio::spawn(async move {
            let mut sys = System::new_with_specifics(
                RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
            );
            let pid = Pid::from_u32(std::process::id());
            // sysinfo necesita dos muestras separadas por MINIMUM_CPU_UPDATE_INTERVAL
            // para calcular % de CPU; sin esto la primera lectura siempre sería 0.
            sys.refresh_cpu_all();
            tokio::time::sleep(Duration::from_millis(250)).await;
            loop {
                sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
                sys.refresh_cpu_all();

                let app_mem = sys
                    .process(pid)
                    .map(|p| p.memory() / 1024 / 1024)
                    .unwrap_or(0);
                let total_mem = sys.used_memory() / 1024 / 1024;
                let cpu = sys.global_cpu_usage();

                mon_clone.app_ram_mb.store(app_mem, Ordering::Relaxed);
                mon_clone.sys_ram_mb.store(total_mem, Ordering::Relaxed);
                mon_clone
                    .cpu_usage_bits
                    .store(cpu.to_bits(), Ordering::Relaxed);

                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });

        monitor
    }

    pub fn snapshot(&self) -> MonitorSnapshot {
        MonitorSnapshot {
            uptime_seconds: self.start_time.elapsed().as_secs(),
            total_requests: self.requests_total.load(Ordering::Relaxed),
            current_active: self.active_connections.load(Ordering::Relaxed),
            total_failures: self.failures_total.load(Ordering::Relaxed),
            ram_usage_mb: self.app_ram_mb.load(Ordering::Relaxed),
            system_ram_mb: self.sys_ram_mb.load(Ordering::Relaxed),
            cpu_usage: f32::from_bits(self.cpu_usage_bits.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, utoipa::ToSchema, Debug)]
pub struct MonitorSnapshot {
    pub uptime_seconds: u64,
    pub total_requests: usize,
    pub current_active: usize,
    pub total_failures: usize,
    pub cpu_usage: f32,
    pub ram_usage_mb: u64,
    pub system_ram_mb: u64,
}
