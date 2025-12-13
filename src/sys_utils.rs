
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

pub async fn ask_user_proceed_yn<T: AsRef<str>>(msg: T) -> bool {
    use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt};

    // Print prompt without newline
    let mut stdout = io::stdout();
    let _ = stdout.write_all(msg.as_ref().as_bytes()).await;
    let _ = stdout.flush().await;

    // Read response
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin);
    let mut input = String::new();
    let _ = reader.read_line(&mut input).await;
    let input = input.trim_end().to_string();

    input.contains("y") || input.contains("Y") || input.contains("1")
}



