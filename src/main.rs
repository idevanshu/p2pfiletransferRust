use clap::{Arg, Command};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use libp2p::{
    dcutr, identify, mdns, noise, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, StreamProtocol,
};
use libp2p_stream as stream_proto;
use memmap2::MmapOptions;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio_util::compat::FuturesAsyncReadCompatExt;

const BUF: usize = 256 * 1024; // 256 KB chunk size
const MAX_NAME: usize = 240;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const STREAM_TIMEOUT: Duration = Duration::from_secs(10);
const PROTOCOL: StreamProtocol = StreamProtocol::new("/p2pfiletransfer/transfer/1");

type Res<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    identify: identify::Behaviour,
    mdns: mdns::tokio::Behaviour,
    stream: stream_proto::Behaviour,
}

// ── main ─────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "current_thread")]
async fn main() -> Res {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .compact()
        .init();

    let m = Command::new("p2pfiletransfer")
        .version("5.0")
        .author("github.com/idevanshu")
        .about("P2P file transfer with NAT traversal")
        .arg(Arg::new("mode").short('m').long("mode").required(true).value_parser(["send", "receive"]))
        .arg(Arg::new("file").short('f').long("file").required_if_eq("mode", "send"))
        .arg(Arg::new("address").short('a').long("address").required_if_eq("mode", "receive")
            .help("Multiaddr: /ip4/x.x.x.x/tcp/PORT/p2p/PEER_ID"))
        .arg(Arg::new("relay").short('r').long("relay").help("Relay server multiaddr for NAT traversal"))
        .arg(Arg::new("port").short('p').long("port").default_value("0").help("Listen port (0 = random)"))
        .get_matches();

    let relay = m.get_one::<String>("relay").cloned();
    let port: u16 = m.get_one::<String>("port").unwrap().parse().unwrap_or(0);

    match m.get_one::<String>("mode").map(|s| s.as_str()) {
        Some("send") => send(m.get_one::<String>("file").unwrap(), relay, port).await,
        Some("receive") => receive(m.get_one::<String>("address").unwrap(), relay, port).await,
        _ => unreachable!(),
    }
}

// ── swarm ────────────────────────────────────────────────────────────────

fn build_swarm() -> Res<(libp2p::Swarm<Behaviour>, stream_proto::Control)> {
    let swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(tcp::Config::default().nodelay(true), noise::Config::new, yamux::Config::default)?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|kp, relay_client| {
            let pid = kp.public().to_peer_id();
            Ok(Behaviour {
                relay: relay_client,
                dcutr: dcutr::Behaviour::new(pid),
                identify: identify::Behaviour::new(
                    identify::Config::new("/p2pfiletransfer/1".into(), kp.public()),
                ),
                mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), pid)?,
                stream: stream_proto::Behaviour::new(),
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(IDLE_TIMEOUT))
        .build();
    let ctl = swarm.behaviour().stream.new_control();
    Ok((swarm, ctl))
}

/// Dial relay + listen on relay circuit if relay addr is provided.
fn setup_relay(swarm: &mut libp2p::Swarm<Behaviour>, relay: &Option<String>) -> Res {
    if let Some(r) = relay {
        let ma: Multiaddr = r.parse()?;
        swarm.dial(ma)?;
        eprintln!("[relay] connecting...");
    }
    Ok(())
}

fn listen_on_relay(swarm: &mut libp2p::Swarm<Behaviour>, relay: &Option<String>) {
    if let Some(r) = relay {
        if let Ok(ma) = format!("{}/p2p-circuit", r).parse::<Multiaddr>() {
            let _ = swarm.listen_on(ma);
        }
    }
}

fn extract_peer_id(ma: &Multiaddr) -> Res<PeerId> {
    ma.iter()
        .find_map(|p| match p {
            libp2p::multiaddr::Protocol::P2p(id) => Some(id),
            _ => None,
        })
        .ok_or_else(|| "address must end with /p2p/<PEER_ID>".into())
}

// ── sender ───────────────────────────────────────────────────────────────

async fn send(path: &str, relay: Option<String>, port: u16) -> Res {
    // Validate path exists before setting up networking
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("path not found: {}", path).into());
    }

    let (mut swarm, mut ctl) = build_swarm()?;
    let pid = *swarm.local_peer_id();

    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{}", port).parse()?)?;
    setup_relay(&mut swarm, &relay)?;

    let mut incoming = ctl.accept(PROTOCOL)?;
    let mut relay_listened = false;

    eprintln!("──────────────────────────────────");
    eprintln!("  p2pfiletransfer sender");
    eprintln!("  peer: {pid}");
    eprintln!("──────────────────────────────────");

    let path = path.to_string();

    loop {
        tokio::select! {
            // Graceful Ctrl+C
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\nshutting down.");
                return Ok(());
            }
            ev = swarm.select_next_some() => {
                match ev {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        eprintln!("[listen] {address}/p2p/{pid}");
                        if !relay_listened {
                            relay_listened = true;
                            listen_on_relay(&mut swarm, &relay);
                        }
                    }
                    SwarmEvent::ConnectionEstablished { peer_id: p, .. } => {
                        eprintln!("[conn] peer {p}");
                    }
                    SwarmEvent::Behaviour(BehaviourEvent::Mdns(
                        mdns::Event::Discovered(list),
                    )) => {
                        for (p, a) in list { eprintln!("[mdns] {p} at {a}"); }
                    }
                    SwarmEvent::Behaviour(BehaviourEvent::Relay(
                        relay::client::Event::ReservationReqAccepted { relay_peer_id, .. },
                    )) => {
                        eprintln!("[relay] reservation accepted by {relay_peer_id}");
                    }
                    SwarmEvent::Behaviour(BehaviourEvent::Dcutr(ev)) => {
                        eprintln!("[dcutr] {ev:?}");
                    }
                    _ => {}
                }
            }
            Some((peer, stream)) = incoming.next() => {
                eprintln!("[transfer] request from {peer}");
                let path = path.clone();
                tokio::spawn(async move {
                    let compat = stream.compat();
                    let mut w = BufWriter::with_capacity(BUF, compat);
                    if let Err(e) = send_payload(&mut w, &path).await {
                        eprintln!("[transfer] failed: {e:#}");
                    }
                });
            }
        }
    }
}

// ── receiver ─────────────────────────────────────────────────────────────

async fn receive(address: &str, relay: Option<String>, port: u16) -> Res {
    let (mut swarm, mut ctl) = build_swarm()?;

    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{}", port).parse()?)?;
    setup_relay(&mut swarm, &relay)?;

    let sender_addr: Multiaddr = address.parse()?;
    let sender_pid = extract_peer_id(&sender_addr)?;

    eprintln!("[dial] connecting to {sender_pid}...");
    swarm.dial(sender_addr)?;

    // Wait for connection with timeout
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, async {
        loop {
            match swarm.select_next_some().await {
                SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == sender_pid => {
                    return Ok(true);
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. }
                    if peer_id == Some(sender_pid) =>
                {
                    return Err(format!("connection failed: {error}"));
                }
                _ => {} // keep driving swarm
            }
        }
    })
    .await;

    match connected {
        Ok(Ok(true)) => eprintln!("[conn] connected to sender"),
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err("connection timed out".into()),
        _ => unreachable!(),
    }

    // Open transfer stream with timeout
    let stream = tokio::time::timeout(STREAM_TIMEOUT, ctl.open_stream(sender_pid, PROTOCOL))
        .await
        .map_err(|_| "stream open timed out")?
        .map_err(|e| format!("stream open failed: {e}"))?;

    eprintln!("[transfer] stream opened, receiving...\n");

    let compat = stream.compat();
    let mut r = BufReader::with_capacity(BUF, compat);

    let mut mode = [0u8; 1];
    r.read_exact(&mut mode).await?;

    let dest = Path::new("received");
    fs::create_dir_all(dest).await?;

    if mode[0] == 0 {
        // Single file
        let name = recv_name(&mut r).await?;
        let path = safe_join(dest, &name)?;
        recv_file(&mut r, &path).await?;
        eprintln!("\nreceived: {}", path.display());
    } else {
        // Directory
        let folder = recv_name(&mut r).await?;
        let base = safe_join(dest, &folder)?;
        fs::create_dir_all(&base).await?;

        let mut buf4 = [0u8; 4];
        r.read_exact(&mut buf4).await?;
        let count = u32::from_le_bytes(buf4);

        for i in 0..count {
            let rel = recv_name(&mut r).await?;
            let path = safe_join(&base, &rel)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            recv_file(&mut r, &path).await?;
            eprintln!("  [{}/{}] {}", i + 1, count, path.display());
        }
        eprintln!("\nreceived folder: {}", base.display());
    }

    Ok(())
}

// ── transfer protocol ────────────────────────────────────────────────────

async fn send_payload<W: tokio::io::AsyncWrite + Unpin>(w: &mut BufWriter<W>, path: &str) -> Res {
    let p = Path::new(path);
    if p.is_file() {
        w.write_all(&[0u8]).await?;
        send_name(w, p).await?;
        send_file(w, p).await?;
    } else if p.is_dir() {
        w.write_all(&[1u8]).await?;
        send_name(w, p).await?;
        let files = collect_files(p).await?;
        w.write_all(&(files.len() as u32).to_le_bytes()).await?;
        for (abs, rel) in &files {
            send_name(w, rel).await?;
            send_file(w, abs).await?;
        }
    } else {
        return Err(format!("not a file or directory: {}", path).into());
    }
    w.flush().await?;
    eprintln!("[transfer] complete");
    Ok(())
}

async fn send_name<W: tokio::io::AsyncWrite + Unpin>(w: &mut BufWriter<W>, path: &Path) -> Res {
    let name = path
        .file_name()
        .or_else(|| path.to_str().map(std::ffi::OsStr::new)) // for relative paths like "a/b.txt"
        .unwrap_or_default()
        .to_string_lossy()
        .replace('\0', "");
    let bytes = name.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err("filename too long".into());
    }
    w.write_all(&(bytes.len() as u16).to_le_bytes()).await?;
    w.write_all(bytes).await?;
    Ok(())
}

async fn send_file<W: tokio::io::AsyncWrite + Unpin>(w: &mut BufWriter<W>, path: &Path) -> Res {
    let file = File::open(path).await?;
    let meta = file.metadata().await?;
    let size = meta.len();

    w.write_all(&size.to_le_bytes()).await?;

    if size == 0 {
        // Empty file — nothing to transfer
        return Ok(());
    }

    let mmap = unsafe { MmapOptions::new().map(&file)? };
    let pb = progress(size, path);
    let mut pos = 0usize;

    while pos < mmap.len() {
        let end = (pos + BUF).min(mmap.len());
        w.write_all(&mmap[pos..end]).await?;
        pos = end;
        pb.set_position(pos as u64);
    }

    w.flush().await?;
    pb.finish_with_message("done");
    Ok(())
}

async fn recv_name<R: tokio::io::AsyncRead + Unpin>(r: &mut BufReader<R>) -> Res<String> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf).await?;
    let len = u16::from_le_bytes(buf) as usize;
    if len == 0 || len > MAX_NAME {
        return Err(format!("invalid name length: {len}").into());
    }
    let mut name = vec![0u8; len];
    r.read_exact(&mut name).await?;
    Ok(String::from_utf8_lossy(&name)
        .replace('\0', "")
        .replace(|c: char| c.is_control(), "")
        .trim()
        .to_string())
}

async fn recv_file<R: tokio::io::AsyncRead + Unpin>(r: &mut BufReader<R>, dest: &Path) -> Res {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf).await?;
    let size = u64::from_le_bytes(buf);

    let mut file = BufWriter::with_capacity(BUF, File::create(dest).await?);

    if size == 0 {
        file.flush().await?;
        return Ok(());
    }

    let pb = progress(size, dest);
    let mut got: u64 = 0;

    while got < size {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            return Err(format!(
                "transfer interrupted: got {got}/{size} bytes for {}",
                dest.display()
            )
            .into());
        }
        let n = chunk.len().min((size - got) as usize);
        file.write_all(&chunk[..n]).await?;
        got += n as u64;
        pb.set_position(got);
        r.consume(n);
    }

    file.flush().await?;
    pb.finish_with_message("done");
    Ok(())
}

// ── utilities ────────────────────────────────────────────────────────────

/// Iterative directory traversal. Skips symlinks.
async fn collect_files(root: &Path) -> std::io::Result<Vec<(PathBuf, PathBuf)>> {
    let mut result = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let ft = entry.file_type().await?;
            if ft.is_symlink() {
                continue; // skip symlinks — prevents loops
            }
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
                    .to_path_buf();
                result.push((path, rel));
            }
        }
    }

    result.sort_by(|a, b| a.1.cmp(&b.1)); // deterministic order
    Ok(result)
}

/// Join base + untrusted name, rejecting path traversal (.. components).
fn safe_join(base: &Path, name: &str) -> Res<PathBuf> {
    let raw = Path::new(name);
    let cleaned: PathBuf = raw
        .components()
        .filter(|c| !matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        .map(|c| {
            let s = c.as_os_str().to_string_lossy();
            s.replace(|ch: char| ch.is_control() || "<>:\"|?*".contains(ch), "_")
                .trim()
                .chars()
                .take(MAX_NAME)
                .collect::<String>()
        })
        .collect();

    if cleaned.as_os_str().is_empty() {
        return Err("empty filename after sanitization".into());
    }

    let full = base.join(&cleaned);

    // Final check: resolved path must stay under base
    if !full.starts_with(base) {
        return Err(format!("path traversal blocked: {name}").into());
    }

    Ok(full)
}

fn progress(len: u64, path: &Path) -> ProgressBar {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?");
    let name = if name.len() > 30 {
        format!("{}...", &name[..27])
    } else {
        name.to_string()
    };
    ProgressBar::new(len).with_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "  [{name}] {{bar:30}} {{bytes}}/{{total_bytes}} {{bytes_per_sec}} ({{eta}})"
            ))
            .unwrap(),
    )
}
