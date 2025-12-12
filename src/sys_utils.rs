
use crate::*;

pub fn epoch_seconds_now_utc0() -> u64 {
    let now = std::time::SystemTime::now();
    match now.duration_since(std::time::UNIX_EPOCH) {
        Ok(dur) => dur.as_secs(),
        Err(e) => {
            // Yell at the poor time-traveler for
            // making us handle their nonsense edge-case
            tracing::info!("WARNING: Time-Travel Detected! ({:?})", e);
            0u64
        }
    }
}


