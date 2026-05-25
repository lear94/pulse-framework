use super::{FlightRecord, FlightRecorder};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::path::Path;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const STORAGE_PATH: &str = "storage/blackbox.jsonl";
const ROTATED_PATH: &str = "storage/blackbox.jsonl.1";
// Rota el archivo al superar ~16 MiB para evitar crecimiento ilimitado.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

pub struct DiskRecorder;

impl DiskRecorder {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl FlightRecorder for DiskRecorder {
    async fn record(&self, record: FlightRecord) -> Result<(), String> {
        if let Some(parent) = Path::new(STORAGE_PATH).parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        // Rotación por tamaño: conservamos un único segmento previo (.1).
        if let Ok(meta) = tokio::fs::metadata(STORAGE_PATH).await {
            if meta.len() >= MAX_FILE_BYTES {
                let _ = tokio::fs::rename(STORAGE_PATH, ROTATED_PATH).await;
            }
        }
        let mut json_line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
        json_line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(STORAGE_PATH)
            .await
            .map_err(|e| e.to_string())?;
        file.write_all(json_line.as_bytes())
            .await
            .map_err(|e| e.to_string())
    }

    async fn tail(&self, limit: usize) -> Vec<FlightRecord> {
        let file = match tokio::fs::File::open(STORAGE_PATH).await {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        if limit == 0 {
            return Vec::new();
        }
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        // Ring buffer acotado a `limit`: memoria O(limit) en vez de O(archivo).
        let mut ring: VecDeque<FlightRecord> = VecDeque::with_capacity(limit);
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(record) = serde_json::from_str::<FlightRecord>(&line) {
                if ring.len() == limit {
                    ring.pop_front();
                }
                ring.push_back(record);
            }
        }
        ring.into_iter().collect()
    }

    async fn scan_id(&self, target_id: &str) -> Option<FlightRecord> {
        let file = match tokio::fs::File::open(STORAGE_PATH).await {
            Ok(f) => f,
            Err(_) => return None,
        };
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(record) = serde_json::from_str::<FlightRecord>(&line) {
                if record.id == target_id {
                    return Some(record);
                }
            }
        }
        None
    }
}
