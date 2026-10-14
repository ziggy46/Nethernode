//! A log writer that masks secrets (see [`nn_auth::redact`]).

use std::io::{self, Write};

use tracing_subscriber::fmt::MakeWriter;

/// Wraps a log destination so every line passes through
/// [`nn_auth::redact::redact_log_line`] first.
#[derive(Debug, Clone)]
pub struct Redacting<M>(pub M);

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for Redacting<M> {
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter(self.0.make_writer())
    }
}

/// See [`Redacting`].
#[derive(Debug)]
pub struct RedactingWriter<W>(W);

impl<W: Write> Write for RedactingWriter<W> {
    // The formatter hands over each event as one complete buffer.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.0
            .write_all(nn_auth::redact::redact_log_line(&text).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn secrets_never_reach_the_log() {
        let out = Captured::default();
        let sink = out.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(true)
            .with_writer(Redacting(move || sink.clone()))
            .finish();
        // A secret NetherNode decrypted, as an RCON password would be.
        let key = nn_auth::SecretBox::from_hex_key(&"cd".repeat(32)).unwrap();
        let sealed = key.seal("rcon-pass-7Hq2xKp9");
        let rcon = key.open(&sealed).unwrap();
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(password = %rcon, "couldn't reach the server");
            tracing::error!(
                error =
                    "error sending request for url (https://discord.com/api/webhooks/42/tOkEn-abc)",
                "couldn't notify"
            );
            tracing::info!("connecting with secret {rcon} as part of a message");
            tracing::info!(server = "smp", "server started");
        });
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert!(!text.contains("rcon-pass-7Hq2xKp9"), "{text}");
        assert!(!text.contains("tOkEn-abc"), "{text}");
        assert!(text.contains("[redacted]"));
        assert!(text.contains("server started"), "ordinary lines still log");
    }
}
