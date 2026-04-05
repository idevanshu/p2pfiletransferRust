use async_recursion::async_recursion;
use clap::{Arg, Command};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use memmap2::MmapOptions;
use socket2::{Socket, TcpKeepalive};
use std::error::Error;
use std::ffi::OsStr;
use std::io;
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};

// ── Performance tuning (100 Gbps target) ───────────────────────
const IO_BUF: usize = 32 * 1024 * 1024; // 32 MB I/O chunks
const TCP_BUF: usize = 8 * 1024 * 1024; // 8 MB socket bufs (Windows-friendly)
const MAX_COMP_LEN: usize = 240;
const KEEPALIVE: Duration = Duration::from_secs(60);
const DEFAULT_STREAMS: usize = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(30);

type Res = Result<(), Box<dyn Error + Send + Sync>>;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
//  Entry — custom runtime sized to hardware
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn main() -> Res {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_cpus::get())
        .max_blocking_threads(64)
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Res {
    let m = Command::new("Fast File Transfer")
        .version("5.0")
        .author("github.com/idevanshu")
        .about("High-speed multi-stream file transfer over TCP (100 Gbps optimised)")
        .arg(
            Arg::new("mode")
                .short('m')
                .long("mode")
                .required(true)
                .value_parser(["send", "receive"]),
        )
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .required_if_eq("mode", "send"),
        )
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
                .required_if_eq("mode", "receive"),
        )
        .arg(
            Arg::new("streams")
                .short('s')
                .long("streams")
                .value_parser(clap::value_parser!(usize))
                .help("Parallel TCP data streams [default: max(num_cpus, 16)]"),
        )
        .get_matches();

    match m.get_one::<String>("mode").map(String::as_str) {
        Some("send") => {
            let n = m
                .get_one::<usize>("streams")
                .copied()
                .unwrap_or_else(|| num_cpus::get().max(DEFAULT_STREAMS));
            send(m.get_one::<String>("file").unwrap(), n).await
        }
        Some("receive") => receive(m.get_one::<String>("address").unwrap()).await,
        _ => unreachable!(),
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
//  SENDER
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn send(path: &str, num_streams: usize) -> Res {
    // Validate path before binding
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("path does not exist: {path}").into());
    }
    if !p.is_file() && !p.is_dir() {
        return Err(format!("not a file or directory: {path}").into());
    }

    let lis = TcpListener::bind("0.0.0.0:8000").await.map_err(|e| {
        format!("failed to bind on port 8000 (is another instance running?): {e}")
    })?;
    println!(
        "Listening on {} ({num_streams} data streams)",
        lis.local_addr()?
    );

    loop {
        // 1 ─ accept control connection
        let (ctrl, addr) = lis.accept().await?;
        println!("\nReceiver connected: {addr}");
        let ctrl = setup(ctrl).map_err(|e| format!("control socket setup failed: {e}"))?;
        let mut ctrl = BufWriter::with_capacity(IO_BUF, ctrl);

        // 2 ─ handshake: tell receiver how many data streams to open
        ctrl.write_all(&(num_streams as u32).to_le_bytes()).await?;
        ctrl.flush().await?;

        // 3 ─ accept N data streams (with timeout)
        let mut data = Vec::with_capacity(num_streams);
        for i in 0..num_streams {
            let accept_fut = lis.accept();
            let (s, _) = tokio::time::timeout(ACCEPT_TIMEOUT, accept_fut)
                .await
                .map_err(|_| {
                    format!(
                        "timed out waiting for data stream {}/{num_streams} ({}s)",
                        i + 1,
                        ACCEPT_TIMEOUT.as_secs()
                    )
                })??;
            data.push(
                setup(s).map_err(|e| format!("data stream {} setup failed: {e}", i + 1))?,
            );
        }
        println!("{num_streams} data streams established");

        // 4 ─ transfer (spawned so we can accept the next receiver)
        let p = path.to_string();
        tokio::spawn(async move {
            if let Err(e) = do_send(ctrl, data, &p).await {
                eprintln!("Transfer error: {e:#}");
            }
        });
    }
}

async fn do_send(mut ctrl: BufWriter<TcpStream>, data: Vec<TcpStream>, path: &str) -> Res {
    let p = Path::new(path);

    if p.is_file() {
        // ── single file ──
        ctrl.write_all(&[0u8]).await?;
        write_name(&mut ctrl, p).await?;
        let size = fs::metadata(p).await?.len();
        ctrl.write_all(&size.to_le_bytes()).await?;
        ctrl.flush().await?;
        send_file_striped(data, p, size).await
    } else if p.is_dir() {
        // ── folder ──
        ctrl.write_all(&[1u8]).await?;
        write_name(&mut ctrl, p).await?;

        let entries = collect_files(p, p).await?;
        let mut manifest = Vec::with_capacity(entries.len());
        for (abs, rel) in &entries {
            let sz = fs::metadata(abs).await?.len();
            manifest.push((abs.clone(), rel.to_string_lossy().into_owned(), sz));
        }

        // send manifest
        ctrl.write_all(&(manifest.len() as u32).to_le_bytes())
            .await?;
        for (_, rel, sz) in &manifest {
            let b = rel.as_bytes();
            ctrl.write_all(&(b.len() as u16).to_le_bytes()).await?;
            ctrl.write_all(b).await?;
            ctrl.write_all(&sz.to_le_bytes()).await?;
        }
        ctrl.flush().await?;

        send_folder_striped(data, &manifest).await
    } else {
        Err(format!("{path}: not a file or directory").into())
    }
}

/// Stripe a single file across N parallel data streams.
/// Zero-copy path: mmap (pre-populated) → socket, no intermediate BufWriter.
async fn send_file_striped(streams: Vec<TcpStream>, path: &Path, size: u64) -> Res {
    if size == 0 {
        return Ok(());
    }
    let n = streams.len() as u64;
    let share = (size + n - 1) / n;
    let mp = MultiProgress::new();
    let path = path.to_path_buf();
    let mut tasks = Vec::new();

    for (i, mut stream) in streams.into_iter().enumerate() {
        let off = i as u64 * share;
        if off >= size {
            break;
        }
        let len = share.min(size - off) as usize;
        let path = path.clone();
        let pb = bar(&mp, i, len as u64);

        tasks.push(tokio::spawn(async move {
            let fd = std::fs::File::open(&path)?;
            let map = unsafe {
                MmapOptions::new()
                    .offset(off)
                    .len(len)
                    .populate()
                    .map(&fd)?
            };
            // Write directly from mmap → socket (no BufWriter copy)
            let mut pos = 0;
            while pos < len {
                let end = (pos + IO_BUF).min(len);
                stream.write_all(&map[pos..end]).await?;
                pos = end;
                pb.set_position(pos as u64);
            }
            pb.finish_with_message("done");
            Ok::<_, Box<dyn Error + Send + Sync>>(())
        }));
    }
    join(tasks).await
}

/// Distribute folder files across N data streams (greedy load-balance).
/// Zero-copy: mmap (pre-populated) → socket directly.
async fn send_folder_striped(streams: Vec<TcpStream>, manifest: &[(PathBuf, String, u64)]) -> Res {
    let n = streams.len();
    let assign = balance(manifest.len(), |i| manifest[i].2, n);
    let mp = MultiProgress::new();
    let manifest = Arc::new(manifest.to_vec());
    let mut tasks = Vec::new();

    for (i, (mut stream, indices)) in streams.into_iter().zip(assign).enumerate() {
        let manifest = manifest.clone();
        let total: u64 = indices.iter().map(|&j| manifest[j].2).sum();
        let pb = bar(&mp, i, total);

        tasks.push(tokio::spawn(async move {
            // header: how many files this stream will carry
            stream
                .write_all(&(indices.len() as u32).to_le_bytes())
                .await?;

            for idx in indices {
                let (ref abs, _, _) = manifest[idx];
                stream.write_all(&(idx as u32).to_le_bytes()).await?;

                let fd = std::fs::File::open(abs)?;
                let meta = fd.metadata()?;
                if meta.len() > 0 {
                    let map = unsafe { MmapOptions::new().populate().map(&fd)? };
                    let mut pos = 0;
                    while pos < map.len() {
                        let end = (pos + IO_BUF).min(map.len());
                        stream.write_all(&map[pos..end]).await?;
                        pb.inc((end - pos) as u64);
                        pos = end;
                    }
                }
            }
            pb.finish_with_message("done");
            Ok::<_, Box<dyn Error + Send + Sync>>(())
        }));
    }
    join(tasks).await
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
//  RECEIVER
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn receive(address: &str) -> Res {
    // 1 ─ control connection (with timeout)
    println!("Connecting to {address}...");
    let ctrl = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
        .await
        .map_err(|_| {
            format!(
                "connection timed out after {}s — is the sender running at {address}?",
                CONNECT_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("failed to connect to {address}: {e}"))?;
    let ctrl =
        setup(ctrl).map_err(|e| format!("control socket setup failed: {e}"))?;
    let mut ctrl = BufReader::with_capacity(IO_BUF, ctrl);

    let num_streams = read_u32(&mut ctrl).await? as usize;
    if num_streams == 0 || num_streams > 256 {
        return Err(format!("invalid stream count from sender: {num_streams}").into());
    }

    // 2 ─ open data streams in parallel (much faster than sequential)
    let addr = address.to_string();
    let mut futs = Vec::with_capacity(num_streams);
    for i in 0..num_streams {
        let addr = addr.clone();
        futs.push(async move {
            let s = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
                .await
                .map_err(|_| {
                    format!(
                        "data stream {}/{num_streams} timed out connecting to {addr}",
                        i + 1
                    )
                })?
                .map_err(|e| format!("data stream {}/{num_streams} failed: {e}", i + 1, ))?;
            setup(s).map_err(|e| {
                format!("data stream {}/{num_streams} setup failed: {e}", i + 1)
            })
        });
    }
    let data: Result<Vec<TcpStream>, String> =
        futures::future::join_all(futs).await.into_iter().collect();
    let data = data.map_err(|e| -> Box<dyn Error + Send + Sync> { e.into() })?;
    println!("Connected to {address} ({num_streams} streams)");

    // 3 ─ read mode + manifest (sender writes these after data streams connect)
    let mut mode = [0u8; 1];
    ctrl.read_exact(&mut mode).await?;

    let root = PathBuf::from("received");
    fs::create_dir_all(&root).await?;

    match mode[0] {
        0 => {
            let name = read_str(&mut ctrl).await?;
            let size = read_u64(&mut ctrl).await?;
            let dest = sanitize(&root.join(&name));
            println!("Receiving file: {name} ({size} bytes)");
            recv_file_striped(data, &dest, size).await
        }
        1 => {
            let folder = read_str(&mut ctrl).await?;
            let base = sanitize(&root.join(&folder));
            fs::create_dir_all(&base).await?;

            let count = read_u32(&mut ctrl).await? as usize;
            let mut manifest = Vec::with_capacity(count);
            for _ in 0..count {
                manifest.push((read_str(&mut ctrl).await?, read_u64(&mut ctrl).await?));
            }
            println!("Receiving folder: {folder}/ ({count} files)");
            recv_folder_striped(data, &manifest, &base).await
        }
        other => Err(format!("unknown mode byte {other}").into()),
    }
}

/// Reassemble a single file from N striped data streams.
/// Large BufReader on socket, direct writes to pre-allocated file (no BufWriter).
async fn recv_file_striped(streams: Vec<TcpStream>, dest: &Path, size: u64) -> Res {
    if size == 0 {
        File::create(dest).await?;
        return Ok(());
    }

    // pre-allocate the output file
    {
        let f = File::create(dest).await?;
        f.set_len(size).await?;
    }

    let n = streams.len() as u64;
    let share = (size + n - 1) / n;
    let mp = MultiProgress::new();
    let dest = dest.to_path_buf();
    let mut tasks = Vec::new();

    for (i, stream) in streams.into_iter().enumerate() {
        let off = i as u64 * share;
        if off >= size {
            break;
        }
        let len = share.min(size - off);
        let dest = dest.clone();
        let pb = bar(&mp, i, len);

        tasks.push(tokio::spawn(async move {
            let mut r = BufReader::with_capacity(IO_BUF, stream);
            let mut f = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&dest)
                .await?;
            f.seek(io::SeekFrom::Start(off)).await?;

            let mut got = 0u64;
            while got < len {
                let buf = r.fill_buf().await?;
                if buf.is_empty() {
                    break;
                }
                let take = buf.len().min((len - got) as usize);
                f.write_all(&buf[..take]).await?;
                got += take as u64;
                pb.set_position(got);
                r.consume(take);
            }
            f.flush().await?;
            pb.finish_with_message("done");
            Ok::<_, Box<dyn Error + Send + Sync>>(())
        }));
    }
    join(tasks).await
}

/// Receive folder files from N data streams.
/// Direct file writes — no intermediate BufWriter.
async fn recv_folder_striped(
    streams: Vec<TcpStream>,
    manifest: &[(String, u64)],
    base: &Path,
) -> Res {
    let mp = MultiProgress::new();
    let manifest = Arc::new(manifest.to_vec());
    let base = base.to_path_buf();
    let mut tasks = Vec::new();

    for (i, stream) in streams.into_iter().enumerate() {
        let manifest = manifest.clone();
        let base = base.clone();
        let mp_ref = mp.clone();

        tasks.push(tokio::spawn(async move {
            let mut r = BufReader::with_capacity(IO_BUF, stream);

            // how many files arrive on this stream
            let count = read_u32(&mut r).await? as usize;

            let pb = bar(&mp_ref, i, 0);
            let mut total_expected = 0u64;

            for _ in 0..count {
                let idx = read_u32(&mut r).await? as usize;
                let (ref rel, sz) = manifest[idx];
                total_expected += sz;
                pb.set_length(total_expected);

                let full = sanitize(&base.join(rel));
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent).await?;
                }

                let mut file = File::create(&full).await?;
                let mut got = 0u64;
                while got < sz {
                    let buf = r.fill_buf().await?;
                    if buf.is_empty() {
                        break;
                    }
                    let take = buf.len().min((sz - got) as usize);
                    file.write_all(&buf[..take]).await?;
                    got += take as u64;
                    pb.inc(take as u64);
                    r.consume(take);
                }
                file.flush().await?;
            }
            pb.finish_with_message("done");
            Ok::<_, Box<dyn Error + Send + Sync>>(())
        }));
    }
    join(tasks).await
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
//  Helpers
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn join(tasks: Vec<tokio::task::JoinHandle<Res>>) -> Res {
    for t in tasks {
        t.await??;
    }
    Ok(())
}

async fn write_name<W: AsyncWriteExt + Unpin>(w: &mut W, p: &Path) -> Res {
    let name = p
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no filename"))?
        .to_string_lossy()
        .replace('\0', "");
    let b = name.as_bytes();
    w.write_all(&(b.len() as u16).to_le_bytes()).await?;
    w.write_all(b).await?;
    Ok(())
}

async fn read_str<R: AsyncReadExt + Unpin>(
    r: &mut R,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let len = {
        let mut b = [0u8; 2];
        r.read_exact(&mut b).await?;
        u16::from_le_bytes(b) as usize
    };
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf)
        .replace('\0', "")
        .replace(|c: char| c.is_control(), "")
        .trim()
        .to_string())
}

async fn read_u32<R: AsyncReadExt + Unpin>(
    r: &mut R,
) -> Result<u32, Box<dyn Error + Send + Sync>> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).await?;
    Ok(u32::from_le_bytes(b))
}

async fn read_u64<R: AsyncReadExt + Unpin>(
    r: &mut R,
) -> Result<u64, Box<dyn Error + Send + Sync>> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).await?;
    Ok(u64::from_le_bytes(b))
}

fn setup(sock: TcpStream) -> io::Result<TcpStream> {
    let std: StdTcpStream = sock.into_std()?;
    std.set_nodelay(true)?;
    let s = Socket::from(std);
    // Best-effort buffer sizing — Windows may reject large values
    let _ = s.set_send_buffer_size(TCP_BUF);
    let _ = s.set_recv_buffer_size(TCP_BUF);
    let _ = s.set_tcp_keepalive(&TcpKeepalive::new().with_time(KEEPALIVE));
    TcpStream::from_std(s.into())
}

fn sanitize(path: &Path) -> PathBuf {
    path.components()
        .map(|c| {
            let s = c.as_os_str().to_string_lossy();
            OsStr::new(
                &s.replace(
                    |ch: char| ch.is_control() || "<>:\"/\\|?*".contains(ch),
                    "_",
                )
                .trim()
                .chars()
                .take(MAX_COMP_LEN)
                .collect::<String>(),
            )
            .to_os_string()
        })
        .collect()
}

/// Greedy load-balance: assign items to `n` buckets, biggest-first.
fn balance(count: usize, weight: impl Fn(usize) -> u64, n: usize) -> Vec<Vec<usize>> {
    let mut ord: Vec<usize> = (0..count).collect();
    ord.sort_by(|a, b| weight(*b).cmp(&weight(*a)));
    let mut buckets = vec![vec![]; n];
    let mut loads = vec![0u64; n];
    for i in ord {
        let m = loads
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| **l)
            .unwrap()
            .0;
        loads[m] += weight(i);
        buckets[m].push(i);
    }
    buckets
}

fn bar(mp: &MultiProgress, i: usize, total: u64) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(total));
    pb.set_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "{{spinner:.green}} [stream {i}] {{bar:40}} {{bytes}}/{{total_bytes}} ({{bytes_per_sec}})"
            ))
            .unwrap(),
    );
    pb
}

#[async_recursion]
async fn collect_files(root: &Path, base: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    let mut out = Vec::new();
    let mut rd = fs::read_dir(root).await?;
    while let Some(e) = rd.next_entry().await? {
        let p = e.path();
        if p.is_file() {
            out.push((
                p.clone(),
                p.strip_prefix(base)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
                    .to_path_buf(),
            ));
        } else if p.is_dir() {
            out.extend(collect_files(&p, base).await?);
        }
    }
    Ok(out)
}
