use clap::{Arg, Command};
use indicatif::{ProgressBar, ProgressStyle};
use num_cpus;
use std::error::Error;
use std::io::Error as IoError;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder;
use tokio::sync::Semaphore;

const COPY_BUFFER_SIZE: usize = 128 * 1024; //128KB

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let threads = num_cpus::get();
    let runtime = Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_io()
        .enable_time()
        .build()?;
    runtime.block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let matches = Command::new("File Transfer")
        .version("2.0")
        .author("github.com/idevanshu")
        .about("Send and receive files using TCP with a progress bar.")
        .arg(
            Arg::new("mode")
                .short('m')
                .long("mode")
                .required(true)
                .num_args(1)
                .help("Mode: send or receive"),
        )
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .num_args(1)
                .help("File path for sending"),
        )
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
                .num_args(1)
                .help("Address for receiving (e.g. 127.0.0.1:8000)"),
        )
        .get_matches();
    let mode = matches
        .get_one::<String>("mode")
        .map(|m| m.as_str())
        .unwrap_or_default();
    match mode {
        "send" => {
            let file_path = matches
                .get_one::<String>("file")
                .expect("File path is required for sending");
            send_file(file_path).await?;
        }
        "receive" => {
            let address = matches
                .get_one::<String>("address")
                .expect("Address is required for receiving");
            receive_file(address).await?;
        }
        _ => eprintln!("Invalid mode. Use 'send' or 'receive'."),
    }
    Ok(())
}

async fn send_file(file_path: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let file_name = file_path
        .rsplit('/')
        .next()
        .unwrap_or("file")
        .to_string();
    let listener = TcpListener::bind("0.0.0.0:8000").await?;
    let local_addr = listener.local_addr()?;
    println!("Sender: Listening on {}", local_addr);
    let semaphore = Arc::new(Semaphore::new(num_cpus::get()));
    loop {
        let (socket, addr) = listener.accept().await?;
        println!("Sender: New connection from {}", addr);
        let permit = semaphore.clone().acquire_owned().await?;
        let file_path_cloned = file_path.to_string();
        let file_name_cloned = file_name.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = handle_send_connection(socket, &file_path_cloned, &file_name_cloned).await {
                eprintln!("Connection handling failed: {}", e);
            }
        });
    }
}

async fn handle_send_connection(
    mut socket: TcpStream,
    file_path: &str,
    file_name: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    socket.set_nodelay(true)?;
    let mut file = File::open(file_path).await?;
    let file_size = file.metadata().await?.len();
    socket.write_all(&file_size.to_le_bytes()).await?;
    socket.write_all(file_name.as_bytes()).await?;
    socket.write_all(b"\n").await?;
    let progress = ProgressBar::new(file_size);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );
    let mut writer = BufWriter::new(socket);
    copy_with_progress(&mut file, &mut writer, COPY_BUFFER_SIZE, &progress).await?;
    writer.flush().await?;
    progress.finish_with_message("File sent successfully!");
    println!("Sender: Finished sending '{}', size: {} bytes", file_name, file_size);
    Ok(())
}

async fn receive_file(address: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let address = address.strip_prefix("tcp://").unwrap_or(address);
    let stream = TcpStream::connect(address).await?;
    println!("Receiver: Connected to {}", address);
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream);
    let mut size_buf = [0u8; 8];
    reader.read_exact(&mut size_buf).await?;
    let file_size = u64::from_le_bytes(size_buf);
    let mut file_name = String::new();
    reader.read_line(&mut file_name).await?;
    file_name.truncate(file_name.trim_end_matches(&['\r', '\n'][..]).len());
    if file_name.is_empty() {
        file_name = "received_file".to_string();
    }
    println!("Receiver: Expecting to write file '{}', size: {} bytes", file_name, file_size);
    let progress = ProgressBar::new(file_size);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );
    let mut file = File::create(&file_name).await?;
    let mut limited_reader = reader.take(file_size);
    copy_with_progress(&mut limited_reader, &mut file, COPY_BUFFER_SIZE, &progress).await?;
    progress.finish_with_message("File received successfully!");
    println!("Receiver: File '{}' received successfully!", file_name);
    Ok(())
}

async fn copy_with_progress<R, W>(
    reader: &mut R,
    writer: &mut W,
    buffer_size: usize,
    progress: &ProgressBar,
) -> Result<u64, IoError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = vec![0u8; buffer_size];
    let mut total_copied = 0u64;
    loop {
        let bytes_read = match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Err(e),
        };
        writer.write_all(&buf[..bytes_read]).await?;
        total_copied += bytes_read as u64;
        progress.set_position(total_copied);
    }
    Ok(total_copied)
}
