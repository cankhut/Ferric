//! The small amount of HTTP this app does.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One client with sane timeouts. A music app must never hang the whole
/// screen because a website is slow.
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
}

/// Keeps requests to one website far enough apart.
///
/// MusicBrainz asks for no more than one request a second and blocks the
/// address of anyone who ignores it; Discogs counts sixty a minute, which
/// is the same thing. Each site gets one of these as a `static`, and every
/// request to it goes through `wait` first.
pub struct Pace {
    gap: Duration,
    last: Mutex<Option<Instant>>,
}

impl Pace {
    pub const fn new(gap: Duration) -> Self {
        Pace {
            gap,
            last: Mutex::new(None),
        }
    }

    /// Returns once it is this website's turn again. Held while waiting, so
    /// two threads asking at the same moment queue up instead of both
    /// going out together.
    pub fn wait(&self) {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        std::thread::sleep(wait_before_next(
            self.gap,
            last.map(|at: Instant| at.elapsed()),
        ));

        *last = Some(Instant::now());
    }
}

/// How long to wait, given the gap a website wants and how long ago the
/// last request to it went out.
fn wait_before_next(gap: Duration, since_last: Option<Duration>) -> Duration {
    match since_last {
        Some(since) if since < gap => gap - since,
        _ => Duration::ZERO,
    }
}

/// Percent encodes a value so it can go in a query string.
///
/// Everything outside the unreserved set becomes %XX of its UTF-8 bytes,
/// which is what keeps an album called "Tago Mago / Live" from turning
/// into two parameters.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());

    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_are_left_alone() {
        assert_eq!(encode("Nevermind"), "Nevermind");
        assert_eq!(
            encode("OK-Computer_1997.remaster~"),
            "OK-Computer_1997.remaster~"
        );
    }

    #[test]
    fn everything_else_is_escaped() {
        assert_eq!(encode("Kind of Blue"), "Kind%20of%20Blue");
        assert_eq!(encode("AC/DC"), "AC%2FDC");
        assert_eq!(encode("Sigur Rós"), "Sigur%20R%C3%B3s");
        assert_eq!(encode("a&b=c?d"), "a%26b%3Dc%3Fd");
    }

    #[test]
    fn requests_are_spaced_out() {
        let second = Duration::from_secs(1);

        // The first request to a website goes straight out
        assert_eq!(wait_before_next(second, None), Duration::ZERO);

        // One that went out 300 ms ago leaves 700 ms to wait
        assert_eq!(
            wait_before_next(second, Some(Duration::from_millis(300))),
            Duration::from_millis(700)
        );

        // Long enough ago, no wait at all
        assert_eq!(
            wait_before_next(second, Some(Duration::from_secs(5))),
            Duration::ZERO
        );
    }

    #[test]
    fn two_requests_in_a_row_are_held_apart() {
        static PACE: Pace = Pace::new(Duration::from_millis(120));

        let started = Instant::now();

        PACE.wait();
        PACE.wait();

        assert!(started.elapsed() >= Duration::from_millis(110));
    }
}
