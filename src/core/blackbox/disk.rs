use super::{FlightRecord, FlightRecorder};
use async_trait::async_trait;
use std::path::Path;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const STORAGE_PATH: &str = "storage/blackbox.jsonl";

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
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let mut records = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(record) = serde_json::from_str::<FlightRecord>(&line) {
                records.push(record);
            }
        }
        let start = records.len().saturating_sub(limit);
        records.into_iter().skip(start).collect()
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
