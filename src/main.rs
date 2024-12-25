use clap::{Arg, Command};
use indicatif::{ProgressBar, ProgressStyle};
use num_cpus;
use std::error::Error;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

const BUFFER_SIZE: usize = 64 * 1024; // 64 KB

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // Determine number of threads to use
    let num_threads = num_cpus::get();

    // Build the Tokio runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_threads)
        .enable_all()
        .build()?;

    runtime.block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let matches = Command::new("File Transfer")
        .version("1.0")
        .author("github.com/idevanshu")
        .about("Send and receive files using TCP")
        .arg(
            Arg::new("mode")
                .short('m')
                .long("mode")
                .help("Mode: send or receive")
                .required(true),
        )
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .help("File path for sending"),
        )
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
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
        .split('/')
        .last()
        .unwrap_or("file")
        .to_string();

    // Bind to 0.0.0.0:8000 (or your desired address/port)
    let listener = TcpListener::bind("0.0.0.0:8000").await?;
    println!("Sender: Listening on 0.0.0.0:8000");

    // Limit concurrent connections to number of CPU cores
    let semaphore = Arc::new(Semaphore::new(num_cpus::get()));

    // Accept connections in a loop
    loop {
        let (socket, addr) = listener.accept().await?;
        println!("Sender: New connection from {:?}", addr);

        let permit = semaphore.clone().acquire_owned().await?;
        let file_path_cloned = file_path.to_string();
        let file_name_cloned = file_name.clone();

        // Spawn a task to handle each connection
        tokio::spawn(async move {
            let _permit = permit; // Keep the permit alive in this task

            if let Err(e) = handle_send_connection(socket, &file_path_cloned, &file_name_cloned).await
            {
                eprintln!("Connection handling failed: {}", e);
            }
        });
    }
}

/// Handle a single connection in send mode.
async fn handle_send_connection(
    mut socket: TcpStream,
    file_path: &str,
    file_name: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // Open the file
    let mut file = tokio::fs::File::open(file_path).await?;
    let file_size = file.metadata().await?.len();

    // 1) Send file size (8 bytes, little-endian)
    socket.write_all(&file_size.to_le_bytes()).await?;

    // 2) Send the file name + newline
    socket.write_all(file_name.as_bytes()).await?;
    socket.write_all(b"\n").await?;

    // 3) Send the file in chunks
    let progress = ProgressBar::new(file_size);
    progress.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
            )
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut buffer = vec![0; BUFFER_SIZE];
    let mut total_sent = 0u64;

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break; // EOF
        }
        socket.write_all(&buffer[..n]).await?;
        total_sent += n as u64;
        progress.set_position(total_sent);
    }

    progress.finish_with_message("File sent successfully!");

    Ok(())
}

async fn receive_file(address: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    // Strip optional "tcp://" prefix if provided
    let address = address.strip_prefix("tcp://").unwrap_or(address);

    // Connect to sender
    let stream = TcpStream::connect(address).await?;
    println!("Receiver: Connected to {}", address);

    // Use a BufReader so we can read lines for the file name easily
    let mut reader = BufReader::new(stream);

    // 1) Read the file size (8 bytes)
    let mut size_buf = [0u8; 8];
    reader.read_exact(&mut size_buf).await?;
    let file_size = u64::from_le_bytes(size_buf);

    // 2) Read the file name (until newline)
    let mut file_name = String::new();
    reader.read_line(&mut file_name).await?; 
    // Remove trailing newline
    if let Some('\n') = file_name.chars().last() {
        file_name.pop();
    }
    // In case there's a Windows-style \r\n
    if let Some('\r') = file_name.chars().last() {
        file_name.pop();
    }
    if file_name.is_empty() {
        file_name = "received_file".to_string();
    }

    println!("Receiver: Expecting to write file: {} (size: {} bytes)", file_name, file_size);

    // Create output file
    let mut file = tokio::fs::File::create(&file_name).await?;

    // 3) Receive the file content
    let progress_bar = ProgressBar::new(file_size);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
            )
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut buffer = vec![0; BUFFER_SIZE];
    let mut total_received = 0u64;

    // Read until we've received `file_size` bytes (or EOF)
    while total_received < file_size {
        // The maximum left to read in case the last chunk is smaller
        let remaining = (file_size - total_received) as usize;
        let read_len = remaining.min(BUFFER_SIZE);

        let n = reader.read(&mut buffer[..read_len]).await?;
        if n == 0 {
            // EOF reached unexpectedly (or sender closed)
            break;
        }
        // Write to file
        file.write_all(&buffer[..n]).await?;

        total_received += n as u64;
        progress_bar.set_position(total_received);
    }

    progress_bar.finish_with_message("File received successfully!");

    Ok(())
}
