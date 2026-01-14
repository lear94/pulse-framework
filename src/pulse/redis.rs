use super::{PulseReactor, PulseSignal};
use async_trait::async_trait;
use deadpool_redis::{
    redis::{AsyncCommands, Client},
    Pool,
};
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info};

const PULSE_CHANNEL: &str = "pulse:global:events";

pub struct RedisReactor {
    pool: Pool,
    broadcaster: broadcast::Sender<PulseSignal>,
}

impl RedisReactor {
    pub fn new(pool: Pool, redis_url: String) -> (Arc<Self>, broadcast::Receiver<PulseSignal>) {
        let (tx_out, rx_out) = broadcast::channel(2048);
        let tx_clone = tx_out.clone();

        tokio::spawn(async move {
            info!("📡 Redis Event Bus Listener attached.");
            loop {
                if let Err(e) = Self::listen_loop(&redis_url, &tx_clone).await {
                    error!("Redis Event Bus disconnected: {}. Reconnecting in 5s...", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            }
        });

        let reactor = Arc::new(Self {
            pool,
            broadcaster: tx_out,
        });
        (reactor, rx_out)
    }

    async fn listen_loop(
        url: &str,
        broadcaster: &broadcast::Sender<PulseSignal>,
    ) -> Result<(), String> {
        let client = Client::open(url).map_err(|e| e.to_string())?;
        let mut pubsub = client.get_async_pubsub().await.map_err(|e| e.to_string())?;
        pubsub
            .subscribe(PULSE_CHANNEL)
            .await
            .map_err(|e| e.to_string())?;
        let mut stream = pubsub.into_on_message();
        while let Some(msg) = stream.next().await {
            let payload: String = msg.get_payload().map_err(|e| e.to_string())?;
            if let Ok(signal) = serde_json::from_str::<PulseSignal>(&payload) {
                let _ = broadcaster.send(signal);
            }
        }
        Err("Connection closed".to_string())
    }
}

#[async_trait]
impl PulseReactor for RedisReactor {
    async fn emit(&self, signal: PulseSignal) {
        if let Ok(json) = serde_json::to_string(&signal) {
            if let Ok(mut conn) = self.pool.get().await {
                let _: Result<(), _> = conn.publish(PULSE_CHANNEL, json).await;
            } else {
                error!("Failed to acquire Redis connection for Event Emission");
            }
        }
    }
    fn subscribe(&self) -> broadcast::Receiver<PulseSignal> {
        self.broadcaster.subscribe()
    }
}
