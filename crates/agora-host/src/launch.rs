//! Starts plugin processes the way the Stream Dock app does:
//! `<program> -port <port> -pluginUUID <uuid> -registerEvent registerPlugin -info <json>`,
//! with the plugin folder as the working directory.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use crate::host::{HostEvent, Msg};
use crate::manifest::{Kind, Plugin};

pub(crate) enum Running {
    /// Runs in a hidden browser view owned by the app.
    Html,
    /// Send to stop the process.
    Process(oneshot::Sender<()>),
}

/// Node.js to use: the configured one, else `node` on PATH.
fn node_program(configured: Option<&Path>) -> PathBuf {
    configured
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "node.exe" } else { "node" }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn(
    plugin: &Plugin,
    kind: &Kind,
    node: Option<&Path>,
    port: u16,
    info: &str,
    log_dir: &Path,
    host: mpsc::UnboundedSender<Msg>,
    events: std::sync::mpsc::Sender<HostEvent>,
) -> Result<Running, String> {
    let mut cmd = match kind {
        Kind::Node(script) => {
            let mut c = Command::new(node_program(node));
            if let Some(debug) = plugin
                .manifest
                .nodejs
                .as_ref()
                .and_then(|n| n.debug.clone())
            {
                // Only the documented inspector flag, bound to localhost.
                if debug.starts_with("--inspect=127.0.0.1:") {
                    c.arg(debug);
                }
            }
            c.arg(script);
            c
        }
        Kind::Exe(program) => Command::new(program),
        Kind::Html(_) => return Ok(Running::Html),
    };
    cmd.args([
        "-port",
        &port.to_string(),
        "-pluginUUID",
        &plugin.uuid,
        "-registerEvent",
        "registerPlugin",
        "-info",
        info,
    ])
    .current_dir(&plugin.dir)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console window per plugin.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| match kind {
        Kind::Node(_) if e.kind() == std::io::ErrorKind::NotFound => {
            "Node.js wasn't found. Install Node.js 20 or newer, or set its path in Agora's settings.".to_string()
        }
        _ => e.to_string(),
    })?;

    let _ = std::fs::create_dir_all(log_dir);
    let log_path = log_dir.join(format!("{}.log", plugin.uuid));
    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
    for (stream, label) in [
        (
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
            "out",
        ),
        (
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
            "err",
        ),
    ] {
        let Some(stream) = stream else { continue };
        let tx = line_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(format!("[{label}] {line}"));
            }
        });
    }
    drop(line_tx);

    // One writer for the plugin's log file; also forwards lines to the app.
    let uuid = plugin.uuid.clone();
    let events2 = events.clone();
    tokio::spawn(async move {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .ok();
        // Keep log files from growing without limit.
        if let Ok(meta) = tokio::fs::metadata(&log_path).await {
            if meta.len() > 2_000_000 {
                let _ = tokio::fs::rename(&log_path, log_path.with_extension("log.old")).await;
                file = tokio::fs::File::create(&log_path).await.ok();
            }
        }
        while let Some(line) = line_rx.recv().await {
            if let Some(f) = file.as_mut() {
                let _ = f.write_all(format!("{line}\n").as_bytes()).await;
            }
            let _ = events2.send(HostEvent::Log {
                plugin: uuid.clone(),
                message: line,
            });
        }
    });

    let (kill_tx, kill_rx) = oneshot::channel();
    let uuid = plugin.uuid.clone();
    tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                let why = match status {
                    Ok(s) => format!("exit status {s}"),
                    Err(e) => e.to_string(),
                };
                let _ = host.send(Msg::PluginExited(uuid, why));
            }
            _ = kill_rx => {
                let _ = child.kill().await;
            }
        }
    });
    Ok(Running::Process(kill_tx))
}
