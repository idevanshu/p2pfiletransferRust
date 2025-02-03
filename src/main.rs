use clap::{Arg, Command};
use humansize::{BINARY, format_size};
use indicatif::{ProgressBar, ProgressStyle};
use memmap2::MmapOptions;
use num_cpus;
use socket2::Socket;
use std::error::Error;
use std::io;
use std::net::TcpStream as StdTcpStream;
use std::path::Path;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

// Reduce buffer sizes for lower latency
const COPY_BUFFER_SIZE: usize = 456 * 1024; //456KB
const TCP_BUFFER_SIZE: usize = 1 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let matches = Command::new("High-Speed File Transfer")
        .version("2.1")
        .author("github.com/idevanshu")
        .about("Cross-platform file transfer tool")
        .arg(
            Arg::new("mode")
                .short('m')
                .long("mode")
                .required(true)
                .value_parser(["send", "receive"])
                .help("Transfer mode: send or receive"),
        )
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .required_if_eq("mode", "send")
                .help("File path to send"),
        )
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
                .required_if_eq("mode", "receive")
                .help("Receiver address (e.g., for LAN:-> 192.168.1.10:8000, for WAN: example.com:8000 or <public_ip_address>:8000)"),
        )
        .get_matches();

    match matches.get_one::<String>("mode").map(|s| s.as_str()) {
        Some("send") => {
            let file_path = matches.get_one::<String>("file").unwrap();
            send_file(file_path).await
        }
        Some("receive") => {
            let address = matches.get_one::<String>("address").unwrap();
            receive_file(address).await
        }
        _ => unreachable!(),
    }
}

async fn send_file(file_path: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let listener = TcpListener::bind("0.0.0.0:8000").await?;
    println!("Sender ready at: {}", listener.local_addr()?);
    let semaphore = Arc::new(Semaphore::new(num_cpus::get()));
    let file_path = Arc::new(file_path.to_string());
    
    loop {
        let (socket, addr) = listener.accept().await?;
        println!("New receiver connected: {}", addr);
        let permit = semaphore.clone().acquire_owned().await?;
        let file_path = file_path.clone();
        
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = handle_send(socket, &file_path).await {
                eprintln!("Transfer failed: {}", e);
            }
        });
    }
}

async fn handle_send(socket: TcpStream, file_path: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let socket = configure_socket(socket)?;
    let file = File::open(file_path).await?;
    let metadata = file.metadata().await?;
    let path = Path::new(file_path);
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let mmap = unsafe { MmapOptions::new().map(&file)? };

    let progress = ProgressBar::new(metadata.len());
    progress.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] {bar:40} {bytes:>7}/{total_bytes:7} {bytes_per_sec:7} ({eta})")
        .unwrap()
        .progress_chars("##-"));

    let mut writer = BufWriter::with_capacity(COPY_BUFFER_SIZE, socket);
    
    // Send metadata with immediate flush
    writer.write_all(&metadata.len().to_le_bytes()).await?;
    writer.write_all(filename.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut sent = 0;
    while sent < mmap.len() {
        let chunk_size = std::cmp::min(COPY_BUFFER_SIZE, mmap.len() - sent);
        writer.write_all(&mmap[sent..sent + chunk_size]).await?;
        writer.flush().await?;  // Immediate flush after each chunk
        sent += chunk_size;
        progress.set_position(sent as u64);
    }

    progress.finish_with_message("Transfer complete");
    Ok(())
}

async fn receive_file(address: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let socket = TcpStream::connect(address).await?;
    let socket = configure_socket(socket)?;
    let mut reader = BufReader::with_capacity(COPY_BUFFER_SIZE, socket);

    let mut size_buf = [0u8; 8];
    reader.read_exact(&mut size_buf).await?;
    let file_size = u64::from_le_bytes(size_buf);
    
    let mut filename = String::new();
    reader.read_line(&mut filename).await?;
    let filename = filename.trim_end();
    
    println!("Receiving: {} ({})", filename, format_size(file_size, BINARY));

    let mut file = BufWriter::with_capacity(COPY_BUFFER_SIZE, File::create(filename).await?);
    let progress = ProgressBar::new(file_size);
    progress.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] {bar:40} {bytes:>7}/{total_bytes:7} {bytes_per_sec:7} ({eta})")
        .unwrap()
        .progress_chars("##-"));

    let mut received: u64 = 0;
    let mut buffer = vec![0u8; COPY_BUFFER_SIZE];
    
    while received < file_size {
        let read_size = reader.read(&mut buffer).await?;
        if read_size == 0 { break; }
        file.write_all(&buffer[..read_size]).await?;
        file.flush().await?;  // Flush after each write
        received += read_size as u64;
        progress.set_position(received);
    }

    progress.finish_with_message("Receive complete");
    Ok(())
}

fn configure_socket(socket: TcpStream) -> io::Result<TcpStream> {
    let std_stream: StdTcpStream = socket.into_std()?;
    std_stream.set_nodelay(true)?;
    let socket2 = Socket::from(std_stream);
    socket2.set_send_buffer_size(TCP_BUFFER_SIZE)?;
    socket2.set_recv_buffer_size(TCP_BUFFER_SIZE)?;
    TcpStream::from_std(socket2.into())
}