//! The sendmail transport.

use async_trait::async_trait;
use illuminate_support::Result;
use illuminate_support::error::RuntimeException;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

use super::Transport;
use crate::message::{Message, SentMessage};

/// The default sendmail command (`mail.mailers.sendmail.path`).
pub const DEFAULT_SENDMAIL_COMMAND: &str = "/usr/sbin/sendmail -bs -i";

/// Sends mail through the local `sendmail` binary.
///
/// Like Symfony's transport, two modes are supported: `-bs` speaks SMTP
/// with the process over its standard input and output, while `-t` pipes
/// the message to it and lets it read the recipients from the headers.
///
/// ```
/// use illuminate_mail::{SendmailTransport, Transport};
///
/// let transport = SendmailTransport::new("/usr/sbin/sendmail -bs -i").unwrap();
/// assert_eq!(transport.name(), "smtp://sendmail");
///
/// assert!(SendmailTransport::new("/usr/sbin/sendmail -x").is_err());
/// ```
#[derive(Clone, Debug)]
pub struct SendmailTransport {
    command: String,
}

impl SendmailTransport {
    /// Create a transport running the given command.
    pub fn new(command: impl Into<String>) -> Result<Self> {
        let command = command.into();
        let command = if command.trim().is_empty() {
            DEFAULT_SENDMAIL_COMMAND.to_string()
        } else {
            command
        };
        if !command.contains(" -bs") && !command.contains(" -t") {
            return Err(RuntimeException::new(format!(
                "Unsupported sendmail command flags \"{command}\"; must be one of \"-bs\" or \"-t\" but can include additional flags."
            ))
            .into());
        }
        Ok(Self { command })
    }

    /// The command being run.
    pub fn command(&self) -> &str {
        &self.command
    }

    fn uses_smtp(&self) -> bool {
        self.command.contains(" -bs")
    }

    async fn send_piped(&self, message: &Message, bytes: Vec<u8>) -> Result<String> {
        let mut command = self.command.clone();
        if !command.contains(" -f")
            && let Some(sender) = message.envelope_sender()
        {
            command.push_str(&format!(" -f'{}'", sender.address.replace('\'', "")));
        }
        let mut content = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
        if !command.contains(" -i") && !command.contains(" -oi") {
            content = content.replace("\n.", "\n..");
        }

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                RuntimeException::new(format!("Unable to run sendmail [{command}]: {e}"))
            })?;
        if let Some(mut stdin) = child.stdin.take() {
            // A process that exits early closes its input; its exit status
            // tells the real story, so write errors are ignored here.
            let _ = stdin.write_all(content.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        let output = child.wait_with_output().await?;
        if !output.status.success() {
            return Err(RuntimeException::new(format!(
                "Process failed with exit code {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn send_over_smtp(&self, message: &Message, bytes: Vec<u8>) -> Result<String> {
        let envelope = crate::mime::envelope(message)?;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                RuntimeException::new(format!("Unable to run sendmail [{}]: {e}", self.command))
            })?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let mut transcript = String::new();

        let result = async {
            expect(&mut stdout, &mut transcript, &[220]).await?;
            command(
                &mut stdin,
                &mut stdout,
                &mut transcript,
                "EHLO [127.0.0.1]",
                &[250],
            )
            .await?;
            let sender = envelope.from().map(ToString::to_string).unwrap_or_default();
            command(
                &mut stdin,
                &mut stdout,
                &mut transcript,
                &format!("MAIL FROM:<{sender}>"),
                &[250],
            )
            .await?;
            for recipient in envelope.to() {
                command(
                    &mut stdin,
                    &mut stdout,
                    &mut transcript,
                    &format!("RCPT TO:<{recipient}>"),
                    &[250, 251, 252],
                )
                .await?;
            }
            command(&mut stdin, &mut stdout, &mut transcript, "DATA", &[354]).await?;
            let mut data = String::from_utf8_lossy(&bytes).into_owned();
            if data.starts_with('.') {
                data.insert(0, '.');
            }
            let data = data.replace("\r\n.", "\r\n..");
            stdin.write_all(data.as_bytes()).await?;
            if !data.ends_with("\r\n") {
                stdin.write_all(b"\r\n").await?;
            }
            command(&mut stdin, &mut stdout, &mut transcript, ".", &[250]).await?;
            command(&mut stdin, &mut stdout, &mut transcript, "QUIT", &[221]).await?;
            Ok::<(), illuminate_support::Error>(())
        }
        .await;

        drop(stdin);
        let _ = child.wait().await;
        result.map(|()| transcript)
    }
}

async fn command(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    transcript: &mut String,
    line: &str,
    codes: &[u16],
) -> Result<()> {
    transcript.push_str(&format!("> {line}\n"));
    stdin.write_all(format!("{line}\r\n").as_bytes()).await?;
    stdin.flush().await?;
    expect(stdout, transcript, codes).await
}

/// Read an SMTP reply (which may span several lines) and check its code.
async fn expect(
    stdout: &mut BufReader<ChildStdout>,
    transcript: &mut String,
    codes: &[u16],
) -> Result<()> {
    loop {
        let mut line = String::new();
        if stdout.read_line(&mut line).await? == 0 {
            return Err(RuntimeException::new(format!(
                "Connection to sendmail closed unexpectedly.\n{transcript}"
            ))
            .into());
        }
        transcript.push_str(&format!("< {}\n", line.trim_end()));
        let code: u16 = line.get(..3).and_then(|c| c.parse().ok()).unwrap_or(0);
        let last = line.as_bytes().get(3) != Some(&b'-');
        if last {
            if codes.contains(&code) {
                return Ok(());
            }
            return Err(RuntimeException::new(format!(
                "Expected response code \"{}\" but got \"{}\".\n{transcript}",
                codes
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join("/"),
                line.trim_end()
            ))
            .into());
        }
    }
}

#[async_trait]
impl Transport for SendmailTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        let bytes = crate::mime::format(message)?;
        let debug = if self.uses_smtp() {
            self.send_over_smtp(message, bytes).await?
        } else {
            self.send_piped(message, bytes).await?
        };
        let mut sent = SentMessage::new(message.clone());
        sent.debug = debug;
        Ok(sent)
    }

    fn name(&self) -> String {
        if self.uses_smtp() {
            "smtp://sendmail".into()
        } else {
            "sendmail://default".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> Message {
        let mut message = Message::new();
        message
            .from("hello@example.com")
            .to("taylor@example.com")
            .subject("Sendmail")
            .text("Hello from sendmail\n.dot line");
        message
    }

    fn script(dir: &std::path::Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        format!("sh {}", path.display())
    }

    #[tokio::test]
    async fn piped_mode_writes_the_message_to_the_process() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.eml");
        let args = dir.path().join("args.txt");
        let command = script(
            dir.path(),
            "sendmail.sh",
            &format!(
                "echo \"$@\" > {}\ncat > {}\n",
                args.display(),
                out.display()
            ),
        );
        let transport = SendmailTransport::new(format!("{command} -t -i")).unwrap();
        assert_eq!(transport.name(), "sendmail://default");
        transport.send(&message()).await.unwrap();

        let eml = std::fs::read_to_string(&out).unwrap();
        assert!(eml.contains("Subject: Sendmail\n"));
        assert!(eml.contains("Hello from sendmail"));
        assert!(!eml.contains("\r\n"));
        let args = std::fs::read_to_string(&args).unwrap();
        assert!(args.contains("-t -i -fhello@example.com"), "{args}");
    }

    #[tokio::test]
    async fn failing_processes_report_errors() {
        let transport = SendmailTransport::new("echo broken >&2; exit 3 -t").unwrap();
        let error = transport.send(&message()).await.unwrap_err();
        assert!(error.to_string().contains("exit code 3"), "{error}");
        assert!(error.to_string().contains("broken"));
    }

    #[tokio::test]
    async fn smtp_mode_speaks_smtp_over_stdio() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("data.txt");
        let command = script(
            dir.path(),
            "smtp.sh",
            &format!(
                r#"echo "220 localhost ESMTP"
data=0
while IFS= read -r line; do
  line=$(printf '%s' "$line" | tr -d '\r')
  if [ $data = 1 ]; then
    if [ "$line" = "." ]; then data=0; echo "250 2.0.0 queued"; else printf '%s\n' "$line" >> {out}; fi
    continue
  fi
  case "$line" in
    EHLO*) echo "250-localhost"; echo "250 HELP" ;;
    DATA*) data=1; echo "354 go ahead" ;;
    QUIT*) echo "221 bye"; exit 0 ;;
    *) echo "250 ok" ;;
  esac
done
"#,
                out = out.display()
            ),
        );
        let transport = SendmailTransport::new(format!("{command} -bs -i")).unwrap();
        assert_eq!(transport.name(), "smtp://sendmail");
        let sent = transport.send(&message()).await.unwrap();
        assert!(sent.debug().contains("> MAIL FROM:<hello@example.com>"));
        assert!(sent.debug().contains("< 221 bye"));

        let data = std::fs::read_to_string(&out).unwrap();
        assert!(data.contains("Subject: Sendmail"));
        assert!(data.contains("Hello from sendmail"));
    }

    #[tokio::test]
    async fn smtp_mode_reports_unexpected_replies() {
        let transport = SendmailTransport::new("echo '554 go away' -bs").unwrap();
        let error = transport.send(&message()).await.unwrap_err();
        assert!(
            error.to_string().contains("Expected response code \"220\""),
            "{error}"
        );
    }
}
