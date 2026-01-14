use super::{PulseReactor, PulseSignal};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, Duration};

pub struct MemoryReactor {
    sender: mpsc::Sender<PulseSignal>,
    broadcaster: broadcast::Sender<PulseSignal>,
}

impl MemoryReactor {
    pub fn new(frequency_ms: u64) -> (Arc<Self>, broadcast::Receiver<PulseSignal>) {
        let (tx_in, mut rx_in) = mpsc::channel::<PulseSignal>(2048);
        let (tx_out, rx_out) = broadcast::channel(2048);
        let tx_out_clone = tx_out.clone();

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_millis(frequency_ms));
            loop {
                ticker.tick().await;
                while let Ok(signal) = rx_in.try_recv() {
                    let _ = tx_out_clone.send(signal);
                }
            }
        });

        let reactor = Arc::new(Self {
            sender: tx_in,
            broadcaster: tx_out,
        });
        (reactor, rx_out)
    }
}

#[async_trait]
impl PulseReactor for MemoryReactor {
    async fn emit(&self, signal: PulseSignal) {
        let _ = self.sender.send(signal).await;
    }
    fn subscribe(&self) -> broadcast::Receiver<PulseSignal> {
        self.broadcaster.subscribe()
    }
}