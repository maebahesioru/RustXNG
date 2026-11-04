// SPDX-License-Identifier: AGPL-3.0-or-later

use std::io::{Seek, Write};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) async fn extract(bytes: Vec<u8>, max_chars: usize) -> Result<String, String> {
    extract_with_command(bytes, max_chars, Command::new("pdftotext")).await
}

async fn extract_with_command(
    bytes: Vec<u8>,
    max_chars: usize,
    mut command: Command,
) -> Result<String, String> {
    let input = tokio::task::spawn_blocking(move || {
        let mut input = tempfile::tempfile()?;
        input.write_all(&bytes)?;
        input.rewind()?;
        Ok::<_, std::io::Error>(input)
    })
    .await
    .map_err(|error| format!("PDF preparation failed: {error}"))?
    .map_err(|error| format!("PDF preparation failed: {error}"))?;
    let mut child = command
        .args(["-enc", "UTF-8", "-nopgbrk", "-", "-"])
        .stdin(Stdio::from(input))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "PDF extraction requires pdftotext (Poppler) on PATH".to_string()
            } else {
                format!("PDF extractor could not be started: {error}")
            }
        })?;
    let bytes = collect_output(&mut child, EXTRACT_TIMEOUT, MAX_OUTPUT_BYTES).await?;
    let text = String::from_utf8(bytes).map_err(|_| "PDF extractor returned invalid UTF-8")?;
    let text = crate::content::normalise(&text);
    if text.is_empty() {
        return Err("PDF has no extractable text; scanned documents require OCR".into());
    }
    Ok(crate::content::truncate(&text, max_chars))
}

async fn collect_output(
    child: &mut Child,
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let stdout = child.stdout.take().expect("stdout is piped");
    let result = tokio::time::timeout(timeout, async {
        let mut bytes = Vec::new();
        stdout
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| format!("PDF text could not be read: {error}"))?;
        if bytes.len() > max_bytes {
            return Err(format!(
                "PDF text exceeds the {max_bytes} byte extraction limit"
            ));
        }
        let status = child
            .wait()
            .await
            .map_err(|error| format!("PDF extractor could not be awaited: {error}"))?;
        if !status.success() {
            return Err(format!("PDF text extraction failed ({status})"));
        }
        Ok(bytes)
    })
    .await
    .unwrap_or_else(|_| Err("PDF text extraction timed out".into()));
    if result.is_err() {
        let _ = child.kill().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_extractor_has_an_installation_error() {
        let directory = tempfile::tempdir().unwrap();
        let command = Command::new(directory.path().join("missing-pdftotext"));
        let error = extract_with_command(Vec::new(), 100, command)
            .await
            .unwrap_err();
        assert!(error.contains("requires pdftotext (Poppler) on PATH"));
    }

    #[tokio::test]
    async fn pdf_without_text_requires_ocr() {
        let error = extract(
            include_bytes!("../../tests/fixtures/empty.pdf").to_vec(),
            100,
        )
        .await
        .unwrap_err();
        assert!(error.contains("no extractable text"), "{error}");
    }

    #[tokio::test]
    async fn malformed_pdf_is_an_error() {
        let error = extract(b"%PDF-1.4\ninvalid\n".to_vec(), 100)
            .await
            .unwrap_err();
        assert!(error.contains("PDF text extraction failed"), "{error}");
    }

    fn output_child(mode: &str) -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fetch::pdf::tests::child_output", "--nocapture"])
            .env("RUSTXNG_PDF_TEST_CHILD", mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    #[test]
    fn child_output() {
        let Ok(mode) = std::env::var("RUSTXNG_PDF_TEST_CHILD") else {
            return;
        };
        if mode == "output" {
            std::io::stdout().write_all(b"fixture output").unwrap();
            std::io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        if mode == "excess" {
            std::io::stdout().write_all(&[b'x'; 4096]).unwrap();
            std::io::stdout().flush().unwrap();
        }
        std::thread::sleep(Duration::from_secs(30));
    }

    #[tokio::test]
    async fn extraction_limits_terminate_and_reap_the_child() {
        let mut excess = output_child("excess");
        let error = collect_output(&mut excess, EXTRACT_TIMEOUT, 64)
            .await
            .unwrap_err();
        assert!(error.contains("extraction limit"), "{error}");
        assert!(excess.try_wait().unwrap().is_some());

        let mut stalled = output_child("stall");
        let error = collect_output(&mut stalled, Duration::from_millis(200), 64)
            .await
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(stalled.try_wait().unwrap().is_some());
    }

    #[tokio::test]
    async fn output_limit_accepts_exactly_the_cap() {
        let output = collect_output(&mut output_child("output"), EXTRACT_TIMEOUT, 1024)
            .await
            .unwrap();
        let exact = collect_output(&mut output_child("output"), EXTRACT_TIMEOUT, output.len())
            .await
            .unwrap();
        assert_eq!(exact, output);
        assert!(
            collect_output(
                &mut output_child("output"),
                EXTRACT_TIMEOUT,
                output.len() - 1,
            )
            .await
            .unwrap_err()
            .contains("extraction limit")
        );
    }
}
